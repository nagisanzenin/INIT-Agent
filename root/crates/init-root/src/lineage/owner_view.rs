//! TUI-OBSERVE: `lineage.tail` on owner.sock (tui.md §3.3, §3.6): the newest signed records as short
//! rows plus a check of the local chain, for the owner's client, which reads no file itself (T-2).
//! Read-only and bounded: at most [`MAX_ROWS`] rows of fixed scalar fields, strings cut to
//! [`FIELD_MAX`] bytes. The check is `init-root audit verify` without `--full` and without the
//! mirror: rules 1–6 over the whole chain with signatures over the last `root.lineage_verify_tail`
//! records, and rule 7 (`head.json`), under the key the Root signs with. Owner channel only: the
//! kernel's control link has no such request, and the zone cannot connect to owner.sock.

use super::verify::{chain, check_head};
use super::TAIL;
use crate::link::{Refusal, Root, lock};
use crate::util::{hex, rfc3339};
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::fs;

/// The most rows one answer carries; the default is [`DEFAULT_ROWS`].
pub const MAX_ROWS: u64 = 20;
const DEFAULT_ROWS: u64 = 10;
/// A string field of a row is cut to this many bytes (on a char boundary).
const FIELD_MAX: usize = 80;
/// The `data` fields a row may show: scalars that tell a kept change from a refused or rolled-back one.
const FIELDS: [&str; 12] = ["verdict", "reason", "sha", "from_sha", "to_sha", "from", "to", "lkg", "tuple", "gate_kind", "scope", "stage"];

fn cut(s: &str) -> String {
    let mut n = s.len().min(FIELD_MAX);
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    s[..n].to_string()
}

/// One record as a row: `seq`, `at`, `by`, `kind`, the first 12 hex of its hash, and [`FIELDS`].
pub fn row(r: &Value) -> Value {
    let mut data = Map::new();
    for k in FIELDS {
        let v = match &r["data"][k] {
            Value::String(s) => json!(cut(s)),
            v @ (Value::Bool(_) | Value::Number(_)) => v.clone(),
            _ => continue,
        };
        data.insert(k.into(), v);
    }
    let hash = r["hash"].as_str().and_then(|h| h.strip_prefix("sha256:")).map_or(String::new(), |h| cut(&h[..h.len().min(12)]));
    json!({"at": r["at"], "by": cut(r["by"].as_str().unwrap_or("")), "data": data, "hash": hash, "kind": cut(r["kind"].as_str().unwrap_or("")), "seq": r["seq"]})
}

