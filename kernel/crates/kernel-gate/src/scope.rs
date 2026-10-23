//! Gate scope from the changed paths (kernel/gate.md §3.1; evolution.md §3.2, §3.3), including the
//! lockfile rule for what counts as a dependency change (ext.md E10).

use std::collections::BTreeMap;

use serde::Serialize;

/// Scope of a body candidate (kernel candidates are scope `k`, decided by their own command).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    T0,
    T0Escalated,
    Dep,
    T1,
    T2,
}

/// When G4 runs relative to the version going live (supervise.md Q5b, §3.6.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum G4Need {
    /// No G4: LKG on a G0/G1 pass.
    None,
    /// Before `main` moves (t0_escalated) or before the handoff (dep).
    Sync,
    /// After the version becomes provisional.
    Async,
}

/// The classification supervise needs for one candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ScopeDecision {
    pub scope: Scope,
    pub stages: &'static [&'static str],
    pub needs_handoff: bool,
    pub needs_g4: G4Need,
}

impl Scope {
    pub fn decision(self) -> ScopeDecision {
        let (stages, needs_handoff, needs_g4): (&'static [&'static str], bool, G4Need) = match self
        {
            Scope::T0 => (&["G0", "G1"], false, G4Need::None),
            Scope::T0Escalated => (&["G0", "G1", "G4"], false, G4Need::Sync),
            Scope::Dep => (&["G0", "G1", "G2", "G3", "G4"], true, G4Need::Sync),
            Scope::T1 => (&["G0", "G1", "G2", "G3"], false, G4Need::Async),
            Scope::T2 => (&["G0", "G1", "G2", "G3"], true, G4Need::Async),
        };
        ScopeDecision {
            scope: self,
            stages,
            needs_handoff,
            needs_g4,
        }
    }

    /// The decision on the free path (ADR-011 §2.1, charter `extend.mode = "free"`): no G4 on any
    /// scope (`needs_g4 = None`), a wholesale T0 rewrite is plain `t0`, and the stages are the
    /// kernel-run admission checks: none for `t0` (A0 and A5 are the Root's), A1 = G0 (with G1, which
    /// G2 reads) for `t1`, A1 and A2 = G0, G1, G2 for `t2` and `dep`. G3 and the body's tests run
    /// after the change is live (A3) and can only veto.
    pub fn decision_free(self) -> ScopeDecision {
        let scope = if self == Scope::T0Escalated { Scope::T0 } else { self };
        let (stages, needs_handoff): (&'static [&'static str], bool) = match scope {
            Scope::T0 | Scope::T0Escalated => (&[], false),
            Scope::T1 => (&["G0", "G1"], false),
            Scope::Dep | Scope::T2 => (&["G0", "G1", "G2"], true),
        };
        ScopeDecision {
            scope,
            stages,
            needs_handoff,
            needs_g4: G4Need::None,
        }
    }

    /// The scope name `change.open` takes (`t0`|`t1`|`t2`|`dep`; ADR-011 Records).
    pub fn change_scope(self) -> &'static str {
        match self {
            Scope::T0 | Scope::T0Escalated => "t0",
            Scope::T1 => "t1",
            Scope::T2 => "t2",
            Scope::Dep => "dep",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Scope::T0 => "t0",
            Scope::T0Escalated => "t0_escalated",
            Scope::Dep => "dep",
            Scope::T1 => "t1",
            Scope::T2 => "t2",
        }
    }
}

/// One file of `git diff --numstat <running> <candidate>` plus the parent's line count of that file
/// (0 for a new file; binary files have no counts and never count as a wholesale rewrite).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub added: Option<u64>,
    pub deleted: Option<u64>,
    pub old_lines: u64,
}

/// T0 paths: `skills/**`, `agents/**` (ADR-011 §1.1: sub-agent specs), `SELF.md`, `GENESIS.md`.
pub fn is_t0(path: &str) -> bool {
    path.starts_with("skills/")
        || path.starts_with("agents/")
        || path == "SELF.md"
        || path == "GENESIS.md"
}

/// ADR-011 §6.2–§6.3: `archive/**` and `quarantine/**` hold archived and quarantined caps
/// (`archive/<kind>/<name>/`). They are never loaded, never built and outside the `t0` blob, so a
/// change there never raises the scope: moving a skill out of `skills/` is `t0`, moving an ext
/// component out of `ext/` is `t1`.
pub fn is_inert(path: &str) -> bool {
    path.starts_with("archive/") || path.starts_with("quarantine/")
}

fn is_ext(path: &str) -> bool {
    path.starts_with("ext/")
}

