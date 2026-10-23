//! The served view `state/kernel/self.json` (selfmap.md §3.10–§3.11): the running version's committed
//! `self.json`, unchanged in every field, plus a `live` section built by the kernel from kernel-held
//! data. Serve mode runs in-process in supervise (it executes no candidate code); this module is the
//! pure builder plus the atomic write. supervise decides when to rebuild (§3.11 triggers).

use std::io::Write;
use std::path::Path;

use serde_json::{Value, json};

use crate::icj::to_pretty;
use crate::kernel_tree::component_summary;

/// `live.level` (from the Root's published status, R7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Level {
    pub current: String,
    pub max: String,
    pub since: String,
}

/// What `live.kernel` is built from.
#[derive(Debug, Clone)]
pub struct KernelInputs {
    pub version: String,
    /// SHA-256 of the running `init-kernel` binary.
    pub build_sha256: String,
    /// The running kernel sha in `lineage/kernel.git`, if that repo exists.
    pub kernel_sha: Option<String>,
    /// The build digest recorded when `kernel_sha` was admitted (K-stage report / Root `kernel_admitted`).
    pub admitted_build_sha256: Option<String>,
    /// `kernel-self.json` at `kernel_sha`.
    pub kernel_self: Option<Value>,
    /// The component table embedded in the maintainer's build (used when there is no kernel repo).
    pub embedded: Vec<Value>,
}

/// Everything `live` holds.
#[derive(Debug, Clone)]
pub struct LiveInputs {
    pub as_of: String,
    pub running_sha: String,
    pub lkg_sha: String,
    pub level: Level,
    /// The Root's `ledger.json`, verbatim (`None` → null).
    pub ladder: Option<Value>,
    pub kernel: KernelInputs,
    /// The `capability-v1` object ([`crate::capability::capability`]).
    pub capability: Value,
    /// The Root's sealed-audit summary, verbatim.
    pub sealed_audit: Option<Value>,
}

fn level_rank(l: &str) -> Option<u8> {
    l.strip_prefix('L').and_then(|d| d.parse().ok())
}

/// `live.kernel` (selfmap.md §3.11).
pub fn kernel_live(k: &KernelInputs, level_current: &str) -> Value {
    let repo = k.kernel_sha.is_some();
    let ks = if repo { k.kernel_self.as_ref() } else { None };
    let verified = repo && k.admitted_build_sha256.as_deref() == Some(k.build_sha256.as_str());
    let mut components = match ks {
        Some(v) => component_summary(v),
        None => k.embedded.clone(),
    };
    components.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    json!({
        "budget_tokens": ks.and_then(|v| v.pointer("/kernel/budget/tokens")).cloned().unwrap_or(Value::Null),
        "build_sha256": k.build_sha256,
        "components": components,
        "evolvable": level_rank(level_current).is_some_and(|r| r >= 3),
        "kernel_sha": k.kernel_sha,
        "source_digest": ks.and_then(|v| v.pointer("/kernel/source_digest")).cloned().unwrap_or(Value::Null),
        "verified": verified,
        "version": k.version,
    })
}

/// The `live` object.
pub fn live_value(l: &LiveInputs) -> Value {
    json!({
        "as_of": l.as_of,
        "capability": l.capability,
        "kernel": kernel_live(&l.kernel, &l.level.current),
        "ladder": l.ladder.clone().unwrap_or(Value::Null),
        "level": {"current": l.level.current, "max": l.level.max, "since": l.level.since},
        "lkg_sha": l.lkg_sha,
        "running_sha": l.running_sha,
        "sealed_audit": l.sealed_audit.clone().unwrap_or(Value::Null),
    })
}

/// The served view: the committed file (any committed `live` dropped, D12) plus `live`, pretty ICJ.
pub fn build_served(committed: &[u8], l: &LiveInputs) -> Result<Vec<u8>, String> {
    let mut v: Value = serde_json::from_slice(committed)
        .map_err(|e| format!("committed self.json does not parse: {e}"))?;
    let o = v
        .as_object_mut()
        .ok_or("committed self.json is not an object")?;
    o.insert("live".into(), live_value(l));
    Ok(to_pretty(&v))
}

