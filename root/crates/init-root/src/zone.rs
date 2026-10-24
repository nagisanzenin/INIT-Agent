//! R1: the spawn broker (root/root.md §3.3–§3.5, §3.14): V1–V5 and profile composition (crate
//! `init-sandbox-profiles`, WP6a), marks, `exec-clean` spawns in their own process group, the fd
//! hand-over, reapers, the instance table (`state/root.json`), kills and quotas.
//! Live-system safety (CONVENTIONS §1, D1): the Root signals only pids and process groups it spawned and
//! has not reaped yet (a zombie leader keeps both ours). The D1 scan (`sweep.rs`) runs only in builds
//! with the `live-sweep` feature, which nothing on the owner's machine enables (ADR-008: owner-machine
//! mode; `sys::signal` itself refuses any pid that is not an unreaped child).

use crate::link::{Root, lock};
use crate::sys;
use crate::util::{self, now_ms, rfc3339, write_atomic};
use init_sandbox_profiles::ProfileId;
use init_sandbox_profiles::access::Grant;
use init_sandbox_profiles::params::{self, ValidationCtx};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::{UnixDatagram, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// A refusal on the control link: code, message, `error.data`.
pub type Err3 = (&'static str, String, Value);

#[derive(Clone, Debug)]
pub struct Inst {
    pub kind: String,
    pub lane: Option<String>,
    pub pid: i32,
    pub at: i64,
    reaped: bool,
}

/// The instance table. Every zone process is one of these, from spawn until its reaper is done.
#[derive(Default)]
pub struct Broker {
    insts: Mutex<BTreeMap<String, Inst>>,
    pub cpu_ms: AtomicU64,
}

/// What the kernel sends with `spawn` (§4.1.3).
#[derive(Deserialize)]
struct SpawnReq {
    kind: String,
    profile: String,
    params: BTreeMap<String, String>,
    program: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    cwd: String,
    lane: Option<String>,
    replaces: Option<String>,
}

pub struct KernelProc {
    pub inst: String,
    pub pid: i32,
    pub ctl: UnixStream,
    pub fdc: UnixDatagram,
}

fn info_path(root: &Root, k: &str) -> Option<PathBuf> {
    root.info[k].as_str().map(PathBuf::from)
}

/// The validation context of V1–V5 (§3.3.4): install.json paths and the charter's `extra_read`.
fn vctx(root: &Root) -> ValidationCtx {
    let extra = lock(&root.st).charter.as_ref().map(|l| l.ch.sandbox.extra_read.iter().map(PathBuf::from).collect()).unwrap_or_default();
    let kdir = root.p.i("kernel"); // a harmless readable root when install.json has no toolchain or devdir
    ValidationCtx { init_home: root.p.ih.clone(), toolchain: info_path(root, "toolchain").unwrap_or_else(|| kdir.clone()),
        devdir: info_path(root, "devdir").unwrap_or(kdir), home_real: root.p.home.clone(), extra_read: extra }
}

/// §3.3.1: PRELUDE + SECTION + ROOT_ALLOWS + the zone tail, as a `sandbox-exec` argv (values only as `-D`).
/// ADR-009 (SBX-009): the deny lists for every instance, and a body's `grant`. ADR-012: a full grant takes the full
/// lists (and the full tail).
#[cfg(target_os = "macos")]
fn launcher(root: &Root, id: ProfileId, given: &BTreeMap<String, String>, mark: &Path, grant: &Grant) -> Result<Vec<OsString>, String> {
    use init_sandbox_profiles::seatbelt::{ZoneLayout, compose};
    let c = vctx(root);
    let svc = crate::install::service_path(&root.p);
    let home = crate::access::home(&root.p);
    let (read_deny, write_deny) = if grant.full { (crate::access::deny_full(root), crate::access::write_deny_full(root)) }
        else { (crate::access::deny(root), crate::access::write_deny(root)) }; // tail Z2, Z3a
    let layout = ZoneLayout { root_home: root.p.rh.clone(), init_home: root.p.ih.clone(), root_svc: svc, extra_read: c.extra_read, home, read_deny, write_deny };
    let given = if id == ProfileId::Kernel { &layout.kernel_profile_params(&c.toolchain, &c.devdir, &c.home_real) } else { given };
    compose(id, given, &layout, mark, grant).map(|r| r.sandbox_exec_argv()).map_err(|e| e.to_string())
}

#[cfg(target_os = "linux")]
fn launcher(root: &Root, id: ProfileId, given: &BTreeMap<String, String>, mark: &Path, grant: &Grant) -> Result<Vec<OsString>, String> {
    let c = vctx(root);
    // ADR-012: a full grant takes the full lists; workspace-mode grants stay refused by the Linux plan
    let (read_deny, write_deny) = if grant.full { (crate::access::deny_full(root), crate::access::write_deny_full(root)) } else { (crate::access::deny(root), vec![]) };
    let home = crate::access::home(&root.p);
    if grant.full {
        // systemd reads `init-root.service.d/` drop-ins from each user unit directory: a missing one would get no mount
        // and the zone could create it, so the Root creates it empty first (the plan then binds it read-only)
        for d in init_sandbox_profiles::access::LINUX_UNIT_DIRS {
            if let Err(e) = fs::create_dir_all(home.join(d)) {
                return Err(format!("full access: cannot create {}: {e}", home.join(d).display()));
            }
        }
    }
    let layout = init_sandbox_profiles::linux::Layout { root_home: root.p.rh.clone(), init_home: root.p.ih.clone(),
        toolchain: c.toolchain, extra_read: c.extra_read, read_deny, write_deny, home,
        frozen: root.benchmark.as_ref().is_some_and(|p| p.frozen()), benchmark: root.benchmark.is_some() };
    init_sandbox_profiles::linux::compose(id, given, &layout, mark, grant)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn launcher(_: &Root, _: ProfileId, _: &BTreeMap<String, String>, _: &Path, _: &Grant) -> Result<Vec<OsString>, String> {
    Err("the Linux zone (bwrap, root/root.md §3.4) is not built yet (WP6b)".into())
}

impl Broker {
    fn add(&self, id: &str, kind: &str, lane: Option<String>, pid: i32) -> i64 {
        let at = now_ms();
        lock(&self.insts).insert(id.into(), Inst { kind: kind.into(), lane, pid, at, reaped: false });
        at
    }

    /// Live instances, by kind or all.
    pub fn count(&self, kind: Option<&str>) -> usize {
        lock(&self.insts).values().filter(|i| !i.reaped && kind.is_none_or(|k| i.kind == k)).count()
    }

    /// MEM-RUNAWAY: the held instances, as the memory watchdog's leaders (memwatch.rs).
    pub fn leaders(&self) -> Vec<crate::memwatch::Leader> {
        lock(&self.insts).iter().filter(|(_, i)| !i.reaped).map(|(id, i)| crate::memwatch::Leader { key: id.clone(), kind: i.kind.clone(),
            pid: i.pid }).collect()
    }

    pub fn pid_of(&self, id: &str) -> Option<i32> {
        lock(&self.insts).get(id).filter(|i| !i.reaped).map(|i| i.pid)
    }

    /// `instances` (§4.1.3).
    pub fn list(&self) -> Value {
        let t = lock(&self.insts);
        t.iter().filter(|(_, i)| !i.reaped).map(|(id, i)| json!({"instance": id, "kind": i.kind, "lane": i.lane, "pid": i.pid,
            "started_at": rfc3339(i.at)})).collect()
    }

    /// Signals one held instance: its process group and its leader. None for an unknown instance.
    pub fn kill(&self, id: &str, sig: i32) -> Option<usize> {
        let t = lock(&self.insts);
        let i = t.get(id)?;
        Some(usize::from(!i.reaped && (sys::signal(i.pid, sig, true) | sys::signal(i.pid, sig, false))))
    }

    /// A kernel whose control link closed (`Ev::Lost`): SIGKILL, except that on Linux the leader is the outer bwrap,
    /// which outlives the kernel by a moment while it collects the kernel's exit status: give it up to 2 s to exit by
    /// itself, so the Root records the kernel's own exit code (78: config, 75: swap) and not a kill.
    pub fn kill_lost(&self, id: &str) {
        if cfg!(target_os = "linux") && let Some(pid) = self.pid_of(id) {
            let t = std::time::Instant::now();
            while !sys::has_exited(pid) && t.elapsed() < std::time::Duration::from_secs(2) {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        self.kill(id, libc::SIGKILL);
    }

    /// SIGKILL to every held instance (panic step 4, a kernel crash): (instances, processes signalled).
    pub fn kill_all(&self) -> (usize, usize) {
        let t = lock(&self.insts);
        let live: Vec<i32> = t.values().filter(|i| !i.reaped).map(|i| i.pid).collect();
        let n = live.iter().filter(|&&p| sys::signal(p, libc::SIGKILL, true) | sys::signal(p, libc::SIGKILL, false)).count();
        (live.len(), n)
    }
}

/// `INIT_HOME/run/mark/<id>`: empty, 0400, `O_CREAT|O_EXCL|O_NOFOLLOW` (§3.5.1).
fn mark(root: &Root, id: &str) -> Result<PathBuf, String> {
    let m = root.p.i("run/mark").join(id);
    OpenOptions::new().write(true).create_new(true).mode(0o400).custom_flags(libc::O_NOFOLLOW).open(&m).map_err(|e| format!("mark: {e}"))?;
    Ok(m)
}

/// `bin/init-root exec-clean [--keep 3 --keep 4] -- <sandbox-exec argv> -- <program> <args>`, own process group.
#[allow(clippy::too_many_arguments)]
fn launch(root: &Root, id: &str, profile: ProfileId, given: &BTreeMap<String, String>, grant: &Grant, prog: &str, args: &[String],
    setup: impl FnOnce(&mut Command)) -> Result<i32, String> {
    let m = mark(root, id)?;
    let res = launcher(root, profile, given, &m, grant).and_then(|mut argv| {
        let mut c = Command::new(root.p.r("bin/init-root"));
        c.arg("exec-clean");
        if profile == ProfileId::Kernel {
            c.args(["--keep", "3", "--keep", "4"]);
        }
        #[cfg(target_os = "linux")]
        let seccomp = sys::linux_seccomp(grant.full).map_err(|e| e.to_string())?;
        #[cfg(target_os = "linux")]
        c.args(["--keep", "5"]);
        #[cfg(target_os = "linux")]
        let mounts = sys::linux_bind_fds(&mut argv).map_err(|e| e.to_string())?;
        #[cfg(target_os = "linux")]
        for fd in &mounts { c.arg("--keep").arg(fd.as_raw_fd().to_string()); }
        c.arg("--").args(argv).arg("--").arg(prog).args(args).process_group(0);
        setup(&mut c);
        #[cfg(target_os = "linux")]
        sys::map_seccomp(&mut c, seccomp.as_raw_fd());
        #[cfg(target_os = "linux")]
        sys::inherit_fds(&mut c, mounts.iter().map(AsRawFd::as_raw_fd).collect());
        // LINUX: a workload (every instance but the kernel) starts inside its own memory.max, where delegated
        #[cfg(target_os = "linux")]
        let cg = (profile != ProfileId::Kernel).then(|| lock(&root.st).charter.as_ref().map(|l| crate::memwatch::Limits::of(&l.ch).cap))
            .flatten().and_then(|cap| crate::cgroup::workload(id, cap));
        #[cfg(target_os = "linux")]
        if let Some(fd) = &cg { sys::join_cgroup_on_exec(&mut c, fd.as_raw_fd()); }
        c.spawn().map(|ch| ch.id() as i32).map_err(|e| format!("spawn: {e}")) // reaped by wait4, never by `Child`
    });
    if res.is_err() {
        let _ = fs::remove_file(&m); // exact path, created above
    }
    res
}

/// The kernel instance (§3.5.1 end, §4.4): profile `kernel`, control link on fds 3 and 4, logs to `logs/kernel-<seq>.stderr`.
/// `args`: `supervise --under-root`, or `install --from-bundle <path> [--seed-tag <tag>]` at install (§4.3 step 8).
pub fn spawn_kernel(root: &Arc<Root>, seq: u64, args: &[String]) -> Result<KernelProc, String> {
    let e = |x: std::io::Error| x.to_string();
    crate::install::prepare_kernel_scratch(&root.p).map_err(e)?;
    let (ctl, ctl_k) = UnixStream::pair().map_err(e)?;
    let (fdc, fdc_k) = UnixDatagram::pair().map_err(e)?;
    let (a, b) = (sys::dup_high(&ctl_k).map_err(e)?, sys::dup_high(&fdc_k).map_err(e)?);
    let log = crate::sup::klog::kernel_log(&root.p.rh, seq).map_err(e)?; // capped at 4 MiB, 10 kept (WP21 D12)
    let ih = root.p.ih.display().to_string();
    let tool = info_path(root, "toolchain").map(|t| format!("{}/bin:", t.display())).unwrap_or_default();
    let dev = if cfg!(target_os = "macos") { info_path(root, "devdir").map(|d| format!("{}/usr/bin:", d.display())).unwrap_or_default() } else { String::new() };
    let mut env: BTreeMap<String, String> = [("INIT_HOME", ih.clone()), ("INIT_ROOT_FD", "3".into()), ("INIT_ROOT_FDPASS", "4".into()),
        ("PATH", format!("{ih}/kernel/bin:{tool}{dev}/usr/bin:/bin:/usr/sbin:/sbin")), ("HOME", format!("{ih}/cache/kernel-home")),
        ("TMPDIR", format!("{ih}/cache/tmp/")), ("LANG", "en_US.UTF-8".into())].into_iter().map(|(k, v)| (k.into(), v)).collect();
    kernel_platform_env(&root.info, &mut env);
    if lock(&root.st).charter.as_ref().is_some_and(|l| l.ch.root.test_mode) {
        env.extend(std::env::vars().filter(|(k, _)| k == "INIT_TEST" || k.starts_with("INIT_TEST_") || k == "INIT_FAULT"));
    }
    let id = util::hex(&util::random::<6>().map_err(e)?);
    let prog = root.p.i("kernel/bin/init-kernel").display().to_string();
    let pid = launch(root, &id, ProfileId::Kernel, &BTreeMap::new(), &Grant::default(), &prog, args, |c| {
        c.env_clear().envs(&env).current_dir(&root.p.ih).stdin(Stdio::null());
        if let Ok(l2) = log.try_clone() {
            c.stdout(l2);
        }
        c.stderr(log);
        sys::map_fds(c, [a.as_raw_fd(), b.as_raw_fd()]);
    })?;
    drop((a, b, ctl_k, fdc_k)); // the parent's copies of the kernel's ends
    root.zone.add(&id, "kernel", None, pid);
    save(root);
    reaper(root.clone(), id.clone(), pid);
    Ok(KernelProc { inst: id, pid, ctl, fdc })
}

fn kernel_platform_env(info: &Value, env: &mut BTreeMap<String, String>) {
    #[cfg(target_os = "macos")]
    for (key, field) in [("SDKROOT", "sdkroot"), ("DEVELOPER_DIR", "devdir")] {
        if let Some(value) = info[field].as_str() { env.insert(key.into(), value.into()); }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (info, env);
}

#[cfg(test)]
mod platform_env_tests {
    use super::*;
    #[test]
    fn kernel_platform_env_only_adds_apple_sdk_variables_on_macos() {
        let info = json!({"devdir": "/developer", "sdkroot": "/sdk"});
        let mut env = BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]);
        kernel_platform_env(&info, &mut env);
        assert_eq!(env["PATH"], "/usr/bin:/bin");
        #[cfg(target_os = "macos")]
        { assert_eq!(env["DEVELOPER_DIR"], "/developer"); assert_eq!(env["SDKROOT"], "/sdk"); }
        #[cfg(not(target_os = "macos"))]
        { assert!(!env.contains_key("DEVELOPER_DIR")); assert!(!env.contains_key("SDKROOT")); }
    }
}

/// `spawn` from the kernel (§3.5.1): validate (V1–V5, class rules, state, quotas), mark, spawn, record,
/// then the caller hands `[stdin_w, stdout_r, stderr_r]` over and answers `{instance, pid, started_at}`.
pub fn spawn(root: &Arc<Root>, a: &Value) -> Result<(Value, [OwnedFd; 3]), Err3> {
    let args = |m: String| ("root.args", m, Value::Null);
    let r: SpawnReq = serde_json::from_value(a.clone()).map_err(|e| args(format!("spawn: {e}")))?;
    let mode = match &lock(&root.st).slot {
        crate::sup::Slot::Starting { .. } | crate::sup::Slot::Running => None,
        s => Some(s.name()),
    };
    if let Some(m) = mode {
        return Err(("root.mode", format!("no spawns while {m}"), json!({"mode": m})));
    }
    // MEM-RUNAWAY: below the workload disk floor no workload starts (a body may: its tool processes are policed by
    // the memory watchdog, which also kills them while the disk stays low; root/root.md §3.14.1)
    let lim = lock(&root.st).charter.as_ref().map(|l| crate::memwatch::Limits::of(&l.ch));
    if r.kind != "body"
        && let Some(free) = lim.and_then(|l| l.disk_low(&root.p.ih).map(|f| (l, f)))
    {
        let (l, f) = free;
        return Err(("root.quota", l.disk_msg(f), json!({"free_mib": f >> 20, "floor_mib": l.floor >> 20, "quota": "disk_floor"})));
    }
    let profile = ProfileId::from_wire(&r.profile).ok_or_else(|| args(format!("unknown profile {:?}", r.profile)))?;
    let c = vctx(root);
    params::validate_spawn_params(&r.kind, profile, &r.params, &c).map_err(|e| args(e.to_string()))?;
    params::check_spawn_misc(&r.program, &r.cwd, &r.env, &r.args, &c).map_err(|e| args(e.to_string()))?;
    if let Err(q) = quotas(root, &r.kind, r.replaces.as_deref()) {
        if q.0 == "root.level" {
            root.append("ceiling_hit", json!({"attempted": q.2["used"].as_i64().unwrap_or(0) + 1, "ceiling": "parallel_bodies",
                "code": "root.level", "demotes": true, "limit": q.2["limit"], "rule": "b", "source": "broker"}));
        }
        return Err(q);
    }
    let e = |x: std::io::Error| ("root.internal", x.to_string(), Value::Null);
    let ((in_r, in_w), (out_r, out_w), (err_r, err_w)) = (std::io::pipe().map_err(e)?, std::io::pipe().map_err(e)?, std::io::pipe().map_err(e)?);
    let id = util::hex(&util::random::<6>().map_err(e)?);
    // ADR-009 (SBX-009): a body's grant follows its kind and lane; the Root sets INIT_ACCESS and INIT_WORKSPACE
    // itself, overwriting the kernel's, so they always match the instance's sandbox.
    let mut env = r.env.clone();
    let live = r.params.get("INIT_HOME").is_some_and(|h| Path::new(h) == root.p.ih);
    // ADR-011 §2.3: on the free path the live body runs an admitted binary (lkg, current, an open group's), else
    // `root.unwatched_change`, a demoting rule-(b) refusal: an agent kernel cannot run an unannounced change.
    let free = lock(&root.st).charter.as_ref().is_some_and(|l| l.ch.extend.free());
    if admission_required(free, live, profile)
        && let Err(reason) = crate::tuple::unwatched_body(root, Path::new(&r.program))
    {
        root.append("ceiling_hit", json!({"reason": reason, "ceiling": "unwatched_change", "code": "root.unwatched_change", "demotes": true,
            "rule": "b", "source": "broker"}));
        root.alert("unwatched_change", &format!("[root] refused live body: {reason} (ADR-011 §2.3)"));
        return Err(("root.unwatched_change", "the live body could not be matched to an admitted tuple; change.open admits a change first".into(),
            json!({"reason": reason})));
    }
    let g = (profile == ProfileId::Body).then(|| crate::access::spawn_grant(root, &id, &r.kind, live));
    if let Some(g) = &g {
        env.insert("INIT_ACCESS".into(), g.access.into());
        env.insert("INIT_WORKSPACE".into(), g.workspace.as_ref().map(|w| w.display().to_string()).unwrap_or_default());
        if g.grant.full {
            full_env(&mut env, &crate::access::home(&root.p));
        }
    }
    // MEM-RUNAWAY: a body's side of the watchdog (the cap for its messages, the events file it reads), set by the Root
    if let Some(l) = lim.filter(|_| profile == ProfileId::Body) {
        env.extend(crate::memwatch::body_env(&l, &crate::memwatch::zone_events(&root.p.ih)).map(|(k, v)| (k.to_string(), v)));
    }
    let grant = g.as_ref().map(|g| g.grant.clone()).unwrap_or_default();
    let pid = launch(root, &id, profile, &r.params, &grant, &r.program, &r.args, |cmd| {
        cmd.env_clear().envs(&env).current_dir(&r.cwd).stdin(in_r).stdout(out_w).stderr(err_w);
    }).map_err(|m| {
        crate::access::unspawned(root, &id); // a member's workspace-write slot, if it took one
        ("root.internal", m, Value::Null)
    })?;
    if let Some(g) = g.filter(|_| live && r.kind == "body") {
        crate::access::spawned(root, &id, &g);
    }
    let at = root.zone.add(&id, &r.kind, r.lane, pid);
    save(root); // §3.5.1 step 5: recorded before the answer
    reaper(root.clone(), id.clone(), pid);
    Ok((json!({"instance": id, "pid": pid, "started_at": rfc3339(at)}), [in_w.into(), out_r.into(), err_r.into()]))
}

/// ADR-012: a full grant's environment is the owner's: `HOME` is the owner's canonical home (so `~/Downloads` is the
/// owner's), `CARGO_NET_OFFLINE` is gone (the network is open), and the owner's `~/.local/bin` and `~/.cargo/bin` follow
/// the kernel's `PATH`.
fn full_env(env: &mut BTreeMap<String, String>, home: &Path) {
    let h = home.display();
    env.insert("HOME".into(), h.to_string());
    env.remove("CARGO_NET_OFFLINE");
    let path = env.get("PATH").cloned().unwrap_or_else(|| "/usr/bin:/bin:/usr/sbin:/sbin".into());
    env.insert("PATH".into(), format!("{path}:{h}/.local/bin:{h}/.cargo/bin"));
}

#[cfg(test)]
mod full_env_tests {
    use super::*;
    #[test]
    fn full_env_is_the_owners() {
        let mut env: BTreeMap<String, String> = [("HOME", "/h/.init/work/home"), ("CARGO_NET_OFFLINE", "true"), ("PATH", "/h/.init/kernel/bin:/usr/bin:/bin"),
            ("TMPDIR", "/h/.init/work/tmp")].into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        full_env(&mut env, Path::new("/h"));
        assert_eq!(env["HOME"], "/h");
        assert!(!env.contains_key("CARGO_NET_OFFLINE"));
        assert_eq!(env["PATH"], "/h/.init/kernel/bin:/usr/bin:/bin:/h/.local/bin:/h/.cargo/bin");
        assert_eq!(env["TMPDIR"], "/h/.init/work/tmp", "TMPDIR stays the body's own");
        let mut bare = BTreeMap::new();
        full_env(&mut bare, Path::new("/h"));
        assert_eq!(bare["PATH"], "/usr/bin:/bin:/usr/sbin:/sbin:/h/.local/bin:/h/.cargo/bin");
    }
}

/// §3.14 and §3.6.4: the zone's share of `max_instances`, parallel bodies per level (with the `replaces`
/// allowance), and system safe mode (one body; no build, check or probe).
fn quotas(root: &Root, kind: &str, replaces: Option<&str>) -> Result<(), Err3> {
    let (level, safe, max, par, sealed, free) = {
        let st = lock(&root.st);
        let (max, sealed) = st.charter.as_ref().map_or((12, 2), |l| (l.ch.quotas.max_instances, l.ch.sealed.parallel));
        (st.level.level, st.mode == "safe", max, par_limit(&st), sealed, st.charter.as_ref().is_some_and(|l| l.ch.extend.free()))
    };
    if safe && matches!(kind, "build" | "check" | "probe") {
        return Err(("root.mode", format!("system safe mode refuses {kind}"), json!({"mode": "safe"})));
    }
    let t = lock(&root.zone.insts);
    let live = |k: Option<&str>| t.values().filter(|i| !i.reaped && k.is_none_or(|k| i.kind == k)).count() as i64;
    let (used, limit) = (live(None), max - sealed);
    if used >= limit {
        return Err(("root.quota", "the zone's instance limit is reached".into(), json!({"limit": limit, "quota": "instances", "used": used})));
    }
    if kind == "body" {
        let limit = par;
        let extra = i64::from(replaces.is_some_and(|r| t.get(r).is_some_and(|i| !i.reaped && i.kind == "body")));
        let used = live(Some("body"));
        if used >= limit + extra {
            let d = json!({"level": level, "limit": limit, "need": 2, "quota": "parallel_bodies", "used": used});
            return Err(if level < 2 && !safe && !free { ("root.level", format!("L{level} allows one body"), d) }
                else { ("root.quota", "the parallel-body limit is reached".into(), d) });
        }
    }
    Ok(())
}

/// One thread per instance (§3.5.4): wait for the leader's exit without reaping it, kill what is left in
/// its own process group (the zombie keeps the pgid ours), reap it with `wait4`, delete the mark, account
/// the CPU, and report the exit.
fn reaper(root: Arc<Root>, id: String, pid: i32) {
    std::thread::spawn(move || {
        let _ = sys::wait_exit(pid);
        let kind = {
            let mut t = lock(&root.zone.insts);
            sys::signal(pid, libc::SIGKILL, true);
            t.get_mut(&id).map(|i| {
                i.reaped = true;
                i.kind.clone()
            })
        };
        let (code, sig, cpu) = sys::reap(pid).unwrap_or((None, None, 0));
        let m = root.p.i("run/mark").join(&id);
        scan(&root, &m);
        let _ = fs::remove_file(&m); // exact path
        #[cfg(target_os = "linux")]
        crate::cgroup::release(&id);
        lock(&root.zone.insts).remove(&id);
        root.zone.cpu_ms.fetch_add(cpu, Ordering::Relaxed);
        save(&root);
        crate::sup::exited(&root, &id, kind.as_deref().unwrap_or(""), code, sig, cpu);
        if root.zone.count(None) == 0 {
            let after = lock(&root.st).slot.name();
            escapees(&root, if after == "panicked" { "panic" } else if after == "stopped" { "stop" } else { after });
        }
    });
}

/// Review F5, ADR-008: once the zone holds nothing (after a panic, a stop, a crash) and at cold start, the
/// held-only build looks for what its kills could not reach: processes that still meet D1 for the zone mark,
/// such as a `setsid` escapee. Each one found on two passes 300 ms apart (so a group member still dying
/// from the Root's own `killpg` is not reported) becomes an `escapee` record and an owner alert with its
/// pids. Nothing is ever signalled here: ending it is the owner's call. `live-sweep` builds sweep instead.
#[cfg(all(target_os = "macos", not(feature = "live-sweep")))]
fn escapees(root: &Root, after: &str) -> usize {
    use crate::sweep::{Marks, ProcTable, member};
    let (t, me) = (sys::LiveTable, std::process::id() as i32);
    if t.sandboxed(me).is_none() {
        return 0; // no sandbox_check (RF13): nothing can be told apart
    }
    let (z, d, c) = (root.p.i("run/zone-mark"), root.p.i("run/mark/decoy"), root.p.r("run/canary"));
    let m = Marks { mark: &z, decoy: &d, canary: &c };
    let pass = || {
        let held: Vec<i32> = lock(&root.zone.insts).values().filter(|i| !i.reaped).map(|i| i.pid).collect();
        t.pids().into_iter().filter(|p| *p != me && !held.contains(p) && member(&t, *p, &m)).collect::<Vec<i32>>()
    };
    let first = pass();
    if first.is_empty() {
        return 0;
    }
    std::thread::sleep(std::time::Duration::from_millis(300));
    let still: Vec<i32> = pass().into_iter().filter(|p| first.contains(p)).collect();
    if !still.is_empty() {
        root.append("escapee", json!({"after": after, "mark": "zone-mark", "pids": still, "signalled": false}));
        root.alert("escapee", &format!("[root] after {after}: {} zone process(es) outside the Root's hold still match the sweep rule \
            (D1): pids {still:?}. This build never signals them (ADR-008); inspect with `ps -p <pid>` and end them yourself if needed.",
            still.len()));
    }
    still.len()
}

#[cfg(not(all(target_os = "macos", not(feature = "live-sweep"))))]
fn escapees(_: &Root, _: &str) -> usize {
    0 // live-sweep builds sweep in `scan`; the Linux zone (WP6b) confirms by pid namespace
}

/// The D1 sweep of one mark over the live process table: only with the `live-sweep` feature (§3.5.4), and
/// even then only where the disposable-environment sentinel exists (ADR-008). Without the feature this
/// returns 0 and sends nothing: the default and release builds signal only what the Root holds.
#[allow(unused_variables)]
fn scan(root: &Root, mark: &Path) -> usize {
    #[cfg(all(feature = "live-sweep", target_os = "macos"))]
    {
        if let Err(e) = crate::sweep::disposable() {
            util::log(&root.p.rh, "warn", &format!("live-sweep build outside a disposable environment: no scan ({e})"));
            return 0;
        }
        let (d, c) = (root.p.i("run/mark/decoy"), root.p.r("run/canary"));
        let m = crate::sweep::Marks { mark, decoy: &d, canary: &c };
        return crate::sweep::sweep(&sys::LiveTable, &m, &[std::process::id() as i32], &mut |p| sys::signal_member(p)).len();
    }
    #[allow(unreachable_code)]
    0
}

/// §3.6.5 step 5: instances a previous Root recorded. They are never signalled by pid here (the pid may
/// have been reused); their marks are deleted, the D1 scans run only with `live-sweep`, and the kernel and
/// bodies exit on their own when the control link and their stdin close (kernel/supervise.md §4.5).
pub fn cold_start(root: &Root) {
    let prev: Value = fs::read_to_string(root.p.r("state/root.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    for i in prev["instances"].as_array().into_iter().flatten() {
        let Some(id) = i["id"].as_str().filter(|s| s.len() == 12 && s.bytes().all(|c| c.is_ascii_hexdigit())) else { continue };
        let m = root.p.i("run/mark").join(id);
        let n = scan(root, &m);
        let _ = fs::remove_file(&m); // exact path
        util::log(&root.p.rh, "warn", &format!("orphan instance {id} (pid {}) of a previous Root: mark removed, {n} swept, never signalled by pid", i["pid"]));
    }
    scan(root, &root.p.i("run/zone-mark"));
    escapees(root, "cold_start"); // held-only builds: report what is left, signal nothing (ADR-008)
    save(root);
}

/// `state/root.json` (§4.5), fsynced: written before every `spawn` answer and after every exit.
pub fn save(root: &Root) {
    let insts: Vec<Value> = lock(&root.zone.insts).iter().filter(|(_, i)| !i.reaped).map(|(id, i)| json!({"id": id, "kind": i.kind,
        "lane": i.lane, "mark": format!("run/mark/{id}"), "pgid": i.pid, "pid": i.pid, "pidns_ino": null, "started_at": rfc3339(i.at)})).collect();
    let (slot, mode, counters) = { let st = lock(&root.st); (st.slot.name(), st.mode, st.level.counters.json()) }; // WP21 D10
    let v = json!({"counters": counters, "cpu_ms": root.zone.cpu_ms.load(Ordering::Relaxed), "crash": root.sup.crash_json(&root.p.rh),
        "instances": insts, "kernel": {"boot_seq": root.sup.boot_seq()},
        "mode": mode, "slot": slot, "swap": null, "tuples": lock(&root.sup.ptrs).json(), "v": 1});
    if let Ok(s) = util::icj(&v) {
        let _ = write_atomic(&root.p.r("state/root.json"), format!("{s}\n").as_bytes(), 0o600);
    }
}

/// Bodies allowed at once (§3.14, §3.6.4): one at L0–L1 and in system safe mode, else `quotas.parallel_bodies`. On
/// the free path (ADR-011 §4.5, X7) members up to `quotas.parallel_bodies` (with any owner grant) at every level.
pub fn par_limit(st: &crate::link::St) -> i64 {
    let par = st.charter.as_ref().map_or(4, |l| l.ch.quotas.parallel_bodies);
    let free = st.charter.as_ref().is_some_and(|l| l.ch.extend.free());
    if st.mode == "safe" || (st.level.level < 2 && !free) { 1 } else { par }
}

/// `instances` and `parallel_bodies` for `ceilings.json` and `ceilings.get` (§4.5).
pub fn counts(root: &Root) -> (Value, Value) {
    let (max, par) = { let st = lock(&root.st); (st.charter.as_ref().map_or(12, |l| l.ch.quotas.max_instances), par_limit(&st)) };
    (json!({"active": root.zone.count(None), "limit": max}), json!({"active": root.zone.count(Some("body")), "limit": par}))
}

/// The validated sandbox home, not the kernel's arbitrary lane label (production uses `b0`),
/// determines whether a body is live. The profile also catches a body mislabeled as a probe.
/// Isolated probe homes and build/check/git-read processes keep their own spawn policy.
fn admission_required(free: bool, live: bool, profile: ProfileId) -> bool {
    free && live && profile == ProfileId::Body
}

#[cfg(test)]
mod admission_tests {
    use super::*;

    #[test]
    fn live_body_admission_is_independent_of_lane_labels() {
        for lane in [None, Some("live"), Some("b0"), Some("wrapper")] {
            for kind in ["body", "probe"] {
                let request: SpawnReq = serde_json::from_value(json!({
                    "kind": kind, "profile": "body", "params": {}, "program": "/bin/sh",
                    "cwd": "/test/work", "lane": lane
                })).unwrap();
                assert!(admission_required(true, true, ProfileId::from_wire(&request.profile).unwrap()));
            }
        }
        for profile in [ProfileId::Build, ProfileId::Check, ProfileId::GitRead] {
            assert!(!admission_required(true, true, profile));
        }
        assert!(!admission_required(true, false, ProfileId::Body));
        assert!(!admission_required(false, true, ProfileId::Body));
    }
}
