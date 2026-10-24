//! LINUX lane: a hard per-workload memory limit (cgroup v2 `memory.max`) where the Root owns a delegated subtree, as a
//! backstop under the memory watchdog (memwatch.rs), which polls once a second and so can be outrun by a fast
//! allocation. Where no delegated subtree exists (the Root started from a terminal, cgroup v1, a container without
//! delegation), nothing here applies and the polling watchdog plus the disk floor are the guarantee, as on macOS.
//!
//! - **Delegation.** The systemd user unit (install.rs) says `Delegate=memory`, so the unit's cgroup belongs to the
//!   owner. At daemon start, before the kernel is spawned, `init` checks that the Root is that cgroup's only process,
//!   moves itself into the leaf `<base>/init-root` (cgroup v2 allows processes only in leaves once a controller is
//!   enabled for children) and enables `+memory` for the children. Any failure: no tree, logged once.
//! - **Workloads.** Every zone instance except the kernel (which the watchdog never kills) and every sealed run gets a
//!   leaf `<base>/w-<id>` with `memory.max` = `sandbox.mem_cap_mib` and `memory.swap.max` = 0 (the watchdog counts
//!   resident + swap; this keeps the same total). The child joins it after fork and before exec
//!   (`sys::join_cgroup_on_exec`), so bwrap, its namespaces and the payload all start inside it. `memory.oom.group`
//!   stays 0: the kernel's OOM killer takes the largest process (the runaway tool call), not the whole body.
//! - **Events.** `oom_kills` reads `memory.events`; the zone watchdog records each new kill as a `mem_cap` event with
//!   scope `cgroup` and alerts the owner. `release` removes the leaf once the instance is reaped (rmdir only succeeds on
//!   an empty cgroup, so nothing is ever killed here).
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

static TREE: OnceLock<Option<PathBuf>> = OnceLock::new();
static SEEN: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();

/// The Root's own cgroup v2 directory (`0::<path>` in `/proc/self/cgroup`), None on cgroup v1 or hybrid-only hosts.
pub fn own() -> Option<PathBuf> {
    let s = fs::read_to_string("/proc/self/cgroup").ok()?;
    let rel = s.lines().find_map(|l| l.strip_prefix("0::"))?;
    let base = Path::new("/sys/fs/cgroup").join(rel.trim_start_matches('/'));
    base.join("cgroup.controllers").is_file().then_some(base)
}

/// Prepares `base` (the Root's delegated cgroup) for workload leaves: Err says why it cannot (the caller falls back to
/// the polling watchdog alone).
pub fn prepare(base: &Path) -> Result<PathBuf, String> {
    let me = crate::sys::euid();
    let owned = |p: &Path| fs::metadata(p).is_ok_and(|m| m.uid() == me);
    if !fs::read_to_string(base.join("cgroup.controllers")).is_ok_and(|c| c.split_whitespace().any(|w| w == "memory")) {
        return Err(format!("the memory controller is not delegated to {}", base.display()));
    }
    for f in ["", "cgroup.procs", "cgroup.subtree_control"] {
        if !owned(&base.join(f)) {
            return Err(format!("{} is not delegated to uid {me} (systemd: Delegate=memory)", base.join(f).display()));
        }
    }
    let leaf = base.join("init-root");
    let mine = std::process::id().to_string();
    let procs = fs::read_to_string(base.join("cgroup.procs")).map_err(|e| e.to_string())?;
    let others: Vec<&str> = procs.lines().filter(|p| *p != mine).collect();
    if !others.is_empty() {
        return Err(format!("{} holds other processes ({}); the Root needs a cgroup of its own", base.display(), others.len()));
    }
    if !procs.lines().any(|p| p == mine) && !fs::read_to_string(leaf.join("cgroup.procs")).is_ok_and(|s| s.lines().any(|p| p == mine)) {
        return Err(format!("the Root is not in {}", base.display()));
    }
    match fs::create_dir(&leaf) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("{}: {e}", leaf.display())),
    }
    fs::write(leaf.join("cgroup.procs"), "0").map_err(|e| format!("moving the Root into {}: {e}", leaf.display()))?;
    fs::write(base.join("cgroup.subtree_control"), "+memory").map_err(|e| format!("enabling +memory under {}: {e}", base.display()))?;
    // leaves of an earlier daemon that are empty now (rmdir fails on a populated cgroup: nothing is killed)
    for e in fs::read_dir(base).into_iter().flatten().flatten() {
        if e.file_name().to_str().is_some_and(|n| n.starts_with("w-")) {
            let _ = fs::remove_dir(e.path());
        }
    }
    Ok(base.to_path_buf())
}

/// Once per daemon, before the first spawn: the delegated tree, or None (logged in `ROOT_HOME/logs`).
pub fn init(rh: &Path) -> Option<&'static Path> {
    TREE.get_or_init(|| {
        let r = own().ok_or_else(|| "no cgroup v2 hierarchy".to_string()).and_then(|b| prepare(&b));
        match &r {
            Ok(b) => crate::util::log(rh, "info", &format!("cgroup v2: workloads get memory.max under {}", b.display())),
            Err(e) => crate::util::log(rh, "info", &format!("cgroup v2 limits off ({e}); the polling memory watchdog applies")),
        }
        r.ok()
    }).as_deref()
}

/// Sets the tree explicitly (tests): `prepare` must have succeeded on it.
#[cfg(test)]
fn set_tree(base: PathBuf) -> bool {
    TREE.set(Some(base)).is_ok()
}

