//! The drift check (selfmap.md §3.8 items 1–5, v03-decisions D12), used by `--check` and the kernel
//! tree's `--kernel --check`.
//!
//! The semantics are exactly those of `kernel-gate`'s `g1::compare_self_json` (WP8), so the agent's
//! `--check` and the gate's G1 never disagree; a test runs both on the same cases and requires equal
//! codes, warnings and report bytes. Codes here are bare (`self_json_drift`); the gate prefixes them with
//! `gate.g1.` (body) or `gate.k0.` (kernel tree, where `self_json_drift` reads `kernel_self_drift`).
//!
//! 1. Presence: committed file missing → `self_json_missing`.
//! 2. `generator.version` or `self_v` differs → `generator_changed`.
//! 3. `live` is dropped from both sides before comparing; a committed `live` → warning `live_ignored`.
//! 4. Equality as `serde_json::Value`; equal values with different bytes → warning
//!    `non_canonical_format`.
//! 5. Otherwise `self_json_drift` with a report ≤ 4 KiB: changed files first, then every differing leaf
//!    as an RFC 6901 pointer with both values cut at 120 bytes.

use std::collections::BTreeMap;

use serde_json::Value;

/// Max size of the drift report.
pub const REPORT_MAX: usize = 4096;
/// Values in the report are cut at this many bytes.
pub const VALUE_MAX: usize = 120;

/// A failed comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// `self_json_missing`, `generator_changed`, `self_json_drift` or `selfmap_error`.
    pub code: &'static str,
    /// Agent-visible text, ≤ 4 KiB.
    pub detail: String,
}

/// A passing comparison's observations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Warning {
    LiveIgnored,
    NonCanonicalFormat,
}

impl Warning {
    pub fn code(self) -> &'static str {
        match self {
            Warning::LiveIgnored => "live_ignored",
            Warning::NonCanonicalFormat => "non_canonical_format",
        }
    }
}

fn failure(code: &'static str, detail: impl Into<String>) -> Failure {
    Failure {
        code,
        detail: cap(detail.into(), REPORT_MAX),
    }
}

fn cap(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut cut = max.saturating_sub(4);
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str(" …");
    }
    s
}

fn cut_value(v: Option<&Value>) -> String {
    match v {
        None => "(absent)".to_string(),
        Some(v) => cap(v.to_string(), VALUE_MAX),
    }
}

fn escape_token(t: &str) -> String {
    t.replace('~', "~0").replace('/', "~1")
}

fn diff_leaves(
    ptr: &str,
    a: Option<&Value>,
    b: Option<&Value>,
    out: &mut Vec<(String, String, String)>,
) {
    match (a, b) {
        (Some(Value::Object(x)), Some(Value::Object(y))) => {
            let mut keys: Vec<&String> = x.keys().chain(y.keys()).collect();
            keys.sort();
            keys.dedup();
            for k in keys {
                diff_leaves(
                    &format!("{ptr}/{}", escape_token(k)),
                    x.get(k),
                    y.get(k),
                    out,
                );
            }
        }
        (Some(Value::Array(x)), Some(Value::Array(y))) => {
            for i in 0..x.len().max(y.len()) {
                diff_leaves(&format!("{ptr}/{i}"), x.get(i), y.get(i), out);
            }
        }
        (a, b) if a == b => {}
        (a, b) => out.push((
            if ptr.is_empty() {
                "/".to_string()
            } else {
                ptr.to_string()
            },
            cut_value(a),
            cut_value(b),
        )),
    }
}

fn file_hashes(v: &Value) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    // kernel-self.json nests its inventory under `kernel`.
    let files = v
        .get("files")
        .or_else(|| v.get("kernel").and_then(|k| k.get("files")))
        .and_then(Value::as_array);
    if let Some(files) = files {
        for f in files {
            if let (Some(p), Some(h)) = (
                f.get("path").and_then(Value::as_str),
                f.get("sha256").and_then(Value::as_str),
            ) {
                m.insert(p.to_string(), h.to_string());
            }
        }
    }
    m
}

