//! §3.2 sealed runs (decision D5) and §3.4 the reference evaluator. A run root `ROOT_HOME/sealed/run/<id>/`
//! is listed in `sealed/runs.json` before anything spawns; its processes run under the sealed Seatbelt
//! profile (PRELUDE + the run root + its lane socket; INIT_HOME and the rest of ROOT_HOME denied), reach the
//! model only through a sealed egress lane, and the root is deleted by its exact path afterwards. The Root
//! reads only exit statuses, the check's last stdout line and the gate's `--out` record.

use super::task::{self, Task};
use crate::charter::Charter;
use crate::link::{Root, lock};
use crate::memwatch::{self, Leader, Limits, Verdict, Watch};
use crate::util::{self, icj, write_atomic};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::time::{Duration, Instant};

/// The sealed instances this process holds (run id → pid) and the cancel flag (`cancel_all`).
static HELD: Mutex<BTreeMap<String, i32>> = Mutex::new(BTreeMap::new());
static CANCEL: AtomicBool = AtomicBool::new(false);
/// One sealed operation (audit, gate-of-gates, draw, slice, calibration, milestone) at a time in this process.
static OP: Mutex<()> = Mutex::new(());

/// The sealed text (root/root.md §3.3.6), compiled into `init-sandbox-profiles` with the zone texts so
/// its `doc_drift` test checks it against the doc byte for byte.
#[cfg(target_os = "macos")]
const TAIL: &str = init_sandbox_profiles::seatbelt::SEALED_TAIL;

/// What a sealed run needs from the Root.
pub struct Env {
    pub rh: PathBuf,
    pub ih: PathBuf,
    pub home: PathBuf,
    pub info: Value,
    pub ch: Charter,
    pub level: i64,
    /// The LKG tuple's body commit: where every sealed body run takes its T0 from (REAL-M7; see [`body_home`]).
    pub t0_sha: Option<String>,
}

/// The body commit of tuple `id` (`body.sha`).
fn body_sha(rh: &Path, id: Option<&str>) -> Option<String> {
    id.and_then(|i| crate::tuple::load(rh, i)).and_then(|t| t["body"]["sha"].as_str().map(str::to_string)).filter(|s| !s.is_empty())
}

impl Env {
    pub fn of(root: &Root) -> Result<Env, String> {
        let st = lock(&root.st);
        let ch = st.charter.as_ref().ok_or("no active charter")?.ch.clone();
        let t0_sha = body_sha(&root.p.rh, lock(&root.sup.ptrs).lkg.as_deref());
        Ok(Env { rh: root.p.rh.clone(), ih: root.p.ih.clone(), home: root.p.home.clone(), info: root.info.clone(), ch, level: st.level.level, t0_sha })
    }
    /// Without a running Root (`sealed import`): the active charter, install.json and the folded level.
    pub fn load(p: &crate::install::Paths) -> Result<Env, String> {
        let ch = crate::charter::load(&p.r("state/charter.active.toml"), &p.ctx()).map_err(|_| "no valid active charter (install first)")?.ch;
        let info = fs::read_to_string(p.r("install.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let recs = crate::lineage::records(&p.rh).unwrap_or_default();
        let level = crate::ledger::Level::fold(&recs).level;
        let mut ptrs = crate::tuple::Ptrs::default();
        recs.iter().for_each(|r| ptrs.apply(r));
        let t0_sha = body_sha(&p.rh, ptrs.lkg.as_deref());
        Ok(Env { rh: p.rh.clone(), ih: p.ih.clone(), home: p.home.clone(), info, ch, level, t0_sha })
    }
    fn path(&self, k: &str) -> PathBuf {
        self.info[k].as_str().map_or_else(|| self.ih.join("kernel"), PathBuf::from)
    }
}

/// A run root, deleted (by exact path) and unlisted on drop.
pub struct RunRoot {
    pub id: String,
    pub dir: PathBuf,
    rh: PathBuf,
    /// MEM-RUNAWAY: the run's events file (`INIT_MEM_EVENTS`), created before anything spawns and written only
    /// through this handle, so a sandboxed run that replaces the path never redirects a Root write.
    events: Mutex<Option<fs::File>>,
}

fn runs(rh: &Path, f: impl FnOnce(&mut Vec<String>)) {
    static L: Mutex<()> = Mutex::new(());
    let _g = lock(&L);
    let p = rh.join("sealed/runs.json");
    let mut v: Vec<String> = fs::read_to_string(&p).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| serde_json::from_value(v["runs"].clone()).ok()).unwrap_or_default();
    f(&mut v);
    let _ = write_atomic(&p, format!("{}\n", icj(&json!({"runs": v, "v": 1})).unwrap_or_default()).as_bytes(), 0o600);
}

impl RunRoot {
    pub fn new(rh: &Path) -> Result<RunRoot, String> {
        let id = util::hex(&util::random::<6>().map_err(|e| e.to_string())?);
        let dir = rh.join("sealed/run").join(&id);
        runs(rh, |v| v.push(id.clone()));
        for d in ["", "bin", "work", "home", "tmp", "out"] {
            fs::DirBuilder::new().recursive(true).mode(0o700).create(dir.join(d)).map_err(|e| e.to_string())?;
        }
        use std::os::unix::fs::OpenOptionsExt;
        let events = fs::OpenOptions::new().create_new(true).append(true).mode(0o644).custom_flags(libc::O_NOFOLLOW)
            .open(dir.join(EVENTS)).map_err(|e| e.to_string())?;
        Ok(RunRoot { id, dir, rh: rh.to_path_buf(), events: Mutex::new(Some(events)) })
    }

    /// The run's events file, for the body's `INIT_MEM_EVENTS`.
    pub fn events_path(&self) -> PathBuf {
        self.dir.join(EVENTS)
    }

    fn event(&self, v: &Value) {
        if let Some(f) = lock(&self.events).as_mut() {
            memwatch::append(f, v);
        }
    }
}

/// A run root's events file (MEM-RUNAWAY).
const EVENTS: &str = "mem-events.jsonl";

impl Drop for RunRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir); // exact path, created by new()
        runs(&self.rh, |v| v.retain(|x| *x != self.id));
    }
}

