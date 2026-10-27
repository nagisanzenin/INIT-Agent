//! ADR-009 (build/specs/ADR-009-spec.md) and ADR-012: what the agent zone may read and write.
//!
//! Workspace mode (ADR-009, `sandbox.mode = "workspace"`):
//! - **Read:** a body may read the owner's home, except INIT_HOME (whose own rules stay the SECTION's), the
//!   Root's paths (tail Z1, Z2) and the read deny list ([`read_deny`]; tail Z2).
//! - **Write:** the live body may write the owner's workspace ([`validate_workspace`]), never INIT_HOME's protected
//!   parts, the Root's paths or the deny lists.
//!
//! - **Full access (ADR-012, v0.3.1, the default):** the live body reads, writes and runs anything outside INIT_HOME
//!   and uses the IP network, except the Root-protected set: ROOT_HOME, the Root's service file and its directory,
//!   INIT's launchers, INIT_HOME's Root-written parts, [`FULL_READ_DENY`] and the charter's `sandbox.read_deny`
//!   ([`full_read_deny`], [`full_write_deny`]; the full tail `zone-tail-full.sb`).
//!
//! The Root decides the grant from the spawn's kind and lane ([`grant_for`]); the kernel never supplies it.
//! Every path reaches a profile as a Root-set `-DZ_*=` parameter, never as text. Platform-neutral.

use std::path::{Component, Path, PathBuf};

/// The built-in read deny list, relative to the owner's home (spec item 1). ROOT_HOME is covered by tail Z1.
pub const BUILTIN_READ_DENY: [&str; 23] = [
    ".config/init-agent",
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".config/gcloud",
    ".kube",
    ".docker/config.json",
    ".netrc",
    ".npmrc",
    ".pypirc",
    "Library/Keychains",
    "Library/Cookies",
    "Library/Mail",
    "Library/Messages",
    "Library/Safari",
    "Library/Application Support/Google/Chrome",
    "Library/Application Support/Firefox",
    "Library/Application Support/BraveSoftware",
    "Library/Application Support/Microsoft Edge",
    "Library/Application Support/Arc",
    "Library/Application Support/1Password",
    "Library/Group Containers",
];

/// The built-in write deny list, relative to the owner's home (tail Z3a; ADR-009 §9.3 Q5): what the owner's own
/// tools run or load unsandboxed, denied in every mode. The read deny list is write-denied too, and the Root
/// adds INIT's bin directory from `install.json` (`bin_dir`) when it is elsewhere.
pub const BUILTIN_WRITE_DENY: [&str; 20] = [
    ".zshrc",
    ".zprofile",
    ".zshenv",
    ".zlogin",
    ".zlogout",
    ".bashrc",
    ".bash_profile",
    ".bash_login",
    ".bash_logout",
    ".profile",
    ".config/fish",
    ".gitconfig",
    ".config/git",
    ".local/bin",
    "bin",
    "Library/LaunchAgents",
    ".config/autostart",
    ".config/systemd/user",
    ".config/environment.d",
    ".pam_environment",
];

/// ADR-012: what full access still never reads (nor writes), relative to the owner's home: INIT's documented provider
/// key directory. The charter's `sandbox.read_deny` is added (still honoured in full mode); ROOT_HOME is the full tail's Z1.
pub const FULL_READ_DENY: [&str; 1] = [".config/init-agent"];

/// ADR-012: what full access still never writes, relative to the owner's home: the directories the Root's service file
/// lives in on each platform, and every user unit directory systemd reads `init-root.service.d/*.conf` drop-ins from (a
/// drop-in could override the Root's unit or job). INIT's launchers are added by [`full_write_deny`].
pub const FULL_WRITE_DENY: [&str; 4] = ["Library/LaunchAgents", ".config/systemd/user", ".config/systemd/user.control", ".local/share/systemd/user"];

/// ADR-012, Linux: the [`FULL_WRITE_DENY`] directories the Root creates (empty) before a full spawn when they are missing,
/// so the zone can bind them read-only instead of leaving them free to create.
pub const LINUX_UNIT_DIRS: [&str; 3] = [".config/systemd/user", ".config/systemd/user.control", ".local/share/systemd/user"];

/// The charter's `sandbox.read_deny`: at most this many entries.
pub const MAX_CHARTER_DENY: usize = 32;
/// A path parameter's byte limit (V2's).
pub const MAX_PATH_BYTES: usize = 1024;

/// One instance's grant beyond its SECTION. `Default` is none (v0.1).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grant {
    /// The owner's home except INIT_HOME (ROOT_ALLOWS' read line, `Z_HOME`).
    pub read_home: bool,
    /// The workspace (ROOT_ALLOWS' write line and tail Z3, `Z_WRITE`).
    pub write: Option<PathBuf>,
    /// ADR-012 full access (ROOT_ALLOWS' `Z_FULL` group and the full tail): the live body only, never with
    /// `read_home` or `write`, which it subsumes.
    pub full: bool,
}

