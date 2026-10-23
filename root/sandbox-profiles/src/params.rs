//! Checks on the parameter values a kernel supplies with `spawn` (root/root.md §3.3.4 V1–V5) and
//! the class rules that keep each template inside its intended area (kernel/sandbox.md §3.12).
//!
//! Parameters always travel as `-D<NAME>=<value>`; no value is ever spliced into profile text, so
//! no escaping exists anywhere. The tail holds whatever the values are, for writes, the network and
//! the Root's paths. For reads it does not (D9): V3 and the class rules are what keep an inner read
//! rule from pointing outside `INIT_HOME`.
//!
//! Platform-neutral. V6 (writable Linux bind mounts) belongs to the Linux renderer, WP6b.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use crate::ProfileId;

/// V1: at most this many kernel-supplied parameters.
pub const MAX_PARAMS: usize = 32;
/// V2: at most this many bytes per value.
pub const MAX_VALUE_BYTES: usize = 1024;
/// V4 limits.
pub const MAX_ENV: usize = 128;
pub const MAX_ARGS: usize = 256;
pub const MAX_ITEM_BYTES: usize = 32 * 1024;

/// Every profile's PRELUDE (sections 1–4) uses these three.
const PRELUDE_PARAMS: [&str; 3] = ["DEVDIR", "HOME_REAL", "TOOLCHAIN"];

/// The parameters a kernel supplies for each profile: exactly the names the composed text uses,
/// minus `MARK` (the Root sets `MARK = Z_MARK`) and every `Z_*` (V1). For `kernel` these are the
/// parameters the Root sets itself; the kernel can never request that profile (V5).
pub fn kernel_params(profile: ProfileId) -> Vec<&'static str> {
    let section: &[&str] = match profile {
        ProfileId::Body => &[
            "BIN",
            "BODY",
            "CACHE",
            "INIT_HOME",
            "KERNEL",
            "KERNEL_SOCK",
            "LINEAGE",
            "LINK_BIN",
            "LINK_KERNEL",
            "LINK_LINEAGE",
            "PROXY_SOCK",
            "PUB",
            "RUN",
            "STATE",
            "STATE_KERNEL",
            "VAULT",
            "WORK",
        ],
        ProfileId::Build | ProfileId::Check => &["INIT_HOME", "KERNEL", "OUT", "SRC", "VAULT"],
        ProfileId::GitRead => &["BODY", "CACHE", "INIT_HOME", "VAULT", "WORK"],
        ProfileId::Kernel => &["EGRESS_SOCK", "INIT_HOME", "MARKS", "PROBE", "RUN", "SLOT_OUT"],
    };
    let mut all: Vec<&'static str> = PRELUDE_PARAMS.iter().chain(section).copied().collect();
    all.sort_unstable();
    all
}

/// A refused `spawn` argument. On the control link this is error code `root.args`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArgsError {
    /// `V1`…`V5` (root/root.md §3.3.4) or `class` (kernel/sandbox.md §3.12).
    pub rule: &'static str,
    pub param: Option<String>,
    pub message: String,
}

impl ArgsError {
    pub const CODE: &'static str = "root.args";

    pub fn new(rule: &'static str, param: Option<&str>, message: impl Into<String>) -> Self {
        ArgsError { rule, param: param.map(str::to_owned), message: message.into() }
    }
}

impl fmt::Display for ArgsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.param {
            Some(p) => write!(f, "{} {}: {}: {}", Self::CODE, self.rule, p, self.message),
            None => write!(f, "{} {}: {}", Self::CODE, self.rule, self.message),
        }
    }
}

impl std::error::Error for ArgsError {}

/// Paths fixed at install (`ROOT_HOME/install.json`) plus the charter's `sandbox.extra_read`.
/// Every path is absolute and canonical.
#[derive(Clone, Debug)]
pub struct ValidationCtx {
    pub init_home: PathBuf,
    pub toolchain: PathBuf,
    pub devdir: PathBuf,
    /// The owner's home (`getpwuid`), used only by a metadata rule.
    pub home_real: PathBuf,
    pub extra_read: Vec<PathBuf>,
}

/// V5: the (kind, profile) pairs a kernel may request. `kernel` and kind `sealed` never.
pub fn check_kind(kind: &str, profile: ProfileId) -> Result<(), ArgsError> {
    let want = match kind {
        "body" | "probe" => ProfileId::Body,
        "build" => ProfileId::Build,
        "check" => ProfileId::Check,
        "git_read" => ProfileId::GitRead,
        _ => {
            return Err(ArgsError::new("V5", None, format!("kind {kind:?} is not available")));
        }
    };
    if profile != want {
        return Err(ArgsError::new("V5", None, format!("kind {kind:?} cannot use profile {:?}", profile.wire())));
    }
    Ok(())
}

