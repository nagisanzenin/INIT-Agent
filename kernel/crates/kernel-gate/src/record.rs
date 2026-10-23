//! The gate record (gate.md §3.13), its agent-safe Root-lineage subset, the redacted
//! `refs/notes/gate` note, the agent report (`state/kernel/gate/<job>/report.json`, §4.2) and the
//! stage notices (§3.13 "Events to the body").
//!
//! The full record is built by the job driver as JSON (one `stages.<G>` object per stage). The
//! functions here derive every outward copy from it, so redaction is applied in one place.

use std::path::Path;
use std::process::Command;

use serde_json::{Map, Value, json};

/// The record version.
pub const RECORD_V: u64 = 2;
/// The free path's record version (ADR-011 Records: `accept:"free"`, stages `A0`–`A5`).
pub const RECORD_V_FREE: u64 = 5;
/// Agent-visible failure text cap.
pub const REPORT_TEXT_MAX: usize = 4096;

/// A fraction as permille, rounded half away from zero.
pub fn permille(x: f64) -> i64 {
    if x.is_finite() {
        (x * 1000.0).round() as i64
    } else {
        0
    }
}

/// Replace every float with its rounded integer (records are ICJ: integers and strings only).
pub fn integers_only(v: Value) -> Value {
    match v {
        Value::Number(n) if n.is_f64() => {
            let f = n.as_f64().unwrap_or(0.0);
            json!(permille(f))
        }
        Value::Array(a) => Value::Array(a.into_iter().map(integers_only).collect()),
        Value::Object(m) => {
            Value::Object(m.into_iter().map(|(k, x)| (k, integers_only(x))).collect())
        }
        other => other,
    }
}

/// Cut a string to `max` bytes on a char boundary.
pub fn cap(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut cut = max.saturating_sub(4);
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{} …", &s[..cut])
}

/// Names of the stages that passed, in order.
pub fn stages_passed(full: &Value) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for s in ["G0", "G1", "G2", "G3", "G4", "A0", "A1", "A2", "A3", "A4", "A5"] {
        let st = &full["stages"][s];
        let pass = st.get("pass").and_then(Value::as_bool) == Some(true)
            || st.get("status").and_then(Value::as_str) == Some("accept");
        if pass {
            v.push(s.to_string());
        }
    }
    v
}

/// The agent-safe subset for Root lineage (`lineage.append{kind:"gate"}`, root/root.md §3.10.5 and
/// X9): `gate_id`, `gate_kind`, `sha`, `verdict`, `reason`, `stages` (names passed), `spent_musd`,
/// `config_sha256`, plus `level`, `scope`, `escalated`, `audit_required`, `charter_sha256`, `n` and
/// `capability`. Never holdout or sealed ids, per-task values, paths, the kernel build hash or prompts.
pub fn root_subset(full: &Value) -> Value {
    let g4 = &full["stages"]["G4"];
    let mut v = json!({
        "v": full.get("v").filter(|x| x.is_u64()).cloned().unwrap_or(json!(RECORD_V)),
        "gate_id": full["gate_id"],
        "gate_kind": full["gate_kind"],
        "sha": full["candidate"]["sha"],
        "verdict": full["verdict"],
        "reason": full["reason"],
        "stages": stages_passed(full),
        "spent_musd": full["spent_musd"],
        "config_sha256": full["kernel"]["config_sha256"],
        "charter_sha256": full["kernel"]["charter_sha256"],
        "level": full["level"],
        "scope": full["scope"],
        "escalated": full["escalated"],
        "audit_required": g4.get("audit_required").cloned().unwrap_or(json!(false)),
    });
    if let Some(n) = g4.get("n") {
        v["n"] = n.clone();
    }
    // G4 v2 (M7; m7-design.md §3.2.11, §4.4): the tier, the slice's agent view (rounded, no ids),
    // `milestone_required` on a `lineage_gain` and the accept class (better | lineage_gain |
    // tie_break:<which>), so Root lineage tells a tie-break accept from a gain.
    for k in ["tier", "sealed", "milestone_required", "accept_class"] {
        if let Some(x) = g4.get(k) {
            v[k] = x.clone();
        }
    }
    // ADR-011 v5: the free path's admission (`accept:"free"`, the Root's group and `change_open` seq).
    for k in ["accept", "group", "change_seq"] {
        if let Some(x) = full.get(k) {
            v[k] = x.clone();
        }
    }
    if let Some(c) = g4.get("capability").filter(|c| !c.is_null()) {
        v["capability"] = json!({
            "labels": c.get("labels").cloned().unwrap_or(json!([])),
            "n_families": c.get("n_families").cloned().unwrap_or(json!(0)),
            "l_permille": c.get("l_permille").or_else(|| c.get("l")).cloned().unwrap_or(json!(0)),
            "status": c.get("status").cloned().unwrap_or(Value::Null),
        });
    }
    if let Value::Object(m) = &mut v {
        m.retain(|_, x| !x.is_null());
    }
    v
}

