//! macOS: the SBPL texts compiled into `init-root` and the Root's composition of them
//! (root/root.md §3.3.1):
//!
//! ```text
//! profile(id) = PRELUDE + SECTION(id) + ROOT_ALLOWS + ZONE_TAIL      (the tail is deny-only and always last)
//! profile(kind sealed) = PRELUDE + SEALED_TAIL                       (root/root.md §3.3.6)
//! ```
//!
//! The texts under `profiles/` are verbatim copies of the docs' code blocks; `tests/doc_drift.rs`
//! fails if they drift. Values only ever travel as `-D<NAME>=<value>`; nothing is spliced into text.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::ProfileId;
use crate::access::Grant;
use crate::params::ArgsError;

/// Absolute launcher path, never resolved through `PATH` (kernel/sandbox.md §3.7).
pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// kernel/sandbox.md §3.3.1 sections 1–4, identical for every profile.
pub const PRELUDE: &str = include_str!("../profiles/prelude.sb");
/// kernel/sandbox.md §3.3.1 sections 5–8.
pub const BODY_SECTION: &str = include_str!("../profiles/body.sb");
/// kernel/sandbox.md §3.4 (also `check`).
pub const BUILD_SECTION: &str = include_str!("../profiles/build.sb");
/// kernel/sandbox.md §3.5.
pub const GIT_READ_SECTION: &str = include_str!("../profiles/git-read.sb");
/// root/root.md §3.3.3.
pub const KERNEL_SECTION: &str = include_str!("../profiles/kernel.sb");
/// root/root.md §3.3.1 ROOT_ALLOWS, with `<i>` standing for the `extra_read` index, and the ADR-009 grant lines.
pub const ROOT_ALLOWS_TEMPLATE: &str = include_str!("../profiles/root-allows.template");
/// root/root.md §3.3.2, Z1–Z7 (Z2 per D9; Z2 and Z3 per ADR-009), with `<i>` standing for the deny-list index.
pub const ZONE_TAIL: &str = include_str!("../profiles/zone-tail.sb");
/// root/root.md §3.3.6, the sealed text: instance kind `sealed` is PRELUDE + this, with no SECTION and
/// no ROOT_ALLOWS (root/sealed-eval.md §3.2, D5). `init-root`'s `sealed::run` renders it.
pub const SEALED_TAIL: &str = include_str!("../profiles/sealed-tail.sb");

/// root/root.md §3.3.1: at most 8 charter `sandbox.extra_read` entries.
pub const MAX_EXTRA_READ: usize = 8;
/// Tail Z2 (ADR-009): at most this many read deny entries (the built-in list and the charter's ≤ 32, each
/// possibly twice: the literal path and its resolution).
pub const MAX_DENY: usize = 128;

/// SECTION(id). `check` is the `build` text with different parameters.
pub fn section(id: ProfileId) -> &'static str {
    match id {
        ProfileId::Kernel => KERNEL_SECTION,
        ProfileId::Body => BODY_SECTION,
        ProfileId::Build | ProfileId::Check => BUILD_SECTION,
        ProfileId::GitRead => GIT_READ_SECTION,
    }
}

/// A template as groups: a run of `;` comment lines and the rule lines that follow it.
fn groups(t: &str) -> Vec<(Vec<&str>, Vec<&str>)> {
    let mut out: Vec<(Vec<&str>, Vec<&str>)> = Vec::new();
    for l in t.lines() {
        let comment = l.trim_start().starts_with(';');
        match out.last_mut() {
            Some((c, rules)) if comment && rules.is_empty() => c.push(l),
            Some((_, rules)) if !comment => rules.push(l),
            _ => out.push(if comment { (vec![l], vec![]) } else { (vec![], vec![l]) }),
        }
    }
    out
}

