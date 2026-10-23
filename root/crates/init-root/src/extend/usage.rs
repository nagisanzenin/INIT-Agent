//! ADR-011 §5.2: the usage ledger. One line per closed turn in `ROOT_HOME/extend/usage/<UTC date>.jsonl` (0600,
//! never mirrored, 90 days), the per-owner-day counters folded from those lines, and the daily `cap_use` digest.
//! A line holds cap ids, counts and turn figures only, never request or argument text.

use crate::sentinel::Turn;
use crate::util::rfc3339;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// §5.2: at most 256 caps in a day's digest; the most recently used are kept.
const DIGEST_MAX: usize = 256;
/// A lineage record's data is at most 16 KiB (root.md §3.10.1): a digest is split into parts below that.
const PART_BYTES: usize = 15 * 1024;
const KEEP_DAYS: i64 = 90;
const DAY_MS: i64 = 86_400_000;

/// One cap's counters over an owner day.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ctr {
    pub turns: u64,
    pub uses: u64,
    pub harm_turns: u64,
    pub last_turn_seq: u64,
    pub root: bool,
}

pub type Day = BTreeMap<String, Ctr>;

/// The usage line of a closed turn (§5.2): turn id, kind, caps with counts and evidence, harm flags, cost, open groups.
pub fn line(t: &Turn, day: &str) -> Value {
    let caps: Vec<Value> = t.caps.iter().filter(|(id, _)| super::valid_id(id))
        .map(|(id, (n, ev))| json!({"evidence": if ev == "root" { "root" } else { "zone" }, "id": id, "uses": n})).collect();
    let harm: Vec<&str> = [("correction", t.correction), ("crash", t.crash), ("kill", t.kill), ("provider_error", t.provider_errors > 0),
        ("capped", t.capped), ("refusal", t.refusal)].into_iter().filter(|x| x.1).map(|x| x.0).collect();
    json!({"caps": caps, "cost_musd": t.cost_musd, "day": day, "end": rfc3339(t.end), "groups": t.groups, "harm": harm,
        "harmed": t.harmed(), "kind": t.kind, "req_bytes": t.req_bytes, "requests": t.requests, "start": rfc3339(t.start),
        "turn": t.seq, "v": 1, "wall_ms": t.wall_ms})
}

/// Folds one usage line into its day's counters.
pub fn add(day: &mut Day, l: &Value) {
    let (seq, harmed) = (l["turn"].as_u64().unwrap_or(0), l["harmed"].as_bool().unwrap_or(false));
    for c in l["caps"].as_array().into_iter().flatten() {
        let Some(id) = c["id"].as_str().filter(|i| super::valid_id(i)) else { continue };
        let e = day.entry(id.to_string()).or_default();
        e.turns += 1;
        e.uses += c["uses"].as_u64().unwrap_or(0);
        e.harm_turns += u64::from(harmed);
        e.last_turn_seq = e.last_turn_seq.max(seq);
        e.root |= c["evidence"] == "root";
    }
}

/// At the first turn after a start: the lines of every day without a digest, from the files still kept.
pub fn load(dir: &Path, digested: &BTreeSet<String>, pending: &mut BTreeMap<String, Day>) {
    let mut files: Vec<_> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl")).collect();
    files.sort();
    for f in files {
        let Ok(s) = std::fs::read_to_string(&f) else { continue };
        for l in s.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()) {
            if let Some(d) = l["day"].as_str().filter(|d| !digested.contains(*d)) {
                add(pending.entry(d.to_string()).or_default(), &l);
            }
        }
    }
}

/// The `cap_use` records of a finished owner day (§5.2): `{day, part, caps:[{id, turns, uses, harm_turns,
/// last_turn_seq, evidence}]}`, at most 256 caps (the most recently used), sorted by id, split so each record's
/// data stays under the lineage's 16 KiB.
pub fn digest(day: &str, c: &Day) -> Vec<Value> {
    let mut v: Vec<(&String, &Ctr)> = c.iter().collect();
    v.sort_by(|a, b| b.1.last_turn_seq.cmp(&a.1.last_turn_seq).then(b.1.uses.cmp(&a.1.uses)).then(a.0.cmp(b.0)));
    v.truncate(DIGEST_MAX);
    v.sort_by(|a, b| a.0.cmp(b.0));
    let (mut out, mut cur, mut bytes) = (Vec::new(), Vec::new(), 0);
    for (id, k) in v {
        let e = json!({"evidence": if k.root { "root" } else { "zone" }, "harm_turns": k.harm_turns, "id": id,
            "last_turn_seq": k.last_turn_seq, "turns": k.turns, "uses": k.uses});
        let n = e.to_string().len() + 1;
        if bytes + n > PART_BYTES && !cur.is_empty() {
            out.push(json!({"caps": std::mem::take(&mut cur), "day": day, "part": out.len()}));
            bytes = 0;
        }
        bytes += n;
        cur.push(e);
    }
    if !cur.is_empty() {
        out.push(json!({"caps": cur, "day": day, "part": out.len()}));
    }
    out
}

/// A refused digest's counters back into its day.
pub fn readd(day: &mut Day, d: &Value) {
    for c in d["caps"].as_array().into_iter().flatten() {
        let Some(id) = c["id"].as_str() else { continue };
        let e = day.entry(id.to_string()).or_default();
        let n = |k: &str| c[k].as_u64().unwrap_or(0);
        (e.turns, e.uses, e.harm_turns) = (e.turns + n("turns"), e.uses + n("uses"), e.harm_turns + n("harm_turns"));
        e.last_turn_seq = e.last_turn_seq.max(n("last_turn_seq"));
        e.root |= c["evidence"] == "root";
    }
}

/// 90-day retention: usage files dated before `now − 90 days` are removed, each by its exact path.
pub fn sweep(dir: &Path, now: i64) {
    let cut = rfc3339(now - KEEP_DAYS * DAY_MS)[..10].to_string();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let dated = name.len() == 16 && name.ends_with(".jsonl") && name[..10].bytes().all(|c| c.is_ascii_digit() || c == b'-');
        if dated && name[..10] < *cut && e.file_type().is_ok_and(|t| t.is_file()) {
            let _ = std::fs::remove_file(dir.join(&name));
        }
    }
}
