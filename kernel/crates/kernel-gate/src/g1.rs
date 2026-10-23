//! G1 checks that need no generator run: the drift comparison of the committed `self.json` against the
//! regenerated one (kernel/selfmap.md §3.8, v03-decisions D12: committed fields only), the
//! trailer/marker check (selfmap.md §3.8 check 3) and the WIT copy drift check (kernel/gate.md §3.4 item 3).
//!
//! The generator itself (`init-kernel selfmap`) is WP9's; G1 runs it in the `build` sandbox and hands
//! its output here.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::evo::ChangeType;

/// Max size of the agent-visible drift report (gate.md §3.4, selfmap.md §3.8).
pub const REPORT_MAX: usize = 4096;
/// Values in the drift report are cut at this many bytes.
pub const VALUE_MAX: usize = 120;

/// A G1 failure: `code` is `gate.g1.<reason>`, `detail` the agent-visible text (≤ 4 KiB).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct G1Failure {
    pub code: &'static str,
    pub detail: String,
}

impl G1Failure {
    fn new(code: &'static str, detail: impl Into<String>) -> Self {
        G1Failure {
            code,
            detail: cap(detail.into(), REPORT_MAX),
        }
    }
}

/// Non-failing observations, shown to the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum G1Warning {
    /// The committed file carries a `live` section; it is dropped before comparing and never read (D12).
    LiveIgnored,
    /// Equal JSON values with different bytes: the agent hand-edited the formatting.
    NonCanonicalFormat,
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

/// RFC 6901 escaping of one reference token.
fn escape_token(t: &str) -> String {
    t.replace('~', "~0").replace('/', "~1")
}

/// Collects the JSON Pointers of every differing leaf (objects by key, arrays by index).
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