/// The Root's access state that a grant is derived from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccessState {
    /// Charter `sandbox.read_home` (default true).
    pub read_home: bool,
    /// The validated, canonical workspace, if any.
    pub workspace: Option<PathBuf>,
    /// Full access in effect (ADR-012: the charter's `sandbox.mode`, or the owner's session choice; never with
    /// `sandbox.allow_full = false`).
    pub full: bool,
}

impl AccessState {
    /// `INIT_ACCESS` of a live body: `full`, `workspace`, `read_home` or `none`.
    pub fn level(&self) -> &'static str {
        if self.full {
            "full"
        } else if self.workspace.is_some() {
            "workspace"
        } else if self.read_home {
            "read_home"
        } else {
            "none"
        }
    }
}

/// What a `body`-profile spawn gets: the grant, `INIT_ACCESS` and `INIT_WORKSPACE`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnGrant {
    pub grant: Grant,
    pub access: &'static str,
    pub workspace: Option<PathBuf>,
}

/// The grant for a spawn of profile `body`: the live body (kind `body`, `live`) gets everything the state
/// allows; a fleet member (kind `body`, not live) only the home read; a probe (kind `probe`) nothing, so gate
/// evaluation stays hermetic. With full access (ADR-012) the live body gets `full` alone: it subsumes the home
/// read and the workspace write. Members and probes are never full.
pub fn grant_for(kind: &str, live: bool, st: &AccessState, _home: &Path) -> SpawnGrant {
    let none = SpawnGrant { grant: Grant::default(), access: "none", workspace: None };
    match (kind, live) {
        ("body", true) if st.full => SpawnGrant { grant: Grant { read_home: false, write: None, full: true }, access: "full", workspace: st.workspace.clone() },
        ("body", true) => {
            let grant = Grant { read_home: st.read_home, write: st.workspace.clone(), full: false };
            SpawnGrant { grant, access: st.level(), workspace: st.workspace.clone() }
        }
        ("body", false) if st.read_home => SpawnGrant { grant: Grant { read_home: true, ..Grant::default() }, access: "read_home", workspace: None },
        _ => none,
    }
}

/// Resolve `p` the way Seatbelt sees it: an existing path canonicalized (symlinks resolved); a missing one
/// as its deepest existing ancestor canonicalized plus the rest, kept literal.
pub fn resolve(p: &Path) -> PathBuf {
    if let Ok(c) = std::fs::canonicalize(p) {
        return c;
    }
    let mut rest = Vec::new();
    let mut cur = p;
    while let Some(parent) = cur.parent() {
        if let Some(name) = cur.file_name() {
            rest.push(name.to_os_string());
        }
        if let Ok(c) = std::fs::canonicalize(parent) {
            return rest.iter().rev().fold(c, |acc, n| acc.join(n));
        }
        cur = parent;
    }
    p.to_path_buf()
}

/// A charter `sandbox.read_deny` entry (C15): absolute, no `.`/`..`, no NUL, at most 1024 bytes, not `/`.
/// None when the entry is malformed.
pub fn charter_entry(entry: &str) -> Option<PathBuf> {
    let p = PathBuf::from(entry);
    let ok = entry.starts_with('/')
        && entry.len() <= MAX_PATH_BYTES
        && !entry.contains('\0')
        && !entry.split('/').any(|c| c == "." || c == "..")
        && p.components().all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
        && p != Path::new("/");
    ok.then_some(p)
}

/// The read deny list as tail parameters (`Z_DENY_<i>`): the built-in entries under `home`, then the
/// charter's, each resolved as Seatbelt sees it; when a symlink resolves elsewhere, the literal path is
/// denied too. No duplicates; order kept.
pub fn read_deny(home: &Path, charter: &[PathBuf]) -> Vec<PathBuf> {
    resolved(BUILTIN_READ_DENY.iter().map(|r| home.join(r)).chain(charter.iter().cloned()))
}

/// The write deny list as tail parameters (`Z_WDENY_<i>`, Z3a): the built-in entries under `home`, then
/// `extra` (INIT's bin directory), resolved as [`read_deny`] does.
pub fn write_deny(home: &Path, extra: &[PathBuf]) -> Vec<PathBuf> {
    resolved(BUILTIN_WRITE_DENY.iter().map(|r| home.join(r)).chain(extra.iter().cloned()))
}