fn leaf(id: &str) -> Option<PathBuf> {
    let safe = !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    TREE.get()?.as_ref().filter(|_| safe).map(|b| b.join(format!("w-{id}")))
}

/// A workload leaf with `memory.max` = `max` bytes: the open `cgroup.procs` the child writes itself into, or None
/// (no tree, or the leaf could not be made: the polling watchdog still applies).
pub fn workload(id: &str, max: u64) -> Option<OwnedFd> {
    let dir = leaf(id)?;
    let made = (|| -> io::Result<OwnedFd> {
        fs::create_dir(&dir)?;
        fs::write(dir.join("memory.max"), max.to_string())?;
        if dir.join("memory.swap.max").exists() {
            fs::write(dir.join("memory.swap.max"), "0")?;
        }
        Ok(OpenOptions::new().write(true).open(dir.join("cgroup.procs"))?.into())
    })();
    match made {
        Ok(fd) => Some(fd),
        Err(_) => {
            let _ = fs::remove_dir(&dir); // exact path, made above; only succeeds while empty
            None
        }
    }
}

/// New OOM kills in a workload's leaf since the last call (0 when none, or no leaf).
pub fn oom_kills(id: &str) -> u64 {
    let Some(dir) = leaf(id) else { return 0 };
    let Some(n) = fs::read_to_string(dir.join("memory.events")).ok()
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix("oom_kill ")).and_then(|v| v.trim().parse::<u64>().ok())) else { return 0 };
    let mut seen = SEEN.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    let before = seen.insert(id.to_string(), n).unwrap_or(0);
    n.saturating_sub(before)
}

/// Removes a reaped workload's leaf, waiting up to 2 s for its last processes to leave (rmdir only succeeds on an
/// empty cgroup). Nothing is signalled.
pub fn release(id: &str) {
    let Some(dir) = leaf(id) else { return };
    if let Some(m) = SEEN.get() {
        m.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    }
    for _ in 0..40 {
        match fs::remove_dir(&dir) {
            Err(e) if e.raw_os_error() == Some(libc::EBUSY) => std::thread::sleep(std::time::Duration::from_millis(50)),
            _ => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, Stdio};

    #[test]
    fn a_tree_that_is_not_delegated_is_refused_without_side_effects() {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/test-homes/cgroup-{}", std::process::id()));
        fs::create_dir_all(&base).unwrap();
        assert!(prepare(&base).unwrap_err().contains("memory controller"), "no cgroup.controllers");
        fs::write(base.join("cgroup.controllers"), "cpu io memory pids\n").unwrap();
        fs::write(base.join("cgroup.procs"), "1\n2\n").unwrap();
        fs::write(base.join("cgroup.subtree_control"), "").unwrap();
        assert!(prepare(&base).unwrap_err().contains("other processes"));
        assert!(!base.join("init-root").exists(), "nothing made before every check passed");
        fs::remove_dir_all(&base).unwrap();
    }

    /// Native qualification (needs a delegated cgroup v2 subtree holding only this test process, INIT_TEST_CGROUP=1;
    /// run alone, it moves this process): a workload over `memory.max` dies by the OOM killer inside bwrap's user and
    /// pid namespaces, the kill is counted, a sibling outside the leaf is untouched, the leaf is removed afterwards.
    #[test]
    #[ignore = "requires a delegated cgroup v2 subtree (INIT_TEST_CGROUP=1) and bubblewrap"]
    fn a_workload_over_memory_max_is_oom_killed_inside_its_leaf() {
        assert_eq!(std::env::var("INIT_TEST_CGROUP").as_deref(), Ok("1"), "set INIT_TEST_CGROUP=1 in a delegated subtree");
        let base = prepare(&own().expect("cgroup v2")).expect("delegated subtree");
        assert!(set_tree(base.clone()));
        let fd = workload("qualify", 64 << 20).expect("leaf");
        assert_eq!(fs::read_to_string(base.join("w-qualify/memory.max")).unwrap().trim(), (64u64 << 20).to_string());
        let mut sibling = Command::new("/usr/bin/python3").args(["-c", "import time; b=bytearray(96<<20); time.sleep(30)"])
            .stdin(Stdio::null()).spawn().unwrap();
        let mut c = Command::new("/usr/bin/bwrap");
        c.args(["--unshare-user", "--unshare-pid", "--die-with-parent", "--ro-bind", "/", "/", "--proc", "/proc", "--dev", "/dev", "--",
            "/usr/bin/python3", "-c", "b=bytearray(512<<20); print('survived')"]).stdin(Stdio::null()).stdout(Stdio::piped());
        crate::sys::join_cgroup_on_exec(&mut c, fd.as_raw_fd());
        let out = c.output().unwrap();
        drop(fd);
        assert!(!String::from_utf8_lossy(&out.stdout).contains("survived"), "the hog ran to completion over memory.max");
        assert!(!out.status.success(), "{:?}", out.status);
        assert!(oom_kills("qualify") >= 1, "memory.events counted no oom_kill: {}",
            fs::read_to_string(base.join("w-qualify/memory.events")).unwrap_or_default());
        assert!(sibling.try_wait().unwrap().is_none(), "the sibling outside the leaf was affected");
        release("qualify");
        assert!(!base.join("w-qualify").exists(), "the empty leaf is removed");
        sibling.kill().unwrap();
        assert_eq!(sibling.wait().unwrap().signal(), Some(libc::SIGKILL));
        eprintln!("cgroup qualification passed: memory.max held through bwrap namespaces; leaf {} removed", base.join("w-qualify").display());
    }
}