/// The redacted note line (§3.13 "Notes"): the full record without the kernel build hash and,
/// at L0–L2, without anything holdout-specific beyond aggregates (the record never carries holdout
/// ids; per-task values are not in it either). From L3 the same shape, with `holdout_visible`.
pub fn redact_for_note(full: &Value) -> Value {
    let mut v = full.clone();
    if let Some(k) = v.get_mut("kernel").and_then(Value::as_object_mut) {
        k.remove("build_sha256");
    }
    if let Some(s) = v.get_mut("stages").and_then(Value::as_object_mut) {
        for (_, st) in s.iter_mut() {
            if let Some(o) = st.as_object_mut() {
                o.remove("runs_detail");
                o.remove("holdout_ids");
            }
        }
    }
    v
}

/// The agent report: stages (pass, reason), ≤ 4 KiB failure text, warnings, G4 aggregates (no
/// holdout ids), verdict.
pub fn report(full: &Value, failure_text: Option<&str>) -> Value {
    let mut stages = Map::new();
    if let Some(s) = full["stages"].as_object() {
        for (name, st) in s {
            let mut o = json!({"pass": st.get("pass").cloned().unwrap_or(Value::Null)});
            for k in [
                "reason",
                "code",
                "warnings",
                "status",
                "score",
                "n",
                "capability",
                "audit_required",
                "continuity",
                "prefix_hash",
                "parent_prefix_hash",
                "commit",
                "mode",
                "tier",
                "screen",
                "sealed",
                "sealed_refusal",
                "cpp",
                "milestone_required",
                "accept_class",
            ] {
                if let Some(x) = st.get(k) {
                    o[k] = x.clone();
                }
            }
            stages.insert(name.clone(), o);
        }
    }
    json!({
        "v": 1,
        "gate_id": full["gate_id"],
        "gate_kind": full["gate_kind"],
        "sha": full["candidate"]["sha"],
        "scope": full["scope"],
        "verdict": full["verdict"],
        "reason": full["reason"],
        "stages": Value::Object(stages),
        "failure": failure_text.map(|t| cap(t, REPORT_TEXT_MAX)),
    })
}

/// The stage notice sent to the body (§3.13 "Events to the body").
#[allow(clippy::too_many_arguments)]
pub fn stage_notice(
    gate_id: &str,
    sha: &str,
    stage: &str,
    pass: Option<bool>,
    reason: Option<&str>,
    score: Option<i64>,
    cost_musd: u64,
    wall_ms: u64,
    cache_hit: Option<u64>,
) -> Value {
    json!({"gate_id": gate_id, "sha": sha, "stage": stage, "pass": pass, "reason": reason,
           "score": score, "cost": cost_musd, "wall": wall_ms, "cache_hit": cache_hit})
}

