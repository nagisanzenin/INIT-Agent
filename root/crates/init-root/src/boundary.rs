//! ADR-011 A0 and INV-2 (docs/foundations.md §5): the Root's own boundary, recorded at start and re-checked at every
//! admission. At every daemon start the Root hashes the binary it runs from, the sandbox profile texts compiled into it
//! and its active charter, and records them as `root_start{root_version, root_sha256, profiles_sha256, charter}`
//! (root.md §3.10.5). A0 at every `change.open` re-hashes the same three against that record (the charter file's bytes
//! against the text of the active charter, which only the owner's `charter check --apply` moves). A mismatch means a Root file changed
//! under the running Root, which no zone event may do (INV-2): the change is refused (`root.boundary`) and the zone
//! stops (`Halted{boundary}`, sup.rs `boundary_halt`).
//!
//! At start the binary is also compared with the one the newest `root_start` recorded. It may differ only when an owner
//! upgrade installed it or an upgrade rollback restored it (upgrade.rs's journal `state/upgrade.json`, `to.root_sha256`
//! and `from.root_sha256`). Otherwise the zone does not start (`Halted{boundary}`, standing across a Root restart) until
//! the owner's `/resume` accepts the binary. A Root started from a binary someone else wrote cannot be relied on to run
//! this check, so the start check only catches a replaced binary that is honest about itself; the per-admission re-hash
//! is the part an untrusted zone cannot dodge, since the running Root started from a recorded binary.

use crate::link::Root;
use crate::util;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// What this start recorded in `root_start`: A0's reference until the Root exits.
#[derive(Clone, Debug)]
pub struct Start {
    /// The file this Root was started from (`std::env::current_exe` at start), re-hashed at every admission.
    pub exe: PathBuf,
    pub root_sha256: String,
    pub profiles_sha256: String,
}

/// SHA-256 over the sandbox profile texts compiled into this binary, each length-prefixed, in a fixed order: the
/// Seatbelt texts (macOS) and the platform-neutral deny lists (`init_sandbox_profiles::access`). The Linux zone
/// is rendered by code, which `root_sha256` covers.
pub fn profiles_sha256() -> String {
    use init_sandbox_profiles::access as a;
    let mut h = Sha256::new();
    let mut put = |s: &str| {
        h.update((s.len() as u64).to_be_bytes());
        h.update(s.as_bytes());
    };
    #[cfg(target_os = "macos")]
    {
        use init_sandbox_profiles::seatbelt as s;
        for t in [s::PRELUDE, s::BODY_SECTION, s::BUILD_SECTION, s::GIT_READ_SECTION, s::KERNEL_SECTION, s::ROOT_ALLOWS_TEMPLATE, s::ZONE_TAIL,
            s::ZONE_TAIL_FULL, s::SEALED_TAIL]
        {
            put(t);
        }
    }
    for list in [&a::BUILTIN_READ_DENY[..], &a::BUILTIN_WRITE_DENY[..], &a::FULL_READ_DENY[..], &a::FULL_WRITE_DENY[..]] {
        put(&list.len().to_string());
        list.iter().for_each(|x| put(x));
    }
    util::hex(&h.finalize())
}

/// The newest `root_start`'s binary hash (None before the first start of a Root that writes one, v0.5.3 on).
fn last_start(recs: &[Value]) -> Option<&str> {
    recs.iter().rev().find(|r| r["kind"] == "root_start" && r["by"] == "root").and_then(|r| r["data"]["root_sha256"].as_str())
}

/// The start check: `sha` (this binary) against the newest `root_start`. Ok also when the owner's upgrade installed
/// this binary (`to.root_sha256` of a journal in `replaced`, `committing` or `committed`) or its rollback restored it
/// (`from.root_sha256` of one in `rolled_back` or `failed`). Err: the message for the owner.
pub fn expected(recs: &[Value], journal: &Value, sha: &str) -> Result<(), String> {
    let Some(prev) = last_start(recs) else { return Ok(()) };
    if prev == sha {
        return Ok(());
    }
    let phase = journal["phase"].as_str().unwrap_or("");
    let installed = matches!(phase, "replaced" | "committing" | "committed") && journal["to"]["root_sha256"] == sha;
    let restored = matches!(phase, "rolled_back" | "failed") && journal["from"]["root_sha256"] == sha;
    if installed || restored {
        return Ok(());
    }
    Err(format!("this Root binary (sha256 {}…) is not the one the last Root start recorded ({}…), and no upgrade installed it",
        &sha[..sha.len().min(16)], &prev[..prev.len().min(16)]))
}