/// How many entries each `<i>` list of a template has: `extra_read` (`Z_EXTRA_READ_<i>`), the read deny list
/// (`Z_DENY_<i>`) and the write deny list (`Z_WDENY_<i>`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub extra_read: usize,
    pub deny: usize,
    pub wdeny: usize,
}

/// Render a template group by group: a group whose rules hold `<i>` is repeated once per entry of its list
/// ([`Counts`]), with its comment only when the list is non-empty; a group naming `Z_HOME` only with a read
/// grant; with `write_cond`, a group naming `Z_WRITE` only with a write grant. Every other group is copied.
fn expand(t: &str, n: Counts, grant: &Grant, write_cond: bool) -> String {
    let uses = |rules: &[&str], p: &str| rules.iter().any(|l| l.split(';').next().unwrap_or("").contains(p));
    let mut out = String::new();
    for (comments, rules) in groups(t) {
        let n = if uses(&rules, "(param \"Z_EXTRA_READ_<i>\")") {
            n.extra_read
        } else if uses(&rules, "(param \"Z_DENY_<i>\")") {
            n.deny
        } else if uses(&rules, "(param \"Z_WDENY_<i>\")") {
            n.wdeny
        } else if uses(&rules, "(param \"Z_HOME\")") {
            usize::from(grant.read_home)
        } else if write_cond && uses(&rules, "(param \"Z_WRITE\")") {
            usize::from(grant.write.is_some())
        } else {
            1
        };
        if n == 0 {
            continue;
        }
        for l in comments {
            out.push_str(l);
            out.push('\n');
        }
        let indexed = rules.iter().any(|l| l.contains("<i>"));
        for i in 0..(if indexed { n } else { 1 }) {
            for l in &rules {
                out.push_str(&l.replace("<i>", &i.to_string()));
                out.push('\n');
            }
        }
    }
    out
}

/// ROOT_ALLOWS: the instance mark and the zone mark, then two lines per `extra_read` entry (only when the
/// list is non-empty, D9), then the ADR-009 grant lines the instance has (a body's only).
pub fn root_allows(extra_read: usize, grant: &Grant) -> Result<String, ArgsError> {
    if extra_read > MAX_EXTRA_READ {
        return Err(ArgsError::new("V3", None, "more than 8 extra_read entries"));
    }
    Ok(expand(ROOT_ALLOWS_TEMPLATE, Counts { extra_read, ..Counts::default() }, grant, true))
}

/// The zone tail with one Z2 line per read deny entry and one Z3a line per write deny entry (ADR-009).
pub fn zone_tail(deny: usize, wdeny: usize) -> Result<String, ArgsError> {
    if deny > MAX_DENY || wdeny > MAX_DENY {
        return Err(ArgsError::new("V3", None, "more than 128 read or write deny entries"));
    }
    Ok(expand(ZONE_TAIL, Counts { deny, wdeny, ..Counts::default() }, &Grant::default(), false))
}

/// The full text of profile `id`: PRELUDE + SECTION(id) + ROOT_ALLOWS + ZONE_TAIL. Only `body` takes a grant.
pub fn render(id: ProfileId, n: Counts, grant: &Grant) -> Result<String, ArgsError> {
    if id != ProfileId::Body && *grant != Grant::default() {
        return Err(ArgsError::new("V5", None, format!("profile {:?} takes no ADR-009 grant", id.wire())));
    }
    Ok([PRELUDE, section(id), &root_allows(n.extra_read, grant)?, &zone_tail(n.deny, n.wdeny)?].concat())
}

/// The full text of instance kind `sealed` (root/root.md §3.3.1): PRELUDE + SEALED_TAIL.
pub fn render_sealed() -> String {
    [PRELUDE, SEALED_TAIL].concat()
}

/// ROOT_ALLOWS + ZONE_TAIL over an arbitrary inner text (root/root.md RR-1: the tail over
/// `(version 1)(allow default)`, the worst possible inner profile).
pub fn render_over(inner: &str, n: Counts, grant: &Grant) -> Result<String, ArgsError> {
    Ok([inner, &root_allows(n.extra_read, grant)?, &zone_tail(n.deny, n.wdeny)?].concat())
}