/// §4.2 `cancel_all`: kill the sealed instances this process holds (by pid, their own process groups), and
/// delete the run roots `sealed/runs.json` lists (by exact path); a previous Root's instances are never
/// signalled (their pids may be reused), they lose their lane with that Root.
pub fn cancel_all(rh: &Path) {
    CANCEL.store(true, SeqCst);
    let held: Vec<i32> = lock(&HELD).values().copied().collect();
    for pid in held {
        crate::sys::signal(pid, libc::SIGKILL, true);
    }
    clean_stale(rh);
}

/// At Root start (WP24 D8): the run roots a previous Root's sealed runs left listed in `sealed/runs.json` are
/// deleted by exact path. Nothing is signalled: this process holds no sealed instance yet, and a previous
/// Root's pids may have been reused (they lose their lane with that Root).
pub fn clean_stale(rh: &Path) {
    let mut stale = Vec::new();
    runs(rh, |v| stale = std::mem::take(v));
    for id in stale.iter().filter(|i| i.len() == 12 && i.bytes().all(|c| c.is_ascii_hexdigit())) {
        let _ = fs::remove_dir_all(rh.join("sealed/run").join(id));
    }
}

/// The start of an audit or a gate-of-gates: one sealed operation at a time in this process (the Root lock
/// makes it one per ROOT_HOME); the run roots a crashed run left in `sealed/runs.json` are deleted first.
pub fn begin(rh: &Path) -> std::sync::MutexGuard<'static, ()> {
    let g = lock(&OP);
    cancel_all(rh);
    CANCEL.store(false, SeqCst);
    g
}

/// `begin` without waiting: None while another sealed operation runs (the G4 slice answers `root.busy`).
pub fn try_begin(rh: &Path) -> Option<std::sync::MutexGuard<'static, ()>> {
    let g = match OP.try_lock() {
        Ok(g) => g,
        Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return None,
    };
    cancel_all(rh);
    CANCEL.store(false, SeqCst);
    Some(g)
}

/// The lane socket: `ROOT_HOME/run/sl-<run>.sock`, because `<run root>/run/proxy.sock` overflows `sun_path`
/// (104 bytes on macOS) for a 64-byte ROOT_HOME.
pub fn lane_sock(rh: &Path, run: &RunRoot) -> PathBuf {
    rh.join("run").join(format!("sl-{}.sock", run.id))
}

pub fn cancelled() -> bool {
    CANCEL.load(SeqCst)
}

/// `argv` under the sealed profile with the run root and (optionally) its lane, in a fresh environment and
/// its own process group.
pub fn sandboxed(env: &Env, run: &RunRoot, lane: Option<&Path>, argv: &[OsString]) -> Command {
    let lane_path = lane.map_or_else(|| run.dir.join("run/none"), Path::to_path_buf);
    #[cfg(target_os = "macos")]
    let mut c = {
        let p = |k: &str, v: &Path| OsString::from(format!("-D{k}={}", v.display()));
        let mut c = Command::new("/usr/bin/sandbox-exec");
        c.arg("-p").arg([init_sandbox_profiles::seatbelt::PRELUDE, TAIL].concat());
        c.args([p("HOME_REAL", &env.home), p("DEVDIR", &env.path("devdir")), p("TOOLCHAIN", &env.path("toolchain")), p("S_RUN", &run.dir),
            p("S_LANE", &lane_path), p("S_ROOT_HOME", &env.rh), p("S_INIT_HOME", &env.ih)]).arg("--").args(argv);
        c
    };
    #[cfg(target_os = "linux")]
    let mut c = {
        let built = (|| -> Result<Command, String> {
            let toolchain = env.info["toolchain"].as_str().ok_or("sealed Linux run requires installed toolchain")?;
            let mut launcher = init_sandbox_profiles::linux::sealed(&run.dir, &env.rh, Path::new(toolchain), lane)?;
            let mut mounts = crate::sys::linux_bind_fds(&mut launcher).map_err(|e| e.to_string())?;
            let filter = crate::sys::linux_seccomp().map_err(|e| e.to_string())?;
            use std::os::fd::AsRawFd;
            let mut c = Command::new(env.rh.join("bin/init-root"));
            // fds 3 and 4: the control link a probe kernel gets (sup/kprobe.rs maps them, as the zone's kernel launch
            // does); every other Root descriptor is close-on-exec, so nothing else can sit there
            c.args(["exec-clean", "--keep", "3", "--keep", "4", "--keep", "5"]);
            for fd in &mounts { c.arg("--keep").arg(fd.as_raw_fd().to_string()); }
            c.arg("--").args(launcher).arg("--").args(argv);
            crate::sys::map_seccomp(&mut c, filter.as_raw_fd());
            crate::sys::inherit_fds(&mut c, mounts.iter().map(AsRawFd::as_raw_fd).collect());
            mounts.push(filter);
            crate::sys::hold_spawn_fds(&mut c, mounts);
            Ok(c)
        })();
        match built {
            Ok(c) => c,
            Err(e) => {
                crate::util::log(&env.rh, "error", &format!("sealed Linux confinement refused: {e}"));
                let mut c = Command::new("/usr/bin/false");
                crate::sys::refuse_unsupported_spawn(&mut c);
                c
            }
        }
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let mut c = {
        // Unsupported confinement is a spawn error, never an unconfined fallback.
        let mut c = Command::new(argv.first().map_or(std::ffi::OsStr::new(""), |a| a.as_os_str()));
        crate::sys::refuse_unsupported_spawn(&mut c);
        c
    };
    // M7: the toolchain and the Command Line Tools first (Rust checks need cargo; `$DEVDIR/usr/bin/python3` skips the
    // xcrun shim, which cannot write its cache in the sandbox)
    let mut path = String::new();
    for (k, sub) in [("toolchain", "bin"), ("devdir", "usr/bin")] {
        if let Some(d) = env.info[k].as_str() {
            path.push_str(&format!("{d}/{sub}:"));
        }
    }
    c.env_clear().envs([("PATH", format!("{path}/usr/bin:/bin:/usr/sbin:/sbin").as_str()), ("LANG", "C"), ("INIT_MODE", "normal")]);
    for (k, var) in [("sdkroot", "SDKROOT"), ("devdir", "DEVELOPER_DIR")] {
        if let Some(v) = env.info[k].as_str().filter(|v| !v.is_empty()) {
            c.env(var, v); // the Rust checks link with the Command Line Tools' SDK, as the zone's builds do
        }
    }
    c.env("HOME", run.dir.join("home")).env("TMPDIR", run.dir.join("tmp")).env("INIT_PROXY_SOCK", &lane_path)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0);
    c
}

