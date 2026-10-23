//! Spawn parameters and environments: kernel/sandbox.md §3.3.2–§3.5 (profile parameters, the
//! exact name sets WP6a's validator checks) and supervise.md §4.1 (the body environment).
//!
//! `MARK` and every `Z_*` parameter are the Root's, never the kernel's (root/root.md V1).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Paths outside `INIT_HOME` that every profile's PRELUDE names (`[sandbox]`, kernel/config.md).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SandboxPaths {
    /// `TOOLCHAIN`: the rustup toolchain directory.
    pub toolchain: PathBuf,
    /// `DEVDIR`: `/Library/Developer/CommandLineTools` on macOS.
    pub devdir: PathBuf,
    /// `HOME_REAL`: the owner's home from the passwd entry (metadata only).
    pub home_real: PathBuf,
    /// `SDKROOT` (macOS only).
    pub sdkroot: Option<PathBuf>,
}

impl SandboxPaths {
    fn prelude(&self, m: &mut BTreeMap<String, String>) {
        m.insert("TOOLCHAIN".into(), s(&self.toolchain));
        m.insert("DEVDIR".into(), s(&self.devdir));
        m.insert("HOME_REAL".into(), s(&self.home_real));
    }
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Which root a body-profile instance lives in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BodyLane {
    /// The primary, rooted at `INIT_HOME`.
    Live,
    /// `INIT_HOME/probe/<id>`.
    Probe(String),
    /// `INIT_HOME/fleet/<bid>` (lane sockets under `run/fleet/<bid>/`).
    Member(String),
}

impl BodyLane {
    /// The instance root.
    pub fn root(&self, home: &Path) -> PathBuf {
        match self {
            BodyLane::Live => home.to_path_buf(),
            BodyLane::Probe(id) => home.join("probe").join(id),
            BodyLane::Member(bid) => home.join("fleet").join(bid),
        }
    }

    /// The directory holding the instance's `kernel.sock` and `proxy.sock`.
    pub fn run_dir(&self, home: &Path) -> PathBuf {
        match self {
            BodyLane::Live => home.join("run"),
            BodyLane::Probe(id) => home.join("probe").join(id).join("run"),
            BodyLane::Member(bid) => home.join("run/fleet").join(bid),
        }
    }
}

/// The `body` profile's parameters (sandbox.md §3.3.2 live, §3.3.3 probe; members like probes
/// with their sockets under `run/fleet/<bid>/`). `home` must be canonical. For probe and member
/// roots `KERNEL` and `BIN` are the root's own clones and `LINK_*` equal them (D8).
pub fn body_params(home: &Path, lane: &BodyLane, sp: &SandboxPaths) -> BTreeMap<String, String> {
    let r = lane.root(home);
    let run = lane.run_dir(home);
    let (kernel, bin) = match lane {
        BodyLane::Live => (home.join("kernel"), home.join("bin")),
        _ => (r.join("kernel"), r.join("bin")),
    };
    let mut m = BTreeMap::new();
    sp.prelude(&mut m);
    m.insert("INIT_HOME".into(), s(&r));
    m.insert("BODY".into(), s(&r.join("body")));
    m.insert("WORK".into(), s(&r.join("work")));
    m.insert("STATE".into(), s(&r.join("state")));
    m.insert("STATE_KERNEL".into(), s(&r.join("state/kernel")));
    m.insert("RUN".into(), s(&run));
    m.insert("KERNEL_SOCK".into(), s(&run.join("kernel.sock")));
    m.insert("PROXY_SOCK".into(), s(&run.join("proxy.sock")));
    m.insert("KERNEL".into(), s(&kernel));
    m.insert("BIN".into(), s(&bin));
    m.insert("LINEAGE".into(), s(&home.join("lineage")));
    m.insert("VAULT".into(), s(&home.join("vault")));
    m.insert("CACHE".into(), s(&home.join("cache")));
    m.insert("PUB".into(), s(&home.join("root")));
    m.insert("LINK_KERNEL".into(), s(&kernel));
    m.insert("LINK_LINEAGE".into(), s(&home.join("lineage")));
    m.insert("LINK_BIN".into(), s(&bin));
    m
}

/// The `build` / `check` profile's parameters (sandbox.md §3.4).
pub fn build_params(home: &Path, src: &Path, out: &Path, sp: &SandboxPaths) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    sp.prelude(&mut m);
    m.insert("INIT_HOME".into(), s(home));
    m.insert("KERNEL".into(), s(&home.join("kernel")));
    m.insert("SRC".into(), s(src));
    m.insert("OUT".into(), s(out));
    m.insert("VAULT".into(), s(&home.join("vault")));
    m
}