/// Every `(param "NAME")` a text references, ignoring `;` comments.
pub fn referenced_params(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in text.lines() {
        let code = line.split(';').next().unwrap_or("");
        let mut rest = code;
        while let Some(at) = rest.find("(param \"") {
            rest = &rest[at + 8..];
            if let Some(end) = rest.find('"') {
                out.insert(rest[..end].to_owned());
                rest = &rest[end..];
            }
        }
    }
    out
}

/// The Root's own paths, chosen at install, plus the charter's `extra_read`. All canonical.
#[derive(Clone, Debug, Default)]
pub struct ZoneLayout {
    pub root_home: PathBuf,
    pub init_home: PathBuf,
    /// The Root's service file (`~/Library/LaunchAgents/dev.init.root.plist`), tail Z2; its directory is
    /// write-denied too.
    pub root_svc: PathBuf,
    pub extra_read: Vec<PathBuf>,
    /// The owner's home, canonical (`Z_HOME` of a read grant; ADR-009).
    pub home: PathBuf,
    /// The read deny list (`Z_DENY_<i>`, tail Z2; [`crate::access::read_deny`]).
    pub read_deny: Vec<PathBuf>,
    /// The write deny list (`Z_WDENY_<i>`, tail Z3a; [`crate::access::write_deny`]).
    pub write_deny: Vec<PathBuf>,
}

impl ZoneLayout {
    /// The lengths of this layout's `<i>` lists.
    pub fn counts(&self) -> Counts {
        Counts { extra_read: self.extra_read.len(), deny: self.read_deny.len(), wdeny: self.write_deny.len() }
    }
    pub fn marks_dir(&self) -> PathBuf {
        self.init_home.join("run/mark")
    }
    pub fn mark_path(&self, instance: &str) -> PathBuf {
        self.marks_dir().join(instance)
    }
    /// The sibling decoy every instance is denied (sweep condition c, root/root.md §3.5.4).
    pub fn decoy(&self) -> PathBuf {
        self.marks_dir().join("decoy")
    }
    pub fn zone_mark(&self) -> PathBuf {
        self.init_home.join("run/zone-mark")
    }
    /// `ROOT_HOME/run/canary`, denied to every instance by Z1.
    pub fn canary(&self) -> PathBuf {
        self.root_home.join("run/canary")
    }
    pub fn egress_sock(&self) -> PathBuf {
        self.root_home.join("run/egress.sock")
    }
    pub fn owner_sock(&self) -> PathBuf {
        self.root_home.join("run/owner.sock")
    }
    /// The G0 build slot's `OUT` (kernel/sandbox.md §3.4): the only place outside `RUN` and `PROBE`
    /// where the `kernel` profile binds and connects Unix sockets (root/root.md §3.3.3 section 6a).
    pub fn slot_out(&self) -> PathBuf {
        self.init_home.join("cache/build/slot/out")
    }

    /// The tail's and ROOT_ALLOWS' parameters (root/root.md §3.3.2 table) for an instance whose
    /// mark is `mark` and whose ADR-009 grant is `grant`.
    pub fn zone_params(&self, mark: &Path, grant: &Grant) -> BTreeMap<String, String> {
        let i = &self.init_home;
        let mut m = BTreeMap::new();
        let mut put = |k: &str, v: &Path| {
            m.insert(k.to_owned(), path_str(v));
        };
        put("Z_ROOT_HOME", &self.root_home);
        put("Z_ROOT_SVC", &self.root_svc);
        put("Z_ROOT_SVC_DIR", self.root_svc.parent().unwrap_or(&self.root_svc));
        put("Z_WRITE", grant.write.as_deref().unwrap_or(i));
        if grant.read_home {
            put("Z_HOME", &self.home);
        }
        for (n, p) in self.read_deny.iter().enumerate() {
            put(&format!("Z_DENY_{n}"), p);
        }
        for (n, p) in self.write_deny.iter().enumerate() {
            put(&format!("Z_WDENY_{n}"), p);
        }
        put("Z_INIT_HOME", i);
        put("Z_RUN", &i.join("run"));
        put("Z_KERNEL", &i.join("kernel"));
        put("Z_PUB", &i.join("root"));
        put("Z_KGIT", &i.join("lineage/kernel.git"));
        put("Z_MARKS", &self.marks_dir());
        put("Z_MARK", mark);
        put("Z_ZONE_MARK", &self.zone_mark());
        put("Z_EGRESS_SOCK", &self.egress_sock());
        for (n, p) in self.extra_read.iter().enumerate() {
            put(&format!("Z_EXTRA_READ_{n}"), p);
        }
        m
    }