/// Append one redacted record line to `refs/notes/gate` on `sha` (§3.13 "Notes"): show the existing
/// note into `tmp`, append the line, `notes --ref=gate add -f -F tmp`. `git` builds a hardened `$G`
/// command on the canonical repo (the caller's prefix).
pub fn append_note(
    git: &dyn Fn(&[&str]) -> Command,
    sha: &str,
    line: &Value,
    tmp: &Path,
) -> Result<(), String> {
    let run = |args: &[&str]| -> Result<std::process::Output, String> {
        git(args)
            .output()
            .map_err(|e| format!("git {}: {e}", args.join(" ")))
    };
    let existing = run(&["notes", "--ref=gate", "show", sha])?;
    let mut body = if existing.status.success() {
        existing.stdout
    } else {
        Vec::new()
    };
    if !body.is_empty() && !body.ends_with(b"\n") {
        body.push(b'\n');
    }
    body.extend_from_slice(line.to_string().as_bytes());
    body.push(b'\n');
    if let Some(d) = tmp.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    std::fs::write(tmp, &body).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let tmp_s = tmp.to_string_lossy().into_owned();
    let o = run(&[
        "-c",
        "user.name=INIT Kernel",
        "-c",
        "user.email=kernel@init.local",
        "notes",
        "--ref=gate",
        "add",
        "-f",
        "-F",
        &tmp_s,
        sha,
    ])?;
    let _ = std::fs::remove_file(tmp);
    if !o.status.success() {
        return Err(format!(
            "git notes add: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ));
    }
    Ok(())
}

/// The note lines on `sha` (oldest first).
pub fn read_notes(git: &dyn Fn(&[&str]) -> Command, sha: &str) -> Vec<Value> {
    match git(&["notes", "--ref=gate", "show", sha]).output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect(),
        _ => Vec::new(),
    }
}

/// Every string in `v` (for privacy assertions in tests and in the driver).
pub fn strings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
        Value::Object(m) => m.iter().for_each(|(k, x)| {
            out.push(k.clone());
            strings(x, out)
        }),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> Value {
        json!({"kind": "gate", "v": 2, "level": "L0", "gate_id": "j-000043", "gate_kind": "promote", "scope": "t2",
               "candidate": {"sha": "c".repeat(40), "branch": "evo/x", "change_type": "feat", "capability_labels": []},
               "parent": {"running": "p".repeat(40), "lkg": "p".repeat(40)},
               "kernel": {"build_sha256": "deadbeef", "charter_sha256": "ch", "config_sha256": "cf", "version": "0.3.0"},
               "stages": {"G0": {"pass": true}, "G1": {"pass": true}, "G2": {"pass": true, "continuity": "pass"},
                          "G3": {"pass": true, "commit": "c3"},
                          "G4": {"status": "accept", "reason": "better", "audit_required": false,
                                 "n": {"pool": 2, "holdout": 1, "agent": 0, "canaries": 1, "capability": 0, "repro": 0}}},
               "verdict": "accept", "reason": "better", "spent_musd": 12, "escalated": false})
    }

    /// ADR-011 X2: a free-path record is v5 with `accept:"free"`, its A-stages count as passed
    /// stages, and the Root subset carries the group and the `change_open` seq.
    #[test]
    fn root_subset_of_a_v5_free_record() {
        let mut f = full();
        f["v"] = json!(RECORD_V_FREE);
        f["accept"] = json!("free");
        f["group"] = json!(7);
        f["change_seq"] = json!(1203);
        f["reason"] = json!("free");
        let st = f["stages"].as_object_mut().unwrap();
        st.remove("G3");
        st.remove("G4");
        st.insert("A0".into(), json!({"pass": true, "by": "root"}));
        st.insert("A1".into(), json!({"pass": true, "stages": ["G0", "G1"]}));
        st.insert("A2".into(), json!({"pass": true, "stages": ["G2"]}));
        st.insert("A3".into(), json!({"status": "async"}));
        st.insert("A4".into(), json!({"pass": true, "by": "root"}));
        st.insert("A5".into(), json!({"pass": true, "by": "root"}));
        let s = root_subset(&f);
        assert_eq!((s["v"].as_u64(), s["accept"].as_str(), s["group"].as_u64(), s["change_seq"].as_u64()), (Some(5), Some("free"), Some(7), Some(1203)));
        assert_eq!(s["stages"], json!(["G0", "G1", "G2", "A0", "A1", "A2", "A4", "A5"]));
        assert!(s.get("n").is_none(), "no G4 on the free path");
        // A gated record keeps v2 and no free keys.
        let g = root_subset(&full());
        assert_eq!(g["v"], 2);
        assert!(g.get("accept").is_none() && g.get("group").is_none());
    }

    #[test]
    fn root_subset_has_the_x9_fields_only() {
        let s = root_subset(&full());
        for k in [
            "gate_id",
            "gate_kind",
            "sha",
            "verdict",
            "reason",
            "stages",
            "spent_musd",
            "config_sha256",
            "level",
            "scope",
            "escalated",
            "audit_required",
            "charter_sha256",
            "n",
        ] {
            assert!(s.get(k).is_some(), "{k}");
        }
        assert_eq!(s["stages"], json!(["G0", "G1", "G2", "G3", "G4"]));
        let mut all = Vec::new();
        strings(&s, &mut all);
        assert!(
            !all.iter().any(|x| x == "deadbeef" || x.starts_with('/')),
            "no build hash, no paths"
        );
    }

    #[test]
    fn note_drops_the_build_hash() {
        let n = redact_for_note(&full());
        assert!(n["kernel"].get("build_sha256").is_none());
        assert_eq!(n["kernel"]["config_sha256"], "cf");
    }

    #[test]
    fn report_caps_text_and_keeps_aggregates() {
        let r = report(&full(), Some(&"x".repeat(10_000)));
        assert!(r["failure"].as_str().unwrap().len() <= REPORT_TEXT_MAX);
        assert_eq!(r["stages"]["G4"]["n"]["pool"], 2);
    }

    #[test]
    fn integers() {
        assert_eq!(
            integers_only(json!({"a": 0.25, "b": [1, 0.0016]})),
            json!({"a": 250, "b": [1, 2]})
        );
        assert_eq!(permille(-0.0384), -38);
    }

    /// G-30: notes on a bare repo with the system git: add, append (rewrite), show.
    #[test]
    fn g30_notes_on_a_bare_repo() {
        let home = crate::testutil::test_home("g30");
        let bare = home.join("body.git");
        let git = |dir: &Path| {
            let dir = dir.to_path_buf();
            move |args: &[&str]| {
                let mut c = Command::new("/usr/bin/git");
                c.env_clear()
                    .env("PATH", "/usr/bin:/bin")
                    .env("HOME", dir.parent().unwrap())
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .env("GIT_CEILING_DIRECTORIES", dir.parent().unwrap())
                    .arg("--git-dir")
                    .arg(&dir)
                    .args(args);
                c
            }
        };
        let g = git(&bare);
        assert!(
            Command::new("/usr/bin/git")
                .args(["init", "-q", "--bare", "--initial-branch=main"])
                .arg(&bare)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("HOME", &home)
                .status()
                .unwrap()
                .success()
        );
        let tree = g(&["mktree"])
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        let tree = String::from_utf8(tree.stdout).unwrap().trim().to_string();
        let c = g(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@x",
            "commit-tree",
            &tree,
            "-m",
            "root",
        ])
        .output()
        .unwrap();
        let sha = String::from_utf8(c.stdout).unwrap().trim().to_string();
        assert_eq!(sha.len(), 40);
        let tmp = home.join("tmp/note");
        append_note(&g, &sha, &json!({"gate_kind": "quick", "n": 1}), &tmp).unwrap();
        append_note(&g, &sha, &json!({"gate_kind": "promote", "n": 2}), &tmp).unwrap();
        let lines = read_notes(&g, &sha);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["gate_kind"], "quick");
        assert_eq!(lines[1]["gate_kind"], "promote");
        let r = g(&["rev-parse", "refs/notes/gate"]).output().unwrap();
        assert!(r.status.success(), "refs/notes/gate exists");
        crate::testutil::remove_home(&home);
    }
}
