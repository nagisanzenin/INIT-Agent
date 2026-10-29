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
//!   INIT's launchers, INIT_HOME's Root-written parts, [`FULL_READ_DENY`], every other INIT install on the account
//!   ([`other_installs`]) and the charter's `sandbox.read_deny` ([`full_read_deny`], [`full_write_deny`]; the full tail
//!   `zone-tail-full.sb`).
//!
//! The Root decides the grant from the spawn's kind and lane ([`grant_for`]); the kernel never supplies it.
//! Every path reaches a profile as a Root-set `-DZ_*=` parameter, never as text. Platform-neutral.

use std::path::{Component, Path, PathBuf};

/// The built-in read deny list, relative to the owner's home (spec item 1): the owner's secret stores. ROOT_HOME is
/// covered by tail Z1, other INIT installs by [`other_installs`]. v0.5.3 (lane H) added git's and GitHub's credential
/// stores, the cargo, gem, Cloudflare (`cloudflared`'s `~/.cloudflared` too), Terraform, Vault, 1Password-CLI, pass, Codex and Claude Code credentials, four
/// more browser and password-manager profiles, and `.init-release` (the maintainer's release signing key, [`FULL_READ_DENY`]).
pub const BUILTIN_READ_DENY: [&str; 46] = [
    ".config/init-agent",
    ".init-release",
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".config/gcloud",
    ".kube",
    ".docker/config.json",
    ".netrc",
    ".git-credentials",
    ".config/git/credentials",
    ".config/gh",
    ".config/hub",
    ".npmrc",
    ".pypirc",
    ".cargo/credentials",
    ".cargo/credentials.toml",
    ".gem/credentials",
    ".config/cloudflare",
    ".cloudflared",
    ".wrangler",
    "Library/Preferences/.wrangler",
    ".terraform.d/credentials.tfrc.json",
    ".vault-token",
    ".config/op",
    ".password-store",
    ".codex/auth.json",
    ".claude/.credentials.json",
    "Library/Keychains",
    "Library/Cookies",
    "Library/Mail",
    "Library/Messages",
    "Library/Safari",
    "Library/Application Support/Google/Chrome",
    "Library/Application Support/Chromium",
    "Library/Application Support/Firefox",
    "Library/Application Support/BraveSoftware",
    "Library/Application Support/Microsoft Edge",
    "Library/Application Support/Arc",
    "Library/Application Support/Vivaldi",
    "Library/Application Support/com.operasoftware.Opera",
    "Library/Application Support/1Password",
    "Library/Application Support/Bitwarden",
    "Library/Group Containers",
    ".local/share/keyrings",
];