/// evolution.md §3.3: `min(added, deleted) > 0.5 × old_line_count` on any T0 file.
pub fn wholesale(c: &FileChange) -> bool {
    match (c.added, c.deleted) {
        (Some(a), Some(d)) => (a.min(d) as f64) > 0.5 * c.old_lines as f64,
        _ => false,
    }
}

/// Error parsing a lockfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockError(pub String);

/// (name, version, source) → (checksum, sorted dependencies).
type RegistryPackages = BTreeMap<(String, String, String), (String, Vec<String>)>;

/// `[[package]]` entries with a `source`.
fn registry_packages(lock: &str) -> Result<RegistryPackages, LockError> {
    let v: toml::Table = lock
        .parse()
        .map_err(|e: toml::de::Error| LockError(e.to_string()))?;
    let mut m = BTreeMap::new();
    if let Some(pkgs) = v.get("package").and_then(|p| p.as_array()) {
        for p in pkgs {
            let get = |k: &str| p.get(k).and_then(|x| x.as_str()).map(str::to_string);
            let Some(source) = get("source") else {
                continue;
            };
            let name = get("name").unwrap_or_default();
            let version = get("version").unwrap_or_default();
            let checksum = get("checksum").unwrap_or_default();
            let mut deps: Vec<String> = p
                .get("dependencies")
                .and_then(|d| d.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            deps.sort();
            m.insert((name, version, source), (checksum, deps));
        }
    }
    Ok(m)
}

/// gate.md §3.1: a lockfile change is a dependency change iff some entry **with a `source`** is added,
/// removed, or changes `checksum` or `dependencies`. Source-less entries (workspace members, e.g. a new
/// extension in `ext/Cargo.lock`) never are. A missing file reads as empty.
pub fn lock_dep_change(parent: Option<&str>, candidate: Option<&str>) -> Result<bool, LockError> {
    let a = registry_packages(parent.unwrap_or(""))?;
    let b = registry_packages(candidate.unwrap_or(""))?;
    Ok(a != b)
}

/// ADR-011 §7.4: the registry crates a `dep` change brings in, as (name, version, checksum), sorted
/// and de-duplicated over both lockfiles: every entry with a `source` that is new or whose checksum
/// or dependencies changed. Removed crates bring nothing in. The Root admits the change only when a
/// live owner `dep` grant covers exactly this set.
pub fn dep_crates(locks: &Lockfiles<'_>) -> Result<Vec<(String, String, String)>, LockError> {
    let mut out = std::collections::BTreeSet::new();
    for (parent, candidate) in [
        (locks.body_parent, locks.body_candidate),
        (locks.ext_parent, locks.ext_candidate),
    ] {
        let a = registry_packages(parent.unwrap_or(""))?;
        for (k, v) in registry_packages(candidate.unwrap_or(""))? {
            if a.get(&k) != Some(&v) {
                out.insert((k.0, k.1, v.0));
            }
        }
    }
    Ok(out.into_iter().collect())
}

/// The lockfiles of both versions; `None` = the file does not exist at that version.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lockfiles<'a> {
    pub body_parent: Option<&'a str>,
    pub body_candidate: Option<&'a str>,
    pub ext_parent: Option<&'a str>,
    pub ext_candidate: Option<&'a str>,
}