/// The `git-read` profile's parameters (sandbox.md §3.5): `root` is `INIT_HOME`, a probe root or
/// a fleet root whose `body/` and `work/` are read.
pub fn git_read_params(home: &Path, root: &Path, sp: &SandboxPaths) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    sp.prelude(&mut m);
    m.insert("INIT_HOME".into(), s(home));
    m.insert("BODY".into(), s(&root.join("body")));
    m.insert("WORK".into(), s(&root.join("work")));
    m.insert("VAULT".into(), s(&home.join("vault")));
    m.insert("CACHE".into(), s(&home.join("cache")));
    m
}

/// The owner's workspace and the live body's access (ADR-009), as the Root last stated them. The
/// Root sets the same two variables on every body-profile spawn itself (SBX-009 §D, overwriting
/// these), so the kernel's copy only has to agree with it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Access {
    /// The canonical workspace path, or "" when there is none.
    pub workspace: String,
    /// `none` | `read_home` | `workspace` | `full`.
    pub access: String,
}

impl Access {
    /// From the Root's `access` object (the `hello` result's, or the `access` event's).
    pub fn from_data(d: &init_proto::root_link::AccessData) -> Access {
        Access { workspace: d.workspace.clone().unwrap_or_default(), access: d.access.clone() }
    }

    /// `INIT_WORKSPACE` / `INIT_ACCESS` of the kernel's own environment (the Root may set them when
    /// it starts the kernel); None when `INIT_ACCESS` is absent or empty.
    pub fn from_env() -> Option<Access> {
        let access = std::env::var("INIT_ACCESS").ok().filter(|a| !a.is_empty())?;
        Some(Access { workspace: std::env::var("INIT_WORKSPACE").unwrap_or_default(), access })
    }
}

/// What varies per body spawn in the environment of §4.1.
#[derive(Clone, Debug)]
pub struct BodyEnv<'a> {
    /// The instance root (`INIT_HOME` as the body sees it).
    pub root: &'a Path,
    /// The lane socket directory.
    pub run_dir: &'a Path,
    pub session: &'a str,
    pub sha: &'a str,
    pub boot_id: &'a str,
    pub boot_reason: &'a str,
    pub body_id: &'a str,
    pub mode: &'a str,
    pub kernel_version: &'a str,
    pub sandbox: &'a SandboxPaths,
    /// `proxy.test_mode`: adds `INIT_TEST=1` and `test_vars`.
    pub test_mode: bool,
    /// `INIT_TEST_*` variables copied from the kernel's own environment (test mode only).
    pub test_vars: &'a BTreeMap<String, String>,
    /// ADR-009: relayed as `INIT_WORKSPACE` and `INIT_ACCESS` (the live primary only; probes and
    /// fleet members are evaluation copies and never get the owner's workspace). None: neither set.
    pub access: Option<&'a Access>,
}