/// v0.5.3 (lane H): where another INIT install on this account lives by default, relative to the owner's home: the
/// default ROOT_HOME (its keys, charter and lineage, and its Root's binary) and the default INIT_HOME. An install
/// whose own homes are elsewhere (a test install) must not read or write them, nor they its. [`other_installs`] adds
/// the ones it finds.
pub const OTHER_INSTALLS: [&str; 2] = [".init-root", ".init"];
/// At most this many [`other_installs`] entries (the read deny list's room: [`MAX_CHARTER_DENY`] and the built-ins).
pub const MAX_OTHER_INSTALLS: usize = 16;

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
/// key directory, and (v0.5.3) `.init-release`, where the maintainer keeps INIT's release signing key: an agent that read
/// it could sign a release. Every other INIT install ([`other_installs`]) and the charter's `sandbox.read_deny` are added
/// (still honoured in full mode); ROOT_HOME is the full tail's Z1.
pub const FULL_READ_DENY: [&str; 2] = [".config/init-agent", ".init-release"];

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
    /// `INIT_ACCESS` of a live body: `full`, `workspace` (the workspace and the home read), `workspace_only` (v0.5.3: the
    /// workspace without the home read, `sandbox.read_home = false`), `read_home` or `none`. Each grant has its own
    /// value, so the body's boot block states the grant it has, and a change of `read_home` under a workspace is a
    /// change of the live body's grant (the Root's `restart_pending`).
    pub fn level(&self) -> &'static str {
        match (self.full, self.workspace.is_some(), self.read_home) {
            (true, _, _) => "full",
            (false, true, true) => "workspace",
            (false, true, false) => "workspace_only",
            (false, false, true) => "read_home",
            (false, false, false) => "none",
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

/// v0.5.3 (lane H): the other INIT installs on this account, read- and write-denied to this install's zone in every
/// mode (the Root puts them after the built-in entries of [`read_deny`] and [`full_read_deny`]): [`OTHER_INSTALLS`]
/// under `home`; every top-level entry of `home` named `.init-root*` or that is an INIT ROOT_HOME under another name
/// (`keys/` and an `install.json` naming it); the ROOT_HOME the default launcher `~/.local/bin/init-agent` names
/// (install.sh's `INIT_ROOT_HOME_DEFAULT`); and the INIT_HOME each of those ROOT_HOMEs records. Left out: an entry that
/// is not a valid absolute path ([`charter_entry`]), the home or a directory above it, and anything that is, holds or
/// lies in this install's own `root_home` (tail Z1 has it) or `init_home` (a deny there would lock its own zone out).
/// At most [`MAX_OTHER_INSTALLS`], in a stable order. A ROOT_HOME outside the home under another name is not found
/// (docs/guide.md, "A second install").
pub fn other_installs(home: &Path, root_home: &Path, init_home: &Path) -> Vec<PathBuf> {
    let home = resolve(home);
    let own: Vec<PathBuf> = [root_home, init_home].iter().flat_map(|p| [resolve(p), p.to_path_buf()]).collect();
    let mut found: Vec<PathBuf> = OTHER_INSTALLS.iter().map(|r| home.join(r)).collect();
    let mut top: Vec<PathBuf> = std::fs::read_dir(&home).into_iter().flatten().flatten().map(|e| e.path()).collect();
    top.sort();
    found.extend(top.into_iter().filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(".init-root")) || installed(p).is_some()));
    let launcher = std::fs::read_to_string(home.join(".local/bin/init-agent")).unwrap_or_default();
    let named = launcher.lines().find_map(|l| l.strip_prefix("INIT_ROOT_HOME_DEFAULT=\"")?.strip_suffix('"').map(PathBuf::from));
    found.extend(named.filter(|r| installed(r).is_some()));
    let recorded: Vec<PathBuf> = found.iter().filter_map(|r| installed(r)?["init_home"].as_str().map(PathBuf::from)).collect();
    found.extend(recorded);
    let bad = |p: &Path| home.starts_with(p) || own.iter().any(|o| p.starts_with(o) || o.starts_with(p));
    let mut out: Vec<PathBuf> = Vec::new();
    for p in found {
        let ok = p.to_str().and_then(charter_entry).is_some() && !bad(&p) && !bad(&resolve(&p));
        if ok && !out.contains(&p) && out.len() < MAX_OTHER_INSTALLS {
            out.push(p);
        }
    }
    out
}

