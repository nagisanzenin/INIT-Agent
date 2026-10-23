//! The capability self-model `live.capability`, method `capability-v1` (selfmap.md §3.12).
//!
//! Input: the gate's compacted RunRecords (kernel/gate.md §4.4, v2) in `<results>/<sha12>.jsonl`. Kept:
//! - runs whose arm `sha` is the LKG;
//! - sets `pool` and `capability` only; a run marked `"sealed": true` is excluded, and so is a
//!   `capability` run not marked `"sealed": false` (gate.md §3.7.2 / D2: capability set A holds only
//!   sealed instances), so holdout, canary and sealed results never enter the model;
//! - the last 20 G4 jobs (by `gate_id`), and of those the runs within 30 days of the newest run.
//!
//! RunRecord v2 has no timestamp and no explicit default model. This module reads an optional `ts`
//! (RFC 3339) and an optional `model` (`"<provider>/<model>"`); without `model`, the model is the `models`
//! key with the most requests (ties: the smaller name). Without `ts`, the window is the job window
//! alone, and `window.from/to` and `last_run` are null. See `build/status/WP9.md` (needs from the gate).
//!
//! The bootstrap uses the gate's primitives (gate.md §3.8.3) bit for bit: `seed_u64`, SplitMix64,
//! Lemire `idx`; B = 2,000; lo = D*[floor(0.025·B)], hi = D*[ceil(0.975·B) − 1].

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Bootstrap resamples per key.
pub const B: usize = 2000;
/// Window bounds.
pub const MAX_GATES: usize = 20;
pub const MAX_AGE_MS: i64 = 30 * 86_400_000;

/// `seed_u64(id, purpose)`: the first 8 bytes, big-endian, of `sha256(id + "/" + purpose)`.
pub fn seed_u64(id: &str, purpose: &str) -> u64 {
    let mut h = Sha256::new();
    h.update(id.as_bytes());
    h.update(b"/");
    h.update(purpose.as_bytes());
    let d = h.finalize();
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[..8]);
    u64::from_be_bytes(b)
}

/// SplitMix64 with wrapping arithmetic and Lemire's `idx` (gate.md §3.8.3).
pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        SplitMix64(seed)
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn idx(&mut self, n: usize) -> usize {
        ((self.next() as u128 * n as u128) >> 64) as usize
    }
}

fn lo_index(alpha: f64, b: usize) -> usize {
    (((alpha * b as f64) + 1e-9).floor() as usize).min(b.saturating_sub(1))
}

fn hi_index(alpha: f64, b: usize) -> usize {
    let k = (((1.0 - alpha) * b as f64) - 1e-9).ceil() as usize;
    k.clamp(1, b.max(1)) - 1
}

/// Two-sided 95% percentile cluster bootstrap of the mean of `s` (one value per task).
pub fn interval(id: &str, purpose: &str, s: &[f64]) -> (f64, f64) {
    let mut rng = SplitMix64::new(seed_u64(id, purpose));
    let n = s.len();
    let mut means = Vec::with_capacity(B);
    for _ in 0..B {
        let mut t = 0.0;
        for _ in 0..n {
            t += s[rng.idx(n)];
        }
        means.push(t / n as f64);
    }
    means.sort_by(|a, b| a.total_cmp(b));
    (means[lo_index(0.025, B)], means[hi_index(0.025, B)])
}

/// Milliseconds since the Unix epoch of an RFC 3339 timestamp (`Z` or `±HH:MM` offset).
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || !matches!(b[10], b'T' | b't')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> { s.get(r)?.parse().ok() };
    let (y, mo, d, h, mi, sec) = (
        num(0..4)?,
        num(5..7)?,
        num(8..10)?,
        num(11..13)?,
        num(14..16)?,
        num(17..19)?,
    );
    let mut i = 19;
    let mut ms = 0i64;
    if b.get(i) == Some(&b'.') {
        let start = i + 1;
        i = start;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        let frac = s.get(start..i)?;
        if frac.is_empty() {
            return None;
        }
        let f3: String = frac.chars().chain("000".chars()).take(3).collect();
        ms = f3.parse().ok()?;
    }
    let off = match s.get(i..)? {
        "Z" | "z" => 0,
        o if o.len() == 6 && (o.starts_with('+') || o.starts_with('-')) && &o[3..4] == ":" => {
            let v = o[1..3].parse::<i64>().ok()? * 60 + o[4..6].parse::<i64>().ok()?;
            if o.starts_with('-') { -v } else { v }
        }
        _ => return None,
    };
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    // days_from_civil (Howard Hinnant).
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + h) * 60 + mi - off) * 60_000 + sec * 1000 + ms)
}

