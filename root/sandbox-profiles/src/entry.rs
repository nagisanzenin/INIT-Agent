//! The Linux zone's entry binaries: util-linux `unshare` (the outer stage) and bubblewrap (docs/platforms.md §3.4.6).
//!
//! Ubuntu 24.04 and later restrict unprivileged user namespaces through AppArmor
//! (`kernel.apparmor_restrict_unprivileged_userns = 1`). There the owner installs, once and with sudo, root-owned copies
//! of both binaries in [`HELPER_DIR`] and an AppArmor profile that grants those two paths `userns` and nothing else
//! (`scripts/install.sh --print-apparmor` prints the commands; INIT never runs sudo by itself and never asks for the
//! global sysctl). The Root then starts every zone through those copies. A copy is trusted only when nobody but root can
//! change it: the canonical file is a regular file owned by root and writable by root alone, and so is every directory
//! above it. A copy that exists and fails that check refuses the zone (fail closed); it is never skipped for the system
//! binary, because a writable copy that AppArmor grants `userns` is exactly what an attacker would plant. With no copy
//! the system binaries in `/usr/bin` are used, as before.
//!
//! Inside the zone nothing changes: bwrap's `--disable-userns` and the zone's seccomp filter still refuse every new
//! namespace, whichever binary started it.
use std::io;
use std::path::{Path, PathBuf};

/// Where the owner's root-owned copies live. Not a bin directory (issue nagisanzenin/INIT-Agent#1).
pub const HELPER_DIR: &str = "/usr/local/lib/init-agent";
/// The AppArmor profile file that grants the copies `userns`.
pub const PROFILE: &str = "/etc/apparmor.d/init-agent";

/// What the trust check needs to know about one path (from `symlink_metadata`).
#[derive(Clone, Copy, Debug)]
pub struct Meta {
    pub uid: u32,
    pub mode: u32,
    pub file: bool,
    pub dir: bool,
}

impl Meta {
    #[cfg(unix)]
    pub fn of(m: &std::fs::Metadata) -> Meta {
        use std::os::unix::fs::MetadataExt;
        Meta { uid: m.uid(), mode: m.mode() & 0o7777, file: m.file_type().is_file(), dir: m.file_type().is_dir() }
    }
}

/// The real filesystem, without following a final symlink.
#[cfg(unix)]
pub fn look(p: &Path) -> io::Result<Meta> {
    std::fs::symlink_metadata(p).map(|m| Meta::of(&m))
}

/// `canonical` is a canonical absolute path (no symlink anywhere in it). It may be used as a zone entry when it is a
/// regular file owned by root (uid 0) that neither group nor others can write, and every directory above it is owned by
/// root and writable by root alone (no sticky-bit exception: a sticky world-writable directory still lets another user
/// create a name in it).
pub fn root_only(canonical: &Path, look: &dyn Fn(&Path) -> io::Result<Meta>) -> Result<(), String> {
    let show = canonical.display();
    if !canonical.is_absolute() {
        return Err(format!("zone entry {show} is not an absolute path"));
    }
    let m = look(canonical).map_err(|e| format!("zone entry {show}: {e}"))?;
    if !m.file {
        return Err(format!("zone entry {show} is not a regular file"));
    }
    if m.uid != 0 || m.mode & 0o022 != 0 {
        return Err(format!("zone entry {show} must be owned by root and writable by root alone (uid {}, mode {:o})", m.uid, m.mode));
    }
    for dir in canonical.ancestors().skip(1) {
        let d = look(dir).map_err(|e| format!("zone entry directory {}: {e}", dir.display()))?;
        if !d.dir || d.uid != 0 || d.mode & 0o022 != 0 {
            return Err(format!("zone entry directory {} must be a directory owned by root and writable by root alone (uid {}, mode {:o})",
                dir.display(), d.uid, d.mode));
        }
    }
    Ok(())
}

/// The binary the zone starts as `name` (`unshare` or `bwrap`): the owner's root-owned copy in `helper_dir` when there is
/// one and it passes [`root_only`], else `system`. `canon` resolves symlinks (the copy is checked and used at its
/// canonical path, so a root-owned link cannot point the check at one file and the exec at another).
pub fn resolve(name: &str, helper_dir: &Path, system: &Path, canon: &dyn Fn(&Path) -> io::Result<PathBuf>,
    look: &dyn Fn(&Path) -> io::Result<Meta>) -> Result<PathBuf, String> {
    let helper = helper_dir.join(name);
    match look(&helper) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(system.to_path_buf()),
        Err(e) => Err(format!("zone entry {}: {e}", helper.display())),
        Ok(_) => {
            let c = canon(&helper).map_err(|e| format!("zone entry {}: {e}", helper.display()))?;
            root_only(&c, look)?;
            Ok(c)
        }
    }
}