    /// The `kernel` profile's parameters, which the Root sets itself (root/root.md §3.3.3).
    pub fn kernel_profile_params(&self, toolchain: &Path, devdir: &Path, home_real: &Path) -> BTreeMap<String, String> {
        let i = &self.init_home;
        [
            ("INIT_HOME", i.clone()),
            ("RUN", i.join("run")),
            ("PROBE", i.join("probe")),
            ("MARKS", self.marks_dir()),
            ("EGRESS_SOCK", self.egress_sock()),
            ("SLOT_OUT", self.slot_out()),
            ("TOOLCHAIN", toolchain.to_path_buf()),
            ("DEVDIR", devdir.to_path_buf()),
            ("HOME_REAL", home_real.to_path_buf()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), path_str(&v)))
        .collect()
    }
}

/// A composed profile: its text and the complete parameter map, ready for `sandbox-exec`.
#[derive(Clone, Debug)]
pub struct Rendered {
    pub text: String,
    pub params: BTreeMap<String, String>,
}

impl Rendered {
    /// `["/usr/bin/sandbox-exec", "-p", text, "-DNAME=value"…]`. The caller appends `"--"`, the
    /// program and its arguments (root/root.md §3.3.1).
    pub fn sandbox_exec_argv(&self) -> Vec<OsString> {
        let mut v: Vec<OsString> = vec![SANDBOX_EXEC.into(), "-p".into(), self.text.clone().into()];
        v.extend(self.params.iter().map(|(k, val)| OsString::from(format!("-D{k}={val}"))));
        v
    }
}

/// Compose profile `id` for one instance. `given` are the kernel-supplied (already validated,
/// [`crate::params::validate_spawn_params`]) or Root-chosen parameters; this adds `MARK = Z_MARK`
/// and every `Z_*` (with the instance's ADR-009 `grant`, a body's only), then checks that the text's
/// parameters and the map agree exactly.
pub fn compose(id: ProfileId, given: &BTreeMap<String, String>, layout: &ZoneLayout, mark: &Path, grant: &Grant) -> Result<Rendered, ArgsError> {
    let text = render(id, layout.counts(), grant)?;
    finish(text, given, layout, mark, grant)
}

/// As [`compose`], over an arbitrary inner text (RR-1).
pub fn compose_over(inner: &str, given: &BTreeMap<String, String>, layout: &ZoneLayout, mark: &Path, grant: &Grant) -> Result<Rendered, ArgsError> {
    let text = render_over(inner, layout.counts(), grant)?;
    finish(text, given, layout, mark, grant)
}

fn finish(text: String, given: &BTreeMap<String, String>, layout: &ZoneLayout, mark: &Path, grant: &Grant) -> Result<Rendered, ArgsError> {
    let mut params = given.clone();
    let referenced = referenced_params(&text);
    let mut root_set = layout.zone_params(mark, grant);
    if referenced.contains("MARK") {
        root_set.insert("MARK".into(), path_str(mark));
    }
    for (k, v) in root_set {
        if params.insert(k.clone(), v).is_some() {
            return Err(ArgsError::new("V1", Some(&k), "set by the Root, not by the caller"));
        }
    }
    if let Some(missing) = referenced.iter().find(|n| !params.contains_key(*n)) {
        return Err(ArgsError::new("V1", Some(missing), "used by the profile and missing"));
    }
    if let Some(extra) = params.keys().find(|n| !referenced.contains(*n)) {
        return Err(ArgsError::new("V1", Some(extra), "not used by the profile"));
    }
    Ok(Rendered { text, params })
}