/// The `self_json_drift` report.
pub fn drift_report(committed: &Value, regenerated: &Value) -> String {
    let (fc, fr) = (file_hashes(committed), file_hashes(regenerated));
    let mut paths: Vec<&String> = fc.keys().chain(fr.keys()).collect();
    paths.sort();
    paths.dedup();
    let mut s = String::new();
    let changed: Vec<String> = paths
        .into_iter()
        .filter_map(|p| match (fc.get(p), fr.get(p)) {
            (Some(a), Some(b)) if a != b => Some(format!("  changed: {p}")),
            (Some(_), None) => Some(format!("  removed: {p}")),
            (None, Some(_)) => Some(format!("  added:   {p}")),
            _ => None,
        })
        .collect();
    if !changed.is_empty() {
        s.push_str("files whose sha256 differs (regenerate with `init-kernel selfmap --write` and commit):\n");
        for line in changed {
            s.push_str(&line);
            s.push('\n');
        }
    }
    let mut leaves = Vec::new();
    diff_leaves("", Some(committed), Some(regenerated), &mut leaves);
    leaves.sort();
    if !leaves.is_empty() {
        s.push_str("differing fields (committed → regenerated):\n");
        for (p, a, b) in leaves {
            s.push_str(&format!("  {p}: {a} → {b}\n"));
        }
    }
    cap(s, REPORT_MAX)
}

