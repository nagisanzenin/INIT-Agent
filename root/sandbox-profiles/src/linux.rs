//! Fail-closed Linux mount plan. A fresh root exposes only these mounts; no host-root bind.
//! Broad ADR-009 home/workspace grants are refused until their deny overlays are implemented.
//! ADR-012 full access (the live body only) is the exception: the host root bound read-write, the host network, and
//! overlays for the Root-protected set (ROOT_HOME, INIT_HOME's own plan, the read and write deny lists, the user
//! service manager's sockets).
use crate::{ProfileId, access::Grant};
use std::{collections::BTreeMap, ffi::OsString, path::{Path, PathBuf}};

pub struct Layout {
    pub root_home: PathBuf,
    pub init_home: PathBuf,
    pub toolchain: PathBuf,
    pub extra_read: Vec<PathBuf>,
    /// The read deny list. For a full grant the Root passes the full list ([`crate::access::full_read_deny`]), and each
    /// existing entry is hidden (a directory under a read-only tmpfs, a file under `/dev/null`).
    pub read_deny: Vec<PathBuf>,
    /// ADR-012, a full grant only: the full write deny list ([`crate::access::full_write_deny`]); each existing entry is
    /// bound read-only onto itself. Unused otherwise (workspace-mode grants stay refused on Linux).
    pub write_deny: Vec<PathBuf>,
    /// ADR-012, a full grant only: the owner's canonical home. The ancestors of each deny entry below it (below `/` for
    /// an entry outside it) become mount points, so they cannot be renamed away.
    pub home: PathBuf,
    pub frozen: bool,
    pub benchmark: bool,
}
fn add(v: &mut Vec<OsString>, flag: &str, p: &Path) {
    v.extend([flag.into(), p.as_os_str().to_owned(), p.as_os_str().to_owned()]);
}
fn path<'a>(p: &'a BTreeMap<String, String>, key: &str) -> Result<&'a Path, String> {
    p.get(key).map(Path::new).ok_or_else(|| format!("Linux profile missing {key}"))
}
fn canonical(p: &Path) -> Result<(), String> {
    if !p.is_absolute() || std::fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()))? != p {
        return Err(format!("Linux mount must be canonical: {}", p.display()));
    }
    Ok(())
}
fn overlap(a: &Path, b: &Path) -> bool { a.starts_with(b) || b.starts_with(a) }

/// Debian-style compiler names in /usr/bin can point through /etc/alternatives.
/// Recreate only those selector links, with their targets already exposed by the
/// pinned `/usr` mount.  A bind-mounted driver appears to GCC as executing from
/// `/etc/alternatives`, so it cannot locate sibling helpers such as `cc1`.
/// A symlink retains the driver's canonical `/usr/bin/...` executable path.
/// Binding all of `/etc` (or all alternatives) is unnecessary.
fn compiler_selectors(v: &mut Vec<OsString>, alternatives: &Path, binaries: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    for name in ["cc", "c++"] {
        let selector = alternatives.join(name);
        match std::fs::symlink_metadata(&selector) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("Linux compiler selector {}: {e}", selector.display())),
            Ok(_) => {},
        }
        let target = std::fs::canonicalize(&selector).map_err(|e| format!("Linux compiler selector {}: {e}", selector.display()))?;
        let metadata = std::fs::metadata(&target).map_err(|e| format!("Linux compiler target {}: {e}", target.display()))?;
        if !target.starts_with(binaries) || !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err(format!("Linux compiler selector must resolve to a regular executable under {}: {}", binaries.display(), selector.display()));
        }
        v.extend(["--symlink".into(), target.into_os_string(), selector.into_os_string()]);
    }
    Ok(())
}

/// The calling process's real uid and gid, from `/proc/self/status` (no FFI here).
fn owner_ids() -> Result<(u32, u32), String> {
    let status = std::fs::read_to_string("/proc/self/status").map_err(|e| format!("/proc/self/status: {e}"))?;
    let first = |key: &str| status.lines().find_map(|l| l.strip_prefix(key)).and_then(|v| v.split_whitespace().next()?.parse().ok());
    match (first("Uid:"), first("Gid:")) {
        (Some(u), Some(g)) => Ok((u, g)),
        _ => Err("cannot read the owner's uid and gid from /proc/self/status".into()),
    }
}