/// V1: `^[A-Z][A-Z0-9_]{0,31}$`, not `Z_*`, not `MARK`.
pub fn check_name(name: &str) -> Result<(), ArgsError> {
    let b = name.as_bytes();
    let shape =
        !b.is_empty() && b.len() <= 32 && b[0].is_ascii_uppercase() && b[1..].iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b'_');
    if !shape {
        return Err(ArgsError::new("V1", Some(name), "name must match ^[A-Z][A-Z0-9_]{0,31}$"));
    }
    if name.starts_with("Z_") {
        return Err(ArgsError::new("V1", Some(name), "Z_* parameters are the Root's"));
    }
    if name == "MARK" {
        return Err(ArgsError::new("V1", Some(name), "MARK is set by the Root (MARK = Z_MARK)"));
    }
    Ok(())
}

/// V2: absolute, ≤ 1024 bytes, no NUL, no `.`/`..`, and canonical: an existing path equals its
/// `fs::canonicalize`; for a missing path its parent does.
///
/// `LINK_*` (D8): root/root.md lets them name a symlink inside a probe or fleet root, so for a
/// `LINK_*` whose `lstat` shows a symlink only the parent must be canonical (the last component is
/// kept literal). kernel/sandbox.md §3.3.3 instead sets `LINK_*` equal to real, canonical
/// directories; those pass the ordinary rule. Both forms are accepted (status file, Deviations).
pub fn check_value(name: &str, value: &str) -> Result<(), ArgsError> {
    let err = |m: &str| Err(ArgsError::new("V2", Some(name), m));
    if value.len() > MAX_VALUE_BYTES {
        return err("longer than 1024 bytes");
    }
    if value.contains('\0') {
        return err("contains NUL");
    }
    if !value.starts_with('/') {
        return err("not absolute");
    }
    if value.split('/').any(|c| c == "." || c == "..") {
        return err("has a . or .. component");
    }
    if value != "/" && (value.ends_with('/') || value.contains("//")) {
        return err("not canonical (empty component)");
    }
    let path = Path::new(value);
    match std::fs::symlink_metadata(path) {
        Ok(md) if md.file_type().is_symlink() && name.starts_with("LINK_") => parent_is_canonical(name, path),
        Ok(_) => match std::fs::canonicalize(path) {
            Ok(c) if c == path => Ok(()),
            Ok(c) => err(&format!("not canonical (resolves to {})", c.display())),
            Err(e) => err(&format!("cannot canonicalize: {e}")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => parent_is_canonical(name, path),
        Err(e) => err(&format!("cannot lstat: {e}")),
    }
}

fn parent_is_canonical(name: &str, path: &Path) -> Result<(), ArgsError> {
    let err = |m: String| Err(ArgsError::new("V2", Some(name), m));
    let Some(Component::Normal(_)) = path.components().next_back() else {
        return err("no final component".into());
    };
    let Some(parent) = path.parent() else {
        return err("no parent".into());
    };
    match std::fs::canonicalize(parent) {
        Ok(c) if c == parent => Ok(()),
        Ok(c) => err(format!("parent not canonical (resolves to {})", c.display())),
        Err(e) => err(format!("parent cannot be canonicalized: {e}")),
    }
}

/// V3: under `INIT_HOME`, `TOOLCHAIN`, `DEVDIR` or a charter `extra_read` entry, or equal to the
/// owner's home. "Under" includes equality and is component-wise.
pub fn check_root(name: &str, value: &str, ctx: &ValidationCtx) -> Result<(), ArgsError> {
    let p = Path::new(value);
    if p == ctx.home_real {
        return Ok(());
    }
    let mut roots = [&ctx.init_home, &ctx.toolchain, &ctx.devdir].into_iter().chain(ctx.extra_read.iter());
    if roots.any(|r| p.starts_with(r)) { Ok(()) } else { Err(ArgsError::new("V3", Some(name), "outside INIT_HOME, TOOLCHAIN, DEVDIR and extra_read")) }
}

/// V4: `program` absolute, `cwd` under `INIT_HOME`, bounded `env` without `INIT_ROOT_*`, bounded `args`.
pub fn check_spawn_misc(program: &str, cwd: &str, env: &BTreeMap<String, String>, args: &[String], ctx: &ValidationCtx) -> Result<(), ArgsError> {
    let err = |m: &str| Err(ArgsError::new("V4", None, m));
    if !program.starts_with('/') || program.contains('\0') {
        return err("program must be an absolute path");
    }
    if !cwd.starts_with('/') || !Path::new(cwd).starts_with(&ctx.init_home) {
        return err("cwd must lie under INIT_HOME");
    }
    if env.len() > MAX_ENV {
        return err("more than 128 env entries");
    }
    for (k, v) in env {
        if k.starts_with("INIT_ROOT_") {
            return err("env must not carry INIT_ROOT_* keys");
        }
        if k.len() + 1 + v.len() > MAX_ITEM_BYTES || k.contains('\0') || v.contains('\0') {
            return err("env entry too large or contains NUL");
        }
    }
    if args.len() > MAX_ARGS {
        return err("more than 256 args");
    }
    if args.iter().any(|a| a.len() > MAX_ITEM_BYTES || a.contains('\0')) {
        return err("arg too large or contains NUL");
    }
    Ok(())
}

/// Which root an instance's `INIT_HOME` parameter names (kernel/sandbox.md §3.3.2–§3.3.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lane {
    Live,
    Probe(String),
    Member(String),
}

fn lane_of(i: &Path, r: &Path, param: &str) -> Result<Lane, ArgsError> {
    if r == i {
        return Ok(Lane::Live);
    }
    let bad = || ArgsError::new("class", Some(param), "must be INIT_HOME, INIT_HOME/probe/<id> or INIT_HOME/fleet/<bid>");
    let rest = r.strip_prefix(i).map_err(|_| bad())?;
    let parts: Vec<&str> = rest.components().map(|c| c.as_os_str().to_str().unwrap_or("")).collect();
    match parts.as_slice() {
        ["probe", id] if !id.is_empty() => Ok(Lane::Probe((*id).to_owned())),
        ["fleet", bid] if !bid.is_empty() => Ok(Lane::Member((*bid).to_owned())),
        _ => Err(bad()),
    }
}

/// kernel/sandbox.md §3.12: the class rules. Assumes the names were already checked (V1, exact set).
pub fn check_class(profile: ProfileId, p: &BTreeMap<String, String>, ctx: &ValidationCtx) -> Result<(), ArgsError> {
    let get = |n: &str| -> Result<&Path, ArgsError> { p.get(n).map(Path::new).ok_or_else(|| ArgsError::new("class", Some(n), "missing")) };
    let eq = |n: &str, want: &Path| -> Result<(), ArgsError> {
        if get(n)? == want { Ok(()) } else { Err(ArgsError::new("class", Some(n), format!("must be {}", want.display()))) }
    };
    let i = ctx.init_home.as_path();
    eq("TOOLCHAIN", &ctx.toolchain)?;
    eq("DEVDIR", &ctx.devdir)?;
    eq("HOME_REAL", &ctx.home_real)?;
    match profile {
        ProfileId::Kernel => Err(ArgsError::new("V5", None, "profile kernel is the Root's")),
        ProfileId::Body => {
            let r = get("INIT_HOME")?;
            let lane = lane_of(i, r, "INIT_HOME")?;
            eq("BODY", &r.join("body"))?;
            eq("WORK", &r.join("work"))?;
            eq("STATE", &r.join("state"))?;
            eq("STATE_KERNEL", &r.join("state/kernel"))?;
            let run = match &lane {
                Lane::Live => i.join("run"),
                Lane::Probe(_) => r.join("run"),
                Lane::Member(bid) => i.join("run/fleet").join(bid),
            };
            eq("RUN", &run)?;
            eq("KERNEL_SOCK", &run.join("kernel.sock"))?;
            eq("PROXY_SOCK", &run.join("proxy.sock"))?;
            for (n, real) in [("KERNEL", "kernel"), ("BIN", "bin")] {
                let v = get(n)?;
                if v != i.join(real) && v != r.join(real) {
                    return Err(ArgsError::new("class", Some(n), "must be INIT_HOME's or the instance root's"));
                }
            }
            eq("LINEAGE", &i.join("lineage"))?;
            eq("VAULT", &i.join("vault"))?;
            eq("CACHE", &i.join("cache"))?;
            eq("PUB", &i.join("root"))?;
            for (link, target) in [("LINK_KERNEL", "KERNEL"), ("LINK_LINEAGE", "LINEAGE"), ("LINK_BIN", "BIN")] {
                let l = get(link)?;
                if l == get(target)? {
                    continue; // D8: real directories, the rule adds nothing
                }
                let in_root = lane != Lane::Live && l.starts_with(r) && l != r;
                let symlink_or_absent = match std::fs::symlink_metadata(l) {
                    Ok(md) => md.file_type().is_symlink(),
                    Err(e) => e.kind() == std::io::ErrorKind::NotFound,
                };
                if !(in_root && symlink_or_absent) {
                    return Err(ArgsError::new("class", Some(link), format!("must equal {target}, or name a symlink inside the probe or member root")));
                }
            }
            Ok(())
        }
        ProfileId::Build | ProfileId::Check => {
            eq("INIT_HOME", i)?;
            let cache = i.join("cache");
            for n in ["SRC", "OUT"] {
                let v = get(n)?;
                if !(v.starts_with(&cache) && v != cache) {
                    return Err(ArgsError::new("class", Some(n), "must lie under INIT_HOME/cache/"));
                }
            }
            eq("KERNEL", &i.join("kernel"))?;
            eq("VAULT", &i.join("vault"))
        }
        ProfileId::GitRead => {
            eq("INIT_HOME", i)?;
            let body = get("BODY")?;
            let r = body.parent().ok_or_else(|| ArgsError::new("class", Some("BODY"), "no parent"))?;
            lane_of(i, r, "BODY")?;
            eq("BODY", &r.join("body"))?;
            eq("WORK", &r.join("work"))?;
            eq("VAULT", &i.join("vault"))?;
            eq("CACHE", &i.join("cache"))
        }
    }
}

/// Everything a kernel-supplied parameter map must pass before the Root renders a profile:
/// V5, V1 (names, count), the exact name set of [`kernel_params`], V2 and V3 per value, then the
/// class rules. V4 is [`check_spawn_misc`].
pub fn validate_spawn_params(kind: &str, profile: ProfileId, params: &BTreeMap<String, String>, ctx: &ValidationCtx) -> Result<(), ArgsError> {
    check_kind(kind, profile)?;
    if params.len() > MAX_PARAMS {
        return Err(ArgsError::new("V1", None, "more than 32 parameters"));
    }
    for name in params.keys() {
        check_name(name)?;
    }
    let want = kernel_params(profile);
    if let Some(missing) = want.iter().find(|n| !params.contains_key(**n)) {
        return Err(ArgsError::new("V1", Some(missing), "required by the profile and missing"));
    }
    if let Some(extra) = params.keys().find(|n| !want.contains(&n.as_str())) {
        return Err(ArgsError::new("V1", Some(extra), "not used by the profile"));
    }
    for (name, value) in params {
        check_value(name, value)?;
        check_root(name, value, ctx)?;
    }
    check_class(profile, params, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_names() {
        assert!(check_name("BODY").is_ok());
        assert!(check_name("LINK_BIN").is_ok());
        for bad in ["", "body", "1A", "A-B", "Z_MARK", "Z_ROOT_HOME", "MARK", &"A".repeat(33)] {
            assert_eq!(check_name(bad).unwrap_err().rule, "V1", "{bad:?}");
        }
    }

    #[test]
    fn v2_shape_without_filesystem() {
        for bad in ["rel/path", "/a/../b", "/a/./b", "/a//b", "/a/b/", "/a\0b"] {
            assert_eq!(check_value("BODY", bad).unwrap_err().rule, "V2", "{bad:?}");
        }
        assert!(check_value("BODY", &format!("/{}", "a".repeat(1100))).is_err());
        assert!(check_value("BODY", "/").is_ok());
    }

    #[test]
    fn v5_pairs() {
        assert!(check_kind("body", ProfileId::Body).is_ok());
        assert!(check_kind("probe", ProfileId::Body).is_ok());
        assert!(check_kind("build", ProfileId::Build).is_ok());
        assert!(check_kind("check", ProfileId::Check).is_ok());
        assert!(check_kind("git_read", ProfileId::GitRead).is_ok());
        assert!(check_kind("body", ProfileId::Kernel).is_err());
        assert!(check_kind("kernel", ProfileId::Kernel).is_err());
        assert!(check_kind("sealed", ProfileId::Body).is_err());
        assert!(check_kind("build", ProfileId::Body).is_err());
    }

    #[test]
    fn param_lists_are_sorted_and_have_no_root_names() {
        for p in ProfileId::ALL {
            let l = kernel_params(p);
            assert!(l.windows(2).all(|w| w[0] < w[1]));
            assert!(l.iter().all(|n| check_name(n).is_ok()));
        }
    }
}