/// A new instance id: 12 hex digits from 6 bytes of `/dev/urandom` (root/root.md §3.5.1).
pub fn new_instance_id() -> io::Result<String> {
    let mut b = [0u8; 6];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// Create an instance mark (or the decoy, the zone mark, the canary): empty, mode 0400,
/// `O_CREAT|O_EXCL|O_NOFOLLOW` (root/root.md §3.5.1).
pub fn create_mark(path: &Path) -> io::Result<()> {
    OpenOptions::new().write(true).create_new(true).mode(0o400).custom_flags(libc::O_NOFOLLOW).open(path).map(drop)
}

pub(crate) fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::kernel_params;

    #[test]
    fn composition_order_and_tail_last() {
        for id in ProfileId::ALL {
            let t = render(id, Counts { deny: 3, wdeny: 2, ..Counts::default() }, &Grant::default()).unwrap();
            assert!(t.starts_with("(version 1)\n"));
            assert!(t.ends_with(&zone_tail(3, 2).unwrap()), "{id:?}: the zone tail must come last");
            let p = t.find(section(id)).unwrap();
            let r = t.find("; ---- R. init-root instance allows").unwrap();
            let z = t.find("; ==== Z. init-root zone tail").unwrap();
            assert!(PRELUDE.len() <= p && p < r && r < z);
        }
    }

    #[test]
    fn tail_is_deny_only() {
        for line in ZONE_TAIL.lines().chain(zone_tail(5, 4).unwrap().lines()) {
            let code = line.split(';').next().unwrap().trim_start();
            assert!(!code.starts_with("(allow"), "allow in the tail: {line}");
            assert!(!code.contains("define"), "define in the tail: {line}");
        }
    }

    #[test]
    fn no_text_redefines_anything() {
        for t in [PRELUDE, BODY_SECTION, BUILD_SECTION, GIT_READ_SECTION, KERNEL_SECTION, ROOT_ALLOWS_TEMPLATE, ZONE_TAIL] {
            assert!(!t.contains("(define"), "SBPL is Scheme: a define could switch the tail off");
        }
    }

    /// kernel/sandbox.md §3.4 section 6: `build`/`check` may use Unix sockets under OUT and nothing
    /// else; no IP rule anywhere in the text.
    #[test]
    fn build_network_is_unix_under_out_only() {
        let net: Vec<&str> = BUILD_SECTION.lines().filter(|l| !l.starts_with(';') && (l.contains("network") || l.contains("system-socket"))).collect();
        assert_eq!(
            net,
            [
                "(allow system-socket (socket-domain AF_UNIX))",
                "(allow network-bind (local unix-socket (subpath (param \"OUT\"))))",
                "(allow network-outbound (remote unix-socket (subpath (param \"OUT\"))))",
            ]
        );
        for t in [BUILD_SECTION, KERNEL_SECTION] {
            assert!(!t.contains(" ip ") && !t.contains("(remote ip") && !t.contains("(local ip"), "no IP rule");
            assert!(!t.contains("network*"), "no blanket network rule");
        }
        // root/root.md §3.3.3 section 6a: the kernel's only new socket area is the slot's OUT.
        let k: Vec<&str> = KERNEL_SECTION.lines().filter(|l| l.contains("SLOT_OUT")).collect();
        assert_eq!(k.len(), 2);
        assert!(k.iter().all(|l| l.contains("unix-socket (subpath (param \"SLOT_OUT\"))")));
        let layout = layout();
        assert_eq!(layout.kernel_profile_params(Path::new("/t"), Path::new("/d"), Path::new("/h"))["SLOT_OUT"], "/h/.init/cache/build/slot/out");
    }

    fn layout() -> ZoneLayout {
        ZoneLayout {
            root_home: "/h/.init-root".into(),
            init_home: "/h/.init".into(),
            root_svc: "/h/Library/LaunchAgents/dev.init.root.plist".into(),
            extra_read: vec![],
            home: "/h".into(),
            read_deny: vec!["/h/.ssh".into(), "/h/.aws".into()],
            write_deny: vec!["/h/.zshrc".into()],
        }
    }

    #[test]
    fn root_allows_extra_read_lines() {
        let none = Grant::default();
        assert_eq!(root_allows(0, &none).unwrap().lines().count(), 2);
        let r = root_allows(2, &none).unwrap();
        assert!(r.contains("(param \"Z_EXTRA_READ_0\")") && r.contains("(param \"Z_EXTRA_READ_1\")"));
        assert!(!r.contains("<i>"));
        assert_eq!(r.lines().count(), 2 + 1 + 4);
        assert!(root_allows(9, &none).is_err());
    }

    /// ADR-009: the read line only with a read grant, the write lines only with a write grant; the
    /// template itself holds every line.
    #[test]
    fn root_allows_grant_lines() {
        let read = Grant { read_home: true, write: None };
        let r = root_allows(0, &read).unwrap();
        assert_eq!(r.lines().count(), 2 + 2);
        assert!(r.contains("(subpath (param \"Z_HOME\"))") && !r.contains("Z_WRITE"));
        let both = Grant { read_home: true, write: Some("/h/p".into()) };
        let r = root_allows(1, &both).unwrap();
        assert_eq!(r.lines().count(), 2 + 3 + 2 + 3);
        assert!(r.ends_with("(allow file-read-metadata (path-ancestors (param \"Z_WRITE\")))\n"));
        let one = Counts { extra_read: 1, ..Counts::default() };
        assert_eq!(expand(ROOT_ALLOWS_TEMPLATE, one, &both, true).replace("_0\"", "_<i>\""), ROOT_ALLOWS_TEMPLATE);
        for l in r.lines().filter(|l| l.contains("Z_HOME") || l.contains("Z_WRITE")).filter(|l| !l.starts_with(';')) {
            assert!(l.contains("(require-not (subpath (param \"Z_INIT_HOME\")))") || l.contains("path-ancestors"), "{l}");
        }
    }

    /// ADR-009 tail Z2 and Z3a: one deny line per entry, the comment only when there is one; Z3 always names
    /// Z_WRITE; Z3b's git rule is fixed text.
    #[test]
    fn tail_deny_lines() {
        let t0 = zone_tail(0, 0).unwrap();
        assert!(!t0.contains("Z_DENY") && !t0.contains("read deny entry") && !t0.contains("Z_WDENY") && !t0.contains("Z3a"));
        assert!(t0.contains("(subpath (param \"Z_WRITE\"))") && t0.contains("(regex #\"/\\.git/hooks(/|$)\")"));
        let t2 = zone_tail(2, 3).unwrap();
        assert_eq!(t2.lines().count(), t0.lines().count() + 3 + 5);
        assert!(t2.contains("(param \"Z_DENY_0\")") && t2.contains("(param \"Z_DENY_1\")") && !t2.contains("<i>"));
        assert!(t2.contains("(param \"Z_WDENY_2\")"));
        let one = Counts { deny: 1, wdeny: 1, ..Counts::default() };
        assert_eq!(expand(ZONE_TAIL, one, &Grant::default(), false).replace("_0\"", "_<i>\""), ZONE_TAIL);
        assert!(zone_tail(MAX_DENY + 1, 0).is_err() && zone_tail(0, MAX_DENY + 1).is_err());
    }

    #[test]
    fn only_body_takes_a_grant() {
        let g = Grant { read_home: true, write: None };
        assert!(render(ProfileId::Body, Counts::default(), &g).is_ok());
        for id in [ProfileId::Kernel, ProfileId::Build, ProfileId::Check, ProfileId::GitRead] {
            assert_eq!(render(id, Counts::default(), &g).unwrap_err().rule, "V5");
        }
    }

    /// The Root's parameters follow the grant: Z_HOME only with a read grant, Z_WRITE always (INIT_HOME
    /// without a write grant), one Z_DENY_<i> per entry; compose's exact-set check holds for each shape.
    #[test]
    fn grant_params_match_the_text() {
        let l = layout();
        let mark = Path::new("/h/.init/run/mark/abc");
        let body: BTreeMap<String, String> = kernel_params(ProfileId::Body).into_iter().map(|n| (n.to_owned(), "/h/.init/x".to_owned())).collect();
        for g in [Grant::default(), Grant { read_home: true, write: None }, Grant { read_home: true, write: Some("/h/p".into()) }] {
            let r = compose(ProfileId::Body, &body, &l, mark, &g).unwrap();
            assert_eq!(r.params.contains_key("Z_HOME"), g.read_home);
            assert_eq!(r.params["Z_WRITE"], g.write.as_ref().map_or("/h/.init".into(), |w| w.display().to_string()));
            assert_eq!((r.params["Z_DENY_0"].as_str(), r.params["Z_DENY_1"].as_str()), ("/h/.ssh", "/h/.aws"));
            assert_eq!(r.params["Z_ROOT_SVC_DIR"], "/h/Library/LaunchAgents");
            assert_eq!(r.params["Z_WDENY_0"], "/h/.zshrc");
        }
    }

    /// The kernel-supplied lists in `params` are exactly what the texts use, minus MARK and Z_*.
    #[test]
    fn param_lists_match_texts() {
        for id in ProfileId::ALL {
            let used: BTreeSet<String> = referenced_params(&render(id, Counts::default(), &Grant::default()).unwrap()).into_iter().filter(|n| n != "MARK" && !n.starts_with("Z_")).collect();
            let listed: BTreeSet<String> = kernel_params(id).into_iter().map(str::to_owned).collect();
            assert_eq!(used, listed, "{id:?}");
        }
    }

    #[test]
    fn compose_refuses_root_names_and_gaps() {
        let layout = ZoneLayout { root_home: "/r".into(), init_home: "/i".into(), root_svc: "/s".into(), extra_read: vec![], ..ZoneLayout::default() };
        let mark = Path::new("/i/run/mark/abc");
        let full: BTreeMap<String, String> = kernel_params(ProfileId::Build).into_iter().map(|n| (n.to_owned(), "/i/x".to_owned())).collect();
        let r = compose(ProfileId::Build, &full, &layout, mark, &Grant::default()).unwrap();
        assert_eq!(r.params["MARK"], "/i/run/mark/abc");
        assert_eq!(r.params["Z_MARK"], "/i/run/mark/abc");
        let mut with_z = full.clone();
        with_z.insert("Z_ROOT_HOME".into(), "/x".into());
        assert!(compose(ProfileId::Build, &with_z, &layout, mark, &Grant::default()).is_err());
        let mut short = full.clone();
        short.remove("SRC");
        assert!(compose(ProfileId::Build, &short, &layout, mark, &Grant::default()).is_err());
        let mut extra = full;
        extra.insert("BODY".into(), "/i/body".into());
        assert!(compose(ProfileId::Build, &extra, &layout, mark, &Grant::default()).is_err());
        let argv = r.sandbox_exec_argv();
        assert_eq!(argv[0], SANDBOX_EXEC);
        assert_eq!(argv[1], "-p");
        assert!(argv[3..].iter().all(|a| a.to_string_lossy().starts_with("-D")));
    }
}