/// The complete body environment (supervise.md §4.1). Nothing else reaches a body; in
/// particular no key and no `INIT_ROOT_*` variable.
pub fn body_env(e: &BodyEnv<'_>) -> BTreeMap<String, String> {
    let r = e.root;
    let mut m = BTreeMap::new();
    let mut put = |k: &str, v: String| {
        m.insert(k.to_string(), v);
    };
    put("INIT_HOME", s(r));
    put("INIT_SESSION", e.session.to_string());
    put("INIT_SHA", e.sha.to_string());
    put("INIT_BOOT_ID", e.boot_id.to_string());
    put("INIT_BOOT_REASON", e.boot_reason.to_string());
    put("INIT_BODY_ID", e.body_id.to_string());
    put("INIT_MODE", e.mode.to_string());
    put("INIT_UI", init_proto::UI_PROTO.to_string());
    put("INIT_KERNEL_VERSION", e.kernel_version.to_string());
    put("INIT_KERNEL_SOCK", s(&e.run_dir.join("kernel.sock")));
    put("INIT_PROXY_SOCK", s(&e.run_dir.join("proxy.sock")));
    if let Some(a) = e.access {
        put("INIT_WORKSPACE", a.workspace.clone());
        put("INIT_ACCESS", a.access.clone());
    }
    let tool = s(&e.sandbox.toolchain.join("bin"));
    let kbin = s(&r.join("kernel/bin"));
    if cfg!(target_os = "macos") {
        let dev = s(&e.sandbox.devdir.join("usr/bin"));
        put("PATH", format!("{kbin}:{tool}:{dev}:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"));
        if let Some(sdk) = &e.sandbox.sdkroot {
            put("SDKROOT", s(sdk));
        }
        put("DEVELOPER_DIR", s(&e.sandbox.devdir));
        put("LANG", "en_US.UTF-8".into());
    } else {
        put("PATH", format!("{kbin}:{tool}:/usr/local/bin:/usr/bin:/bin"));
        put("LANG", "C.UTF-8".into());
    }
    put("HOME", s(&r.join("work/home")));
    put("TMPDIR", format!("{}/", s(&r.join("work/tmp"))));
    put("CARGO_HOME", s(&r.join("work/cargo-home")));
    put("CARGO_TARGET_DIR", s(&r.join("work/target")));
    put("CARGO_NET_OFFLINE", "true".into());
    put("GIT_AUTHOR_NAME", "INIT Agent".into());
    put("GIT_COMMITTER_NAME", "INIT Agent".into());
    put("GIT_AUTHOR_EMAIL", "agent@init.local".into());
    put("GIT_COMMITTER_EMAIL", "agent@init.local".into());
    put("GIT_CONFIG_NOSYSTEM", "1".into());
    put("GIT_TERMINAL_PROMPT", "0".into());
    put("TERM", "dumb".into());
    put("NO_COLOR", "1".into());
    if e.test_mode {
        put("INIT_TEST", "1".into());
        for (k, v) in e.test_vars {
            if k.starts_with("INIT_TEST_") && k.len() <= 64 && v.len() <= 256 {
                put(k, v.clone());
            }
        }
    }
    m
}

