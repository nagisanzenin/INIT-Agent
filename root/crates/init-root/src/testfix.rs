//! Test support (root/root.md §3.16), compiled only with the cargo feature `test-fixtures` and
//! honoured only when the charter has `root.test_mode = true`: the fake clock and test ledgers.
//! Not counted against the size budget (§4.8).

use crate::install::{self, VERSION};
use crate::link::{Refusal, Root, lock};
use crate::lineage::{self, Lineage};
use crate::util::{self, hex, now_ms, parse_rfc3339, rfc3339};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Mutex;

/// The fake clock: frozen at its value, moved only forward, by `test.clock` or `INIT_ROOT_TEST_NOW`.
static FAKE_NOW: Mutex<Option<i64>> = Mutex::new(None);

pub fn fake_now() -> Option<i64> {
    *FAKE_NOW.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn set_now(ms: i64) -> Result<i64, String> {
    let mut f = FAKE_NOW.lock().unwrap_or_else(|e| e.into_inner());
    if f.is_some_and(|cur| ms < cur) {
        return Err(format!("the fake clock never moves back (now {})", rfc3339(f.unwrap_or(0))));
    }
    *f = Some(ms);
    Ok(ms)
}

/// Owner-socket `test.clock{advance_ms}` or `{set}`.
pub fn clock(root: &Root, args: &Value) -> Result<Value, Refusal> {
    if !lock(&root.st).charter.as_ref().is_some_and(|l| l.ch.root.test_mode) {
        return Err(("root.mode", "test.clock needs root.test_mode = true".into()));
    }
    let t = match (args["advance_ms"].as_i64(), args["set"].as_str()) {
        (Some(a), _) if a >= 0 => now_ms() + a,
        (None, Some(s)) => parse_rfc3339(s).ok_or(("root.args", format!("bad RFC 3339 time {s:?}")))?,
        _ => return Err(("root.args", "pass advance_ms ≥ 0 or set".into())),
    };
    let t = set_now(t).map_err(|m| ("root.args", m))?;
    crate::ledger::tick(root); // R7's day-based rules follow the clock at once
    Ok(json!({"now": rfc3339(t)}))
}

/// Owner-socket `test.append{records: [{kind, data, by?}]}`: records appended one after another through the
/// Root's own path (`append_by`, then R7's evaluation), e.g. the `audit` and `mutation` records that
/// root/sealed-eval.md (WP24) will write, or a burst of demoting `ceiling_hit`s.
pub fn append(root: &Root, args: &Value) -> Result<Value, Refusal> {
    if !lock(&root.st).charter.as_ref().is_some_and(|l| l.ch.root.test_mode) {
        return Err(("root.mode", "test.append needs root.test_mode = true".into()));
    }
    let mut seqs = Vec::new();
    for r in args["records"].as_array().into_iter().flatten() {
        let rec = root.append_by(r["by"].as_str().unwrap_or("root"), r["kind"].as_str().unwrap_or(""), r["data"].clone());
        seqs.push(rec.map_err(|e| ("root.lineage", e.to_string()))?["seq"].clone());
    }
    Ok(json!({"seqs": seqs}))
}

/// Kinds the kernel reports (root.md §3.10.5); test ledgers sign them `by:"kernel:000000000000"`.
const KERNEL_KINDS: [&str; 17] = ["promotion", "gate", "task_registered", "task_admit", "task_suspended", "owner",
    "body_rollback", "body_safe", "fleet_spawn", "fleet_stop", "qualified", "adopt", "fleet_campaign", "value_window",
    "kernel_candidate", "kernel_main", "accept"];

fn subst(v: &Value, i: u64) -> Value {
    match v {
        Value::String(s) => Value::String(s.replace("{i}", &i.to_string())),
        Value::Array(a) => Value::Array(a.iter().map(|x| subst(x, i)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), subst(x, i))).collect()),
        x => x.clone(),
    }
}