/// The body's boot contract in a sealed run (supervise.md §4.1, body/boot.md; REAL-M7). Before this the real body
/// exited 78 (`INIT_HOME: missing`) in every sealed run, so no probe, calibration, slice or milestone of a real body
/// ever made a model request (the fixture bodies of the Root's tests need no environment). `INIT_HOME` is the run
/// root, laid out like a kernel probe root: T0 (`GENESIS.md`, `SELF.md`, `self.json`, `skills/`) from the LKG
/// tuple's kernel-exported tree `INIT_HOME/lineage/trees/<sha12>/` (never the agent-writable `body/`), the same for
/// both arms (D-L2: the S path measures body binaries only); `kernel/CONSTITUTION.md` and `config.toml`;
/// `state/kernel/boot.json` = the live boot record with this run's identity. The kernel socket never exists:
/// a sealed run has no kernel (sealed-eval.md §3.2). Returns the environment to add to [`sandboxed`]'s.
pub fn body_home(ih: &Path, rh: &Path, t0_sha: Option<&str>, run: &RunRoot) -> Result<Vec<(&'static str, String)>, String> {
    let r = &run.dir;
    let io = |e: std::io::Error| format!("sealed body home: {e}");
    for d in ["body", "kernel", "state/kernel", "state/sessions", "state/run", "work/tmp", "work/home", "work/cargo-home", "bin/ext", "run"] {
        fs::create_dir_all(r.join(d)).map_err(io)?;
    }
    let sha = t0_sha.ok_or("no lkg tuple: a sealed body run has no T0 source")?;
    let tree = ih.join("lineage/trees").join(sha.get(..12).unwrap_or(sha));
    for f in ["GENESIS.md", "SELF.md", "self.json"] {
        if tree.join(f).is_file() {
            fs::copy(tree.join(f), r.join("body").join(f)).map_err(io)?;
        }
    }
    if tree.join("skills").is_dir() {
        let ok = Command::new("/bin/cp").arg("-R").arg(tree.join("skills")).arg(r.join("body/skills")).status().is_ok_and(|s| s.success());
        if !ok {
            return Err(format!("copy {}/skills failed", tree.display()));
        }
    }
    if !r.join("body/GENESIS.md").is_file() || !r.join("body/SELF.md").is_file() {
        return Err(format!("no T0 in {} (the kernel exports the lkg tree)", tree.display()));
    }
    for f in ["CONSTITUTION.md", "config.toml"] {
        if ih.join("kernel").join(f).is_file() {
            fs::copy(ih.join("kernel").join(f), r.join("kernel").join(f)).map_err(io)?;
        }
    }
    let rnd = |n: usize| crate::util::random::<8>().map(|b| crate::util::hex(&b)[..n].to_string()).map_err(io);
    let now = crate::util::now_ms();
    let t = crate::util::rfc3339(now);
    let session = format!("{}{}{}-{}{}{}-{}", &t[0..4], &t[5..7], &t[8..10], &t[11..13], &t[14..16], &t[17..19], rnd(6)?);
    let boot_id = rnd(16)?;
    let mut b: Value = fs::read(ih.join("state/kernel/boot.json")).ok().and_then(|x| serde_json::from_slice(&x).ok()).unwrap_or_else(|| json!({"v": 1}));
    for (k, v) in [("body_id", json!("sealed")), ("boot_id", json!(boot_id)), ("boot_seq", json!(0)), ("branch", Value::Null),
        ("cause", Value::Null), ("crash", Value::Null), ("from_sha", Value::Null), ("mode", json!("normal")), ("reason", json!("cold")),
        ("provisional", Value::Null), ("safe", Value::Null), ("session", json!(session)), ("sha", json!(sha)), ("spawned_at", json!(t)),
        ("update_failed", Value::Null)] {
        b[k] = v;
    }
    b["env"]["init_home"] = json!(r);
    let kv = b["env"]["kernel_version"].as_str().unwrap_or("0.1.0").to_string();
    crate::util::write_atomic(&r.join("state/kernel/boot.json"), format!("{b}\n").as_bytes(), 0o600).map_err(io)?;
    let s = |p: &Path| p.to_string_lossy().into_owned();
    Ok(vec![("INIT_HOME", s(r)), ("INIT_SESSION", session), ("INIT_SHA", sha.to_string()), ("INIT_BOOT_ID", boot_id),
        ("INIT_BOOT_REASON", "cold".into()), ("INIT_BODY_ID", "sealed".into()), ("INIT_MODE", "normal".into()),
        ("INIT_UI", "stdio-jsonl/1".into()), ("INIT_KERNEL_VERSION", kv),
        ("INIT_KERNEL_SOCK", s(&rh.join("run").join(format!("sk-{}.sock", run.id)))), ("HOME", s(&r.join("work/home"))),
        ("TMPDIR", format!("{}/", s(&r.join("work/tmp")))), ("CARGO_HOME", s(&r.join("work/cargo-home"))),
        ("CARGO_TARGET_DIR", s(&r.join("work/target"))), ("CARGO_NET_OFFLINE", "true".into()),
        ("GIT_AUTHOR_NAME", "INIT Agent".into()), ("GIT_COMMITTER_NAME", "INIT Agent".into()),
        ("GIT_AUTHOR_EMAIL", "agent@init.local".into()), ("GIT_COMMITTER_EMAIL", "agent@init.local".into())])
}

/// The task's owner line as a stdio-jsonl/1 `input` frame (supervise.md §4.5; the body's `parse_frame` needs `id`
/// and `seq`: without them it warns `input: missing id` and never starts the task; REAL-M7).
pub fn input_frame(task_id: &str, work: &Path, prompt: &str) -> Result<Value, String> {
    let id = util::hex(&util::random::<8>().map_err(|e| e.to_string())?);
    Ok(json!({"t": "input", "id": id, "seq": 1, "mode": "steer", "ts": util::rfc3339(util::now_ms()),
        "text": format!("[task {task_id}] Workspace: {}\n\n{prompt}", work.display())}))
}