/// [`resolve`] on the real filesystem with [`HELPER_DIR`] and `/usr/bin/<name>`.
#[cfg(unix)]
pub fn zone_entry(name: &str) -> Result<PathBuf, String> {
    resolve(name, Path::new(HELPER_DIR), &Path::new("/usr/bin").join(name), &|p| std::fs::canonicalize(p), &look)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A fake filesystem: canonical paths only, plus links resolved by `canon`.
    struct Fs {
        meta: BTreeMap<PathBuf, Meta>,
        links: BTreeMap<PathBuf, PathBuf>,
    }
    impl Fs {
        fn new() -> Fs {
            let mut f = Fs { meta: BTreeMap::new(), links: BTreeMap::new() };
            for d in ["/", "/usr", "/usr/bin", "/usr/local", "/usr/local/lib"] {
                f.dir(d, 0, 0o755);
            }
            f
        }
        fn dir(&mut self, p: &str, uid: u32, mode: u32) -> &mut Fs {
            self.meta.insert(p.into(), Meta { uid, mode, file: false, dir: true });
            self
        }
        fn file(&mut self, p: &str, uid: u32, mode: u32) -> &mut Fs {
            self.meta.insert(p.into(), Meta { uid, mode, file: true, dir: false });
            self
        }
        fn look(&self, p: &Path) -> io::Result<Meta> {
            if self.links.contains_key(p) {
                return Ok(Meta { uid: 0, mode: 0o777, file: false, dir: false });
            }
            self.meta.get(p).copied().ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }
        fn canon(&self, p: &Path) -> io::Result<PathBuf> {
            Ok(self.links.get(p).cloned().unwrap_or_else(|| p.to_path_buf()))
        }
        fn resolve(&self, name: &str) -> Result<PathBuf, String> {
            resolve(name, Path::new(HELPER_DIR), &Path::new("/usr/bin").join(name), &|p| self.canon(p), &|p| self.look(p))
        }
    }

    #[test]
    fn no_copy_means_the_system_binaries() {
        let fs = Fs::new();
        assert_eq!(fs.resolve("unshare").unwrap(), Path::new("/usr/bin/unshare"));
        let mut fs = Fs::new();
        fs.dir(HELPER_DIR, 0, 0o755);
        assert_eq!(fs.resolve("bwrap").unwrap(), Path::new("/usr/bin/bwrap"), "an empty helper dir changes nothing");
    }

    #[test]
    fn a_root_only_copy_is_used() {
        let mut fs = Fs::new();
        fs.dir(HELPER_DIR, 0, 0o755).file("/usr/local/lib/init-agent/unshare", 0, 0o755).file("/usr/local/lib/init-agent/bwrap", 0, 0o750);
        assert_eq!(fs.resolve("unshare").unwrap(), Path::new("/usr/local/lib/init-agent/unshare"));
        assert_eq!(fs.resolve("bwrap").unwrap(), Path::new("/usr/local/lib/init-agent/bwrap"), "0750 (the owner's group may run it)");
    }

    #[test]
    fn a_copy_anyone_but_root_can_change_refuses_the_zone() {
        let h = "/usr/local/lib/init-agent/unshare";
        let refused = |fs: &Fs, why: &str| {
            let e = fs.resolve("unshare").unwrap_err();
            assert!(e.contains(why), "{e}");
        };
        // the file: not root's, group- or world-writable, not a regular file
        let mut fs = Fs::new();
        fs.dir(HELPER_DIR, 0, 0o755).file(h, 1000, 0o755);
        refused(&fs, "owned by root");
        let mut fs = Fs::new();
        fs.dir(HELPER_DIR, 0, 0o755).file(h, 0, 0o775);
        refused(&fs, "writable by root alone");
        let mut fs = Fs::new();
        fs.dir(HELPER_DIR, 0, 0o755).file(h, 0, 0o757);
        refused(&fs, "writable by root alone");
        let mut fs = Fs::new();
        fs.dir(HELPER_DIR, 0, 0o755).dir(h, 0, 0o755);
        refused(&fs, "not a regular file");
        // a directory above it: the helper dir, /usr/local/lib, /usr/local, / ; owner, group write, world write, sticky
        for (d, uid, mode) in [(HELPER_DIR, 1000, 0o755), (HELPER_DIR, 0, 0o775), ("/usr/local/lib", 0, 0o777), ("/usr/local", 0, 0o1777), ("/", 1000, 0o755)] {
            let mut fs = Fs::new();
            fs.dir(HELPER_DIR, 0, 0o755).file(h, 0, 0o755).dir(d, uid, mode);
            refused(&fs, "directory owned by root and writable by root alone");
        }
        // a directory above it that is missing from the fake filesystem (an unreadable one) refuses too
        let mut fs = Fs::new();
        fs.file(h, 0, 0o755);
        refused(&fs, "zone entry directory /usr/local/lib/init-agent");
    }

    #[test]
    fn a_link_is_checked_and_used_at_its_target() {
        let h = "/usr/local/lib/init-agent/unshare";
        // a link to a root-only file elsewhere: the target is what runs, so the target is what is checked
        let mut fs = Fs::new();
        fs.dir(HELPER_DIR, 0, 0o755).file("/usr/bin/unshare", 0, 0o755);
        fs.links.insert(h.into(), "/usr/bin/unshare".into());
        assert_eq!(fs.resolve("unshare").unwrap(), Path::new("/usr/bin/unshare"));
        // a link into a directory the owner can write: refused, never a fallback to the system binary
        let mut fs = Fs::new();
        fs.dir(HELPER_DIR, 0, 0o755).dir("/home", 0, 0o755).dir("/home/o", 1000, 0o755).file("/home/o/unshare", 1000, 0o755);
        fs.links.insert(h.into(), "/home/o/unshare".into());
        assert!(fs.resolve("unshare").is_err());
    }

    #[test]
    fn a_relative_path_is_refused() {
        assert!(root_only(Path::new("usr/bin/unshare"), &|_| Ok(Meta { uid: 0, mode: 0o755, file: true, dir: false })).is_err());
    }
}
