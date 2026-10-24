//! Fail-closed Linux mount plan. A fresh root exposes only these mounts; no host-root bind.
//! Broad ADR-009 home/workspace grants are refused until their deny overlays are implemented.
use crate::{ProfileId, access::Grant};
use std::{collections::BTreeMap, ffi::OsString, path::{Path, PathBuf}};

pub struct Layout {
    pub root_home: PathBuf,
    pub init_home: PathBuf,
    pub toolchain: PathBuf,
    pub extra_read: Vec<PathBuf>,
    pub read_deny: Vec<PathBuf>,
    pub frozen: bool,
    pub benchmark: bool,
}
fn add(v: &mut Vec<OsString>, flag: &str, p: &Path) {
    v.extend([flag.into(), p.as_os_str().to_owned(), p.as_os_str().to_owned()]);
}
fn path<'a>(p: &'a BTreeMap<String, String>, key: &str) -> Result<&'a Path, String> {
    p.get(key).map(|v| Path::new(v)).ok_or_else(|| format!("Linux profile missing {key}"))
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

fn foundation() -> Result<Vec<OsString>, String> {
    // unshare execs bwrap (no shell/fork), retaining the Root-held leader PID. The outer
    // net namespace stays isolated; unlike bwrap --unshare-net it never configures lo,
    // which also works on restricted kernels that reject RTM_NEWADDR. Neither stage
    // may fall back to sharing a namespace.
    let mut v: Vec<OsString> = ["/usr/bin/unshare", "--user", "--map-root-user", "--net", "--", "/usr/bin/bwrap", "--die-with-parent", "--new-session", "--unshare-user", "--unshare-pid",
        "--unshare-ipc", "--unshare-uts", "--disable-userns", "--cap-drop", "ALL", "--add-seccomp-fd", "5",
        "--proc", "/proc", "--dev", "/dev", "--size", "67108864", "--tmpfs", "/tmp"].into_iter().map(Into::into).collect();
    // The zone keeps the owner's own uid and gid (as Seatbelt does): `unshare --map-root-user` makes the Root's
    // uid 0 in the outer namespace, and bwrap maps that 0 back to the owner's ids, so getuid() and the passwd entry
    // (/etc/passwd is bound) give the owner's HOME_REAL, which the broker's V3 and class rules compare against.
    let (uid, gid) = owner_ids()?;
    v.extend(["--uid".into(), uid.to_string().into(), "--gid".into(), gid.to_string().into()]);
    // Host /etc, /run, /home, /root and /sys are deliberately not mounted.
    for s in ["/usr", "/bin", "/sbin", "/lib", "/lib64", "/etc/ld.so.cache", "/etc/ld.so.conf", "/etc/ld.so.conf.d", "/etc/passwd", "/etc/group"] {
        let q = Path::new(s);
        if q.exists() { add(&mut v, "--ro-bind", q); }
    }
    compiler_selectors(&mut v, Path::new("/etc/alternatives"), Path::new("/usr/bin"))?;
    Ok(v)
}

pub fn compose(id: ProfileId, p: &BTreeMap<String, String>, l: &Layout, mark: &Path, grant: &Grant) -> Result<Vec<OsString>, String> {
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
    let mut v = foundation()?;
    // Without a Rust toolchain the Root names INIT_HOME/kernel as a harmless placeholder (zone.rs `vctx`): the
    // profile already mounts what it needs of INIT_HOME, so the placeholder adds no mount.
    let placeholder = l.init_home.join("kernel");
    for q in std::iter::once(&l.toolchain).filter(|t| **t != placeholder).chain(l.extra_read.iter()) {
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
        let l = Layout { root_home: dir.join("rh"), init_home: dir.join("ih"), toolchain: dir.join("tool"), extra_read: vec![], read_deny: vec![dir.join("secret")], frozen: false, benchmark: false };
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
        assert!(compose(ProfileId::Kernel, &BTreeMap::new(), &l, &mark, &Grant { read_home: true, write: None }).is_err());
        assert!(compose(ProfileId::Kernel, &BTreeMap::new(), &l, &mark, &Grant { read_home: false, write: Some(dir.clone()) }).is_err());
        l.extra_read.push(dir.clone());
        assert!(compose(ProfileId::Kernel, &BTreeMap::new(), &l, &mark, &Grant::default()).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

/// A sealed run sees only its own copied state/artifacts, the toolchain and its exact lane socket.
pub fn sealed(run: &Path, root_home: &Path, toolchain: &Path, lane: Option<&Path>) -> Result<Vec<OsString>, String> {
    canonical(run)?; canonical(root_home)?; canonical(toolchain)?;
    let parent = root_home.join("sealed/run");
    if run.parent() != Some(parent.as_path()) || overlap(toolchain, root_home) {
        return Err("invalid Linux sealed run or toolchain".into());
    }
    let mut v = foundation()?;
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