/// `INIT_TEST_*` variables of the kernel's own environment (copied to bodies in test mode only).
pub fn own_test_vars() -> BTreeMap<String, String> {
    std::env::vars().filter(|(k, _)| k.starts_with("INIT_TEST_")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp() -> SandboxPaths {
        SandboxPaths { toolchain: "/tc".into(), devdir: "/dev".into(), home_real: "/Users/x".into(), sdkroot: None }
    }

    #[test]
    fn body_params_have_the_validators_exact_name_set() {
        let want = [
            "BIN", "BODY", "CACHE", "DEVDIR", "HOME_REAL", "INIT_HOME", "KERNEL", "KERNEL_SOCK", "LINEAGE", "LINK_BIN",
            "LINK_KERNEL", "LINK_LINEAGE", "PROXY_SOCK", "PUB", "RUN", "STATE", "STATE_KERNEL", "TOOLCHAIN", "VAULT", "WORK",
        ];
        for lane in [BodyLane::Live, BodyLane::Probe("p1".into()), BodyLane::Member("b1".into())] {
            let p = body_params(Path::new("/h"), &lane, &sp());
            assert_eq!(p.keys().map(String::as_str).collect::<Vec<_>>(), want);
        }
        let p = body_params(Path::new("/h"), &BodyLane::Member("b1".into()), &sp());
        assert_eq!(p["KERNEL_SOCK"], "/h/run/fleet/b1/kernel.sock");
        assert_eq!(p["BODY"], "/h/fleet/b1/body");
        assert_eq!(p["LINK_BIN"], p["BIN"]);
        let p = body_params(Path::new("/h"), &BodyLane::Live, &sp());
        assert_eq!((p["BIN"].as_str(), p["PUB"].as_str()), ("/h/bin", "/h/root"));
        let b = build_params(Path::new("/h"), Path::new("/h/cache/build/slot/src"), Path::new("/h/cache/build/slot/out"), &sp());
        assert_eq!(b.keys().map(String::as_str).collect::<Vec<_>>(), ["DEVDIR", "HOME_REAL", "INIT_HOME", "KERNEL", "OUT", "SRC", "TOOLCHAIN", "VAULT"]);
        let g = git_read_params(Path::new("/h"), Path::new("/h"), &sp());
        assert_eq!(g.keys().map(String::as_str).collect::<Vec<_>>(), ["BODY", "CACHE", "DEVDIR", "HOME_REAL", "INIT_HOME", "TOOLCHAIN", "VAULT", "WORK"]);
    }

    #[test]
    fn body_env_is_complete_and_keyless() {
        let tv = BTreeMap::from([("INIT_TEST_X".to_string(), "1".to_string()), ("ANTHROPIC_API_KEY".to_string(), "k".to_string())]);
        let e = BodyEnv {
            root: Path::new("/h"),
            run_dir: Path::new("/h/run"),
            session: "s",
            sha: "a",
            boot_id: "b",
            boot_reason: "cold",
            body_id: "b0",
            mode: "normal",
            kernel_version: "0.1.0",
            sandbox: &sp(),
            test_mode: false,
            test_vars: &tv,
            access: None,
        };
        let m = body_env(&e);
        assert!(!m.contains_key("INIT_TEST") && !m.contains_key("INIT_TEST_X"));
        assert!(!m.contains_key("INIT_WORKSPACE") && !m.contains_key("INIT_ACCESS"), "nothing known: neither is set");
        assert!(m.keys().all(|k| !k.contains("KEY") && !k.starts_with("INIT_ROOT")), "SV-24");
        assert_eq!(m["INIT_UI"], "stdio-jsonl/1");
        let m = body_env(&BodyEnv { test_mode: true, ..e });
        assert_eq!((m["INIT_TEST"].as_str(), m["INIT_TEST_X"].as_str()), ("1", "1"));
        assert!(!m.contains_key("ANTHROPIC_API_KEY"));
    }

    /// ADR-009: the workspace and access reach the body as `INIT_WORKSPACE` / `INIT_ACCESS`; no
    /// workspace is the empty string, never an absent variable.
    #[test]
    fn body_env_relays_workspace_and_access() {
        let tv = BTreeMap::new();
        let sp = sp();
        let a = Access { workspace: "/Users/x/proj".into(), access: "workspace".into() };
        let e = BodyEnv {
            root: Path::new("/h"),
            run_dir: Path::new("/h/run"),
            session: "s",
            sha: "a",
            boot_id: "b",
            boot_reason: "handoff",
            body_id: "b0",
            mode: "normal",
            kernel_version: "0.1.0",
            sandbox: &sp,
            test_mode: false,
            test_vars: &tv,
            access: Some(&a),
        };
        let m = body_env(&e);
        assert_eq!((m["INIT_WORKSPACE"].as_str(), m["INIT_ACCESS"].as_str()), ("/Users/x/proj", "workspace"));
        let none = Access { workspace: String::new(), access: "read_home".into() };
        let m = body_env(&BodyEnv { access: Some(&none), ..e });
        assert_eq!((m["INIT_WORKSPACE"].as_str(), m["INIT_ACCESS"].as_str()), ("", "read_home"));
    }
}