fn foundation(full: bool) -> Result<Vec<OsString>, String> {
    // unshare execs bwrap (no shell/fork), retaining the Root-held leader PID. The outer
    // net namespace stays isolated; unlike bwrap --unshare-net it never configures lo,
    // which also works on restricted kernels that reject RTM_NEWADDR. Neither stage
    // may fall back to sharing a namespace. ADR-012 full access keeps the host network (no `--net`).
    let unshare: &[&str] = if full { &["/usr/bin/unshare", "--user", "--map-root-user", "--"] } else { &["/usr/bin/unshare", "--user", "--map-root-user", "--net", "--"] };
    let mut v: Vec<OsString> = unshare.iter().chain(&["/usr/bin/bwrap", "--die-with-parent", "--new-session", "--unshare-user", "--unshare-pid",
        "--unshare-ipc", "--unshare-uts", "--disable-userns", "--cap-drop", "ALL", "--add-seccomp-fd", "5"]).map(Into::into).collect();
    // Full access: the host root read-write first (every later mount overlays it), then a fresh /proc and /dev; the
    // owner's /tmp stays the host's. Otherwise a fresh root with a private /tmp.
    let mounts: &[&str] = if full { &["--bind", "/", "/", "--proc", "/proc", "--dev", "/dev"] } else { &["--proc", "/proc", "--dev", "/dev", "--size", "67108864", "--tmpfs", "/tmp"] };
    v.extend(mounts.iter().map(Into::into));
    // The zone keeps the owner's own uid and gid (as Seatbelt does): `unshare --map-root-user` makes the Root's
    // uid 0 in the outer namespace, and bwrap maps that 0 back to the owner's ids, so getuid() and the passwd entry
    // (/etc/passwd is bound) give the owner's HOME_REAL, which the broker's V3 and class rules compare against.
    let (uid, gid) = owner_ids()?;
    v.extend(["--uid".into(), uid.to_string().into(), "--gid".into(), gid.to_string().into()]);
    if full {
        return Ok(v); // the system paths and compiler selectors are already visible through the host root
    }
    // Host /etc, /run, /home, /root and /sys are deliberately not mounted.
    for s in ["/usr", "/bin", "/sbin", "/lib", "/lib64", "/etc/ld.so.cache", "/etc/ld.so.conf", "/etc/ld.so.conf.d", "/etc/passwd", "/etc/group"] {
        let q = Path::new(s);
        if q.exists() { add(&mut v, "--ro-bind", q); }
    }
    compiler_selectors(&mut v, Path::new("/etc/alternatives"), Path::new("/usr/bin"))?;
    Ok(v)
}

pub fn compose(id: ProfileId, p: &BTreeMap<String, String>, l: &Layout, mark: &Path, grant: &Grant) -> Result<Vec<OsString>, String> {
    if grant.full && (id != ProfileId::Body || grant.read_home || grant.write.is_some()) {
        return Err("Linux full access is the live body's alone, with no other grant".into());
    }
    if grant.read_home { return Err("Linux broad home reads are not yet supported".into()); }
    if let Some(w) = &grant.write {
        canonical(w)?;
        let tasks = l.init_home.join("work/tasks");
        if !l.benchmark || !w.starts_with(&tasks) || l.read_deny.iter().any(|d| overlap(w, d)) {
            return Err("Linux write grant requires an owner benchmark and INIT_HOME/work/tasks containment".into());
        }
    }
    canonical(&l.init_home)?;
    canonical(&l.root_home)?;
    canonical(mark)?;
    if grant.full && overlap(&l.root_home, &l.init_home) {
        return Err("Linux ROOT_HOME and INIT_HOME must be disjoint".into());
    }
    let mut v = foundation(grant.full)?;
    if grant.full {
        // The deny entries' ancestors pinned first (before INIT_HOME is hidden: a later bind of one of them would bring
        // the host's INIT_HOME back), then INIT_HOME keeps the body's own plan: hidden, then the body's binds below
        // re-expose its parts.
        for a in pinned_ancestors(l) {
            add(&mut v, "--bind", &a);
        }
        v.extend(["--tmpfs".into(), l.init_home.as_os_str().to_owned()]);
    }
    // Without a Rust toolchain the Root names INIT_HOME/kernel as a harmless placeholder (zone.rs `vctx`): the
    // profile already mounts what it needs of INIT_HOME, so the placeholder adds no mount. Full access sees the
    // toolchain and extra_read through the host root (writable, as on macOS): no read-only mounts for them.
    let placeholder = l.init_home.join("kernel");
    let reads: Vec<&PathBuf> = if grant.full { vec![] } else { std::iter::once(&l.toolchain).filter(|t| **t != placeholder).chain(l.extra_read.iter()).collect() };
    for q in reads {
        canonical(q)?;
        if overlap(q, &l.root_home) || overlap(q, &l.init_home) || l.read_deny.iter().any(|d| overlap(q, d)) {
            return Err(format!("Linux read mount overlaps protected tree: {}", q.display()));
        }
        add(&mut v, "--ro-bind", q);
    }
    let mut bind = |key: &str, writable: bool| -> Result<(), String> {
        let q = path(p, key)?;
        // A lane socket the kernel has not bound yet is left out, as the offline kernel plan leaves out an absent
        // gateway: an absent socket grants nothing, and a bind of a missing path would refuse the whole spawn.
        if key.ends_with("_SOCK") && std::fs::symlink_metadata(q).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
            return Ok(());
        }
        canonical(q)?;
        if !q.starts_with(&l.init_home) || overlap(q, &l.root_home) { return Err(format!("Linux {key} outside INIT_HOME")); }
        let frozen = l.frozen && !(id == ProfileId::Body && key == "WORK")
            && ["body", "bin", "kernel", "work/body", "work/kernel"].iter().any(|n| overlap(q, &l.init_home.join(n)));
        add(&mut v, if writable && !frozen { "--bind" } else { "--ro-bind" }, q);
        Ok(())
    };
    match id {
        ProfileId::Kernel => {
            // Writable subtrees, not a writable INIT_HOME parent: anchors cannot be renamed.
            for name in ["body", "work", "state", "cache", "vault", "probe", "fleet", "lineage", "run", "bin"] {
                let q = l.init_home.join(name);
                if q.exists() { canonical(&q)?; add(&mut v, "--bind", &q); }
            }
            for name in ["kernel", "root", "lineage/kernel.git", "run/zone-mark"] {
                let q = l.init_home.join(name);
                if q.exists() { canonical(&q)?; add(&mut v, "--ro-bind", &q); }
            }
            let marks = l.init_home.join("run/mark");
            v.extend(["--tmpfs".into(), marks.as_os_str().to_owned()]);
            // Installation is offline and runs before the gateway is started.
            // A later daemon boot starts it before spawning its kernel. An absent
            // socket grants no network access; neither phase exposes other Root files.
            let gateway = l.root_home.join("run/egress.sock");
            use std::os::unix::fs::FileTypeExt;
            match std::fs::symlink_metadata(&gateway) {
                Ok(m) if m.file_type().is_socket() => add(&mut v, "--ro-bind", &gateway),
                Ok(_) => return Err("Linux gateway path is not a Unix socket".into()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
                Err(e) => return Err(format!("Linux gateway socket: {e}")),
            }
        }
        ProfileId::Body => {
            for k in ["BODY", "WORK", "STATE"] { bind(k, true)?; }
            for k in ["STATE_KERNEL", "KERNEL", "BIN", "LINEAGE", "PUB", "KERNEL_SOCK", "PROXY_SOCK"] { bind(k, false)?; }
            for (link, target) in [("LINK_KERNEL", "KERNEL"), ("LINK_BIN", "BIN"), ("LINK_LINEAGE", "LINEAGE")] {
                let (a, b) = (path(p, link)?, path(p, target)?);
                if a != b {
                    if !a.starts_with(&l.init_home) { return Err("Linux link outside INIT_HOME".into()); }
                    v.extend(["--symlink".into(), b.as_os_str().to_owned(), a.as_os_str().to_owned()]);
                }
            }
        }
        ProfileId::Build | ProfileId::Check => { bind("SRC", false)?; bind("KERNEL", false)?; bind("OUT", true)?; }
        ProfileId::GitRead => { bind("BODY", false)?; bind("WORK", false)?; }
    }
    if l.frozen {
        let paths = match id {
            ProfileId::Kernel => ["body", "bin", "kernel", "work/body", "work/kernel"].iter().map(|s| l.init_home.join(s)).collect::<Vec<_>>(),
            ProfileId::Body => vec![path(p, "BODY")?.to_path_buf(), path(p, "WORK")?.join("body"), path(p, "WORK")?.join("kernel")],
            _ => Vec::new(),
        };
        for q in paths {
            if q.exists() { canonical(&q)?; add(&mut v, "--ro-bind", &q); }
        }
    }
    if let Some(w) = &grant.write { add(&mut v, "--bind", w); }
    // Mark access is read-only; unrelated marks are not exposed by Body/Build.
    add(&mut v, "--ro-bind", mark);
    if id == ProfileId::Kernel {
        v.extend(["--remount-ro".into(), l.init_home.join("run/mark").into_os_string()]);
    }
    if grant.full {
        full_overlays(&mut v, l, &owner_run_dir()?)?;
        return Ok(v); // no final `--remount-ro /`: the owner's files stay writable
    }
    v.extend(["--remount-ro".into(), "/".into()]);
    Ok(v)
}