/// `lineage.tail{n?}` → `{records, rows, verify}`: the last `n` (1–20, default 10) records, oldest
/// first, and `verify{ok, records, head_seq, head_hash, sigs_checked, verified_full_at, key, ms, error}`.
pub fn tail(root: &Root, args: &Value) -> Result<Value, Refusal> {
    let n = args["n"].as_u64().unwrap_or(DEFAULT_ROWS).clamp(1, MAX_ROWS) as usize;
    // Under the lineage lock: no append is half written while the file and head.json are read.
    let (bytes, hj, vk, full_at) = {
        let lg = lock(&root.lin);
        let l = lg.as_ref().ok_or(("root.mode", "the lineage is unavailable".to_string()))?;
        let bytes = fs::read(&l.file).map_err(|e| ("root.io", format!("lineage: {e}")))?;
        let hj: Option<Value> = fs::read_to_string(l.file.with_file_name("head.json")).ok().and_then(|s| serde_json::from_str(&s).ok());
        (bytes, hj, l.sk.verifying_key(), l.verified_full_at())
    };
    let tail_n = lock(&root.st).charter.as_ref().map_or(TAIL, |c| c.ch.root.lineage_verify_tail.max(1) as u64);
    let t0 = std::time::Instant::now();
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let records = lines.len() as u64;
    let from = records.saturating_sub(tail_n);
    let checked = match chain(&text, Some(&vk), from, &HashSet::new()) {
        Err((s, r)) => Err((s, r)),
        Ok(c) => match check_head(hj.as_ref(), c.head.as_ref()) {
            Ok(_) => Ok(c.head),
            Err(r) => Err((c.head.as_ref().map_or(0, |h| h.seq), r)),
        },
    };
    let ms = t0.elapsed().as_millis() as u64;
    let (ok, head, error) = match checked {
        Ok(h) => (true, h, Value::Null),
        Err((s, r)) => (false, None, json!({"rule": r, "seq": s})),
    };
    let rows: Vec<Value> = lines[lines.len().saturating_sub(n)..].iter().map(|l| serde_json::from_str::<Value>(l).map_or_else(|_| json!({"kind": "?"}), |r| row(&r))).collect();
    let verify = json!({
        "error": error, "head_hash": head.as_ref().map(|h| h.hash.clone()), "head_seq": head.as_ref().map(|h| h.seq),
        "key": hex(&vk.to_bytes())[..16].to_string(), "ms": ms, "ok": ok, "records": records,
        "sigs_checked": records - from + u64::from(from > 0), "verified_full_at": full_at.map(rfc3339),
    });
    Ok(json!({"records": records, "rows": rows, "verify": verify}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_keeps_fixed_scalars_and_cuts_strings() {
        let r = json!({"at": "2026-10-27T10:00:00.000Z", "by": "kernel:3f9c0a1b2c4d", "data": {"verdict": "reject", "reason": "x".repeat(300),
            "sha": "06be87a1234567890abc", "lkg": false, "stages": ["G0"], "secret": "not shown", "n": 3, "scope": {"x": 1}},
            "hash": "sha256:a1b2c3d4e5f60718293a4b5c6d7e8f90", "kind": "gate", "prev": "p", "seq": 88, "sig": "ed25519:00", "v": 1});
        let v = row(&r);
        assert_eq!(v["seq"], 88);
        assert_eq!(v["hash"], "a1b2c3d4e5f6");
        assert_eq!((v["kind"].as_str(), v["by"].as_str()), (Some("gate"), Some("kernel:3f9c0a1b2c4d")));
        assert_eq!(v["data"]["reason"].as_str().map(str::len), Some(FIELD_MAX));
        assert_eq!(v["data"]["lkg"], false);
        for gone in ["stages", "secret", "n", "scope"] {
            assert!(v["data"].get(gone).is_none(), "{gone}");
        }
        assert_eq!(cut("ééé"), "ééé");
        assert_eq!(cut(&"é".repeat(60)).len(), 80);
    }

    /// The answer on a real Root: the newest rows oldest first, bounded, and a verification that
    /// turns to ✕ (with the seq and rule) once a record in the file is altered. Test homes need the
    /// 80-byte ROOT_HOME of test-fixtures builds.
    #[cfg(feature = "test-fixtures")]
    #[test]
    fn tail_rows_and_verify_on_a_real_root() {
        let (dir, root) = crate::grant::tests::test_home("lt", "");
        root.append("lkg", json!({"tuple": "tA"}));
        root.append("rollback", json!({"from": "tB", "reason": "harm", "target": "parent", "to": "tA"}));
        let a = tail(&root, &json!({"n": 20})).unwrap();
        let rows = a["rows"].as_array().unwrap();
        assert!(rows.len() >= 2 && rows.len() <= 20);
        let find = |k: &str| rows.iter().find(|r| r["kind"] == k).cloned().unwrap_or_else(|| panic!("no {k} row in {a}"));
        let (lkg, rb) = (find("lkg"), find("rollback"));
        assert!(lkg["seq"].as_u64() < rb["seq"].as_u64(), "oldest first");
        assert_eq!(rb["data"], json!({"from": "tB", "reason": "harm", "to": "tA"}));
        assert_eq!(rows.last().unwrap()["seq"].as_u64(), a["verify"]["head_seq"].as_u64());
        assert_eq!(a["verify"]["ok"], true, "{a}");
        assert_eq!(a["verify"]["error"], Value::Null);
        assert_eq!(a["verify"]["records"], a["records"]);
        assert_eq!(a["verify"]["key"].as_str().map(str::len), Some(16));
        assert_eq!(tail(&root, &json!({"n": 500})).unwrap()["rows"].as_array().map(Vec::len), Some(a["records"].as_u64().unwrap().min(MAX_ROWS) as usize));
        // Alter the lkg record's data in place: rule 4 (the hash) fails at its seq.
        let file = root.p.r("lineage/lineage.jsonl");
        let text = fs::read_to_string(&file).unwrap().replacen("\"tuple\":\"tA\"", "\"tuple\":\"tX\"", 1);
        fs::write(&file, text).unwrap();
        let b = tail(&root, &json!({})).unwrap();
        assert_eq!(b["verify"]["ok"], false);
        assert_eq!(b["verify"]["error"]["seq"], lkg["seq"]);
        assert!(b["verify"]["error"]["rule"].as_str().unwrap().starts_with("4:"), "{b}");
        drop(root);
        let _ = fs::remove_dir_all(&dir); // exact path, created by test_home
    }
}