/// `init-root test ledger --root-home <dir> --spec <file.json>` (§3.16): a signed R4 chain written
/// into a test ROOT_HOME before the Root starts, on a throwaway key, with `genesis{…, test:true}`.
///
/// Spec: `{"start_at": RFC 3339, "start_level": 0..=4, "records": [{"at", "kind", "data", "by"?,
/// "count"?, "step_s"?}]}`. A record with `count` n repeats n times, `step_s` apart, with `{i}`
/// replaced by 0..n in its strings. The start level is reached by `ladder` records whose
/// `trigger.kind` is `test_fixture`; the records follow in time order.
pub fn test_ledger(rh: &Path, spec_file: &Path) -> Result<String, (u8, String)> {
    let refuse = |m: String| (2u8, m);
    let src = std::fs::read_to_string(rh.join("charter.toml")).map_err(|e| refuse(format!("{}/charter.toml: {e}", rh.display())))?;
    let t: toml::Table = toml::from_str(&src).map_err(|e| refuse(format!("charter.toml: {e}")))?;
    if t.get("root").and_then(|r| r.get("test_mode")).and_then(toml::Value::as_bool) != Some(true) {
        return Err(refuse("refused: the home's charter must have root.test_mode = true".into()));
    }
    if std::fs::metadata(rh.join("lineage/lineage.jsonl")).is_ok_and(|m| m.len() > 0) {
        return Err(refuse("refused: the home's lineage is not empty".into()));
    }
    let spec: Value = std::fs::read_to_string(spec_file).ok().and_then(|s| serde_json::from_str(&s).ok())
        .ok_or_else(|| refuse(format!("{}: not a JSON spec", spec_file.display())))?;
    let start = spec["start_at"].as_str().and_then(parse_rfc3339).ok_or_else(|| refuse("spec.start_at must be RFC 3339".into()))?;
    let level = spec["start_level"].as_i64().unwrap_or(0);
    if !(0..=4).contains(&level) {
        return Err(refuse("spec.start_level must be 0–4".into()));
    }
    let mut recs = Vec::new();
    for r in spec["records"].as_array().into_iter().flatten() {
        let (kind, at) = (r["kind"].as_str().unwrap_or(""), r["at"].as_str().and_then(parse_rfc3339));
        let at = at.filter(|a| *a >= start).ok_or_else(|| refuse(format!("record {kind:?}: at must be RFC 3339 and ≥ start_at")))?;
        if kind.is_empty() || !r["data"].is_object() {
            return Err(refuse("each record needs a kind and a data object".into()));
        }
        let by = r["by"].as_str().map(String::from).unwrap_or_else(|| {
            if KERNEL_KINDS.contains(&kind) || kind.starts_with("x_") { "kernel:000000000000".into() } else { "root".into() }
        });
        for i in 0..r["count"].as_u64().unwrap_or(1) {
            let step = r["step_s"].as_i64().unwrap_or(0) * 1000;
            recs.push((at + step * i as i64, by.clone(), kind.to_string(), subst(&r["data"], i)));
        }
    }
    recs.sort_by_key(|r| r.0);
    let io = |e: std::io::Error| (1u8, e.to_string());
    install::make_dirs(rh, &install::RH_DIRS).map_err(io)?;
    let vk = lineage::ensure_key(rh).map_err(io)?;
    let mut lin = Lineage::open(rh, None).map_err(io)?;
    let id = hex(&util::random::<8>().map_err(io)?);
    lin.append_at(start, "root", "genesis", json!({"install_id": id, "pubkey": hex(&vk.to_bytes()), "root_version": VERSION, "test": true})).map_err(io)?;
    for l in 0..level {
        let up = json!({"cause": "unlock", "criteria": null, "direction": "up", "from": l, "to": l + 1, "trigger": {"kind": "test_fixture", "seq": 0}});
        lin.append_at(start, "root", "ladder", up).map_err(io)?;
    }
    let n = recs.len();
    for (at, by, kind, data) in recs {
        lin.append_at(at, &by, &kind, data).map_err(io)?;
    }
    Ok(format!("test ledger: {} records, level L{level}, head seq {}", n + 1 + level as usize, lin.head.map_or(0, |h| h.seq)))
}

#[cfg(test)]
mod tests {
    #[test]
    fn fake_clock_moves_only_forward() {
        super::set_now(1_000).unwrap();
        assert_eq!(crate::util::now_ms(), 1_000);
        assert!(super::set_now(999).is_err());
        super::set_now(5_000).unwrap();
        assert_eq!(super::fake_now(), Some(5_000));
    }
}