/// `/run/user/<uid>`: where the owner's user service manager and session bus listen.
fn owner_run_dir() -> Result<PathBuf, String> {
    Ok(PathBuf::from(format!("/run/user/{}", owner_ids()?.0)))
}

/// The existing, canonical entries of a deny list (with whether each is a directory), outside ROOT_HOME (hidden whole)
/// and INIT_HOME (its own plan), without duplicates. A missing entry gets no mount: a mount point would create it in
/// the owner's real tree.
fn existing(l: &Layout, list: &[PathBuf]) -> Vec<(PathBuf, bool)> {
    let mut out: Vec<(PathBuf, bool)> = Vec::new();
    for q in list.iter().filter_map(|q| std::fs::canonicalize(q).ok()) {
        if overlap(&q, &l.root_home) || overlap(&q, &l.init_home) || out.iter().any(|(o, _)| *o == q) {
            continue;
        }
        let dir = q.is_dir();
        out.push((q, dir));
    }
    out
}

/// ADR-012: a bind mount follows its dentry, so renaming a deny entry's parent away (`mv ~/.local/bin x`) would move
/// the read-only bind with it and leave the original path free to recreate. Every ancestor of every existing deny entry,
/// from just below the owner's home (just below `/` for an entry outside the home) down to its parent, is bound onto
/// itself read-write, outer first: a mount point cannot be renamed or removed (`EBUSY`), and its contents stay writable.
/// Measured by lane fa-linux (bwrap 0.13.0, Linux 6.10).
fn pinned_ancestors(l: &Layout) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for (q, _) in existing(l, &l.read_deny).into_iter().chain(existing(l, &l.write_deny)) {
        let base = if q.starts_with(&l.home) && q != l.home { l.home.as_path() } else { Path::new("/") };
        for a in q.ancestors().skip(1).take_while(|a| *a != base && a.starts_with(base)) {
            if !out.iter().any(|o| o == a) && !overlap(a, &l.init_home) && !overlap(a, &l.root_home) {
                out.push(a.to_path_buf());
            }
        }
    }
    out.sort_by_key(|a| a.components().count()); // outer to inner
    out
}