/// At daemon start (sup.rs `start`): hashes, the start check, then `root_start`, which becomes A0's reference only once
/// it is recorded. Err: the zone must not start (the caller halts it with `boundary`).
pub fn start(root: &Root, recs: &[Value]) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("the Root cannot locate its own binary: {e}"))?;
    let sha = crate::tuple::sha_of(&exe).ok_or_else(|| format!("the Root cannot read its own binary {}", exe.display()))?;
    let journal: Value = std::fs::read_to_string(root.p.r("state/upgrade.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let check = expected(recs, &journal, &sha);
    let charter = crate::link::lock(&root.st).charter.as_ref().map(|l| l.digest.clone());
    let s = Start { exe, root_sha256: sha, profiles_sha256: profiles_sha256() };
    let data = json!({"charter": charter, "profiles_sha256": s.profiles_sha256, "root_sha256": s.root_sha256, "root_version": crate::install::VERSION});
    if root.append("root_start", data).is_some() {
        let _ = root.boundary.set(s);
    }
    check
}

/// A0's re-hash at an admission: the binary file, the profile texts and the active charter file against this start's
/// `root_start` and the active charter's text (its sha256, so a check never depends on the filesystem the charter's
/// rules read). Err names what changed.
pub fn check(root: &Root) -> Result<(), String> {
    let s = root.boundary.get().ok_or("this Root recorded no root_start at its start, so A0 has no reference")?;
    if crate::tuple::sha_of(&s.exe).as_deref() != Some(s.root_sha256.as_str()) {
        return Err(format!("the Root binary {} no longer hashes to {}… (root_start)", s.exe.display(), &s.root_sha256[..16]));
    }
    if profiles_sha256() != s.profiles_sha256 {
        return Err("the compiled-in sandbox profiles no longer hash as at root_start".into());
    }
    // under the lineage lock: `apply_charter` writes the active file and moves the digest while holding it.
    // V3 (v0.5.3): the file's bytes against the text the Root loaded or applied (`Loaded::src`, which every writer of the
    // active file writes), not a full `charter::load`: its rules also read the filesystem (C14: an `extra_read` folder
    // must exist), which the agent can change in full access without any Root file changing
    let _lg = crate::link::lock(&root.lin);
    let active = std::fs::read(root.p.r("state/charter.active.toml")).ok().map(|b| util::sha256_hex(&b));
    let want = crate::link::lock(&root.st).charter.as_ref().map(|l| util::sha256_hex(l.src.as_bytes()));
    if active.is_none() || active != want {
        return Err(format!("the active charter state/charter.active.toml is no longer the text the Root applied (sha256 {}…)",
            want.as_deref().map_or("none", |w| &w[..w.len().min(16)])));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_start_check_follows_root_start_and_the_upgrade_journal() {
        let (a, b, c) = ("a".repeat(64), "b".repeat(64), "c".repeat(64));
        let start = |sha: &str| json!({"by": "root", "data": {"root_sha256": sha}, "kind": "root_start"});
        let none = Value::Null;
        assert!(expected(&[], &none, &a).is_ok(), "no root_start yet: the first start of a Root that writes one");
        assert!(expected(&[start(&a)], &none, &a).is_ok());
        assert!(expected(&[start(&a), start(&b)], &none, &b).is_ok(), "the newest start counts");
        let e = expected(&[start(&b), start(&a)], &none, &b).unwrap_err();
        assert!(e.contains("bbbbbbbbbbbbbbbb") && e.contains("aaaaaaaaaaaaaaaa"), "{e}");
        let kernel_says = json!({"by": "kernel:000000000000", "data": {"root_sha256": b}, "kind": "root_start"});
        assert!(expected(&[start(&a), kernel_says], &none, &b).is_err(), "only the Root's own records count");
        let j = |phase: &str| json!({"from": {"root_sha256": a}, "phase": phase, "to": {"root_sha256": b}});
        for phase in ["replaced", "committing", "committed"] {
            assert!(expected(&[start(&a)], &j(phase), &b).is_ok(), "{phase}: the upgrade installed it");
            assert!(expected(&[start(&a)], &j(phase), &c).is_err(), "{phase}: a third binary");
        }
        for phase in ["rolled_back", "failed"] {
            assert!(expected(&[start(&b)], &j(phase), &a).is_ok(), "{phase}: the rollback restored it");
            assert!(expected(&[start(&a)], &j(phase), &b).is_err(), "{phase}: the release's binary is not the one restored");
        }
        assert!(expected(&[start(&a)], &j("replacing"), &b).is_err(), "a replace that has not finished installs nothing yet");
    }

    #[test]
    fn the_profile_digest_is_stable_and_covers_the_texts() {
        let d = profiles_sha256();
        assert_eq!((d.len(), d.clone()), (64, profiles_sha256()));
        assert_ne!(d, util::sha256_hex(b""));
    }
}