/// `files` of a self.json as path → sha256.
fn file_hashes(v: &Value) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    if let Some(files) = v.get("files").and_then(Value::as_array) {
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

/// The `self_json_drift` report: first the files whose `sha256` differs (usually the whole story),
/// then every differing leaf, sorted, with committed and regenerated values cut at 120 bytes; ≤ 4 KiB.
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

/// The drift comparison of selfmap.md §3.8 items 1–5, with D12's rule: `live` is dropped from both sides
/// before comparing. `committed` is the file at the candidate sha (`None` if absent); `regenerated` is
/// the generator's output for the export.
pub fn compare_self_json(
    committed: Option<&[u8]>,
    regenerated: &[u8],
) -> Result<Vec<G1Warning>, G1Failure> {
    let Some(committed_bytes) = committed else {
        return Err(G1Failure::new(
            "gate.g1.self_json_missing",
            "self.json is not committed; run `init-kernel selfmap --write` and commit it",
        ));
    };
    let mut r: Value = serde_json::from_slice(regenerated).map_err(|e| {
        G1Failure::new(
            "gate.g1.selfmap_error",
            format!("regenerated self.json does not parse: {e}"),
        )
    })?;
    let mut c: Value = match serde_json::from_slice(committed_bytes) {
        Ok(v) => v,
        Err(e) => {
            return Err(G1Failure::new(
                "gate.g1.self_json_drift",
                format!(
                    "committed self.json is not valid JSON ({e}); regenerate with `init-kernel selfmap --write`"
                ),
            ));
        }
    };
    let gen_version = |v: &Value| v.get("generator").and_then(|g| g.get("version")).cloned();
    if gen_version(&c) != gen_version(&r) || c.get("self_v") != r.get("self_v") {
        return Err(G1Failure::new(
            "gate.g1.generator_changed",
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
        warnings.push(G1Warning::LiveIgnored);
    }
    if c == r {
        if !had_live && committed_bytes != regenerated {
            warnings.push(G1Warning::NonCanonicalFormat);
        }
        return Ok(warnings);
    }
    Err(G1Failure::new(
        "gate.g1.self_json_drift",
        drift_report(&c, &r),
    ))
}

/// selfmap.md §3.8 check 3 (evolution.md §3.9 item 10): for `feat`, `fix` and `guard`, every component
/// in the regenerated file `r` but not in the parent's `p` carries the commit's `Compensates-for` slug
/// (`none` when the trailer is `none`).
pub fn check_tag_markers(
    change_type: ChangeType,
    trailer_slug: &str,
    r: &Value,
    p: &Value,
) -> Result<(), G1Failure> {
    if !matches!(
        change_type,
        ChangeType::Feat | ChangeType::Fix | ChangeType::Guard
    ) {
        return Ok(());
    }
    let comps = |v: &Value| -> BTreeMap<String, String> {
        v.get("components")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|c| {
                        let id = c.get("id")?.as_str()?.to_string();
                        let tag = c
                            .get("compensates_for")
                            .and_then(Value::as_str)
                            .unwrap_or("none")
                            .to_string();
                        Some((id, tag))
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let (rc, pc) = (comps(r), comps(p));
    let bad: Vec<String> = rc
        .iter()
        .filter(|(id, tag)| !pc.contains_key(*id) && tag.as_str() != trailer_slug)
        .map(|(id, tag)| format!("{id} is marked `{tag}`, the trailer says `{trailer_slug}`"))
        .collect();
    if bad.is_empty() {
        Ok(())
    } else {
        Err(G1Failure::new(
            "gate.g1.tag_marker_mismatch",
            format!(
                "new components must carry the Compensates-for slug:\n  {}",
                bad.join("\n  ")
            ),
        ))
    }
}

fn walk_files(root: &Path, rel: &Path, out: &mut Vec<(PathBuf, bool)>) -> std::io::Result<()> {
    let mut entries: Vec<_> = fs::read_dir(root.join(rel))?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let ft = e.file_type()?;
        let r = rel.join(e.file_name());
        if ft.is_dir() {
            walk_files(root, &r, out)?;
        } else {
            // A symlink is never a byte-identical copy of a kernel file.
            out.push((r, ft.is_file()));
        }
    }
    Ok(())
}

/// gate.md §3.4 item 3: every file under the export's `wit/` exists at the same relative path under the
/// kernel's WIT directory (`$INIT_HOME/kernel/wit/`) and is byte-identical. The body may hold fewer
/// versions than the kernel, never a different text. Symlinks are not followed and count as drift.
pub fn check_wit_copy(export_root: &Path, kernel_wit: &Path) -> Result<(), G1Failure> {
    let body_wit = export_root.join("wit");
    let meta = match fs::symlink_metadata(&body_wit) {
        Ok(m) => m,
        Err(_) => return Ok(()),
    };
    if !meta.is_dir() {
        return Err(G1Failure::new(
            "gate.g1.wit_copy_drift",
            "wit is not a directory; restore it with `cp -R $INIT_HOME/kernel/wit body/wit`",
        ));
    }
    let mut files = Vec::new();
    walk_files(&body_wit, Path::new(""), &mut files)
        .map_err(|e| G1Failure::new("gate.g1.selfmap_error", format!("cannot read wit/: {e}")))?;
    let mut bad = Vec::new();
    for (rel, regular) in files {
        let shown = rel.to_string_lossy();
        let theirs = kernel_wit.join(&rel);
        let ok = regular
            && match (fs::read(body_wit.join(&rel)), fs::read(&theirs)) {
                (Ok(a), Ok(b)) => a == b,
                _ => false,
            };
        if !ok {
            let why = if !theirs.exists() {
                "no such file in the kernel's WIT"
            } else {
                "differs from the kernel's copy"
            };
            bad.push(format!(
                "wit/{shown}: {why}; fix: cp $INIT_HOME/kernel/wit/{shown} body/wit/{shown}"
            ));
        }
    }
    if bad.is_empty() {
        Ok(())
    } else {
        Err(G1Failure::new("gate.g1.wit_copy_drift", bad.join("\n")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{remove_home, test_home};
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

    fn bytes(v: &Value) -> Vec<u8> {
        serde_json::to_vec(v).unwrap()
    }

    #[test]
    fn equal_files_pass_and_formatting_only_warns() {
        let r = bytes(&sample());
        assert_eq!(compare_self_json(Some(&r), &r), Ok(vec![]));
        let pretty = serde_json::to_vec_pretty(&sample()).unwrap();
        assert_eq!(
            compare_self_json(Some(&pretty), &r),
            Ok(vec![G1Warning::NonCanonicalFormat])
        );
    }

    #[test]
    fn missing_and_generator_changed() {
        let r = bytes(&sample());
        assert_eq!(
            compare_self_json(None, &r).unwrap_err().code,
            "gate.g1.self_json_missing"
        );
        let mut c = sample();
        c["generator"]["version"] = json!(1);
        assert_eq!(
            compare_self_json(Some(&bytes(&c)), &r).unwrap_err().code,
            "gate.g1.generator_changed"
        );
        let mut c = sample();
        c["self_v"] = json!(1);
        assert_eq!(
            compare_self_json(Some(&bytes(&c)), &r).unwrap_err().code,
            "gate.g1.generator_changed"
        );
        assert_eq!(
            compare_self_json(Some(b"{not json"), &r).unwrap_err().code,
            "gate.g1.self_json_drift"
        );
    }

    /// D12: `live` is not a committed field. A committed `live` (e.g. a copy of the served view) changes
    /// nothing and only warns; pass rates in it are never compared.
    #[test]
    fn d12_live_is_dropped_from_both_sides() {
        let r = sample();
        let mut c = sample();
        c["live"] = json!({"capability": {"categories": [{"category": "_all", "pass_permille": 710}]}, "level": {"current": "L1"}});
        assert_eq!(
            compare_self_json(Some(&bytes(&c)), &bytes(&r)),
            Ok(vec![G1Warning::LiveIgnored])
        );
        let mut r2 = sample();
        r2["live"] = json!({"capability": {"categories": []}});
        assert_eq!(
            compare_self_json(Some(&bytes(&c)), &bytes(&r2)),
            Ok(vec![G1Warning::LiveIgnored])
        );
        // A real committed-field drift still fails with `live` present.
        c["body_tokens"] = json!(1);
        assert_eq!(
            compare_self_json(Some(&bytes(&c)), &bytes(&r))
                .unwrap_err()
                .code,
            "gate.g1.self_json_drift"
        );
    }

    #[test]
    fn drift_report_names_files_first_then_pointers() {
        let r = sample();
        let mut c = sample();
        c["files"][1]["sha256"] = json!("old");
        c["body_tokens"] = json!(20000);
        c["components"][0]["tokens"] = json!(3000);
        c["extra/key"] = json!("x");
        let err = compare_self_json(Some(&bytes(&c)), &bytes(&r)).unwrap_err();
        assert_eq!(err.code, "gate.g1.self_json_drift");
        let d = err.detail;
        let files_at = d.find("changed: src/loop.rs").unwrap();
        let ptr_at = d.find("/body_tokens: 20000 → 21873").unwrap();
        assert!(files_at < ptr_at, "{d}");
        assert!(d.contains("/components/0/tokens: 3000 → 3410"));
        assert!(d.contains("/extra~1key: \"x\" → (absent)"));
        assert!(d.contains("/files/1/sha256: \"old\" → \"bb\""));
        assert!(
            !d.contains("src/context.rs\n"),
            "unchanged files are not listed"
        );
    }

    #[test]
    fn drift_report_is_capped() {
        let r = sample();
        let mut c = sample();
        c["big"] = json!(
            (0..2000)
                .map(|i| format!("value-{i}-{}", "x".repeat(150)))
                .collect::<Vec<_>>()
        );
        let err = compare_self_json(Some(&bytes(&c)), &bytes(&r)).unwrap_err();
        assert!(err.detail.len() <= REPORT_MAX);
        assert!(err.detail.lines().all(|l| l.len() < 2 * VALUE_MAX + 40));
    }

    #[test]
    fn tag_markers_for_new_components() {
        let p = sample();
        let mut r = sample();
        r["components"]
            .as_array_mut()
            .unwrap()
            .push(json!({"compensates_for": "loop-detection", "id": "ext/pty", "kind": "ext"}));
        assert!(check_tag_markers(ChangeType::Guard, "loop-detection", &r, &p).is_ok());
        let e = check_tag_markers(ChangeType::Feat, "none", &r, &p).unwrap_err();
        assert_eq!(e.code, "gate.g1.tag_marker_mismatch");
        assert!(e.detail.contains("ext/pty"));
        // Only feat, fix and guard are checked; existing components are not.
        assert!(check_tag_markers(ChangeType::Refactor, "none", &r, &p).is_ok());
        assert!(check_tag_markers(ChangeType::Feat, "none", &p, &p).is_ok());
    }

    /// The repo's own seed copy of the WIT matches the authoritative kernel WIT (read-only check).
    #[test]
    fn repo_seed_wit_matches_kernel_wit() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        assert_eq!(
            check_wit_copy(&repo.join("seed"), &repo.join("wit")),
            Ok(())
        );
    }

    #[test]
    fn wit_copy_drift_cases() {
        let home = test_home("g1-wit");
        let kernel = home.join("kernel/wit");
        let body = home.join("export");
        fs::create_dir_all(kernel.join("init-ext/v1")).unwrap();
        fs::create_dir_all(kernel.join("init-ext/v2")).unwrap();
        fs::write(
            kernel.join("init-ext/v1/init-ext.wit"),
            "package init:ext@1.0.0;\n",
        )
        .unwrap();
        fs::write(
            kernel.join("init-ext/v2/init-ext.wit"),
            "package init:ext@2.0.0;\n",
        )
        .unwrap();
        // No wit/ at all, and fewer versions than the kernel: both fine.
        fs::create_dir_all(&body).unwrap();
        assert_eq!(check_wit_copy(&body, &kernel), Ok(()));
        fs::create_dir_all(body.join("wit/init-ext/v1")).unwrap();
        fs::write(
            body.join("wit/init-ext/v1/init-ext.wit"),
            "package init:ext@1.0.0;\n",
        )
        .unwrap();
        assert_eq!(check_wit_copy(&body, &kernel), Ok(()));
        // A one-byte edit.
        fs::write(
            body.join("wit/init-ext/v1/init-ext.wit"),
            "package init:ext@1.0.1;\n",
        )
        .unwrap();
        let e = check_wit_copy(&body, &kernel).unwrap_err();
        assert_eq!(e.code, "gate.g1.wit_copy_drift");
        assert!(e.detail.contains(
            "cp $INIT_HOME/kernel/wit/init-ext/v1/init-ext.wit body/wit/init-ext/v1/init-ext.wit"
        ));
        // An extra file the kernel does not have.
        fs::write(
            body.join("wit/init-ext/v1/init-ext.wit"),
            "package init:ext@1.0.0;\n",
        )
        .unwrap();
        fs::write(body.join("wit/init-ext/v1/extra.wit"), "interface x {}\n").unwrap();
        let e = check_wit_copy(&body, &kernel).unwrap_err();
        assert!(e.detail.contains("wit/init-ext/v1/extra.wit: no such file"));
        remove_home(&home);
    }
}