/// ADR-012: the Root-protected set over the host root. ROOT_HOME is an empty read-only tmpfs (no keys, no owner.sock,
/// no egress.sock); each existing read deny entry is hidden (a directory under an empty read-only tmpfs, a file under
/// `/dev/null`); each existing write deny entry (the service directories, INIT's launchers) is bound read-only onto
/// itself; the user service manager's private socket directory is hidden and the session bus socket replaced by
/// `/dev/null`, so `systemctl --user stop init-root` cannot reach them. A missing entry gets no mount: a mount point
/// would create it in the owner's real tree. Entries are canonicalized (the bind sources are pinned without following
/// links) and deduplicated.
fn full_overlays(v: &mut Vec<OsString>, l: &Layout, run_user: &Path) -> Result<(), String> {
    v.extend(["--tmpfs".into(), l.root_home.as_os_str().to_owned(), "--remount-ro".into(), l.root_home.as_os_str().to_owned()]);
    for (q, dir) in existing(l, &l.read_deny) {
        if dir {
            v.extend(["--tmpfs".into(), q.clone().into_os_string(), "--remount-ro".into(), q.into_os_string()]);
        } else {
            v.extend(["--ro-bind".into(), "/dev/null".into(), q.into_os_string()]);
        }
    }
    for (q, _) in existing(l, &l.write_deny) {
        add(v, "--ro-bind", &q);
    }
    let systemd = run_user.join("systemd");
    if systemd.is_dir() {
        v.extend(["--tmpfs".into(), systemd.clone().into_os_string(), "--remount-ro".into(), systemd.into_os_string()]);
    }
    let bus = run_user.join("bus");
    if std::fs::symlink_metadata(&bus).is_ok() {
        v.extend(["--ro-bind".into(), "/dev/null".into(), bus.into_os_string()]);
    }
    Ok(())
}