/// Compares the committed file (`None` if absent) with the regenerated one.
pub fn compare(committed: Option<&[u8]>, regenerated: &[u8]) -> Result<Vec<Warning>, Failure> {
    let Some(committed_bytes) = committed else {
        return Err(failure(
            "self_json_missing",
            "self.json is not committed; run `init-kernel selfmap --write` and commit it",
        ));
    };
    let mut r: Value = serde_json::from_slice(regenerated).map_err(|e| {
        failure(
            "selfmap_error",
            format!("regenerated self.json does not parse: {e}"),
        )
    })?;
    let mut c: Value = match serde_json::from_slice(committed_bytes) {
        Ok(v) => v,
        Err(e) => {
            return Err(failure(
                "self_json_drift",
                format!(
                    "committed self.json is not valid JSON ({e}); regenerate with `init-kernel selfmap --write`"
                ),
            ));
        }
    };
    let gen_version = |v: &Value| v.get("generator").and_then(|g| g.get("version")).cloned();
    if gen_version(&c) != gen_version(&r) || c.get("self_v") != r.get("self_v") {
        return Err(failure(
            "generator_changed",
            format!(
                "self.json was written by generator {} (self_v {}), the kernel runs {} (self_v {}); run `init-kernel selfmap --write` and commit",
                cut_value(gen_version(&c).as_ref()),
                cut_value(c.get("self_v")),
                cut_value(gen_version(&r).as_ref()),
                cut_value(r.get("self_v")),
            ),
        ));
    }
    let mut warnings = Vec::new();
    let had_live = c.as_object_mut().and_then(|o| o.remove("live")).is_some();
    if let Some(o) = r.as_object_mut() {
        o.remove("live");
    }
    if had_live {
        warnings.push(Warning::LiveIgnored);
    }
    if c == r {
        if !had_live && committed_bytes != regenerated {
            warnings.push(Warning::NonCanonicalFormat);
        }
        return Ok(warnings);
    }
    Err(failure("self_json_drift", drift_report(&c, &r)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        json!({
            "body_tokens": 21873,
            "components": [
                {"compensates_for": "none", "id": "mod/src/context.rs", "kind": "mod", "tokens": 3410},
                {"compensates_for": "none", "id": "tool/bash", "kind": "tool", "tokens": 1288}
            ],
            "files": [
                {"bytes": 10, "counted": true, "path": "src/context.rs", "sha256": "aa", "tokens": 3},
                {"bytes": 12, "counted": true, "path": "src/loop.rs", "sha256": "bb", "tokens": 4}
            ],
            "generator": {"name": "init-kernel selfmap", "tokenizer": "o200k_base", "version": 2},
            "self_v": 2
        })
    }

    fn b(v: &Value) -> Vec<u8> {
        serde_json::to_vec(v).unwrap()
    }

    /// The cases on which this module and the gate's G1 must agree byte for byte.
    fn cases() -> Vec<(Option<Vec<u8>>, Vec<u8>)> {
        let r = sample();
        let mut v: Vec<(Option<Vec<u8>>, Vec<u8>)> = vec![
            (Some(b(&r)), b(&r)),
            (Some(serde_json::to_vec_pretty(&r).unwrap()), b(&r)),
            (None, b(&r)),
            (Some(b"{not json".to_vec()), b(&r)),
            (Some(b(&r)), b"{bad".to_vec()),
        ];
        let mut c = sample();
        c["generator"]["version"] = json!(1);
        v.push((Some(b(&c)), b(&r)));
        let mut c = sample();
        c["self_v"] = json!(1);
        v.push((Some(b(&c)), b(&r)));
        let mut c = sample();
        c["live"] = json!({"capability": {"categories": [{"pass_permille": 710}]}});
        v.push((Some(b(&c)), b(&r)));
        let mut c2 = c.clone();
        c2["body_tokens"] = json!(1);
        v.push((Some(b(&c2)), b(&r)));
        let mut c = sample();
        c["files"][1]["sha256"] = json!("old");
        c["components"][0]["tokens"] = json!(3000);
        c["extra/key"] = json!("x~y");
        c["files"]
            .as_array_mut()
            .unwrap()
            .push(json!({"path": "gone.rs", "sha256": "cc"}));
        v.push((Some(b(&c)), b(&r)));
        let mut c = sample();
        c["big"] = json!(
            (0..2000)
                .map(|i| format!("v{i}-{}", "é".repeat(90)))
                .collect::<Vec<_>>()
        );
        v.push((Some(b(&c)), b(&r)));
        v
    }

    /// The drift semantics equal `kernel-gate` G1's on every case: same code (modulo the `gate.g1.`
    /// prefix), same warnings, same report bytes.
    #[test]
    fn matches_kernel_gate_g1() {
        use kernel_gate::g1::{G1Warning, compare_self_json};
        for (i, (c, r)) in cases().into_iter().enumerate() {
            let ours = compare(c.as_deref(), &r);
            let theirs = compare_self_json(c.as_deref(), &r);
            match (ours, theirs) {
                (Ok(a), Ok(b)) => {
                    let b: Vec<&str> = b
                        .iter()
                        .map(|w| match w {
                            G1Warning::LiveIgnored => "live_ignored",
                            G1Warning::NonCanonicalFormat => "non_canonical_format",
                        })
                        .collect();
                    let a: Vec<&str> = a.iter().map(|w| w.code()).collect();
                    assert_eq!(a, b, "case {i}");
                }
                (Err(a), Err(b)) => {
                    assert_eq!(format!("gate.g1.{}", a.code), b.code, "case {i}");
                    assert_eq!(a.detail, b.detail, "case {i}");
                }
                (a, b) => panic!("case {i}: selfmap {a:?} vs gate {b:?}"),
            }
        }
    }

    /// S-03 and S-04 and S-13 at the unit level.
    #[test]
    fn s03_s04_s13() {
        let r = b(&sample());
        assert_eq!(
            compare(Some(&serde_json::to_vec_pretty(&sample()).unwrap()), &r),
            Ok(vec![Warning::NonCanonicalFormat])
        );
        let mut c = sample();
        c["generator"]["version"] = json!(1);
        assert_eq!(
            compare(Some(&b(&c)), &r).unwrap_err().code,
            "generator_changed"
        );
        let mut c = sample();
        c["live"] = json!({"capability": {"categories": [{"pass_permille": 710}]}, "level": {"current": "L1"}});
        assert_eq!(compare(Some(&b(&c)), &r), Ok(vec![Warning::LiveIgnored]));
        c["live"]["capability"]["categories"][0]["pass_permille"] = json!(720);
        assert_eq!(compare(Some(&b(&c)), &r), Ok(vec![Warning::LiveIgnored]));
    }

    #[test]
    fn kernel_tree_files_are_named_first() {
        let r = json!({"generator": {"version": 2}, "self_v": 2, "kernel": {"files": [{"path": "a.rs", "sha256": "1"}]}});
        let mut c = r.clone();
        c["kernel"]["files"][0]["sha256"] = json!("0");
        let e = compare(Some(&b(&c)), &b(&r)).unwrap_err();
        assert!(
            e.detail.starts_with("files whose sha256 differs"),
            "{}",
            e.detail
        );
        assert!(e.detail.contains("changed: a.rs"));
    }
}