#[derive(Debug, Clone)]
struct Rec {
    gate_id: String,
    task: String,
    source: &'static str,
    labels: Vec<String>,
    score: f64,
    cost: f64,
    wall: f64,
    model: String,
    ts: Option<(i64, String)>,
}

fn is_label(s: &str) -> bool {
    (1..=40).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn model_of(v: &Value) -> String {
    if let Some(m) = v.get("model").and_then(Value::as_str) {
        return m.to_string();
    }
    let mut best: Option<(u64, String)> = None;
    if let Some(ms) = v.get("models").and_then(Value::as_object) {
        for (k, x) in ms {
            let r = x.get("requests").and_then(Value::as_u64).unwrap_or(0);
            if best
                .as_ref()
                .is_none_or(|(br, bk)| r > *br || (r == *br && k < bk))
            {
                best = Some((r, k.clone()));
            }
        }
    }
    best.map(|b| b.1).unwrap_or_else(|| "unknown".into())
}

fn sha_matches(rec: &str, sha: &str) -> bool {
    !rec.is_empty() && (rec == sha || (rec.len() >= 7 && sha.starts_with(rec)))
}

/// Loads the eligible runs of `sha` from `<results>/<sha12>.jsonl` (missing file → none), applies the
/// window and returns them with the window object.
fn load(results: &Path, sha: &str) -> Result<(Vec<Rec>, Value), String> {
    let file = results.join(format!("{}.jsonl", &sha[..sha.len().min(12)]));
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{}: {e}", file.display())),
    };
    let mut recs = Vec::new();
    for (i, line) in text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let v: Value =
            serde_json::from_str(line).map_err(|e| format!("{}:{}: {e}", file.display(), i + 1))?;
        if !sha_matches(v.get("sha").and_then(Value::as_str).unwrap_or(""), sha) {
            continue;
        }
        let set = v.get("set").and_then(Value::as_str).unwrap_or("");
        let sealed = v.get("sealed").and_then(Value::as_bool);
        let eligible = match set {
            "pool" => sealed != Some(true),
            "capability" => sealed == Some(false),
            _ => false, // holdout, canary, anything else
        };
        if !eligible {
            continue;
        }
        let num = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        let ts = v
            .get("ts")
            .and_then(Value::as_str)
            .and_then(|s| parse_rfc3339(s).map(|ms| (ms, s.to_string())));
        recs.push(Rec {
            gate_id: v
                .get("gate_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            task: v
                .get("task")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            source: if v.get("source").and_then(Value::as_str) == Some("agent") {
                "agent"
            } else {
                "pool"
            },
            labels: v
                .get("labels")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .filter(|l| is_label(l))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            score: num("score_permille") / 1000.0,
            cost: num("cost_ref_musd"),
            wall: num("wall_ms"),
            model: model_of(&v),
            ts,
        });
    }
    let gates: BTreeSet<String> = recs.iter().map(|r| r.gate_id.clone()).collect();
    let keep: BTreeSet<String> = gates.iter().rev().take(MAX_GATES).cloned().collect();
    recs.retain(|r| keep.contains(&r.gate_id));
    if let Some(newest) = recs.iter().filter_map(|r| r.ts.as_ref().map(|t| t.0)).max() {
        recs.retain(|r| r.ts.as_ref().is_none_or(|t| t.0 >= newest - MAX_AGE_MS));
    }
    let from = recs
        .iter()
        .filter_map(|r| r.ts.clone())
        .min_by_key(|t| t.0)
        .map(|t| t.1);
    let to = recs
        .iter()
        .filter_map(|r| r.ts.clone())
        .max_by_key(|t| t.0)
        .map(|t| t.1);
    let ngates = recs
        .iter()
        .map(|r| &r.gate_id)
        .collect::<BTreeSet<_>>()
        .len();
    let window = json!({"from": from, "gates": ngates, "runs": recs.len(), "to": to});
    Ok((recs, window))
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

fn permille(x: f64) -> i64 {
    (x * 1000.0).round().clamp(0.0, 1000.0) as i64
}

type Key = (String, &'static str, String);

fn rows(recs: &[Rec], lkg: &str) -> BTreeMap<Key, Value> {
    let mut groups: BTreeMap<Key, Vec<&Rec>> = BTreeMap::new();
    for r in recs {
        let mut cats: BTreeSet<String> = r.labels.iter().cloned().collect();
        cats.insert("_all".into());
        for c in cats {
            groups
                .entry((c, r.source, r.model.clone()))
                .or_default()
                .push(r);
        }
    }
    let mut out = BTreeMap::new();
    for (key, runs) in groups {
        let mut by_task: BTreeMap<&str, Vec<&Rec>> = BTreeMap::new();
        for r in &runs {
            by_task.entry(r.task.as_str()).or_default().push(r);
        }
        let mean = |rs: &[&Rec], f: fn(&Rec) -> f64| {
            rs.iter().map(|r| f(r)).sum::<f64>() / rs.len() as f64
        };
        let s: Vec<f64> = by_task.values().map(|rs| mean(rs, |r| r.score)).collect();
        let pass = s.iter().sum::<f64>() / s.len() as f64;
        let (lo, hi) = if s.len() == 1 {
            (pass, pass)
        } else {
            interval(
                lkg,
                &format!("capability/{}/{}/{}", key.0, key.1, key.2),
                &s,
            )
        };
        let cost = median(by_task.values().map(|rs| mean(rs, |r| r.cost)).collect());
        let wall = median(by_task.values().map(|rs| mean(rs, |r| r.wall)).collect());
        let last = runs
            .iter()
            .filter_map(|r| r.ts.clone())
            .max_by_key(|t| t.0)
            .map(|t| t.1);
        let (pass_p, lo_p, hi_p) = (permille(pass), permille(lo), permille(hi));
        out.insert(
            key.clone(),
            json!({
                "category": key.0, "cost_ref_musd_p50": cost.round() as i64, "hi_permille": hi_p,
                "last_run": last, "lo_permille": lo_p, "low_n": s.len() < 3, "model": key.2,
                "pass_permille": pass_p, "prev_pass_permille": null, "runs": runs.len(), "source": key.1,
                "tasks": s.len(), "wall_ms_p50": wall.round() as i64,
            }),
        );
    }
    out
}

/// The `capability` object for `lkg` (with `prev_pass_permille` from `prev`, if given).
pub fn capability(results: &Path, lkg: &str, prev: Option<&str>) -> Result<Value, String> {
    let (recs, window) = load(results, lkg)?;
    let mut cur = rows(&recs, lkg);
    if let Some(p) = prev {
        let (precs, _) = load(results, p)?;
        let old = rows(&precs, p);
        for (k, row) in cur.iter_mut() {
            if let Some(o) = old.get(k) {
                row["prev_pass_permille"] = o["pass_permille"].clone();
            }
        }
    }
    Ok(json!({
        "categories": cur.into_values().collect::<Vec<_>>(),
        "lkg_sha": lkg,
        "method": "capability-v1",
        "window": window,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{put, remove_home, test_home};

    const LKG: &str = "0e786c733a9648164a2b4c4aea7ece24d0beb7c7";
    const PREV: &str = "1111111111111111111111111111111111111111";

    #[allow(clippy::too_many_arguments)]
    fn rec(
        gate: &str,
        task: &str,
        set: &str,
        source: &str,
        labels: &[&str],
        score: u32,
        model: &str,
        ts: &str,
    ) -> String {
        json!({"v": 2, "gate_id": gate, "run_id": format!("{gate}-{task}"), "arm": "p", "sha": LKG, "level": "L1",
               "task": task, "set": set, "source": source, "labels": labels, "repeat": 1, "outcome": "done",
               "score_permille": score, "wall_ms": 1000 + score, "cost_ref_musd": 20000, "ts": ts,
               "models": {model: {"cost_ref_musd": 20000, "requests": 3, "routed": 0}}})
        .to_string()
    }

    #[test]
    fn primitives_match_the_gate() {
        use kernel_gate::stats::rng::{SplitMix64 as GateRng, seed_u64 as gate_seed};
        assert_eq!(
            seed_u64(LKG, "capability/_all/pool/x"),
            gate_seed(LKG, "capability/_all/pool/x")
        );
        let (mut a, mut b) = (SplitMix64::new(7), GateRng::new(7));
        for n in 1..100 {
            assert_eq!(a.idx(n), b.idx(n));
        }
        // The interval equals the gate's percentile bootstrap at α = 0.025 on both sides.
        let s = [1.0, 0.0, 0.5, 1.0];
        let g = kernel_gate::stats::boot::bootstrap(LKG, "capability/x", &s, B, 0.025);
        assert_eq!(interval(LKG, "capability/x", &s), (g.lower, g.upper));
    }

    #[test]
    fn rfc3339() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("1970-01-02T00:00:00.5Z"), Some(86_400_500));
        assert_eq!(
            parse_rfc3339("2026-10-20T07:00:00+07:00"),
            parse_rfc3339("2026-10-20T00:00:00Z")
        );
        assert_eq!(parse_rfc3339("2000-03-01T00:00:00Z"), Some(951_868_800_000));
        assert_eq!(parse_rfc3339("nope"), None);
    }

    /// S-15: deterministic; pass/lo/hi of a 3-task fixture match hand-computed values.
    #[test]
    fn s15_determinism_and_hand_values() {
        let home = test_home("s15");
        let ts = "2026-11-19T22:10:00.000Z";
        let lines = [
            rec(
                "j-000001",
                "t1",
                "pool",
                "owner",
                &["refactor"],
                1000,
                "deepseek/deepseek-flash",
                ts,
            ),
            rec(
                "j-000001",
                "t1",
                "pool",
                "owner",
                &["refactor"],
                0,
                "deepseek/deepseek-flash",
                ts,
            ),
            rec(
                "j-000001",
                "t2",
                "pool",
                "owner",
                &["refactor"],
                1000,
                "deepseek/deepseek-flash",
                ts,
            ),
            rec(
                "j-000001",
                "t3",
                "pool",
                "owner",
                &["refactor"],
                0,
                "deepseek/deepseek-flash",
                ts,
            ),
        ];
        put(
            &home,
            &format!("{}.jsonl", &LKG[..12]),
            &(lines.join("\n") + "\n"),
        );
        let a = capability(&home, LKG, None).unwrap();
        let b = capability(&home, LKG, None).unwrap();
        assert_eq!(crate::icj::to_icj(&a), crate::icj::to_icj(&b));
        let rows = a["categories"].as_array().unwrap();
        assert_eq!(rows.len(), 2); // _all and refactor
        let r = &rows[1];
        assert_eq!(r["category"], "refactor");
        // S = [0.5, 1.0, 0.0]: pass = 500. Hand bootstrap with the §3.8.3 primitives:
        let s = [0.5, 1.0, 0.0];
        let mut rng = SplitMix64::new(seed_u64(
            LKG,
            "capability/refactor/pool/deepseek/deepseek-flash",
        ));
        let mut m: Vec<f64> = (0..2000)
            .map(|_| (0..3).map(|_| s[rng.idx(3)]).sum::<f64>() / 3.0)
            .collect();
        m.sort_by(|x, y| x.total_cmp(y));
        assert_eq!(r["pass_permille"], 500);
        assert_eq!(r["lo_permille"], (m[50] * 1000.0).round() as i64);
        assert_eq!(r["hi_permille"], (m[1949] * 1000.0).round() as i64);
        assert_eq!(r["lo_permille"], 0);
        assert_eq!(r["hi_permille"], 1000);
        assert_eq!(r["tasks"], 3);
        assert_eq!(r["runs"], 4);
        assert_eq!(r["low_n"], false);
        assert_eq!(r["wall_ms_p50"], 1500);
        assert_eq!(r["last_run"], ts);
        assert_eq!(
            a["window"],
            json!({"from": ts, "gates": 1, "runs": 4, "to": ts})
        );
        remove_home(&home);
    }

    /// S-16 and S-17: exclusions, sources, models, low_n, prev.
    #[test]
    fn s16_s17_exclusions_sources_models() {
        let home = test_home("s16");
        let ts = "2026-11-19T00:00:00Z";
        let mut lines = vec![
            rec(
                "j-000002",
                "o1",
                "pool",
                "owner",
                &["interactive"],
                1000,
                "deepseek/deepseek-flash",
                ts,
            ),
            rec(
                "j-000002",
                "a1",
                "pool",
                "agent",
                &["interactive"],
                0,
                "deepseek/deepseek-flash",
                ts,
            ),
            rec(
                "j-000002",
                "o1",
                "pool",
                "owner",
                &["interactive"],
                1000,
                "anthropic/claude-sonnet-5-5",
                ts,
            ),
            rec(
                "j-000002",
                "h1",
                "holdout",
                "owner",
                &["interactive"],
                1000,
                "deepseek/deepseek-flash",
                ts,
            ),
            rec(
                "j-000002",
                "c1",
                "canary",
                "owner",
                &["interactive"],
                1000,
                "deepseek/deepseek-flash",
                ts,
            ),
            rec(
                "j-000002",
                "s1",
                "capability",
                "agent",
                &["interactive"],
                1000,
                "deepseek/deepseek-flash",
                ts,
            ),
        ];
        let mut sealed: Value = serde_json::from_str(&rec(
            "j-000002",
            "s2",
            "pool",
            "agent",
            &["interactive"],
            1000,
            "deepseek/deepseek-flash",
            ts,
        ))
        .unwrap();
        sealed["sealed"] = json!(true);
        lines.push(sealed.to_string());
        let mut other_sha: Value = serde_json::from_str(&lines[0]).unwrap();
        other_sha["sha"] = json!(PREV);
        lines.push(other_sha.to_string());
        put(
            &home,
            &format!("{}.jsonl", &LKG[..12]),
            &(lines.join("\n") + "\n"),
        );
        let mut prev: Value = serde_json::from_str(&lines[0]).unwrap();
        prev["sha"] = json!(PREV);
        prev["score_permille"] = json!(250);
        put(
            &home,
            &format!("{}.jsonl", &PREV[..12]),
            &(prev.to_string() + "\n"),
        );
        let c = capability(&home, LKG, Some(PREV)).unwrap();
        let keys: Vec<(String, String, String, u64)> = c["categories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["category"].as_str().unwrap().into(),
                    r["source"].as_str().unwrap().into(),
                    r["model"].as_str().unwrap().into(),
                    r["tasks"].as_u64().unwrap(),
                )
            })
            .collect();
        let want = [
            ("_all", "agent", "deepseek/deepseek-flash", 1),
            ("_all", "pool", "anthropic/claude-sonnet-5-5", 1),
            ("_all", "pool", "deepseek/deepseek-flash", 1),
            ("interactive", "agent", "deepseek/deepseek-flash", 1),
            ("interactive", "pool", "anthropic/claude-sonnet-5-5", 1),
            ("interactive", "pool", "deepseek/deepseek-flash", 1),
        ];
        assert_eq!(keys.len(), want.len(), "{keys:?}");
        for (k, w) in keys.iter().zip(want) {
            assert_eq!((k.0.as_str(), k.1.as_str(), k.2.as_str(), k.3), w);
        }
        assert!(
            c["categories"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["low_n"] == true)
        );
        assert_eq!(
            c["window"]["runs"], 3,
            "holdout, canary, sealed and other-sha runs are excluded"
        );
        let pool_ds = &c["categories"][2];
        assert_eq!(pool_ds["pass_permille"], 1000);
        assert_eq!(pool_ds["prev_pass_permille"], 250);
        assert_eq!(c["categories"][0]["prev_pass_permille"], Value::Null);
        remove_home(&home);
    }

    #[test]
    fn window_keeps_last_20_gates_and_30_days() {
        let home = test_home("capwin");
        let mut lines = Vec::new();
        for g in 0..25 {
            lines.push(rec(
                &format!("j-{g:06}"),
                "t",
                "pool",
                "owner",
                &[],
                1000,
                "m/x",
                "2026-11-19T00:00:00Z",
            ));
        }
        lines.push(rec(
            "j-000024",
            "old",
            "pool",
            "owner",
            &[],
            0,
            "m/x",
            "2026-10-01T00:00:00Z",
        ));
        put(
            &home,
            &format!("{}.jsonl", &LKG[..12]),
            &(lines.join("\n") + "\n"),
        );
        let c = capability(&home, LKG, None).unwrap();
        assert_eq!(c["window"]["gates"], 20);
        assert_eq!(c["window"]["runs"], 20);
        assert_eq!(c["categories"][0]["pass_permille"], 1000);
        // No results at all: an empty model, window.gates = 0.
        let empty = capability(&home, "2222222222222222222222222222222222222222", None).unwrap();
        assert_eq!(empty["categories"], json!([]));
        assert_eq!(
            empty["window"],
            json!({"from": null, "gates": 0, "runs": 0, "to": null})
        );
        remove_home(&home);
    }
}