/// The scope table of gate.md §3.1, on `$G diff --name-only <running> <candidate>` (as [`FileChange`]s;
/// `self.json` is ignored). Returns the decision and whether a lockfile dependency change was seen.
pub fn classify(
    changes: &[FileChange],
    locks: &Lockfiles<'_>,
) -> Result<(ScopeDecision, bool), LockError> {
    let changes: Vec<&FileChange> = changes.iter().filter(|c| c.path != "self.json").collect();
    let mut dep = changes.iter().any(|c| c.path.starts_with("vendor/"));
    let mut lock_dep = false;
    if changes.iter().any(|c| c.path == "Cargo.lock") {
        lock_dep |= lock_dep_change(locks.body_parent, locks.body_candidate)?;
    }
    if changes.iter().any(|c| c.path == "ext/Cargo.lock") {
        lock_dep |= lock_dep_change(locks.ext_parent, locks.ext_candidate)?;
    }
    dep |= lock_dep;
    let scope = if changes.iter().all(|c| is_t0(&c.path) || is_inert(&c.path)) {
        if changes.iter().any(|c| is_t0(&c.path) && wholesale(c)) {
            Scope::T0Escalated
        } else {
            Scope::T0
        }
    } else if dep {
        Scope::Dep
    } else if changes
        .iter()
        .all(|c| is_t0(&c.path) || is_ext(&c.path) || is_inert(&c.path))
    {
        Scope::T1
    } else {
        Scope::T2
    };
    Ok((scope.decision(), lock_dep))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(path: &str, added: u64, deleted: u64, old: u64) -> FileChange {
        FileChange {
            path: path.into(),
            added: Some(added),
            deleted: Some(deleted),
            old_lines: old,
        }
    }

    const LOCK_A: &str = r#"
version = 4

[[package]]
name = "init-body"
version = "0.1.0"
dependencies = ["serde"]

[[package]]
name = "serde"
version = "1.0.229"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "aaaa"
"#;

    /// G-01: each row of the scope table, the wholesale T0 rewrite and a `vendor/` change.
    #[test]
    fn g01_scope_table() {
        let none = Lockfiles::default();
        let s = |c: &[FileChange]| classify(c, &none).unwrap().0;
        assert_eq!(
            s(&[
                ch("SELF.md", 3, 1, 80),
                ch("skills/x/SKILL.md", 10, 0, 0),
                ch("self.json", 5, 5, 300)
            ])
            .scope,
            Scope::T0
        );
        let esc = s(&[ch("GENESIS.md", 41, 42, 80)]);
        assert_eq!(
            (esc.scope, esc.needs_g4, esc.needs_handoff),
            (Scope::T0Escalated, G4Need::Sync, false)
        );
        // Exactly half is not wholesale: min(40, 40) = 40 = 0.5 × 80.
        assert_eq!(s(&[ch("GENESIS.md", 40, 40, 80)]).scope, Scope::T0);
        let dep = s(&[
            ch("vendor/serde/src/lib.rs", 1, 1, 100),
            ch("src/main.rs", 1, 0, 10),
        ]);
        assert_eq!(
            (dep.scope, dep.needs_g4, dep.needs_handoff),
            (Scope::Dep, G4Need::Sync, true)
        );
        let t1 = s(&[
            ch("ext/pty/src/lib.rs", 50, 0, 0),
            ch("ext/pty/Cargo.toml", 8, 0, 0),
            ch("SELF.md", 2, 0, 80),
        ]);
        assert_eq!(
            (t1.scope, t1.needs_g4, t1.needs_handoff),
            (Scope::T1, G4Need::Async, false)
        );
        let t2 = s(&[ch("src/loop.rs", 5, 2, 400)]);
        assert_eq!(
            (t2.scope, t2.needs_g4, t2.needs_handoff, t2.stages.len()),
            (Scope::T2, G4Need::Async, true, 4)
        );
        // A wholesale rewrite outside T0 paths is just t2.
        assert_eq!(s(&[ch("src/loop.rs", 400, 400, 400)]).scope, Scope::T2);
        // Binary T0 files never escalate.
        let bin = FileChange {
            path: "skills/x/logo.png".into(),
            added: None,
            deleted: None,
            old_lines: 0,
        };
        assert_eq!(s(&[bin]).scope, Scope::T0);
    }

    /// ADR-011 X1/X2: `agents/**` is a T0 path, and the free decision has no G4 and no G3.
    #[test]
    fn free_scope_decisions() {
        let none = Lockfiles::default();
        let s = |c: &[FileChange]| classify(c, &none).unwrap().0;
        assert!(is_t0("agents/reviewer/AGENT.md"));
        assert!(!is_t0("agentsx/a"));
        assert_eq!(s(&[ch("agents/reviewer/AGENT.md", 30, 0, 0), ch("SELF.md", 1, 0, 80)]).scope, Scope::T0);
        assert_eq!(s(&[ch("agents/r/AGENT.md", 1, 0, 0), ch("ext/pty/src/lib.rs", 3, 0, 0)]).scope, Scope::T1);
        for sc in [Scope::T0, Scope::T0Escalated, Scope::Dep, Scope::T1, Scope::T2] {
            let d = sc.decision_free();
            assert_eq!(d.needs_g4, G4Need::None, "{sc:?}");
            assert!(!d.stages.contains(&"G3") && !d.stages.contains(&"G4"), "{sc:?}");
        }
        let e = Scope::T0Escalated.decision_free();
        assert_eq!((e.scope, e.stages.len(), e.needs_handoff, Scope::T0Escalated.change_scope()), (Scope::T0, 0, false, "t0"));
        let t2 = Scope::T2.decision_free();
        assert_eq!((t2.stages, t2.needs_handoff), (&["G0", "G1", "G2"][..], true));
        assert_eq!((Scope::T1.decision_free().stages, Scope::T1.decision_free().needs_handoff), (&["G0", "G1"][..], false));
        assert_eq!((Scope::Dep.change_scope(), Scope::T1.change_scope(), Scope::T2.change_scope()), ("dep", "t1", "t2"));
        // ADR-011 §6: archive and quarantine moves never raise the scope.
        assert!(is_inert("archive/skill/x/SKILL.md") && is_inert("quarantine/ext/y/Cargo.toml") && !is_inert("archived/x"));
        let mv = |from: &str, to: &str| [FileChange { path: from.into(), added: Some(0), deleted: Some(40), old_lines: 40 }, ch(to, 40, 0, 0)];
        assert_eq!(s(&mv("skills/x/SKILL.md", "archive/skill/x/SKILL.md")).scope, Scope::T0, "a skill move is t0, never escalated");
        assert_eq!(s(&mv("agents/r/AGENT.md", "quarantine/agent/r/AGENT.md")).scope, Scope::T0);
        assert_eq!(s(&mv("archive/skill/x/SKILL.md", "skills/x/SKILL.md")).scope, Scope::T0, "a restore");
        assert_eq!(s(&mv("ext/y/src/lib.rs", "archive/ext/y/src/lib.rs")).scope, Scope::T1, "an ext move rebuilds the set");
        assert_eq!(s(&[ch("archive/skill/old/SKILL.md", 0, 12, 12)]).scope, Scope::T0, "pruning the archive");
        assert_eq!(s(&[ch("archive/skill/x/SKILL.md", 3, 0, 0), ch("src/main.rs", 1, 0, 10)]).scope, Scope::T2);
    }

    #[test]
    fn lockfile_dependency_rule() {
        // A new extension adds only a source-less member to ext/Cargo.lock: not a dependency change → t1.
        let ext_new = format!("{LOCK_A}\n[[package]]\nname = \"pty\"\nversion = \"0.1.0\"\n");
        let locks = Lockfiles {
            ext_parent: Some(LOCK_A),
            ext_candidate: Some(&ext_new),
            ..Default::default()
        };
        let (d, lock_dep) = classify(
            &[
                ch("ext/Cargo.lock", 4, 0, 20),
                ch("ext/pty/src/lib.rs", 30, 0, 0),
            ],
            &locks,
        )
        .unwrap();
        assert_eq!((d.scope, lock_dep), (Scope::T1, false));
        // A registry entry whose checksum changes → dep.
        let changed = LOCK_A.replace("aaaa", "bbbb");
        let locks = Lockfiles {
            body_parent: Some(LOCK_A),
            body_candidate: Some(&changed),
            ..Default::default()
        };
        let (d, lock_dep) = classify(&[ch("Cargo.lock", 1, 1, 20)], &locks).unwrap();
        assert_eq!((d.scope, lock_dep), (Scope::Dep, true));
        // A new registry package → dep; editing only a member's dependency list → not dep.
        let added = format!(
            "{LOCK_A}\n[[package]]\nname = \"itoa\"\nversion = \"1.0.0\"\nsource = \"registry+x\"\nchecksum = \"cc\"\n"
        );
        assert!(lock_dep_change(Some(LOCK_A), Some(&added)).unwrap());
        let member_edit = LOCK_A.replace("dependencies = [\"serde\"]", "dependencies = []");
        assert!(!lock_dep_change(Some(LOCK_A), Some(&member_edit)).unwrap());
        // A changed registry package dependency list → dep.
        let dep_list = LOCK_A.replace(
            "checksum = \"aaaa\"",
            "checksum = \"aaaa\"\ndependencies = [\"serde_derive\"]",
        );
        assert!(lock_dep_change(Some(LOCK_A), Some(&dep_list)).unwrap());
        assert!(lock_dep_change(Some(LOCK_A), Some("not = [toml")).is_err());
        assert!(!lock_dep_change(None, None).unwrap());
        // ADR-011 §7.4: the crate set a `dep` grant must cover: new or changed registry entries of both
        // lockfiles, sorted and de-duplicated; removals and members bring nothing in.
        let t = |n: &str, v: &str, c: &str| (n.to_string(), v.to_string(), c.to_string());
        let both = Lockfiles { body_parent: Some(LOCK_A), body_candidate: Some(&added), ext_parent: None, ext_candidate: Some(&changed) };
        assert_eq!(dep_crates(&both).unwrap(), vec![t("itoa", "1.0.0", "cc"), t("serde", "1.0.229", "bbbb")]);
        let removed = Lockfiles { body_parent: Some(&added), body_candidate: Some(LOCK_A), ..Default::default() };
        assert!(dep_crates(&removed).unwrap().is_empty());
        let member = Lockfiles { ext_parent: Some(LOCK_A), ext_candidate: Some(&ext_new), ..Default::default() };
        assert!(dep_crates(&member).unwrap().is_empty());
    }
}