/// ADR-012: the full tail's read deny list (`Z_DENY_<i>`): [`FULL_READ_DENY`] under `home`, then the charter's
/// `sandbox.read_deny`, resolved as [`read_deny`] does.
pub fn full_read_deny(home: &Path, charter: &[PathBuf]) -> Vec<PathBuf> {
    resolved(FULL_READ_DENY.iter().map(|r| home.join(r)).chain(charter.iter().cloned()))
}

/// INIT's launchers in the bin directory: `init-agent` and the TUI, and `init`, the launcher's name before v0.4.1 (on
/// macOS v0.4.1 installs a shim under it; nagisanzenin/INIT-Agent#1).
pub const LAUNCHERS: [&str; 3] = ["init-agent", "init", "init-tui"];

/// ADR-012: the full tail's write deny list (`Z_WDENY_<i>`): [`FULL_WRITE_DENY`] under `home`, then INIT's launchers
/// ([`LAUNCHERS`] in `bin_dir`, from `install.json`, default `~/.local/bin`) as file paths: the rest of the bin
/// directory is writable in full mode.
pub fn full_write_deny(home: &Path, bin_dir: Option<&Path>) -> Vec<PathBuf> {
    let bin = bin_dir.map_or_else(|| home.join(".local/bin"), Path::to_path_buf);
    resolved(FULL_WRITE_DENY.iter().map(|r| home.join(r)).chain(LAUNCHERS.iter().map(|n| bin.join(n))))
}

fn resolved(all: impl Iterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for p in all {
        for q in [resolve(&p), p] {
            if !out.contains(&q) {
                out.push(q);
            }
        }
    }
    out
}

/// Why a launch directory gives no workspace (spec item 2). `code()` is the wire `Reason`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Relative,
    TooLong,
    Missing,
    NotDir,
    OutsideHome,
    Home,
    RootPath,
    DenyList,
}

impl Refusal {
    pub fn code(self) -> &'static str {
        match self {
            Refusal::Relative => "relative",
            Refusal::TooLong => "too_long",
            Refusal::Missing => "missing",
            Refusal::NotDir => "not_dir",
            Refusal::OutsideHome => "outside_home",
            Refusal::Home => "home",
            Refusal::RootPath => "root_path",
            Refusal::DenyList => "deny_list",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Refusal::Relative => "the launch directory is not an absolute path",
            Refusal::TooLong => "the launch directory's path is over 1024 bytes",
            Refusal::Missing => "the launch directory does not exist",
            Refusal::NotDir => "the launch directory is not a directory",
            Refusal::OutsideHome => "the launch directory is outside your home",
            Refusal::Home => "the launch directory is your home itself",
            Refusal::RootPath => "the launch directory is, holds or lies in the Root's or the agent's own files",
            Refusal::DenyList => "the launch directory lies in a protected (deny-listed) path",
        }
    }
}

/// The paths a workspace may neither lie in nor contain: ROOT_HOME, INIT_HOME and the Root's service directory.
pub struct Fixed<'a> {
    pub home: &'a Path,
    pub root_home: &'a Path,
    pub init_home: &'a Path,
    pub root_svc_dir: &'a Path,
}