/// Spawns, records the pid as held, waits until `stop` says so or the child exits, then kills its group.
/// Returns (exit code, why it ended). MEM-RUNAWAY: refused below the workload disk floor; while it runs, the memory
/// watchdog polls the run every second (a unit over `sandbox.mem_cap_mib` is killed, the leader over it ends the run
/// as `mem_cap`, the disk below the floor ends it as `disk_floor`), and at the end it kills what is left of the run's
/// units, which a body spawns as their own process groups, out of reach of the group kill.
pub fn supervise(mut c: Command, run: &RunRoot, lim: &Limits, wall: Duration, input: Option<String>,
    mut stop: impl FnMut() -> Option<&'static str>) -> Result<(Option<i32>, &'static str, Child), String> {
    if let Some(free) = lim.disk_low(&run.dir) {
        return Err(format!("sealed run refused: {}", lim.disk_msg(free)));
    }
    if input.is_some() {
        c.stdin(Stdio::piped());
    }
    // LINUX: the run starts inside its own memory.max, where a delegated cgroup v2 subtree exists (cgroup.rs)
    #[cfg(target_os = "linux")]
    let cg = crate::cgroup::workload(&format!("sealed-{}", run.id), lim.cap);
    #[cfg(target_os = "linux")]
    if let Some(fd) = &cg { crate::sys::join_cgroup_on_exec(&mut c, std::os::fd::AsRawFd::as_raw_fd(fd)); }
    let mut child = c.spawn().map_err(|e| format!("sealed spawn: {e}"))?;
    #[cfg(target_os = "linux")]
    drop(cg);
    let pid = child.id() as i32;
    lock(&HELD).insert(run.id.clone(), pid);
    let mut stdin = child.stdin.take();
    if let (Some(s), Some(i)) = (stdin.as_mut(), &input) {
        let _ = writeln!(s, "{i}");
    }
    // The leader is reaped only after its group is killed: until then its pid and process group stay ours, so the
    // killpg cannot reach a reused pid (`sys::signal` refuses any pid that is not an unreaped child, ADR-008). A
    // thread waits for the exit without reaping (`waitid` WNOWAIT); it ends at the exit or once the leader is reaped.
    let exited = std::sync::Arc::new(AtomicBool::new(false));
    let e2 = exited.clone();
    std::thread::spawn(move || {
        let _ = crate::sys::wait_exit(pid);
        e2.store(true, SeqCst);
    });
    let t0 = Instant::now();
    let leader = Leader { key: format!("sealed:{}", run.id), kind: "sealed".into(), pid };
    let (mut w, mut polled) = (Watch::default(), Instant::now());
    let why = loop {
        if exited.load(SeqCst) {
            break None;
        }
        let why = if cancelled() { Some("crash") } else if t0.elapsed() > wall { Some("timeout") } else { stop() };
        if why.is_some() {
            break why;
        }
        if polled.elapsed() >= memwatch::POLL {
            polled = Instant::now();
            if let Some(why) = watch_run(run, lim, &leader, &mut w) {
                break Some(why);
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    // the run's units, then the leader's group: a body's tool calls are process groups of their own
    let (seen, table) = memwatch::observed(std::slice::from_ref(&leader));
    for (_, u) in w.plan(&seen, &table, u64::MAX, &|_| None).units {
        memwatch::kill_members(&u.members);
    }
    crate::sys::signal(pid, libc::SIGKILL, true); // the leader if it still runs, and anything left in its group
    let code = child.wait().ok().and_then(|s| s.code());
    #[cfg(target_os = "linux")]
    crate::cgroup::release(&format!("sealed-{}", run.id));
    lock(&HELD).remove(&run.id);
    drop(stdin);
    Ok(match why {
        Some(w) => (None, w, child),
        None => (code, if code.is_some_and(|c| c == 0 || c == 75) { "done" } else { "crash" }, child),
    })
}

/// One watchdog poll of a sealed run: the disk floor ends it (`disk_floor`), a unit over the cap is killed, the leader
/// over the cap ends it (`mem_cap`). Each kill is in the run's events file before the signal, and alerts the owner.
fn watch_run(run: &RunRoot, lim: &Limits, leader: &Leader, w: &mut Watch) -> Option<&'static str> {
    let mut sink = |v: &Value| run.event(v);
    if let Some(free) = lim.disk_low(&run.dir) {
        sink(&memwatch::record("disk_floor", Some(leader), Some(leader.pid), "instance", 0, lim, Some(free), &[leader.pid]));
        memwatch::alert(&run.rh, "disk_floor", &format!("[root] sealed run {}: {}; the run is stopped", run.id, lim.disk_msg(free)));
        return Some("disk_floor");
    }
    let (seen, table) = memwatch::observed(std::slice::from_ref(leader));
    let plan = w.plan(&seen, &table, lim.cap, &crate::sys::footprint);
    let mut whole = None;
    for v in plan.verdicts {
        match v {
            Verdict::Unit { leader: l, unit } => {
                memwatch::kill_unit(&mut sink, &run.rh, "mem_cap", l.as_ref(), &unit, lim, None);
            }
            Verdict::Leader { leader: l, bytes, units } => {
                sink(&memwatch::record("mem_cap", Some(&l), Some(l.pid), "instance", bytes, lim, None, &[l.pid]));
                memwatch::alert(&run.rh, "mem_cap", &memwatch::mem_msg(Some(&l), "instance", l.pid, 1 + units.len(), bytes, lim));
                whole = Some("mem_cap");
            }
        }
    }
    whole
}

/// One reference-evaluator run (§3.2 steps 1–6, §3.4): `{task, repeat, score_permille, outcome, spent_musd}`, plus
/// M7's `requests`, `turns`, `input_tokens`, `cached_tokens`, `output_tokens`, `caps_hit`, `tamper`, `void` and
/// `wall_ms`. The task's v3 caps (turns, tokens) are set on its lane and enforced on actuals (m7-design.md §3.1.3).
pub fn run_task(env: &Env, body: &Path, t: &Task, repeat: u32, budget: u64) -> Result<Value, String> {
    let t0 = Instant::now();
    let run = RunRoot::new(&env.rh)?;
    let bin = run.dir.join("bin/init-body");
    fs::copy(body, &bin).and_then(|_| fs::set_permissions(&bin, fs::Permissions::from_mode(0o500))).map_err(|e| e.to_string())?;
    let work = run.dir.join("work/task");
    task::extract(&t.dir.join("workspace.tar"), &work)?;
    let lane = crate::egress::sealed_lane(&env.rh, &env.ch, &run.id, &lane_sock(&env.rh, &run), budget.min(t.max_cost), env.level)
        .map_err(|e| format!("sealed lane: {e}"))?;
    let cap = |k: &str| t.limits[k].as_u64().unwrap_or(0);
    lane.set_caps(crate::egress::Caps { turns: cap("max_turns"), tokens_out: cap("max_output_tokens"), tokens_in: cap("max_input_tokens") });
    let home = body_home(&env.ih, &env.rh, env.t0_sha.as_deref(), &run)?;
    let lim = Limits::of(&env.ch);
    let mut c = sandboxed(env, &run, Some(&lane.sock), &[bin.into()]);
    c.envs(home).envs(memwatch::body_env(&lim, &run.events_path()));
    c.current_dir(&work);
    let input = input_frame(&t.id, &work, &t.prompt)?;
    let (code, mut outcome, _) = supervise(c, &run, &lim, Duration::from_secs(t.wall_s), Some(icj(&input).unwrap_or_default()), || {
        let s = lane.status();
        let idle = s["idle_ms"].as_i64().unwrap_or(0);
        match (s["last_finish"].as_str(), s["inflight"].as_u64(), s["capped"].as_u64().unwrap_or(0) > 0) {
            (_, Some(0), true) if idle > 30_000 => Some("cost_capped"),
            (Some("stop"), Some(0), _) if idle > 5_000 => Some("done"),
            _ => None,
        }
    })?;
    let st = lane.close();
    if outcome == "disk_floor" {
        return Err("free disk fell below the workload floor (sandbox.disk_floor_gib): the sealed run was stopped".into());
    }
    if st["capped"].as_u64().unwrap_or(0) > 0 {
        outcome = "cost_capped";
    }
    let ck = check_v(env, &run, t, &work)?;
    let mut o = json!({"outcome": outcome, "repeat": repeat, "score_permille": ck.score, "spent_musd": st["spent_musd"], "task": t.id,
        "tamper": ck.tamper, "void": ck.void, "wall_ms": t0.elapsed().as_millis() as u64, "exit_code": code});
    for k in ["requests", "turns", "input_tokens", "cached_tokens", "output_tokens", "caps_hit"] {
        o[k] = st[k].clone();
    }
    Ok(o)
}

/// What one check run reported: the score, the runner's `tamper` flag (a protected file changed; m7-design.md
/// §3.1.3 item 2), whether it ran out of time, and `void` when the check dir changed under it (item 3).
pub struct Check {
    pub score: i64,
    pub tamper: bool,
    pub timeout: bool,
    pub void: Option<&'static str>,
}

/// The check protocol (kernel/gate.md §3.7.1) in a sealed check instance: cwd = the run's workspace, the
/// check copied into `out/check`; exit 0 and no score line = 1000, a last line `{"score_permille":N}` = N,
/// anything else = 0. M7: the score line `{"score_permille":0,"tamper":true}` is read whatever the exit code,
/// and the copy in `out/check` must equal the draw-time digest before and after the run (checker integrity).
pub fn check_v(env: &Env, run: &RunRoot, t: &Task, ws: &Path) -> Result<Check, String> {
    let out = run.dir.join("out");
    let ok = Command::new("/bin/cp").arg("-R").arg(t.dir.join("check")).arg(out.join("check")).status().is_ok_and(|s| s.success());
    if !ok {
        return Err("cannot copy the check".into());
    }
    let want = t.prov["check_digest"].as_str();
    let intact = || want.is_none_or(|w| tree_digest(&out.join("check")) == w);
    let before = intact();
    let mut argv: Vec<OsString> = vec![out.join("check/run").into()];
    argv.extend(t.argv[1..].iter().map(OsString::from));
    let mut c = sandboxed(env, run, None, &argv);
    let so = fs::File::create(out.join("check.out")).map_err(|e| e.to_string())?;
    c.current_dir(ws).env("INIT_TASK_ID", &t.id).env("INIT_CHECK_DIR", out.join("check")).stdout(so);
    let (code, why, _) = supervise(c, run, &Limits::of(&env.ch), Duration::from_secs(t.timeout_s), None, || None)?;
    if why == "disk_floor" {
        return Err("free disk fell below the workload floor (sandbox.disk_floor_gib): the check was stopped".into());
    }
    let void = (!(before && intact())).then_some("checker_tamper");
    let text = fs::read_to_string(out.join("check.out")).unwrap_or_default();
    let line = text.lines().rev().find(|l| !l.trim().is_empty()).and_then(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v.as_object().is_some_and(|o| o.contains_key("score_permille") && o.keys().all(|k| k == "score_permille" || k == "tamper")));
    // the runner exits 3 on a tamper (so pre-v3 parsers score it 0): the flag counts whatever the exit code
    let tamper = why != "timeout" && line.as_ref().is_some_and(|v| v["tamper"] == true);
    let score = match (why, code, line.map(|v| v["score_permille"].as_i64())) {
        _ if tamper || void.is_some() => 0,
        ("done", Some(0), Some(Some(s))) if (0..=1000).contains(&s) => s,
        ("done", Some(0), None) => 1000,
        _ => 0,
    };
    Ok(Check { score, tamper, timeout: why == "timeout", void })
}

/// m7-design.md §3.1.3 item 3, in s1lib's `tree_digest` format: sha256 over the lines `f <x|-> <sha256> <rel>`,
/// `d <rel>`, `l <target> <rel>` and `o <rel>`, sorted by relative path. Links are not followed.
pub fn tree_digest(root: &Path) -> String {
    fn walk(base: &Path, d: &Path, out: &mut Vec<(String, String)>) {
        for e in fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            let rel = p.strip_prefix(base).map(|r| r.to_string_lossy().to_string()).unwrap_or_default();
            let Ok(m) = fs::symlink_metadata(&p) else { continue };
            let line = if m.file_type().is_symlink() {
                format!("l {} {rel}", fs::read_link(&p).map(|t| t.display().to_string()).unwrap_or_default())
            } else if m.is_dir() {
                walk(base, &p, out);
                format!("d {rel}")
            } else if m.is_file() {
                let x = if m.permissions().mode() & 0o111 != 0 { "x" } else { "-" };
                format!("f {x} {} {rel}", fs::read(&p).map(|b| util::sha256_hex(&b)).unwrap_or_default())
            } else {
                format!("o {rel}")
            };
            out.push((rel, line));
        }
    }
    let mut v = Vec::new();
    walk(root, root, &mut v);
    v.sort();
    util::sha256_hex(v.into_iter().map(|(_, l)| l + "\n").collect::<String>().as_bytes())
}

/// The check alone on an extracted tar (import validation, §3.1).
pub fn check_only(env: &Env, t: &Task, tar: &Path) -> Result<i64, String> {
    check_tar(env, t, Some(tar)).map(|c| c.score)
}

/// The check on an extracted tar, or (None) on an empty workspace: the draw's validation (m7-design.md §3.1.5).
pub fn check_tar(env: &Env, t: &Task, tar: Option<&Path>) -> Result<Check, String> {
    let run = RunRoot::new(&env.rh)?;
    let ws = run.dir.join("work/task");
    match tar {
        Some(tar) => task::extract(tar, &ws)?,
        None => fs::DirBuilder::new().recursive(true).mode(0o700).create(&ws).map_err(|e| e.to_string())?,
    }
    check_v(env, &run, t, &ws)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// REAL-M7: the input frame carries what the body's `parse_frame` requires (id, seq, text) and the task marker
    /// the mock and the kernel's G4 runs use.
    #[test]
    fn input_frame_has_id_and_seq() {
        let f = input_frame("abc123", Path::new("/r/work/task"), "do it").unwrap();
        assert_eq!((f["t"].as_str(), f["seq"].as_u64(), f["mode"].as_str()), (Some("input"), Some(1), Some("steer")));
        assert_eq!(f["id"].as_str().map(str::len), Some(16));
        assert!(f["text"].as_str().unwrap().starts_with("[task abc123] Workspace: /r/work/task\n\ndo it"));
        assert!(icj(&f).is_ok());
    }

    /// REAL-M7: a sealed run of the real body gets a bootable INIT_HOME (before this the body exited 78,
    /// `INIT_HOME: missing`, and no sealed run of a real body made a request). T0 comes from the lkg tree only.
    #[test]
    fn body_home_is_a_bootable_probe_like_root() {
        let base = fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap()
            .join(format!("test-homes/m7-unit/bh-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base); // exact path, created by this test
        let (ih, rh) = (base.join("ih"), base.join("rh"));
        let tree = ih.join("lineage/trees/0123456789ab");
        fs::create_dir_all(tree.join("skills/evolve")).unwrap();
        for (f, b) in [("GENESIS.md", "g"), ("SELF.md", "s"), ("self.json", "{}"), ("skills/evolve/SKILL.md", "k")] {
            fs::write(tree.join(f), b).unwrap();
        }
        fs::create_dir_all(ih.join("kernel")).unwrap();
        fs::create_dir_all(ih.join("state/kernel")).unwrap();
        fs::create_dir_all(ih.join("body")).unwrap();
        fs::write(ih.join("body/GENESIS.md"), "agent-written, never used").unwrap();
        fs::write(ih.join("kernel/CONSTITUTION.md"), "c").unwrap();
        fs::write(ih.join("state/kernel/boot.json"), r#"{"v":1,"body_id":"b0","boot_id":"x","mode":"probe","provisional":{"a":1},
            "env":{"init_home":"/live","kernel_version":"9.9.9"},"model":{"provider":"deepseek","model":"deepseek-flash"}}"#).unwrap();
        fs::create_dir_all(rh.join("run")).unwrap();
        let run = RunRoot::new(&rh).unwrap();
        assert!(body_home(&ih, &rh, None, &run).is_err(), "no lkg: no T0 source");
        assert!(body_home(&ih, &rh, Some("ffffffffffff0000"), &run).is_err(), "no exported tree: refused");
        let env: BTreeMap<&str, String> = body_home(&ih, &rh, Some("0123456789abcdef0123456789abcdef01234567"), &run).unwrap().into_iter().collect();
        assert_eq!(env["INIT_HOME"], run.dir.to_string_lossy());
        assert_eq!((env["INIT_UI"].as_str(), env["INIT_MODE"].as_str(), env["INIT_BOOT_REASON"].as_str(), env["INIT_KERNEL_VERSION"].as_str()),
                   ("stdio-jsonl/1", "normal", "cold", "9.9.9"));
        assert!(env["INIT_SESSION"].len() == 22 && env["INIT_BOOT_ID"].len() == 16 && env["INIT_SHA"].starts_with("0123456789ab"));
        assert!(!Path::new(&env["INIT_KERNEL_SOCK"]).exists(), "a sealed run has no kernel");
        for f in ["body/GENESIS.md", "body/SELF.md", "body/self.json", "body/skills/evolve/SKILL.md", "kernel/CONSTITUTION.md"] {
            assert!(run.dir.join(f).is_file(), "{f}");
        }
        assert_eq!(fs::read_to_string(run.dir.join("body/GENESIS.md")).unwrap(), "g", "T0 from the lkg tree, not INIT_HOME/body");
        let b: Value = serde_json::from_slice(&fs::read(run.dir.join("state/kernel/boot.json")).unwrap()).unwrap();
        assert_eq!((b["boot_id"].as_str(), b["session"].as_str(), b["mode"].as_str(), b["body_id"].as_str(), b["reason"].as_str()),
                   (Some(env["INIT_BOOT_ID"].as_str()), Some(env["INIT_SESSION"].as_str()), Some("normal"), Some("sealed"), Some("cold")));
        assert_eq!((b["env"]["init_home"].clone(), b["model"]["model"].clone(), b["provisional"].clone()),
                   (json!(run.dir), json!("deepseek-flash"), Value::Null));
        drop(run);
        let _ = fs::remove_dir_all(&base); // exact path
    }

    /// M7 checker integrity: the Root's `tree_digest` equals pack s1's reference (`s1lib.tree_digest`) on a tree with
    /// an executable, a plain file, a nested directory and a link. Skipped when python3 or the pack is missing.
    #[test]
    fn tree_digest_matches_s1lib() {
        let lib = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../sealed/packs/s1/lib");
        let base = fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap();
        let d = base.join(format!("test-homes/m7-unit/td-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d); // exact path, created by this test
        fs::create_dir_all(d.join("hidden/sub")).unwrap();
        fs::write(d.join("run"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(d.join("run"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(d.join("hidden/sub/cases.json"), "[1]").unwrap();
        std::os::unix::fs::symlink("run", d.join("link")).unwrap();
        let py = format!("import sys; sys.dont_write_bytecode = True; sys.path.insert(0, {:?}); import s1lib; print(s1lib.tree_digest({:?}))",
            lib.display().to_string(), d.display().to_string());
        let o = Command::new("/usr/bin/python3").args(["-I", "-c", &py]).output();
        let want = o.ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        let got = tree_digest(&d);
        fs::remove_dir_all(&d).unwrap();
        match want {
            Some(w) => assert_eq!(got, w),
            None => eprintln!("SKIP: python3 or root/sealed/packs/s1/lib is not available"),
        }
    }

    /// A leader that exits leaves its group behind: `supervise` kills the group while the leader is still an
    /// unreaped zombie (its pgid still ours), then reaps it. The test signals nothing itself.
    #[test]
    fn the_group_is_killed_before_the_leader_is_reaped() {
        let base = fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap();
        let rh = base.join(format!("test-homes/wp24/unit-sup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&rh); // exact path, created by this test
        fs::create_dir_all(rh.join("sealed/run")).unwrap();
        let run = RunRoot::new(&rh).unwrap();
        let pidf = run.dir.join("tmp/bg.pid");
        let mut c = Command::new("/bin/sh");
        c.arg("-c").arg(format!("/bin/sleep 60 & echo $! > '{}'; exit 0", pidf.display())).stdin(Stdio::null()).stdout(Stdio::null()).process_group(0);
        let lim = Limits { cap: 4096 * memwatch::MIB, floor: 0 };
        let (code, why, _) = supervise(c, &run, &lim, Duration::from_secs(20), None, || None).unwrap();
        assert_eq!((code, why), (Some(0), "done"));
        let bg = fs::read_to_string(&pidf).unwrap().trim().to_string();
        let alive = || Command::new("/bin/ps").args(["-o", "stat=", "-p", &bg]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .is_ok_and(|s| !s.is_empty() && !s.starts_with('Z'));
        let t = Instant::now();
        while alive() && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive(), "the group member {bg} outlived its leader");
        drop(run);
        fs::remove_dir_all(&rh).unwrap();
    }

    /// `ps -o stat=`: is `pid` a live process (not a zombie)? Read-only.
    fn live(pid: &str) -> bool {
        Command::new("/bin/ps").args(["-o", "stat=", "-p", pid]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .is_ok_and(|s| !s.is_empty() && !s.starts_with('Z'))
    }

    /// MEM-RUNAWAY, live ($0, no disk fill): a sealed run whose leader (`sh`) starts three children under a permissive
    /// Seatbelt profile (so they are sandboxed, like every zone process): a hog that allocates and touches 400 MiB, a
    /// sibling sleeper, and a sleeper that leaves the group with `setsid`. With a 256 MiB cap the watchdog kills the hog
    /// alone, by pid, within the poll bound, after recording it in the run's events file; the leader and both siblings
    /// survive it; the owner alert is written. At the end of the run the setsid sibling, out of reach of the group kill,
    /// is killed as a leftover unit. The test signals nothing itself; every process here is its own run's.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_workload_over_the_cap_is_killed_alone_and_recorded() {
        let base = fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap();
        let rh = base.join(format!("test-homes/memwatch/sup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&rh); // exact path, created by this test
        fs::create_dir_all(rh.join("sealed/run")).unwrap();
        let run = RunRoot::new(&rh).unwrap();
        let t = run.dir.join("tmp");
        let sb = "/usr/bin/sandbox-exec -p '(version 1)(allow default)'";
        // the paths hold a space ("INIT Agent"): every one is quoted
        let script = format!("echo $$ > '{t}/leader'; \
            {sb} /usr/bin/perl -e '$x = \"a\" x (400 * 1024 * 1024); sleep 60' & echo $! > '{t}/hog'; \
            {sb} /bin/sleep 60 & echo $! > '{t}/sib'; \
            {sb} /usr/bin/perl -e 'use POSIX (); POSIX::setsid(); sleep 60' & echo $! > '{t}/sid'; wait", t = t.display());
        assert!(!t.display().to_string().contains('\''));
        let mut c = Command::new("/bin/sh");
        c.arg("-c").arg(script).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0);
        let lim = Limits { cap: 256 * memwatch::MIB, floor: 0 };
        let read = |f: &str| fs::read_to_string(t.join(f)).map(|s| s.trim().to_string()).unwrap_or_default();
        let events = run.events_path();
        let (t0, mut seen) = (Instant::now(), None);
        let (code, why, _) = supervise(c, &run, &lim, Duration::from_secs(60), None, || {
            let hog = read("hog");
            if seen.is_none() && !hog.is_empty() && !live(&hog) {
                // the hog is gone: everyone else must still run at this moment
                seen = Some((t0.elapsed(), live(&read("leader")), live(&read("sib")), live(&read("sid"))));
            }
            if seen.is_some() || t0.elapsed() > Duration::from_secs(30) { Some("done") } else { None }
        }).unwrap();
        assert_eq!((code, why), (None, "done"));
        let (after, leader, sib, sid) = seen.expect("the hog was killed within 30 s");
        assert!(leader && sib && sid, "the leader {leader}, the sibling {sib} and the setsid sibling {sid} survive the kill");
        assert!(after < Duration::from_secs(10), "killed {after:?} after start (poll 1 s; allocation ~1 s)");
        let recs: Vec<Value> = fs::read_to_string(&events).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(recs.len(), 1, "{recs:?}");
        let r = &recs[0];
        assert_eq!((r["event"].as_str(), r["scope"].as_str(), r["cap_mib"].as_u64(), r["kind"].as_str()), (Some("mem_cap"), Some("unit"), Some(256), Some("sealed")));
        assert_eq!(r["pgid"].as_i64().map(|p| p.to_string()), Some(read("hog")), "the record names the hog's unit");
        assert!(r["footprint_mib"].as_u64().unwrap() > 256, "{r}");
        let alerts = fs::read_to_string(rh.join("alerts.jsonl")).unwrap();
        assert!(alerts.contains("\"code\":\"mem_cap\"") && alerts.contains("sandbox.mem_cap_mib = 256 MiB"), "{alerts}");
        let t1 = Instant::now();
        while (live(&read("sid")) || live(&read("sib"))) && t1.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!live(&read("sid")), "the setsid sibling (its own group) is killed at the end of the run");
        assert!(!live(&read("sib")), "the sibling is killed at the end of the run");
        drop(run);
        fs::remove_dir_all(&rh).unwrap();
    }

    /// LINUX, live ($0, no disk fill): the macOS test above under the real Linux confinement. The leader is the outer
    /// bwrap of `linux::sealed` (exec-clean → unshare → bwrap → the namespace's init → `sh`); inside, `sh` starts a
    /// 400 MiB hog, a sibling and a setsid sibling. With a 256 MiB cap the watchdog kills the hog alone (not the whole
    /// instance behind the bwrap hops), records it by the pid `sh` knows it by (its pid-namespace pid), alerts the
    /// owner, and kills the siblings at the end of the run. Processes are found from the host by unique argv markers.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires native Linux namespaces, bubblewrap >= 0.13.0 and INIT_TEST_ROOT_BIN"]
    fn linux_a_workload_over_the_cap_is_killed_alone_behind_bwrap_and_recorded() {
        use std::os::fd::AsRawFd;
        let binary = std::env::var_os("INIT_TEST_ROOT_BIN").expect("set INIT_TEST_ROOT_BIN");
        let base = fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap();
        let rh = base.join(format!("test-homes/memwatch/linux-{}", std::process::id()));
        let _ = fs::remove_dir_all(&rh); // exact path, created by this test
        fs::create_dir_all(rh.join("sealed/run")).unwrap();
        let tool = rh.join("..").join(format!("linux-tool-{}", std::process::id()));
        fs::create_dir_all(&tool).unwrap();
        let tool = fs::canonicalize(&tool).unwrap();
        let run = RunRoot::new(&rh).unwrap();
        let t = run.dir.join("tmp");
        let tag = format!("lmw{}", std::process::id());
        let script = format!("echo $$ > '{t}/leader'; \
            /usr/bin/python3 -c 'import time; x = bytearray(400 << 20); time.sleep(60)' {tag}-hog & echo $! > '{t}/hog'; \
            /usr/bin/python3 -c 'import time; time.sleep(60)' {tag}-sib & echo $! > '{t}/sib'; \
            /usr/bin/python3 -c 'import os, time; os.setsid(); time.sleep(60)' {tag}-sid & echo $! > '{t}/sid'; wait", t = t.display());
        let mut launcher = init_sandbox_profiles::linux::sealed(&run.dir, &rh, &tool, None).unwrap();
        let mut mounts = crate::sys::linux_bind_fds(&mut launcher).unwrap();
        let filter = crate::sys::linux_seccomp().unwrap();
        let mut c = Command::new(binary);
        c.args(["exec-clean", "--keep", "5"]);
        for fd in &mounts { c.arg("--keep").arg(fd.as_raw_fd().to_string()); }
        c.arg("--").args(launcher).args(["--", "/bin/sh", "-c", &script, &format!("{tag}-leader")])
            .env_clear().env("PATH", "/usr/bin:/bin").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0);
        crate::sys::map_seccomp(&mut c, filter.as_raw_fd());
        crate::sys::inherit_fds(&mut c, mounts.iter().map(AsRawFd::as_raw_fd).collect());
        mounts.push(filter);
        crate::sys::hold_spawn_fds(&mut c, mounts);
        // a process of this run, found by its argv marker (the files hold pid-namespace pids)
        let live = |what: &str| {
            let want = format!("{tag}-{what}");
            fs::read_dir("/proc").into_iter().flatten().flatten().any(|e| {
                let pid = e.file_name().to_string_lossy().to_string();
                fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| c.split(|b| *b == 0).any(|a| a == want.as_bytes()))
                    && fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| s.rsplit_once(") ").is_some_and(|(_, f)| !f.starts_with('Z')))
            })
        };
        let lim = Limits { cap: 256 * memwatch::MIB, floor: 0 };
        let read = |f: &str| fs::read_to_string(t.join(f)).map(|s| s.trim().to_string()).unwrap_or_default();
        let (t0, mut seen) = (Instant::now(), None);
        let (code, why, _) = supervise(c, &run, &lim, Duration::from_secs(60), None, || {
            if seen.is_none() && !read("hog").is_empty() && !read("sid").is_empty() && !live("hog") {
                seen = Some((t0.elapsed(), live("leader"), live("sib"), live("sid")));
            }
            if seen.is_some() || t0.elapsed() > Duration::from_secs(30) { Some("done") } else { None }
        }).unwrap();
        assert_eq!((code, why), (None, "done"));
        let (after, leader, sib, sid) = seen.expect("the hog was killed within 30 s");
        assert!(leader && sib && sid, "the leader {leader}, the sibling {sib} and the setsid sibling {sid} survive the kill");
        assert!(after < Duration::from_secs(10), "killed {after:?} after start");
        let recs: Vec<Value> = fs::read_to_string(run.events_path()).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(recs.len(), 1, "one unit, not the whole instance behind bwrap: {recs:?}");
        let r = &recs[0];
        assert_eq!((r["event"].as_str(), r["scope"].as_str(), r["cap_mib"].as_u64(), r["kind"].as_str()), (Some("mem_cap"), Some("unit"), Some(256), Some("sealed")));
        assert_eq!(r["pgid"].as_i64().map(|p| p.to_string()), Some(read("hog")), "the record names the hog by the pid sh knows");
        assert!(r["footprint_mib"].as_u64().unwrap() > 256, "{r}");
        let alerts = fs::read_to_string(rh.join("alerts.jsonl")).unwrap();
        assert!(alerts.contains("\"code\":\"mem_cap\"") && alerts.contains("sandbox.mem_cap_mib = 256 MiB"), "{alerts}");
        let t1 = Instant::now();
        while (live("sid") || live("sib") || live("leader")) && t1.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!live("sid") && !live("sib") && !live("leader"), "the run's processes end with it");
        drop(run);
        fs::remove_dir_all(&rh).unwrap();
        fs::remove_dir_all(&tool).unwrap();
    }

    /// MEM-RUNAWAY: below the workload disk floor a sealed run is refused before anything spawns (a fake floor far
    /// above any real free space stands in for a full disk; nothing is filled).
    #[test]
    fn a_sealed_run_is_refused_below_the_disk_floor() {
        let base = fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap();
        let rh = base.join(format!("test-homes/memwatch/floor-{}", std::process::id()));
        let _ = fs::remove_dir_all(&rh); // exact path, created by this test
        fs::create_dir_all(rh.join("sealed/run")).unwrap();
        let run = RunRoot::new(&rh).unwrap();
        let marker = run.dir.join("tmp/ran");
        let mut c = Command::new("/usr/bin/touch");
        c.arg(&marker);
        let lim = Limits { cap: 4096 * memwatch::MIB, floor: 1 << 50 };
        let e = supervise(c, &run, &lim, Duration::from_secs(5), None, || None).expect_err("refused");
        assert!(e.contains("below the workload floor sandbox.disk_floor_gib = 1048576 GiB"), "{e}");
        assert!(!marker.exists(), "nothing ran");
        drop(run);
        fs::remove_dir_all(&rh).unwrap();
    }
}