/// `dir` is an INIT ROOT_HOME: it holds `keys/` and an `install.json` whose `root_home` is `dir` (install.rs writes it).
/// Its `install.json`, parsed.
fn installed(dir: &Path) -> Option<serde_json::Value> {
    if !dir.join("keys").is_dir() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("install.json")).ok()?).ok()?;
    let rh = v["root_home"].as_str().map(Path::new)?;
    (resolve(rh) == resolve(dir)).then_some(v)
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
        // v0.5.3 (lane H): the workspace without the home read has its own value, so the boot block states it and a
        // read_home change under a workspace is a grant change
        let ws_only = AccessState { read_home: false, workspace: ws.clone(), full: false };
        let g = grant_for("body", true, &ws_only, home);
        assert_eq!((g.grant, g.access), (Grant { read_home: false, write: ws.clone(), full: false }, "workspace_only"));
        assert_ne!(ws_only.level(), AccessState { read_home: true, ..ws_only.clone() }.level());
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
        // v0.5.3 (lane H): the credential stores the review found missing
        for e in [".git-credentials", ".config/gh", ".cargo/credentials", ".cargo/credentials.toml", ".config/cloudflare", ".cloudflared", ".init-release"] {
            assert!(d.contains(&h.join(e)), "{e}");
        }
        // the deny list's room in a profile: the built-ins, the other installs and the charter's entries
        assert!(BUILTIN_READ_DENY.len() + MAX_OTHER_INSTALLS + MAX_CHARTER_DENY <= 128);
    }

    /// v0.5.3 (lane H): another INIT install's homes are found, this install's own are never denied, and nothing that is
    /// not an INIT install is taken for one.
    #[test]
    fn other_installs_are_found_and_the_own_homes_left_out() {
        let base = resolve(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../target")).join(format!("test-homes/sbx-other-{}", std::process::id()));
        let h = base.join("home");
        let mk = |d: &Path| std::fs::create_dir_all(d).unwrap();
        let rh_json = |dir: &Path, ih: &Path| {
            mk(&dir.join("keys"));
            std::fs::write(dir.join("install.json"), serde_json::json!({"root_home": dir, "init_home": ih}).to_string()).unwrap();
        };
        let (own_rh, own_ih) = (h.join("test-rh"), h.join("test-ih"));
        rh_json(&own_rh, &own_ih); // this install: a ROOT_HOME under another name, found and left out
        mk(&own_ih);
        rh_json(&h.join(".init-root"), &h.join(".init")); // the owner's default install
        mk(&h.join(".init-root-demo/keys"));
        rh_json(&h.join("demo"), &base.join("demo-ih")); // a ROOT_HOME under another name, its INIT_HOME outside the home
        mk(&h.join("proj/keys")); // a project with keys/ and an install.json that is not INIT's
        std::fs::write(h.join("proj/install.json"), "{\"name\": \"x\"}").unwrap();
        mk(&h.join("backup/keys")); // a copy of an install's files: its install.json names another ROOT_HOME
        std::fs::write(h.join("backup/install.json"), serde_json::json!({"root_home": h.join(".init-root")}).to_string()).unwrap();
        mk(&h.join("Documents"));
        let far = base.join("far-rh");
        rh_json(&far, &base.join("far-ih")); // named by the default launcher only
        mk(&h.join(".local/bin"));
        std::fs::write(h.join(".local/bin/init-agent"), format!("#!/bin/bash\nINIT_ROOT_HOME_DEFAULT=\"{}\"\n", far.display())).unwrap();
        let canon = |p: PathBuf| resolve(&p);
        let got: Vec<PathBuf> = other_installs(&h, &own_rh, &own_ih);
        let want: Vec<PathBuf> = [h.join(".init-root"), h.join(".init"), h.join(".init-root-demo"), h.join("demo"), far.clone(), base.join("demo-ih"),
            base.join("far-ih")].into_iter().map(canon).collect();
        assert_eq!(got, want);
        // the owner's install as seen by itself: its own homes are never denied, the test install's are
        let own: Vec<PathBuf> = other_installs(&h, &h.join(".init-root"), &h.join(".init"));
        assert!(!own.contains(&canon(h.join(".init-root"))) && !own.contains(&canon(h.join(".init"))), "{own:?}");
        assert!(own.contains(&canon(own_rh.clone())) && own.contains(&canon(own_ih.clone())), "{own:?}");
        // an INIT_HOME inside another's ROOT_HOME, or a recorded home above the owner's, is never denied
        let nested = other_installs(&h, &h.join(".init-root-demo/rh"), &h.join(".init-root-demo/ih"));
        assert!(!nested.contains(&canon(h.join(".init-root-demo"))), "{nested:?}");
        std::fs::write(h.join("demo/install.json"), serde_json::json!({"root_home": h.join("demo"), "init_home": "/"}).to_string()).unwrap();
        std::fs::write(far.join("install.json"), serde_json::json!({"root_home": far, "init_home": base}).to_string()).unwrap();
        let up = other_installs(&h, &own_rh, &own_ih);
        assert!(!up.iter().any(|p| h.starts_with(p)), "{up:?}");
        std::fs::remove_dir_all(&base).unwrap(); // exact path, made above
    }

    /// ADR-012: the full lists keep the key directory, the charter's entries, the service directories and INIT's
    /// launchers (file paths in bin_dir, default ~/.local/bin: init-agent, init, init-tui), and nothing of the ADR-009
    /// secrets list.
    #[test]
    fn full_lists_keep_only_the_root_protected_set() {
        let h = Path::new("/nonexistent-adr012-home");
        let r = full_read_deny(h, &[h.join("tax")]);
        assert_eq!(r, vec![h.join(".config/init-agent"), h.join(".init-release"), h.join("tax")]);
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