/// A sealed run sees only its own copied state/artifacts, the toolchain and its exact lane socket.
pub fn sealed(run: &Path, root_home: &Path, toolchain: &Path, lane: Option<&Path>) -> Result<Vec<OsString>, String> {
    canonical(run)?; canonical(root_home)?; canonical(toolchain)?;
    let parent = root_home.join("sealed/run");
    if run.parent() != Some(parent.as_path()) || overlap(toolchain, root_home) {
        return Err("invalid Linux sealed run or toolchain".into());
    }
    let mut v = foundation(false)?;
    add(&mut v, "--ro-bind", toolchain);
    add(&mut v, "--bind", run);
    // Candidate executables cannot overwrite their own launched copy.
    let bin = run.join("bin"); canonical(&bin)?; add(&mut v, "--ro-bind", &bin);
    if let Some(sock) = lane {
        canonical(sock)?;
        if !sock.starts_with(root_home.join("run")) && !sock.starts_with(run) { return Err("sealed socket outside Root run directories".into()); }
        add(&mut v, "--ro-bind", sock);
    }
    v.extend(["--remount-ro".into(), "/".into()]);
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(tag: &str) -> (PathBuf, Layout, PathBuf) {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../target/test-homes/linux-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(dir).unwrap();
        let l = Layout { root_home: dir.join("rh"), init_home: dir.join("ih"), toolchain: dir.join("tool"), extra_read: vec![], read_deny: vec![dir.join("secret")], write_deny: vec![], home: dir.join("home"), frozen: false, benchmark: false };
        for p in [&l.root_home, &l.init_home, &l.toolchain] { std::fs::create_dir_all(p).unwrap(); }
        let mark = l.init_home.join("mark"); std::fs::write(&mark, b"").unwrap();
        (dir, l, mark)
    }
    #[test]
    fn linux_plan_requires_network_seccomp_and_exact_write_mounts() {
        let (dir, l, mark) = fixture("plan");
        let mut p = BTreeMap::new();
        for k in ["SRC", "OUT", "KERNEL"] {
            let d = l.init_home.join(k.to_lowercase()); std::fs::create_dir_all(&d).unwrap();
            p.insert(k.into(), d.display().to_string());
        }
        let v = compose(ProfileId::Build, &p, &l, &mark, &Grant::default()).unwrap();
        let v: Vec<_> = v.iter().map(|x| x.to_str().unwrap()).collect();
        for option in ["--unshare-user", "--unshare-pid", "--net", "--disable-userns", "--add-seccomp-fd", "--die-with-parent"] { assert!(v.contains(&option)); }
        // the zone keeps the owner's ids (getuid() inside is the owner's, not the outer namespace's root)
        let uid = std::fs::metadata("/proc/self").map(|m| std::os::unix::fs::MetadataExt::uid(&m)).unwrap().to_string();
        assert!(v.windows(2).any(|w| w[0] == "--uid" && w[1] == uid), "{v:?}");
        let writable: Vec<_> = v.windows(3).filter(|w| w[0] == "--bind").map(|w| w[1]).collect();
        assert_eq!(writable, vec![p["OUT"].as_str()]);
        assert!(!v.windows(3).any(|w| w[0] == "--ro-bind" && w[1] == "/"));
        assert!(!v.contains(&l.root_home.to_str().unwrap()));
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn linux_compiler_selectors_recreate_only_existing_system_selector_links() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let (dir, _, _) = fixture("compiler-selectors");
        let alternatives = dir.join("etc/alternatives");
        let binaries = dir.join("usr/bin");
        std::fs::create_dir_all(&alternatives).unwrap();
        std::fs::create_dir_all(&binaries).unwrap();
        let gcc = binaries.join("gcc-12");
        std::fs::write(&gcc, b"compiler fixture").unwrap();
        std::fs::set_permissions(&gcc, std::fs::Permissions::from_mode(0o755)).unwrap();
        symlink("gcc-12", binaries.join("gcc")).unwrap();
        symlink(binaries.join("gcc"), alternatives.join("cc")).unwrap();
        // Unrelated alternatives are neither traversed nor exposed.
        symlink(dir.join("private-secret"), alternatives.join("editor")).unwrap();
        let mut args = Vec::new();
        compiler_selectors(&mut args, &alternatives, &binaries).unwrap();
        assert_eq!(args, vec![OsString::from("--symlink"), gcc.into_os_string(), alternatives.join("cc").into_os_string()]);
        assert!(!args.iter().any(|a| a == alternatives.as_os_str() || a == dir.join("etc").as_os_str()));
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn linux_compiler_selectors_refuse_external_dangling_and_non_executable_targets() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let (dir, _, _) = fixture("compiler-selector-denied");
        let alternatives = dir.join("etc/alternatives");
        let binaries = dir.join("usr/bin");
        std::fs::create_dir_all(&alternatives).unwrap();
        std::fs::create_dir_all(&binaries).unwrap();
        let selector = alternatives.join("cc");
        let private = dir.join("private-compiler");
        std::fs::write(&private, b"private").unwrap();
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&private, &selector).unwrap();
        assert!(compiler_selectors(&mut Vec::new(), &alternatives, &binaries).is_err());
        std::fs::remove_file(&private).unwrap();
        assert!(compiler_selectors(&mut Vec::new(), &alternatives, &binaries).is_err());
        std::fs::remove_file(&selector).unwrap();
        let plain = binaries.join("non-executable");
        std::fs::write(&plain, b"not executable").unwrap();
        symlink(&plain, &selector).unwrap();
        assert!(compiler_selectors(&mut Vec::new(), &alternatives, &binaries).is_err());
        std::fs::remove_file(&selector).unwrap();
        symlink(&binaries, &selector).unwrap();
        assert!(compiler_selectors(&mut Vec::new(), &alternatives, &binaries).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn frozen_helpers_cannot_write_artifacts_and_sealed_mounts_are_narrow() {
        let (dir, mut l, mark) = fixture("frozen");
        l.frozen = true;
        let mut p = BTreeMap::new();
        for (key, name) in [("SRC", "cache/src"), ("OUT", "body"), ("KERNEL", "kernel")] {
            let q = l.init_home.join(name);
            std::fs::create_dir_all(&q).unwrap();
            p.insert(key.into(), q.display().to_string());
        }
        // Even a caller violating Vparams' OUT containment cannot regain artifact writes.
        let v = compose(ProfileId::Build, &p, &l, &mark, &Grant::default()).unwrap();
        assert!(!v.windows(3).any(|w| w[0] == "--bind" && w[1] == p["OUT"].as_str()));
        let run = l.root_home.join("sealed/run/candidate");
        std::fs::create_dir_all(run.join("bin")).unwrap();
        let v = sealed(&run, &l.root_home, &l.toolchain, None).unwrap();
        let writable: Vec<_> = v.windows(3).filter(|w| w[0] == "--bind").map(|w| w[1].clone()).collect();
        assert_eq!(writable, vec![run.as_os_str().to_owned()]);
        assert!(!v.iter().any(|a| a == l.init_home.as_os_str()));
        assert!(sealed(&l.init_home, &l.root_home, &l.toolchain, None).is_err());
        assert!(sealed(&run, &l.root_home, &l.toolchain, Some(&mark)).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn offline_kernel_plan_omits_absent_gateway_and_live_plan_pins_only_socket() {
        let (dir, l, mark) = fixture("offline-kernel");
        let args = compose(ProfileId::Kernel, &BTreeMap::new(), &l, &mark, &Grant::default()).unwrap();
        assert!(!args.iter().any(|a| a.to_string_lossy().contains("egress.sock")));
        std::fs::create_dir_all(l.root_home.join("run")).unwrap();
        let socket = l.root_home.join("run/egress.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let args = compose(ProfileId::Kernel, &BTreeMap::new(), &l, &mark, &Grant::default()).unwrap();
        assert!(args.windows(3).any(|w| w[0] == "--ro-bind" && w[1] == socket.as_os_str()));
        assert!(!args.iter().any(|a| a == l.root_home.as_os_str()));
        drop(listener);
        std::fs::remove_file(&socket).unwrap();
        std::fs::create_dir(&socket).unwrap();
        assert!(compose(ProfileId::Kernel, &BTreeMap::new(), &l, &mark, &Grant::default()).is_err());
        std::fs::remove_dir(&socket).unwrap();
        std::fs::write(&socket, b"not a socket").unwrap();
        assert!(compose(ProfileId::Kernel, &BTreeMap::new(), &l, &mark, &Grant::default()).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn linux_kernel_vault_is_writable_without_exposing_init_home_parent() {
        let (dir, l, mark) = fixture("kernel-vault");
        let names = ["body", "work", "state", "cache", "vault", "probe", "fleet", "lineage", "run", "bin"];
        for name in names { std::fs::create_dir_all(l.init_home.join(name)).unwrap(); }
        for name in ["kernel", "root", "lineage/kernel.git"] { std::fs::create_dir_all(l.init_home.join(name)).unwrap(); }
        let args = compose(ProfileId::Kernel, &BTreeMap::new(), &l, &mark, &Grant::default()).unwrap();
        let writable = args.windows(3).filter(|w| w[0] == "--bind").map(|w| PathBuf::from(w[1].clone())).collect::<Vec<_>>();
        assert_eq!(writable, names.iter().map(|n| l.init_home.join(n)).collect::<Vec<_>>());
        assert!(!writable.iter().any(|p| p == &l.init_home || p == &l.root_home));
        assert!(args.windows(3).any(|w| w[0] == "--ro-bind" && w[1] == l.init_home.join("lineage/kernel.git").as_os_str()));
        // Candidate builds keep vault hidden; their only writable tree is OUT.
        let p = [("SRC", "cache/src"), ("OUT", "cache/out"), ("KERNEL", "kernel")].map(|(key, name)| {
            let path = l.init_home.join(name); std::fs::create_dir_all(&path).unwrap();
            (key.to_string(), path.display().to_string())
        }).into_iter().collect();
        let build = compose(ProfileId::Build, &p, &l, &mark, &Grant::default()).unwrap();
        assert!(!build.iter().any(|part| part == l.init_home.join("vault").as_os_str()));
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn linux_body_plan_omits_an_unbound_lane_socket_only() {
        let (dir, l, mark) = fixture("body-socks");
        let mut p = BTreeMap::new();
        for (k, sub) in [("BODY", "body"), ("WORK", "work"), ("STATE", "state"), ("STATE_KERNEL", "state/kernel"), ("KERNEL", "kernel"),
            ("BIN", "bin"), ("LINEAGE", "lineage"), ("PUB", "root")] {
            let d = l.init_home.join(sub); std::fs::create_dir_all(&d).unwrap(); p.insert(k.to_string(), d.display().to_string());
        }
        for (k, target) in [("LINK_KERNEL", "KERNEL"), ("LINK_BIN", "BIN"), ("LINK_LINEAGE", "LINEAGE")] { let v = p[target].clone(); p.insert(k.into(), v); }
        std::fs::create_dir_all(l.init_home.join("run")).unwrap();
        let ks = l.init_home.join("run/kernel.sock");
        p.insert("KERNEL_SOCK".into(), ks.display().to_string());
        p.insert("PROXY_SOCK".into(), l.init_home.join("run/proxy.sock").display().to_string());
        let args = compose(ProfileId::Body, &p, &l, &mark, &Grant::default()).unwrap();
        assert!(!args.iter().any(|a| a.to_string_lossy().ends_with(".sock")), "no unbound socket is mounted");
        let _k = std::os::unix::net::UnixListener::bind(&ks).unwrap();
        let args = compose(ProfileId::Body, &p, &l, &mark, &Grant::default()).unwrap();
        assert!(args.windows(3).any(|w| w[0] == "--ro-bind" && w[1] == ks.as_os_str()), "a bound socket is mounted read-only");
        p.insert("BODY".into(), l.init_home.join("missing").display().to_string());
        assert!(compose(ProfileId::Body, &p, &l, &mark, &Grant::default()).is_err(), "a missing directory still refuses");
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn linux_without_a_toolchain_the_kernel_placeholder_adds_no_mount() {
        let (dir, mut l, mark) = fixture("no-toolchain");
        l.toolchain = l.init_home.join("kernel");
        std::fs::create_dir_all(&l.toolchain).unwrap();
        let args = compose(ProfileId::Kernel, &BTreeMap::new(), &l, &mark, &Grant::default()).unwrap();
        assert!(args.windows(3).any(|w| w[0] == "--ro-bind" && w[1] == l.toolchain.as_os_str()), "the kernel dir stays read-only");
        l.toolchain = l.init_home.join("work");
        std::fs::create_dir_all(&l.toolchain).unwrap();
        assert!(compose(ProfileId::Kernel, &BTreeMap::new(), &l, &mark, &Grant::default()).is_err(), "any other INIT_HOME path is refused");
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn linux_broad_grants_and_protected_reads_fail_closed() {
        let (dir, mut l, mark) = fixture("denied");
        assert!(compose(ProfileId::Kernel, &BTreeMap::new(), &l, &mark, &Grant { read_home: true, ..Grant::default() }).is_err());
        assert!(compose(ProfileId::Kernel, &BTreeMap::new(), &l, &mark, &Grant { write: Some(dir.clone()), ..Grant::default() }).is_err());
        assert!(compose(ProfileId::Kernel, &BTreeMap::new(), &l, &mark, &Grant { full: true, ..Grant::default() }).is_err(), "full access is a body's only");
        l.extra_read.push(dir.clone());
        assert!(compose(ProfileId::Kernel, &BTreeMap::new(), &l, &mark, &Grant::default()).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The live body's parameter map over the fixture's INIT_HOME (every directory created, sockets left unbound).
    fn body_params(l: &Layout) -> BTreeMap<String, String> {
        let mut p = BTreeMap::new();
        for (k, sub) in [("BODY", "body"), ("WORK", "work"), ("STATE", "state"), ("STATE_KERNEL", "state/kernel"), ("KERNEL", "kernel"),
            ("BIN", "bin"), ("LINEAGE", "lineage"), ("PUB", "root")] {
            let d = l.init_home.join(sub); std::fs::create_dir_all(&d).unwrap(); p.insert(k.to_string(), d.display().to_string());
        }
        for (k, target) in [("LINK_KERNEL", "KERNEL"), ("LINK_BIN", "BIN"), ("LINK_LINEAGE", "LINEAGE")] { let v = p[target].clone(); p.insert(k.into(), v); }
        std::fs::create_dir_all(l.init_home.join("run")).unwrap();
        p.insert("KERNEL_SOCK".into(), l.init_home.join("run/kernel.sock").display().to_string());
        p.insert("PROXY_SOCK".into(), l.init_home.join("run/proxy.sock").display().to_string());
        p
    }

    /// ADR-012: a full grant keeps the host network (no `--net`), binds the host root read-write first, keeps a fresh
    /// /proc and /dev and the host /tmp, hides INIT_HOME before the body's own binds re-expose it, hides ROOT_HOME under
    /// a read-only tmpfs, hides the read deny entries, binds the service directory and the two launchers read-only,
    /// and never remounts / read-only. Namespaces, caps, seccomp and die-with-parent stay.
    #[test]
    fn linux_full_plan_binds_the_host_root_and_overlays_the_root_protected_set() {
        let (dir, mut l, mark) = fixture("full");
        let (keys, svc, bin) = (dir.join("home/.config/init-agent"), dir.join("home/.config/systemd/user"), dir.join("home/.local/bin"));
        for d in [&keys, &svc, &bin] { std::fs::create_dir_all(d).unwrap(); }
        for f in ["init", "init-tui", "other-tool"] { std::fs::write(bin.join(f), b"#!/bin/sh\n").unwrap(); }
        let tax = dir.join("home/tax.txt"); std::fs::write(&tax, b"private").unwrap();
        l.read_deny = vec![keys.clone(), tax.clone(), dir.join("home/missing")];
        l.write_deny = vec![svc.clone(), bin.join("init"), bin.join("init-tui"), dir.join("home/Library/LaunchAgents")];
        let p = body_params(&l);
        let full = Grant { full: true, ..Grant::default() };
        let v = compose(ProfileId::Body, &p, &l, &mark, &full).unwrap();
        let v: Vec<String> = v.iter().map(|x| x.to_string_lossy().into_owned()).collect();
        let at = |w: &[&str]| v.windows(w.len()).position(|x| x.iter().zip(w).all(|(a, b)| a == b));
        assert!(!v.iter().any(|a| a == "--net"), "full access keeps the host network: {v:?}");
        for option in ["--unshare-user", "--unshare-pid", "--unshare-ipc", "--unshare-uts", "--disable-userns", "--add-seccomp-fd", "--die-with-parent", "--new-session"] {
            assert!(v.iter().any(|a| a == option), "{option}");
        }
        assert!(at(&["--cap-drop", "ALL"]).is_some());
        let root = at(&["--bind", "/", "/"]).expect("the host root, read-write");
        assert!(root < at(&["--proc", "/proc"]).unwrap() && root < at(&["--dev", "/dev"]).unwrap(), "fresh /proc and /dev over the host root");
        assert!(at(&["--tmpfs", "/tmp"]).is_none(), "the owner's /tmp stays the host's");
        assert!(!v.windows(2).any(|w| w[0] == "--remount-ro" && w[1] == "/"), "no final remount-ro /");
        let (ih, rh) = (l.init_home.display().to_string(), l.root_home.display().to_string());
        let hide = at(&["--tmpfs", &ih]).expect("INIT_HOME hidden first");
        let body = at(&["--bind", &p["BODY"], &p["BODY"]]).expect("the body's own rw bind");
        assert!(root < hide && hide < body, "INIT_HOME is hidden after the host root and before the body's binds");
        assert!(at(&["--ro-bind", &p["KERNEL"], &p["KERNEL"]]).is_some(), "the rest of INIT_HOME as the workspace plan: ro");
        let r = at(&["--tmpfs", &rh, "--remount-ro", &rh]).expect("ROOT_HOME: an empty read-only tmpfs");
        assert!(r > root);
        let keys = keys.display().to_string();
        assert!(at(&["--tmpfs", &keys, "--remount-ro", &keys]).is_some(), "the key dir hidden");
        assert!(at(&["--ro-bind", "/dev/null", &tax.display().to_string()]).is_some(), "a read deny file under /dev/null");
        assert!(!v.iter().any(|a| a.ends_with("home/missing") || a.ends_with("LaunchAgents")), "a missing entry gets no mount");
        for q in [svc.clone(), bin.join("init"), bin.join("init-tui")] {
            let q = q.display().to_string();
            assert!(at(&["--ro-bind", &q, &q]).is_some(), "{q} read-only");
        }
        assert!(!v.iter().any(|a| a.ends_with("other-tool")) && at(&["--ro-bind", &bin.display().to_string()]).is_none(), "the rest of the bin dir stays writable");
        // the deny entries' ancestors below the home are pinned as mount points (outer first), before INIT_HOME is hidden
        let home = dir.join("home");
        let pins: Vec<String> = [".config", ".local", ".config/systemd", ".local/bin"].iter().map(|a| home.join(a).display().to_string()).collect();
        for a in &pins {
            let at_pin = at(&["--bind", a, a]).unwrap_or_else(|| panic!("{a} pinned: {v:?}"));
            assert!(root < at_pin && at_pin < hide, "{a}: after the host root, before the INIT_HOME tmpfs");
        }
        assert!(at(&["--bind", &pins[0], &pins[0]]).unwrap() < at(&["--bind", &pins[2], &pins[2]]).unwrap(), "outer to inner");
        assert!(at(&["--bind", &home.display().to_string()]).is_none(), "the home itself is not a pin");
        let writable: Vec<_> = v.windows(3).filter(|w| w[0] == "--bind").map(|w| w[1].clone()).collect();
        let want: Vec<String> = ["/".to_string()].into_iter().chain(pins.iter().cloned()).chain([p["BODY"].clone(), p["WORK"].clone(), p["STATE"].clone()]).collect();
        assert_eq!(writable, want);
        // the live body only, and nothing else rides along
        for id in [ProfileId::Kernel, ProfileId::Build, ProfileId::Check, ProfileId::GitRead] {
            assert!(compose(id, &p, &l, &mark, &full).is_err(), "{id:?}");
        }
        assert!(compose(ProfileId::Body, &p, &l, &mark, &Grant { read_home: true, ..full.clone() }).is_err());
        assert!(compose(ProfileId::Body, &p, &l, &mark, &Grant { write: Some(dir.clone()), ..full }).is_err());
        // the workspace plan is unchanged: --net, a private /tmp, no host root, / read-only at the end
        let w = compose(ProfileId::Body, &p, &l, &mark, &Grant::default()).unwrap();
        let w: Vec<String> = w.iter().map(|x| x.to_string_lossy().into_owned()).collect();
        assert!(w.iter().any(|a| a == "--net") && w.windows(2).any(|x| x[0] == "--tmpfs" && x[1] == "/tmp"));
        assert!(!w.windows(3).any(|x| x[0] == "--bind" && x[1] == "/") && w.ends_with(&["--remount-ro".to_string(), "/".to_string()]));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// ADR-012 (fa-linux's rename finding): every ancestor of an existing deny entry below the home is a pin, outer
    /// first, never the home itself; an entry outside the home is pinned below `/`; missing entries and INIT_HOME's or
    /// ROOT_HOME's insides give none.
    #[test]
    fn linux_full_pins_deny_ancestors_below_the_home() {
        let (dir, mut l, _) = fixture("full-pins");
        l.home = dir.join("home");
        let (bin, unit, out) = (l.home.join(".local/bin"), l.home.join(".config/systemd/user"), dir.join("elsewhere/bin"));
        for d in [&bin, &unit, &out] { std::fs::create_dir_all(d).unwrap(); }
        std::fs::write(bin.join("init"), b"").unwrap();
        std::fs::write(out.join("init"), b"").unwrap();
        l.read_deny = vec![l.home.join("missing/x"), l.root_home.join("k")];
        l.write_deny = vec![bin.join("init"), unit.clone(), out.join("init"), l.init_home.join("x")];
        let pins = pinned_ancestors(&l);
        let rel = |p: &PathBuf| p.strip_prefix(&dir).map(|r| r.display().to_string()).unwrap_or_else(|_| p.display().to_string());
        let got: Vec<String> = pins.iter().map(rel).collect();
        // the home's own entries, then (outer first) the outside entry's ancestors below `/`, which include the fixture dir
        for want in ["home/.local", "home/.config", "home/.local/bin", "home/.config/systemd", "elsewhere/bin", "elsewhere"] {
            assert!(got.iter().any(|g| g == want), "{want} pinned: {got:?}");
        }
        assert!(!pins.contains(&l.home) && !pins.iter().any(|p| p.starts_with(l.home.join("missing"))), "{got:?}");
        assert!(pins.windows(2).all(|w| w[0].components().count() <= w[1].components().count()), "outer to inner: {got:?}");
        assert!(!pins.iter().any(|p| p == Path::new("/")));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// ADR-012: the user service manager's sockets are hidden when they exist, and nothing is mounted when they do not.
    #[test]
    fn linux_full_overlays_hide_the_user_service_manager() {
        let (dir, mut l, _) = fixture("full-run-user");
        l.read_deny = vec![];
        let run = dir.join("run-user");
        let mut v = Vec::new();
        full_overlays(&mut v, &l, &run).unwrap();
        assert_eq!(v.len(), 4, "only ROOT_HOME without a run dir: {v:?}");
        std::fs::create_dir_all(run.join("systemd")).unwrap();
        std::fs::write(run.join("bus"), b"").unwrap(); // stands for the bus socket: any existing entry is covered
        let mut v = Vec::new();
        full_overlays(&mut v, &l, &run).unwrap();
        let s = |p: PathBuf| p.into_os_string();
        assert!(v.windows(4).any(|w| w[0] == "--tmpfs" && w[1] == s(run.join("systemd")) && w[2] == "--remount-ro"));
        assert!(v.windows(3).any(|w| w[0] == "--ro-bind" && w[1] == "/dev/null" && w[2] == s(run.join("bus"))));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