/// Reads the Root's ladder file verbatim (`None` if absent).
pub fn read_ladder(path: &Path) -> Result<Option<Value>, String> {
    match std::fs::read(path) {
        Ok(b) => serde_json::from_slice(&b)
            .map(Some)
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Atomic write: temp file in the same directory, `fsync`, `rename`.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("self.json");
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{remove_home, test_home};

    const A: &str = "0e786c733a9648164a2b4c4aea7ece24d0beb7c7";
    const B: &str = "1e786c733a9648164a2b4c4aea7ece24d0beb7c7";

    fn committed() -> Vec<u8> {
        to_pretty(
            &json!({"self_v": 2, "generator": crate::generator_value(), "body_tokens": 7, "files": []}),
        )
    }

    fn kernel(repo: bool, admitted: Option<&str>) -> KernelInputs {
        KernelInputs {
            version: "0.3.0".into(),
            build_sha256: "aa".into(),
            kernel_sha: repo.then(|| A.to_string()),
            admitted_build_sha256: admitted.map(str::to_string),
            kernel_self: Some(
                json!({"kernel": {"budget": {"tokens": 99}, "source_digest": "sha256:00",
                "components": [{"id": "kernel/proxy", "tokens": 5, "contracts": ["kernel/proxy.md §7"], "paths": [], "evolvable_from": "L3"},
                               {"id": "kernel/gate", "tokens": 9, "contracts": ["kernel/gate.md §4.1a"], "paths": [], "evolvable_from": "L3"}]}}),
            ),
            embedded: vec![json!({"contracts": [], "id": "kernel/other", "tokens": 1})],
        }
    }

    fn inputs(level: &str, running: &str, lkg: &str, cap: Value) -> LiveInputs {
        LiveInputs {
            as_of: "2026-11-20T09:00:00.000Z".into(),
            running_sha: running.into(),
            lkg_sha: lkg.into(),
            level: Level {
                current: level.into(),
                max: "L3".into(),
                since: "2026-11-02T04:00:00.000Z".into(),
            },
            ladder: Some(json!({"entered_seq": 4, "future_field": [1, 2]})),
            kernel: kernel(true, Some("aa")),
            capability: cap,
            sealed_audit: None,
        }
    }

    /// S-14: rebuilds after a boot, an LKG change, a level change (mock Root L0 → L1) and a G4 verdict.
    #[test]
    fn s14_served_view_rebuilds() {
        let c = committed();
        let cap0 = json!({"categories": [], "lkg_sha": A, "method": "capability-v1", "window": {"from": null, "gates": 0, "runs": 0, "to": null}});
        let mut cap1 = cap0.clone();
        cap1["window"]["gates"] = json!(1);
        let steps = [
            inputs("L0", A, A, cap0.clone()), // boot
            inputs("L0", B, A, cap0.clone()), // promoted, provisional
            inputs("L0", B, B, cap0.clone()), // LKG change
            inputs("L1", B, B, cap0),         // the Root announces L1
            inputs("L1", B, B, cap1),         // a G4 verdict
        ];
        let base: Value = serde_json::from_slice(&c).unwrap();
        for s in &steps {
            let v: Value = serde_json::from_slice(&build_served(&c, s).unwrap()).unwrap();
            assert_eq!(v["live"]["level"]["current"], s.level.current.as_str());
            assert_eq!(v["live"]["running_sha"], s.running_sha.as_str());
            assert_eq!(v["live"]["lkg_sha"], s.lkg_sha.as_str());
            assert_eq!(
                v["live"]["ladder"]["future_field"],
                json!([1, 2]),
                "ladder copied verbatim"
            );
            let mut outside = v.clone();
            outside.as_object_mut().unwrap().remove("live");
            assert_eq!(
                outside, base,
                "every field outside live equals the committed file"
            );
            // A served view committed by mistake passes G1 with `live_ignored` (D12, S-13).
            let served = build_served(&c, s).unwrap();
            assert_eq!(
                crate::drift::compare(Some(&served), &c),
                Ok(vec![crate::drift::Warning::LiveIgnored])
            );
        }
    }

    /// S-19: verified true / false / false; the embedded table without a kernel repo; evolvable from L3.
    #[test]
    fn s19_live_kernel() {
        let k = kernel_live(&kernel(true, Some("aa")), "L1");
        assert_eq!(k["verified"], true);
        assert_eq!(k["evolvable"], false);
        assert_eq!(k["budget_tokens"], 99);
        assert_eq!(k["components"][0]["id"], "kernel/gate");
        assert_eq!(k["components"][0].as_object().unwrap().len(), 3);
        assert_eq!(
            kernel_live(&kernel(true, Some("bb")), "L1")["verified"],
            false
        );
        let none = kernel_live(&kernel(false, Some("aa")), "L3");
        assert_eq!(none["verified"], false);
        assert_eq!(none["kernel_sha"], Value::Null);
        assert_eq!(none["source_digest"], Value::Null);
        assert_eq!(none["budget_tokens"], Value::Null);
        assert_eq!(
            none["components"],
            json!([{"contracts": [], "id": "kernel/other", "tokens": 1}])
        );
        assert_eq!(none["evolvable"], true);
    }

    #[test]
    fn atomic_write_and_ladder_read() {
        let home = test_home("serve");
        let p = home.join("self.json");
        write_atomic(&p, b"{}\n").unwrap();
        write_atomic(&p, b"{\"a\":1}\n").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"{\"a\":1}\n");
        assert_eq!(
            std::fs::read_dir(&home).unwrap().count(),
            1,
            "no temp file left"
        );
        assert_eq!(read_ladder(&home.join("ledger.json")).unwrap(), None);
        std::fs::write(home.join("ledger.json"), "{\"x\":1}").unwrap();
        assert_eq!(
            read_ladder(&home.join("ledger.json")).unwrap(),
            Some(json!({"x": 1}))
        );
        remove_home(&home);
    }
}