/// Spec item 2: absolute and canonical (symlinks resolved), an existing directory inside the owner's home,
/// not the home itself, not inside or containing ROOT_HOME, INIT_HOME or the Root's service directory, and
/// not inside an entry of `deny` (the read and the write deny lists). A workspace that *contains* an entry
/// is allowed: the tail still denies that entry (Z2, Z3a). Returns the canonical workspace.
pub fn validate_workspace(cwd: &str, f: &Fixed, deny: &[PathBuf]) -> Result<PathBuf, Refusal> {
    if !cwd.starts_with('/') || cwd.contains('\0') {
        return Err(Refusal::Relative);
    }
    if cwd.len() > MAX_PATH_BYTES {
        return Err(Refusal::TooLong);
    }
    let ws = std::fs::canonicalize(cwd).map_err(|_| Refusal::Missing)?;
    if !ws.is_dir() {
        return Err(Refusal::NotDir);
    }
    if ws.as_os_str().len() > MAX_PATH_BYTES {
        return Err(Refusal::TooLong);
    }
    let home = resolve(f.home);
    if ws == home {
        return Err(Refusal::Home);
    }
    if !ws.starts_with(&home) {
        return Err(Refusal::OutsideHome);
    }
    let clash = |p: &Path| {
        let p = resolve(p);
        ws.starts_with(&p) || p.starts_with(&ws)
    };
    if [f.root_home, f.init_home, f.root_svc_dir].into_iter().any(clash) {
        return Err(Refusal::RootPath);
    }
    if deny.iter().any(|d| ws.starts_with(d)) {
        return Err(Refusal::DenyList);
    }
    Ok(ws)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grants_by_kind_and_lane() {
        let home = Path::new("/h");
        let ws = Some(PathBuf::from("/h/p"));
        let st = AccessState { read_home: true, workspace: ws.clone(), full: false };
        let live = grant_for("body", true, &st, home);
        assert_eq!(live.grant, Grant { read_home: true, write: ws.clone(), full: false });
        assert_eq!((live.access, live.workspace.clone()), ("workspace", ws.clone()));
        let member = grant_for("body", false, &st, home);
        assert_eq!((member.grant, member.access, member.workspace), (Grant { read_home: true, ..Grant::default() }, "read_home", None));
        let probe = grant_for("probe", true, &st, home);
        assert_eq!((probe.grant, probe.access), (Grant::default(), "none"));
        // ADR-012: full access is the live body's alone, with or without a workspace; it subsumes read and write
        for ws in [None, ws.clone()] {
            let full = AccessState { read_home: false, workspace: ws.clone(), full: true };
            let g = grant_for("body", true, &full, home);
            assert_eq!((g.grant, g.access, g.workspace), (Grant { read_home: false, write: None, full: true }, "full", ws));
            let full = AccessState { read_home: true, ..full };
            assert!(!grant_for("body", false, &full, home).grant.full, "a fleet member is never full");
            assert_eq!(grant_for("body", false, &full, home).grant, Grant { read_home: true, ..Grant::default() });
            assert_eq!(grant_for("probe", true, &full, home).grant, Grant::default(), "a probe is never full");
            assert_eq!(grant_for("probe", false, &full, home).grant, Grant::default());
        }
        let off = AccessState { read_home: false, workspace: None, full: false };
        assert_eq!(grant_for("body", true, &off, home).access, "none");
        assert_eq!(grant_for("body", false, &off, home).grant, Grant::default());
        let ro = AccessState { read_home: true, ..off };
        assert_eq!(grant_for("body", true, &ro, home).access, "read_home");
    }

    #[test]
    fn charter_entries() {
        assert_eq!(charter_entry("/Volumes/x"), Some(PathBuf::from("/Volumes/x")));
        assert_eq!(charter_entry("/h/Tax Returns"), Some(PathBuf::from("/h/Tax Returns")));
        for bad in ["~", "~/x", "rel", "/a/../b", "/a/./b", "/", "", "/a\0b"] {
            assert_eq!(charter_entry(bad), None, "{bad:?}");
        }
        assert_eq!(charter_entry(&format!("/{}", "a".repeat(MAX_PATH_BYTES))), None);
    }

    #[test]
    fn deny_list_has_the_builtins_then_the_charter() {
        let h = Path::new("/nonexistent-adr009-home");
        let d = read_deny(h, &[PathBuf::from("/nonexistent-adr009-home/tax")]);
        assert_eq!(d.first(), Some(&h.join(".config/init-agent")));
        assert!(d.contains(&h.join("Library/Group Containers")));
        assert_eq!(d.last(), Some(&h.join("tax")));
        assert_eq!(d.len(), BUILTIN_READ_DENY.len() + 1, "missing paths resolve to themselves: no duplicates");
    }

    /// ADR-012: the full lists keep the key directory, the charter's entries, the service directories and INIT's
    /// launchers (file paths in bin_dir, default ~/.local/bin: init-agent, init, init-tui), and nothing of the ADR-009
    /// secrets list.
    #[test]
    fn full_lists_keep_only_the_root_protected_set() {
        let h = Path::new("/nonexistent-adr012-home");
        let r = full_read_deny(h, &[h.join("tax")]);
        assert_eq!(r, vec![h.join(".config/init-agent"), h.join("tax")]);
        assert!(!r.contains(&h.join(".ssh")), "the ADR-009 secrets list is dropped in full mode");
        let w = full_write_deny(h, None);
        assert_eq!(w, vec![h.join("Library/LaunchAgents"), h.join(".config/systemd/user"), h.join(".config/systemd/user.control"),
            h.join(".local/share/systemd/user"), h.join(".local/bin/init-agent"), h.join(".local/bin/init"), h.join(".local/bin/init-tui")]);
        assert!(LINUX_UNIT_DIRS.iter().all(|d| FULL_WRITE_DENY.contains(d)));
        let w = full_write_deny(h, Some(Path::new("/nonexistent-adr012-bin")));
        assert_eq!(&w[4..], [PathBuf::from("/nonexistent-adr012-bin/init-agent"), PathBuf::from("/nonexistent-adr012-bin/init"),
            PathBuf::from("/nonexistent-adr012-bin/init-tui")]);
        assert!(!w.contains(&h.join(".zshrc")) && !w.contains(&h.join(".local/bin")), "shell rc and the bin dir are writable in full mode");
    }
}
