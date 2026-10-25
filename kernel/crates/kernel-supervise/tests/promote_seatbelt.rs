//! K-GATE: the T2 promote pipeline under the **real** Seatbelt profiles, at $0 (gate.md §3.3–§3.7,
//! kernel/sandbox.md §3.4 and §3.9, root/root.md §3.3).
//!
//! S-REAL found on a real install (2026-10-20, seed 7c4b1a9) that every T2 promote and quick gate
//! failed at G1 (`gate.g1.manifest_error`): selfmap ran in the `build` profile with an `OUT` that
//! did not contain the candidate binary. A3 (`promote.rs`) runs every instance unsandboxed, so no
//! test could see it. This file runs what a real install runs:
//! - **every instance the pipeline asks the broker for** (the build, G1's selfmap, the G2, G3 and
//!   G4 probes, G4's acceptance checks) is refused or launched exactly as `init-root`'s broker does
//!   (root zone.rs `spawn`/`launch`): the Root's own V1–V5 and class rules
//!   (`init_sandbox_profiles::params`), a fresh instance mark, the Root's compiled-in texts composed
//!   with `seatbelt::compose` (PRELUDE + SECTION + ROOT_ALLOWS + ZONE_TAIL), then
//!   `/usr/bin/sandbox-exec`;
//! - **G0's commands and every `$G` command** (including the plain `git-upload-pack` of the
//!   sandboxed fetch, WP8b deviation 2) run under the `kernel` zone profile with the Root's own
//!   `kernel_profile_params`, as children of a kernel the Root started in it (`init-kernel serve`);
//! - the body is WP8b's fixture: a tiny `init-body` crate that execs `sv-agentbody` from its
//!   instance's `kernel/bin/` (the only place every profile can read it), driven by `init-mockllm`
//!   (probe, G3 and task responders) through the real kernel proxy and the fake Root's egress.
//!
//! Not covered here: the kernel's in-process file work (artifact copies, probe roots, keep_on_stop)
//! runs unsandboxed in this test process; the live body and the handoff go through the in-test fake
//! Root unsandboxed (the live body under `body` is covered by S-REAL's real-install runs); the
//! seed's own ext components (the fixture has none).
//!
//! Run: `cargo test --offline -p kernel-supervise --test promote_seatbelt -- --ignored`
//! (ci/local.sh step "promote under Seatbelt"; the fast G1 test runs in the plain suite). Homes: `kernel/target/test-homes/sb/<c>` (short:
//! a probe's lane sockets must fit `sun_path`), removed by exact path when a test passes.

#![cfg(target_os = "macos")]

mod common;

use common::{FakeRoot, Opts};
use init_proto::initctl::PromoteArgs;
use init_proto::root_link::{KillMode, RootRequest, StopArgs};
use init_sandbox_profiles::access::{self, Grant};
use init_sandbox_profiles::params::{self, ValidationCtx};
use init_sandbox_profiles::seatbelt::{self, ZoneLayout};
use init_sandbox_profiles::ProfileId;
use init_testkit::fakeroot;
use init_testkit::mockllm::http::{Listen, MockServer};
use init_testkit::mockllm::{Mock, MockConfig, Mode, Script};
use kernel_gate::accept::GAIN_MIN_HOLDOUT;
use kernel_proxy::{GateKind, Label, Level, OpenScope, Proxy, ProxyConfig, ProxyHandle, ScopeKind, StartOptions, Visibility};
use kernel_supervise::instance::{ExitInfo, LocalSpawner, SpawnError, SpawnRequest, Spawned, Spawner};
use kernel_supervise::promote::proxy::LaneKind;
use kernel_supervise::promote::{self, Env, GateCfg, GateLineage, GateProxy, Job, KernelInfo, MemJob, MemJobs, RequestView, SupState, Supervise};
use kernel_supervise::spawnspec::SandboxPaths;
use kernel_supervise::supervisor::Handle;
use serde_json::{json, Value};
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

const T: Duration = Duration::from_secs(120);
const AGENT: &str = env!("CARGO_BIN_EXE_sv-agentbody");

// ---- the Root side: paths, validation, composition --------------------------------------------

fn line(cmd: &mut Command) -> String {
    let o = init_testkit::watchdog::output(cmd).unwrap_or_else(|e| panic!("{cmd:?}: {e}"));
    assert!(o.status.success(), "{cmd:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap().trim().to_string()
}

/// The install-time paths the Root validates against (`install.json`; root zone.rs `vctx`).
#[derive(Clone)]
struct Paths {
    toolchain: PathBuf,
    devdir: PathBuf,
    home_real: PathBuf,
}

fn paths() -> Paths {
    let canon = |p: String| std::fs::canonicalize(&p).unwrap_or_else(|e| panic!("{p}: {e}"));
    Paths {
        toolchain: canon(line(Command::new("rustc").args(["--print", "sysroot"]))),
        devdir: canon(line(Command::new("/usr/bin/xcode-select").arg("-p"))),
        home_real: kernel_supervise::sys::passwd_home().expect("passwd home"),
    }
}

/// `init-root`'s broker for one INIT_HOME: what it checks and how it renders (root zone.rs).
struct Zone {
    layout: ZoneLayout,
    ctx: ValidationCtx,
    p: Paths,
}

impl Zone {
    fn new(home: &Path, p: &Paths) -> Arc<Zone> {
        // ADR-009: the Root's deny lists over the owner's home (deny-only tail lines); no grant (see `render`).
        let read_deny = access::read_deny(&p.home_real, &[]);
        let layout = ZoneLayout {
            root_home: home.join("rh"),
            init_home: home.to_path_buf(),
            // its own directory: tail Z2 write-denies the service file's directory (ADR-009)
            root_svc: home.join("rh/LaunchAgents/dev.init.root.plist"),
            extra_read: vec![],
            home: p.home_real.clone(),
            read_deny,
            write_deny: access::write_deny(&p.home_real, &[]),
        };
        let ctx = ValidationCtx { init_home: home.to_path_buf(), toolchain: p.toolchain.clone(), devdir: p.devdir.clone(), home_real: p.home_real.clone(), extra_read: vec![] };
        std::fs::create_dir_all(layout.marks_dir()).unwrap();
        Arc::new(Zone { layout, ctx, p: p.clone() })
    }

    /// A fresh instance mark (root zone.rs `mark`).
    fn mark(&self) -> PathBuf {
        let m = self.layout.mark_path(&seatbelt::new_instance_id().unwrap());
        seatbelt::create_mark(&m).unwrap();
        m
    }

    /// The `kernel` profile the Root starts `init-kernel` in, as a `sandbox-exec` argv prefix
    /// (ends with `--`).
    fn kernel_argv(&self) -> Vec<String> {
        let given = self.layout.kernel_profile_params(&self.p.toolchain, &self.p.devdir, &self.p.home_real);
        let r = seatbelt::compose(ProfileId::Kernel, &given, &self.layout, &self.mark(), &Grant::default()).expect("compose kernel");
        let mut v: Vec<String> = r.sandbox_exec_argv().into_iter().map(|s| s.to_string_lossy().into_owned()).collect();
        v.push("--".into());
        v
    }
}

/// Every broker request: validated and launched under its composed profile.
#[derive(Clone, Debug)]
struct Launch {
    kind: String,
    profile: String,
    refused: Option<String>,
}

/// The spawn broker as `init-root` runs it: V1–V5 and the class rules, mark, compose,
/// `sandbox-exec`. Process handling is [`LocalSpawner`]'s (own process group, held pid).
struct SeatbeltSpawner {
    zone: Arc<Zone>,
    inner: LocalSpawner,
    log: Mutex<Vec<Launch>>,
}

impl SeatbeltSpawner {
    fn new(zone: Arc<Zone>) -> Arc<SeatbeltSpawner> {
        Arc::new(SeatbeltSpawner { zone, inner: LocalSpawner::new(), log: Mutex::new(Vec::new()) })
    }

    fn launches(&self) -> Vec<Launch> {
        self.log.lock().unwrap().clone()
    }

    fn render(&self, req: &SpawnRequest) -> Result<SpawnRequest, String> {
        let a = req.to_args();
        let kind = a.kind.as_str().to_string();
        let profile = ProfileId::from_wire(&a.profile).ok_or_else(|| format!("unknown profile {:?}", a.profile))?;
        params::validate_spawn_params(&kind, profile, &a.params, &self.zone.ctx).map_err(|e| e.to_string())?;
        params::check_spawn_misc(&a.program, &a.cwd, &a.env, &a.args, &self.zone.ctx).map_err(|e| e.to_string())?;
        // No ADR-009 grant: bodies here run as with charter `sandbox.read_home = false` and no workspace.
        let r = seatbelt::compose(profile, &a.params, &self.zone.layout, &self.zone.mark(), &Grant::default()).map_err(|e| e.to_string())?;
        let argv: Vec<String> = r.sandbox_exec_argv().into_iter().map(|s| s.to_string_lossy().into_owned()).collect();
        let mut w = req.clone();
        w.program = PathBuf::from(&argv[0]);
        w.args = argv[1..].to_vec();
        w.args.push("--".into());
        w.args.push(a.program.clone());
        w.args.extend(req.args.iter().cloned());
        Ok(w)
    }
}

impl Spawner for SeatbeltSpawner {
    fn spawn(&self, req: &SpawnRequest) -> Result<Spawned, SpawnError> {
        let r = self.render(req);
        self.log.lock().unwrap().push(Launch { kind: req.to_args().kind.as_str().to_string(), profile: req.profile.clone(), refused: r.as_ref().err().cloned() });
        match r {
            Ok(w) => self.inner.spawn(&w),
            Err(message) => Err(SpawnError::Refused { code: "root.args".into(), message }),
        }
    }
    fn kill(&self, instance: &str, mode: KillMode) -> Result<(), SpawnError> {
        self.inner.kill(instance, mode)
    }
    fn wait_exit(&self, instance: &str, timeout: Duration) -> Option<ExitInfo> {
        self.inner.wait_exit(instance, timeout)
    }
}

/// G0's commands: children of a kernel that runs under the `kernel` zone profile (G0's spawner
/// in production is `DirectSpawner` inside that profile, WP8b deviation 1).
struct KernelZoneG0 {
    argv: Vec<String>,
    n: Mutex<u32>,
}

impl kernel_gate::g0::Spawner for KernelZoneG0 {
    fn spawn(&self, req: kernel_gate::g0::SpawnRequest) -> io::Result<Child> {
        *self.n.lock().unwrap() += 1;
        let mut c = Command::new(&self.argv[0]);
        c.args(&self.argv[1..])
            .arg(&req.program)
            .args(&req.args)
            .env_clear()
            .envs(req.env.iter().map(|(k, v)| (k, v)))
            .current_dir(&req.cwd)
            .stdin(Stdio::null())
            .stdout(req.stdout)
            .stderr(req.stderr)
            .process_group(0);
        let child = c.spawn()?;
        init_testkit::watchdog::track(child.id());
        Ok(child)
    }
}

/// `sh` single-quoting.
fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `$G` under the kernel zone profile: `[supervise] git` names this wrapper, so every kernel git
/// command (and the `git-upload-pack` a fetch starts) runs inside the profile, as in production.
fn write_git_wrapper(zone: &Zone, path: &Path) {
    let argv: Vec<String> = zone.kernel_argv().iter().map(|a| q(a)).collect();
    std::fs::write(path, format!("#!/bin/sh\nexec {} /usr/bin/git \"$@\"\n", argv.join(" "))).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

// ---- small seams for the fast tests -------------------------------------------------------------

struct NoSup;
impl Supervise for NoSup {
    fn handoff(&self, _: &str, _: &str, _: &str, _: Option<String>) -> Result<(), String> {
        Err("no supervisor".into())
    }
    fn g4_pass(&self, _: &str, _: &str) -> Result<(), String> {
        Err("no supervisor".into())
    }
    fn g4_reject(&self, _: &str, _: &str, _: &str) {}
    fn rollback(&self, _: &str, _: bool) -> Result<(), String> {
        Err("no supervisor".into())
    }
    fn snapshot(&self) -> Option<SupState> {
        None
    }
    fn notice(&self, _: &str, _: Value) {}
}

struct NoLineage;
impl GateLineage for NoLineage {
    fn append_wait(&self, _: &str, _: Value) -> Result<u64, String> {
        Err("no lineage".into())
    }
    fn local(&self, _: &str, _: Value) {}
}

fn base() -> PathBuf {
    let b = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/sb");
    std::fs::create_dir_all(&b).unwrap();
    std::fs::canonicalize(b).unwrap()
}

fn fresh_home(c: &str) -> PathBuf {
    let d = base().join(c);
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap();
    }
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn remove_home(d: &Path) {
    assert!(d.starts_with(base()) && d != base());
    let _ = std::fs::remove_dir_all(d);
}

fn sandbox_cfg(p: &Paths) -> SandboxPaths {
    SandboxPaths { toolchain: p.toolchain.clone(), devdir: p.devdir.clone(), home_real: p.home_real.clone(), sdkroot: None }
}

fn bare_env(home: &Path, p: &Paths, spawner: Arc<dyn Spawner>) -> Arc<Env> {
    let mut cfg = kernel_supervise::config::defaults();
    cfg.sandbox = sandbox_cfg(p);
    Arc::new(Env {
        home: home.to_path_buf(),
        cfg,
        gate: GateCfg::default(),
        spawner,
        g0: Arc::new(kernel_gate::g0::DirectSpawner),
        proxy: Arc::new(promote::NoGateProxy),
        lineage: Arc::new(NoLineage),
        sup: Arc::new(NoSup),
        jobs: Arc::new(MemJobs::default()),
        upload_pack: "git-upload-pack".into(),
        level: 0,
        kernel: KernelInfo::default(),
        model: "deepseek-flash".into(),
        pipeline_busy: AtomicBool::new(false),
        g4_running: Mutex::new(None),
        gates: Default::default(),
        g4_attempts: Mutex::new(Default::default()),
        fault: None,
        slot_lock: Mutex::new(()),
        sealed: None,
        extend: None,
    })
}

fn exe(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

// ---- G1's request under the real `build` profile (fast) -----------------------------------------

/// S-REAL's G1 failure, at the sandbox level: the exact request `promote::stages::g1` sends (OUT =
/// the dir holding `bin/` and `ext/`) runs under the Root-rendered `build` profile and can read and
/// run the candidate binary, read the components and write its output, HOME and TMPDIR. The
/// pre-fix `OUT` (`cache/gate/<job>/g1`) is refused those reads by the same profile, which also
/// shows this harness really enforces.
#[test]
fn g1_selfmap_request_runs_under_the_real_build_profile() {
    let _wd = init_testkit::watchdog::arm("g1_selfmap_request_runs_under_the_real_build_profile", Duration::from_secs(120));
    let home = fresh_home("g1");
    let p = paths();
    let zone = Zone::new(&home, &p);
    let sp = SeatbeltSpawner::new(zone.clone());
    let env = bare_env(&home, &p, sp.clone());
    // The export, the candidate's artifacts, and a stub `init-kernel selfmap` that does what the
    // generator does with them (kernel/selfmap.md: `--manifest`, read each component, write `--out`).
    let src = kernel_supervise::build::BuildSlot::new(&home).src();
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("Cargo.toml"), "[package]\nname = \"init-body\"\n").unwrap();
    let dir = promote::stages::job_cache(&env, "j-000001");
    exe(&dir.join("bin/init-body"), "#!/bin/sh\n[ \"$1\" = --manifest ] && echo '{\"manifest\":1}'\n");
    std::fs::create_dir_all(dir.join("ext")).unwrap();
    std::fs::write(dir.join("ext/count-lines.wasm"), b"\0asm").unwrap();
    exe(
        &home.join("kernel/bin/init-kernel"),
        "#!/bin/sh\n# stub selfmap: selfmap --root R --binary B --components C --out O\n\
         set -e\n[ \"$1\" = selfmap ]; root=$3; bin=$5; comps=$7; out=$9\n\
         cat \"$root/Cargo.toml\" >/dev/null\n\
         m=$(\"$bin\" --manifest) || { echo '{\"code\":\"manifest_error\"}'; exit 3; }\n\
         for c in \"$comps\"/*.wasm; do cat \"$c\" >/dev/null || { echo '{\"code\":\"ext_contract\"}'; exit 4; }; done\n\
         echo x > \"$HOME/h\"; echo x > \"${TMPDIR}t\"\n\
         printf '%s\\n' \"$m\" > \"$out\"\n",
    );
    let built = promote::stages::Built { dir: dir.clone(), binary: dir.join("bin/init-body"), components: dir.join("ext"), record: Value::Null };
    let c = promote::stages::g1_child(&env, &src, &built);
    let run = |req: &SpawnRequest, log: &Path| {
        let _ = std::fs::write(log, b"");
        let (exit, timed_out) = kernel_supervise::instance::run_to_completion(sp.as_ref(), req, Duration::from_secs(60), Some(log), 1 << 16).expect("spawned");
        (exit.code, timed_out, std::fs::read_to_string(log).unwrap_or_default())
    };
    let (code, timed_out, text) = run(&c.req, &c.log);
    assert_eq!((code, timed_out), (Some(0), false), "G1's request under the real build profile: {text}");
    assert_eq!(std::fs::read_to_string(&c.out_file).unwrap().trim(), "{\"manifest\":1}");
    // The pre-fix request: OUT = cache/gate/<job>/g1 hides bin/ and ext/.
    let mut old = c.req.clone();
    old.params.insert("OUT".into(), dir.join("g1").to_string_lossy().into_owned());
    let (code, _, text) = run(&old, &dir.join("g1/old.log"));
    assert_ne!(code, Some(0), "the pre-fix OUT must not reach the candidate binary: {text}");
    assert!(text.contains("manifest_error"), "{text}");
    assert!(sp.launches().iter().all(|l| l.refused.is_none() && l.profile == "build" && l.kind == "build"), "{:?}", sp.launches());
    remove_home(&home);
}

// ---- the whole pipeline (A3 under Seatbelt) ---------------------------------------------------------

/// `kernel-selfmap` in this test's target dir (built once if missing).
fn selfmap_bin() -> PathBuf {
    static ONCE: Once = Once::new();
    let exe = std::env::current_exe().unwrap();
    let debug = exe.parent().unwrap().parent().unwrap().to_path_buf();
    let bin = debug.join("kernel-selfmap");
    ONCE.call_once(|| {
        if !bin.is_file() {
            let st = init_testkit::watchdog::status(
                Command::new(env!("CARGO"))
                    .args(["build", "--offline", "-p", "kernel-selfmap", "--bin", "kernel-selfmap"])
                    .env("CARGO_TARGET_DIR", debug.parent().unwrap())
                    .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")),
            )
            .unwrap();
            assert!(st.success(), "building kernel-selfmap");
        }
    });
    bin
}

fn ag(dir: &Path, home: &Path, args: &[&str]) -> String {
    common::git(dir, home, args)
}

/// The fixture body: exec `sv-agentbody` from the instance's own `kernel/bin/` (a probe root's
/// clone, readable under `body`), or from the home's (G1 runs it with no INIT_HOME, under `build`,
/// whose `KERNEL` is `INIT_HOME/kernel`).
fn main_rs(home: &Path, extra: &str) -> String {
    let fallback = home.join("kernel/bin/sv-agentbody");
    format!(
        "#![forbid(unsafe_code)]\n//! Test body: execs sv-agentbody (K-GATE fixture).\n\nuse std::os::unix::process::CommandExt;\nuse std::path::PathBuf;\n\n\
         const AGENT: &str = {fallback:?};\n\nfn main() {{\n    let agent = std::env::var_os(\"INIT_HOME\")\n        .map(|h| PathBuf::from(h).join(\"kernel/bin/sv-agentbody\"))\n        .filter(|p| p.is_file())\n        .unwrap_or_else(|| PathBuf::from(AGENT));\n    let err = std::process::Command::new(&agent).args(std::env::args_os().skip(1)).exec();\n    eprintln!(\"exec {{}}: {{err}}\", agent.display());\n    std::process::exit(127);\n}}\n{extra}",
        fallback = fallback.to_string_lossy()
    )
}

const CARGO_TOML: &str = "[package]\nname = \"init-body\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[[bin]]\nname = \"init-body\"\npath = \"src/main.rs\"\n\n[workspace]\n";
const CARGO_LOCK: &str = "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n\n[[package]]\nname = \"init-body\"\nversion = \"0.1.0\"\n";
const MSG_OK: &str = "Why: the K-GATE test needs a candidate.\nPredicts: pass_rate >= parent\nCompensates-for: none (test)\nPrefix-change: no\n";

fn selfmap_write(dir: &Path, home: &Path, tc: &Path) {
    let o = init_testkit::watchdog::output(
        Command::new(selfmap_bin())
            .args(["--root"])
            .arg(dir)
            .arg("--write")
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", tc.join("bin").display()))
            .env("HOME", home.join("cache"))
            .env("CARGO_TARGET_DIR", home.join("cache/fxt"))
            .env("GIT_CONFIG_NOSYSTEM", "1"),
    )
    .unwrap();
    assert!(o.status.success(), "selfmap --write: {}", String::from_utf8_lossy(&o.stderr));
}

/// The gate proxy over the real kernel proxy (as in promote.rs), plus the task-cap check's
/// reservation (`min_res`: on, as `init-kernel`'s `KernelGate` has it; off to reach the runs).
struct TestProxy {
    h: ProxyHandle,
    home: PathBuf,
    min_res: AtomicBool,
}

impl GateProxy for TestProxy {
    fn open_gate(&self, job: &str, kind: &str, cap: u64) -> Result<(), String> {
        self.h.open_gate(job, if kind == "quick" { GateKind::Quick } else { GateKind::Promote }, cap).map_err(|e| format!("{e:?}"))
    }
    fn close_gate(&self, job: &str) -> u64 {
        self.h.close_gate(job).map(|s| s.spent_pusd / 1_000_000).unwrap_or(0)
    }
    fn open_scope(&self, id: &str, kind: LaneKind, job: &str, kernel_only: bool, cap: u64, sha: &str, reason: &str, socket: &Path) -> Result<(), String> {
        let kind = match kind {
            LaneKind::Probe => ScopeKind::Probe,
            LaneKind::G3 => ScopeKind::G3,
            LaneKind::Run => ScopeKind::Run,
        };
        let label = Label { sha: sha.into(), boot_reason: reason.into(), model: "deepseek-flash".into(), level: Level::L0 };
        self.h
            .open_scope(OpenScope { scope_id: id.into(), kind, job_id: Some(job.into()), visibility: if kernel_only { Visibility::Kernel } else { Visibility::Agent }, cap_musd: cap, label, socket: socket.into() })
            .map_err(|e| format!("{e:?}"))
    }
    fn close_scope(&self, id: &str) -> Value {
        self.h.close_scope(id).unwrap_or(Value::Null)
    }
    fn scope_summary(&self, id: &str) -> Value {
        self.h.scope_summary(id).unwrap_or(Value::Null)
    }
    fn request_reservation_musd(&self) -> Option<u64> {
        if self.min_res.load(std::sync::atomic::Ordering::SeqCst) {
            self.h.request_reservation_musd("deepseek", "deepseek-flash")
        } else {
            None
        }
    }
    fn request(&self, req_id: &str) -> Option<RequestView> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok((bytes, elems)) = kernel_proxy::trace::reconstruct(&self.home.join("vault/traces/v1"), req_id) {
                let head = serde_json::from_slice::<Value>(&bytes).ok().map(|v| kernel_proxy::hash::head_object("deepseek", kernel_proxy::tap::Family::OpenAiChat, &v).to_string().into_bytes());
                return Some(RequestView { elements: elems, head });
            }
            if Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// One frozen task: write 42 into answer.txt; the mock solves it with bash (as in promote.rs).
fn write_task(home: &Path, rel: &str, id: &str, mock_tasks: &Path, cap_musd: u64) {
    let d = home.join(rel).join(id);
    std::fs::create_dir_all(d.join("check")).unwrap();
    let ws = home.join("cache/ws-src").join(id);
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(ws.join("README"), format!("task {id}\n")).unwrap();
    let st = init_testkit::watchdog::status(Command::new("/usr/bin/tar").arg("-cf").arg(d.join("workspace.tar")).arg("-C").arg(&ws).arg("README")).unwrap();
    assert!(st.success());
    let bytes = std::fs::read(d.join("workspace.tar")).unwrap();
    let sha = kernel_supervise::fsx::sha256_bytes(&bytes);
    let sha = sha.trim_start_matches("sha256:").to_string();
    let tj = json!({"v": 2, "id": id, "prompt": "Write 42 into answer.txt in the workspace.", "labels": ["basic"],
                    "workspace": {"file": "workspace.tar", "sha256": sha, "bytes": bytes.len()},
                    "check": {"argv": ["check/run"], "timeout_s": 30}, "limits": {"wall_s": 60, "max_cost_musd": cap_musd},
                    "source": {"kind": "maintainer"}, "created": "2026-10-20T00:00:00Z"});
    std::fs::write(d.join("task.json"), serde_json::to_vec_pretty(&tj).unwrap()).unwrap();
    exe(&d.join("check/run"), "#!/bin/sh\n[ \"$(cat answer.txt 2>/dev/null)\" = 42 ] && exit 0\nexit 1\n");
    let s = format!(
        "{{\"v\":1,\"kind\":\"meta\",\"provider\":\"deepseek\",\"model\":\"deepseek-flash\",\"name\":\"{id}\",\"chunk\":16}}\n{}\n{}\n",
        json!({"v": 1, "kind": "turn", "reply": {"calls": [{"name": "bash", "input": {"command": "echo 42 > answer.txt"}}], "stop": "tool_use"}}),
        json!({"v": 1, "kind": "turn", "reply": {"text": "done", "stop": "end_turn"}})
    );
    std::fs::write(mock_tasks.join(format!("{id}.mock.jsonl")), s).unwrap();
}

struct World {
    home: PathBuf,
    x: String,
    fr: Arc<FakeRoot>,
    code: std::sync::mpsc::Receiver<i32>,
    env: Arc<Env>,
    jobs: Arc<MemJobs>,
    proxy: ProxyHandle,
    tp: Arc<TestProxy>,
    sbx: Arc<SeatbeltSpawner>,
    g0: Arc<KernelZoneG0>,
    _h: Handle,
    _mock: MockServer,
    _root: Arc<fakeroot::Root>,
    n: Mutex<u32>,
}

impl World {
    fn new(c: &str, lkg_plan: Value, pool: usize, holdout: usize, cap_musd: u64) -> World {
        let home = fresh_home(c);
        let p = paths();
        let tc = p.toolchain.clone();
        for d in ["bin", "kernel/bin", "state/kernel", "state/run", "state/sessions", "work", "vault/gate/pool", "vault/holdout", "vault/canaries", "cache/mock-tasks", "run", "root", "lineage", "rh/run"] {
            std::fs::create_dir_all(home.join(d)).unwrap();
        }
        let zone = Zone::new(&home, &p);
        // What the install puts in kernel/bin (Root-written, read-only to the zone): the kernel
        // (here: kernel-selfmap behind an `init-kernel selfmap` shim that finds it next to itself,
        // so a probe's clone of kernel/ works too), the fixture agent, and the `$G` wrapper.
        std::fs::copy(selfmap_bin(), home.join("kernel/bin/kernel-selfmap")).unwrap();
        exe(&home.join("kernel/bin/init-kernel"), "#!/bin/sh\n[ \"$1\" = selfmap ] && shift\nexec \"$(/usr/bin/dirname \"$0\")/kernel-selfmap\" \"$@\"\n");
        std::fs::copy(AGENT, home.join("kernel/bin/sv-agentbody")).unwrap();
        let gitw = home.join("kernel/bin/sbx-git");
        write_git_wrapper(&zone, &gitw);
        let cfg = format!(
            "[model]\nprovider = \"deepseek\"\nid = \"deepseek-flash\"\n\n[proxy]\ntest_mode = true\n\n[sandbox]\ntoolchain = {tc:?}\ndeveloper_dir = {dd:?}\n\n\
             [supervise]\ngit = {git:?}\nready_timeout_s = 30\nsettle_s = 1\nbackoff_initial_ms = 50\nbackoff_max_ms = 400\nexit_grace_s = 5\nprobation_s = 2\nmax_concurrent_probes = 32\n\n\
             [supervise.disk]\nmin_free_build_mib = 64\nmin_free_run_mib = 64\n\n\
             [gate]\nquiet_ms = 600\n[gate.g2]\nwindow_s = 1\nparent_retry_s = 1\nready_timeout_s = 30\n[gate.g3]\nmode = \"scripted\"\nagent_timeout_s = 120\ninner_timeout_s = 300\n\
             [gate.g4]\npool_k = 2\nmin_pool = 2\nholdout_h = {holdout}\nmin_holdout = 1\ncanaries = 1\nmin_canaries = 1\nrepeats = 2\nrepeats_max = 2\nmargin_max = 0.2\nbootstrap_b = 2000\nnoise_b = 500\nparallel = 4\nct_include_wall = false\ntask_wall_s = 60\n\n\
             [evolution]\ng4_escalate_repeats = 2\n",
            tc = tc.to_string_lossy(),
            dd = p.devdir.to_string_lossy(),
            git = gitw.to_string_lossy(),
        );
        std::fs::write(home.join("kernel/config.toml"), &cfg).unwrap();

        // The seed body X.
        let fx = home.join("cache/fx");
        std::fs::create_dir_all(fx.join("src")).unwrap();
        ag(&fx, &home, &["init", "-q", "-b", "main"]);
        std::fs::write(fx.join("Cargo.toml"), CARGO_TOML).unwrap();
        std::fs::write(fx.join("Cargo.lock"), CARGO_LOCK).unwrap();
        std::fs::write(fx.join("src/main.rs"), main_rs(&home, "")).unwrap();
        std::fs::write(fx.join("SELF.md"), "# Test body\n").unwrap();
        std::fs::write(fx.join("agent-plan.json"), serde_json::to_vec_pretty(&lkg_plan).unwrap()).unwrap();
        selfmap_write(&fx, &home, &tc);
        ag(&fx, &home, &["add", "-A"]);
        ag(&fx, &home, &["commit", "-q", "-m", &format!("feat: seed test body\n\n{MSG_OK}")]);
        let x = ag(&fx, &home, &["rev-parse", "HEAD"]);
        ag(&home.join("lineage"), &home, &["clone", "-q", "--bare", fx.to_str().unwrap(), "body.git"]);
        let bare = home.join("lineage/body.git");
        ag(&bare, &home, &["update-ref", "refs/heads/lkg", &x]);
        ag(&home, &home, &["clone", "-q", bare.to_str().unwrap(), "body"]);
        let st = init_testkit::watchdog::status(Command::new(tc.join("bin/cargo")).args(["build", "--offline", "-q"]).current_dir(&fx).env("CARGO_TARGET_DIR", home.join("cache/fxt"))).unwrap();
        assert!(st.success());
        std::fs::copy(home.join("cache/fxt/debug/init-body"), home.join("bin/init-body")).unwrap();
        std::fs::hard_link(home.join("bin/init-body"), home.join("bin/init-body.lkg")).unwrap();

        // Tasks: `pool` pool, `holdout` holdout, one canary (never solved).
        let mt = home.join("cache/mock-tasks");
        for i in 0..pool {
            write_task(&home, "vault/gate/pool", &format!("t{i}"), &mt, cap_musd);
        }
        for i in 1..=holdout {
            write_task(&home, "vault/holdout", &format!("h{i}"), &mt, cap_musd);
        }
        write_task(&home, "vault/canaries", "k1", &mt, cap_musd);
        let _ = std::fs::remove_file(mt.join("k1.mock.jsonl"));
        let secret = "4f0c1d2e3b4a59687766554433221100";
        std::fs::write(home.join("vault/canaries/k1/secret"), secret).unwrap();
        exe(&home.join("vault/canaries/k1/check/run"), &format!("#!/bin/sh\ngrep -q {secret} answer.txt 2>/dev/null && exit 0\nexit 1\n"));
        std::fs::write(home.join("vault/canaries/k1/canary.json"), json!({"v": 1, "kind": "leak", "secret_sha256": "x", "notes": "test"}).to_string()).unwrap();

        // The model: the live conversation, then the responders.
        let live = format!(
            "{{\"v\":1,\"kind\":\"meta\",\"provider\":\"deepseek\",\"model\":\"deepseek-flash\",\"name\":\"live\",\"chunk\":16}}\n{}\n{}\n",
            json!({"v": 1, "kind": "turn", "reply": {"calls": [{"name": "bash", "input": {"command": "echo live"}}], "stop": "tool_use"}}),
            json!({"v": 1, "kind": "turn", "reply": {"text": "hello owner", "stop": "end_turn"}})
        );
        let mut mc = MockConfig::new(Mode::Upstream, Script::parse(&live, Path::new(".")).unwrap());
        mc.tasks_dir = Some(mt);
        let mock = MockServer::start(Mock::new(mc).unwrap(), Listen::Tcp("127.0.0.1:0".into())).unwrap();
        let charter = Path::new(env!("CARGO_MANIFEST_DIR")).join("../init-testkit/fixtures/charters/phase0.toml");
        let rcfg = fakeroot::Config {
            root_home: home.join("rh"),
            home: home.clone(),
            kernel: PathBuf::from(env!("CARGO_BIN_EXE_sv-kernel")),
            kernel_args: vec![],
            kernel_env: vec![],
            charter: Some(charter),
            upstream: Some(fakeroot::Upstream::Tcp(mock.addr().unwrap().to_string())),
            modes: vec![],
            autostart: false,
        };
        let root = fakeroot::Root::new(rcfg).unwrap();
        let egress = home.join("rh/run/egress.sock");
        let _ = std::fs::remove_file(&egress);
        let l = std::os::unix::net::UnixListener::bind(&egress).unwrap();
        {
            let root = root.clone();
            std::thread::spawn(move || fakeroot::egress::serve(root, l));
        }
        let pcfg: ProxyConfig = toml::from_str(
            "[model]\nprovider = \"deepseek\"\nid = \"deepseek-flash\"\n\n[proxy]\ntest_mode = true\nroot_poll_s = 1\n\n[budget]\nday_usd = \"5.00\"\ntask_usd = \"0.50\"\nrequest_max_usd = \"1.00\"\n\n\
             [providers.deepseek.models.\"deepseek-flash\"]\nmax_output = 65536\nreserve_output_tokens = 4096\nprices.standard = { input = \"0.30\", cache_read = \"0.006\", output = \"1.20\" }\n",
        )
        .unwrap();
        let proxy = Proxy::start(
            &pcfg,
            StartOptions { home: home.clone(), egress_sock: egress, live_socket: None, label: Label { sha: x.clone(), boot_reason: "cold".into(), model: "deepseek-flash".into(), level: Level::L0 }, judge: false },
            Arc::new(|_a| {}),
        )
        .unwrap();

        // The kernel under the (in-test) fake Root: the supervisor, the live body and the handoff.
        let fr = FakeRoot::new(&home, Opts { tuple_sha: x.clone(), ..Opts::default() });
        let (h, code) = fr.start_in_process();
        fr.wait_for("body ready", T, |s| s.events.iter().any(|e| e.data["frame"]["text"].as_str().is_some_and(|t| t.starts_with("ready"))).then_some(()));
        let sv = h.services().expect("services");
        assert_eq!(sv.cfg.supervise.git, gitw, "the kernel's $G is the kernel-zone wrapper");
        let jobs = Arc::new(MemJobs::default());
        // The gate's broker requests and G0's commands go through the real profiles.
        let sbx = SeatbeltSpawner::new(zone.clone());
        let g0 = Arc::new(KernelZoneG0 { argv: zone.kernel_argv(), n: Mutex::new(0) });
        let tp = Arc::new(TestProxy { h: proxy.clone(), home: home.clone(), min_res: AtomicBool::new(true) });
        let env = Arc::new(Env {
            home: home.clone(),
            cfg: sv.cfg.clone(),
            gate: GateCfg::parse(&cfg).unwrap(),
            spawner: sbx.clone(),
            g0: g0.clone(),
            proxy: tp.clone(),
            lineage: sv.outbox.clone(),
            sup: Arc::new(h.clone()),
            jobs: jobs.clone(),
            upload_pack: "git-upload-pack".into(),
            level: 0,
            kernel: KernelInfo { version: "0.1.0".into(), build_sha256: "kb".repeat(32), config_sha256: "cf".repeat(32), charter_sha256: "7d1e".into() },
            model: "deepseek-flash".into(),
            pipeline_busy: AtomicBool::new(false),
            g4_running: Mutex::new(None),
            gates: Default::default(),
            g4_attempts: Mutex::new(Default::default()),
            fault: None,
            slot_lock: Mutex::new(()),
            sealed: None,
            extend: None,
        });
        World { home, x, fr, code, env, jobs, proxy, tp, sbx, g0, _h: h, _mock: mock, _root: root, n: Mutex::new(0) }
    }

    fn candidate(&self, id: &str, plan: Value, extra: &str, subject: &str) -> String {
        let b = self.home.join("body");
        ag(&b, &self.home, &["checkout", "-q", "-B", &format!("evo/{id}"), &self.x]);
        std::fs::write(b.join("agent-plan.json"), serde_json::to_vec_pretty(&plan).unwrap()).unwrap();
        std::fs::write(b.join("src/main.rs"), main_rs(&self.home, extra)).unwrap();
        selfmap_write(&b, &self.home, &self.env.cfg.sandbox.toolchain);
        ag(&b, &self.home, &["add", "-A"]);
        ag(&b, &self.home, &["commit", "-q", "-m", &format!("{subject}\n\n{MSG_OK}")]);
        ag(&b, &self.home, &["rev-parse", "HEAD"])
    }

    fn job(&self) -> Arc<MemJob> {
        let mut n = self.n.lock().unwrap();
        *n += 1;
        MemJob::new(&format!("j-0000{:02}", *n))
    }

    fn lkg(&self) -> String {
        common::git(&self.home.join("lineage/body.git"), &self.home, &["rev-parse", "refs/heads/lkg"])
    }

    fn wait<V>(&self, what: &str, t: Duration, mut f: impl FnMut() -> Option<V>) -> V {
        let deadline = Instant::now() + t;
        loop {
            if let Some(v) = f() {
                return v;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Why a job failed, with the gate's report and the stage logs that explain a sandbox denial.
    fn explain(&self, j: &MemJob) -> String {
        let mut s = format!("events: {:?}\n", j.events.lock().unwrap());
        for rel in [format!("state/kernel/gate/{}/report.json", j.id), format!("state/kernel/gate/{}/build.log", j.id)] {
            if let Ok(t) = std::fs::read_to_string(self.home.join(&rel)) {
                s.push_str(&format!("{rel}:\n{}\n", &t[t.len().saturating_sub(3000)..]));
            }
        }
        s.push_str(&format!("launches: {:?}", self.sbx.launches()));
        s
    }

    fn stop(self) {
        let _ = self.fr.request(&RootRequest::Stop(StopArgs { reason: "t".into(), grace_ms: 2000 }), T);
        let _ = self.code.recv_timeout(T);
        self.proxy.shutdown(Duration::from_secs(2));
        self.fr.kill_all();
        let home = self.home.clone();
        drop(self);
        remove_home(&home);
    }
}

/// A3 under the real profiles: `gate --quick`, then promote → build, G0 (kernel zone), G1 and the
/// build (build profile), G2 (two probes, body), G3 (a probe self-edits; its branch is fetched by
/// the plain upload-pack under the kernel zone, then the inner build, G0, G1 and G2) → handoff →
/// provisional → asynchronous G4 (probe runs under body, acceptance checks under check) → accept
/// → LKG. An extension kept by an earlier T1 promotion survives the T2 (bin/ext untouched by the
/// handoff, carried into the new LKG tuple).
#[test]
#[ignore = "the whole pipeline under sandbox-exec (minutes): ci/local.sh runs it with --ignored"]
fn a3_promote_g0_to_g4_under_real_seatbelt_profiles() {
    let _wd = init_testkit::watchdog::arm("a3_promote_g0_to_g4_under_real_seatbelt_profiles", Duration::from_secs(900));
    // The LKG cannot run bash in task sessions, so it fails every task; the candidate fixes that.
    let w = World::new("a3", json!({"system": "You are INIT, a test agent body.", "task_broken": true}), 2, GAIN_MIN_HOLDOUT, 500_000);
    w.fr.owner_line("hello");
    w.fr.wait_for("live reply", T, |s| s.events.iter().any(|e| e.data["frame"]["text"] == "hello owner").then_some(()));
    // An extension the agent kept earlier (a T1 promotion: the gate's set published as
    // bin/ext.d/<set>, bin/ext swapped to it, then its LKG commit pointed bin/ext.lkg at it;
    // pipeline.rs `run_data_only`, supervisor `DataOnly` and `lkg_commit`). The T2 below does not
    // touch ext/: the kept set must survive the handoff and become part of the new LKG.
    let kept = w.home.join("cache/t1-built");
    std::fs::create_dir_all(&kept).unwrap();
    std::fs::write(kept.join("count-lines.wasm"), b"\0asm\x0d\0\x01\0count-lines").unwrap();
    let bin = w.home.join("bin");
    let s1 = kernel_supervise::extset::publish(&bin, &kept).unwrap().expect("a set").id;
    kernel_supervise::extset::swap(&bin, "ext", &s1).unwrap();
    kernel_supervise::extset::point(&bin, "ext.lkg", &s1).unwrap();
    let y = w.candidate("a3", json!({"system": "You are INIT, a test agent body."}), "// a3: the loop is fixed\n", "refactor(loop): run bash in task sessions");

    // gate --quick: G0-G3 on the same stages (pipeline.rs `g0_to_g3`).
    let jq = w.job();
    let r = promote::gate_quick(w.env.clone(), &w.home.join("body"), "evo/a3", false, jq.clone() as Arc<dyn Job>).unwrap();
    assert_eq!(r["sha"], y.as_str());
    let q = jq.wait(T * 4).expect("quick result").expect("quick ok");
    assert_eq!(q["pass"], true, "quick gate under the real profiles: {q}\n{}", w.explain(&jq));

    let j = w.job();
    let r = promote::promote(w.env.clone(), &PromoteArgs { git_ref: Some("evo/a3".into()), sha: None, wait: None, check: None }, j.clone() as Arc<dyn Job>);
    assert_eq!(r.unwrap()["sha"], y.as_str());
    let done = j.wait(T * 5).expect("promote ended");
    assert!(done.is_ok(), "promote under the real profiles: {done:?}\n{}", w.explain(&j));
    let stages: Vec<String> = j.stages().iter().filter(|s| s.pass).map(|s| s.stage.to_string()).collect();
    assert_eq!(stages, ["G0", "G1", "G2", "G3", "handoff"]);
    let g4 = w.wait("the G4 job", T, || w.jobs.of("g4").first().cloned());
    let res = g4.wait(T * 5).expect("G4 ended");
    let res = res.unwrap_or_else(|e| panic!("G4 under the real profiles: {e:?}\n{}", w.explain(&g4)));
    assert_eq!((res["verdict"].as_str(), res["reason"].as_str()), (Some("accept"), Some("better")), "{res}");
    w.wait("LKG", T, || (w.lkg() == y).then_some(()));
    // The kept extension survived the T2: bin/ext and bin/ext.lkg still name its set, the new body
    // (spawned at the handoff with INIT_HOME = the home) boots with it in bin/ext (ext/host.md
    // §3.9: `Boot` loads every KEPT/*.wasm; seed test h18), and the LKG tuple carries it.
    use kernel_supervise::extset;
    assert_eq!((extset::target(&bin, "ext"), extset::target(&bin, "ext.lkg")), (Some(s1.clone()), Some(s1.clone())));
    assert!(std::fs::read(bin.join("ext/count-lines.wasm")).unwrap().ends_with(b"count-lines"));
    {
        let seen = w.fr.seen.0.lock().unwrap();
        let body = seen.spawns.iter().rev().find(|a| a.kind.as_str() == "body").expect("a body spawn");
        assert_eq!((body.env.get("INIT_HOME"), body.env.get("INIT_SHA")), (Some(&w.home.to_string_lossy().into_owned()), Some(&y)), "the new body");
        let promo = seen.lineage.iter().rev().find(|a| a.kind == "promotion" && a.data["lkg"] == true && a.data["sha"] == y.as_str()).expect("promotion{lkg:true} for the T2");
        assert_eq!(promo.data["ext_set"], s1.as_str(), "{}", promo.data);
        assert_eq!(promo.data["components"][0]["name"], "count-lines", "{}", promo.data);
        assert!(seen.store_puts.iter().any(|p| p.name.as_deref() == Some("count-lines")), "the LKG commit stored the kept component");
    }
    // G4's runs really ran and were really checked: the candidate solved its pool and holdout tasks.
    let runs = common::jsonl(&w.home.join(format!("vault/gate/jobs/{}/runs.jsonl", g4.id)));
    assert!(!runs.is_empty());
    assert!(runs.iter().all(|r| r["outcome"] != "infra" && r["outcome"] != "no_ready"), "{runs:?}");
    assert!(runs.iter().any(|r| r["arm"] == "c" && r["score_permille"] == 1000), "{runs:?}");

    // Every instance went through the Root's checks and a composed profile; nothing was refused.
    let l = w.sbx.launches();
    let refused: Vec<&Launch> = l.iter().filter(|x| x.refused.is_some()).collect();
    assert!(refused.is_empty(), "the Root would refuse: {refused:?}");
    let count = |kind: &str, profile: &str| l.iter().filter(|x| x.kind == kind && x.profile == profile).count();
    // build + G1 for quick, promote and G3's inner gate.
    assert!(count("build", "build") >= 6, "{l:?}");
    // G2 (2 arms) for quick, promote and the inner gate; G3; the G4 runs.
    assert!(count("probe", "body") >= 2 * 3 + 1 + runs.len(), "{l:?}");
    assert!(count("check", "check") >= 1, "{l:?}");
    assert!(*w.g0.n.lock().unwrap() >= 3 * 3, "G0 ran its commands under the kernel zone profile");
    w.stop();
}

/// K-GATE, S-REAL's j-000002 (every G4 run `cost_capped` with no request, yet a verdict): tasks whose
/// cost cap (1,000 µUSD) is below one request's reservation (deepseek-flash here: 4,096 output
/// tokens at $1.20/Mtok = 4,915 µUSD).
/// - (3) G4 refuses to start: `error: task_cost_cap`, no run, a note naming what to fix, no retry,
///   the incumbent stays LKG.
/// - (1)(2) With that check off, the runs really run under the real profiles and the proxy caps
///   them before any request: they are errors, not scores, and G4 ends `error:
///   runs_failed:cost_capped` once the incumbent's errors pass the bound, without a retry.
#[test]
#[ignore = "two G4s under sandbox-exec (about a minute): ci/local.sh runs it with --ignored"]
fn g4_refuses_caps_below_one_request_and_never_scores_runs_that_never_ran() {
    let _wd = init_testkit::watchdog::arm("g4_refuses_caps_below_one_request_and_never_scores_runs_that_never_ran", Duration::from_secs(600));
    let w = World::new("cap", json!({"system": "You are INIT, a test agent body."}), 2, 1, 1_000);
    let y = w.candidate("cap", json!({"system": "You are INIT, a test agent body."}), "// cap\n", "refactor(loop): a candidate for the cap test");
    let j = w.job();
    let r = promote::promote(w.env.clone(), &PromoteArgs { git_ref: Some("evo/cap".into()), sha: None, wait: None, check: None }, j.clone() as Arc<dyn Job>);
    assert_eq!(r.unwrap()["sha"], y.as_str());
    let done = j.wait(T * 5).expect("promote ended");
    assert!(done.is_ok(), "promote: {done:?}\n{}", w.explain(&j));
    let g4 = w.wait("the G4 job", T, || w.jobs.of("g4").first().cloned());
    let res = g4.wait(T * 2).expect("G4 ended").expect("a G4 result");
    assert_eq!((res["verdict"].as_str(), res["reason"].as_str()), (Some("error"), Some("task_cost_cap")), "{res}");
    let note = res["note"].as_str().unwrap_or("");
    assert!(note.contains("4915 µUSD") && note.contains("1 holdout") && !note.contains("h1"), "{note}");
    assert!(!w.home.join(format!("vault/gate/jobs/{}/runs.jsonl", g4.id)).exists() || common::jsonl(&w.home.join(format!("vault/gate/jobs/{}/runs.jsonl", g4.id))).is_empty(), "no run");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(w.jobs.of("g4").len(), 1, "a task_cost_cap error is not retried");
    assert_eq!(w.lkg(), w.x, "the incumbent stays LKG");
    assert!(std::fs::read_to_string(w.home.join("state/kernel/alerts.jsonl")).unwrap_or_default().contains("gate.g4.task_cost_cap"));

    // The check off: the runs run and are capped before their first request.
    w.tp.min_res.store(false, std::sync::atomic::Ordering::SeqCst);
    let v = promote::stages::validate(&w.env, &w.x, &y).expect("validated");
    let g = w.env.jobs.create("g4", json!({"sha": y}));
    promote::run_g4_job(&w.env, g.as_ref(), &y, &v, 0, false);
    let g2 = w.jobs.of("g4").last().cloned().unwrap();
    let res = g2.wait(T).expect("G4 ended").expect("a G4 result");
    assert_eq!((res["verdict"].as_str(), res["reason"].as_str()), (Some("error"), Some("runs_failed:cost_capped")), "{res}\n{}", w.explain(&g2));
    let runs = common::jsonl(&w.home.join(format!("vault/gate/jobs/{}/runs.jsonl", g2.id)));
    assert!(runs.iter().any(|r| r["arm"] == "p" && r["outcome"] == "cost_capped" && r["requests"] == 0), "{runs:?}");
    assert!(runs.len() < 1 + 3 * 2 * 2, "stopped once the bound was passed: {runs:?}");
    assert_eq!(w.jobs.of("g4").len(), 2, "runs_failed is not retried");
    assert_eq!(w.lkg(), w.x, "the incumbent stays LKG");
    assert!(w.sbx.launches().iter().all(|l| l.refused.is_none()));
    w.stop();
}

// ---- MERGE-T2CACHE: the deps cache on the real seed (supervise.md §3.6.2) -------------------------

/// `<repo>/target/test-homes` (canonical). The home is `m2` right under it: G0 runs the seed's own
/// tests, which bind Unix sockets under `<home>/cache/build/slot/out/target/sk/`, and those paths
/// must fit `sun_path` (103 bytes on macOS) from this repo's checkout path.
fn t2_base() -> PathBuf {
    let b = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/test-homes");
    std::fs::create_dir_all(&b).unwrap();
    std::fs::canonicalize(b).unwrap()
}

/// The T2 build stage of a real promote, twice, on the real seed: `promote::stages::build_and_g0`
/// (the call the pipeline and G3's inner gate make) with every build instance launched as the
/// Root's broker launches it (`build` profile) and G0's commands under the `kernel` zone profile.
/// 1. The seed as installed (packaging cut, `scripts/slice1/seedpack.py --kind repo`): no deps
///    entry, so the stub workspace builds in the slot, then the candidate, then G0; G0 passes and
///    the staged entry is published.
/// 2. A no-op comment on `src/main.rs`: a verified hit, so the candidate build and G0 compile only
///    the body (and its tests). Prints both records' timings (the cold and warm G0 build seconds).
#[test]
#[ignore = "two real seed builds and G0s under sandbox-exec (about 15-20 min): INIT_T2_WARM_BENCH=1, --ignored --nocapture"]
fn t2_seed_build_is_warm_on_the_second_promote() {
    // ci/local.sh runs this file with --ignored (A3, 1-2 min); this benchmark is opt-in on top of that.
    if std::env::var("INIT_T2_WARM_BENCH").as_deref() != Ok("1") {
        eprintln!("t2_seed_build_is_warm_on_the_second_promote: skipped (set INIT_T2_WARM_BENCH=1 to run it)");
        return;
    }
    let _wd = init_testkit::watchdog::arm("t2_seed_build_is_warm_on_the_second_promote", Duration::from_secs(5400));
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(3).unwrap().to_path_buf();
    let home = t2_base().join("m2");
    if home.exists() {
        std::fs::remove_dir_all(&home).unwrap();
    }
    for d in ["kernel/bin", "state/kernel/gate", "vault/gate/jobs", "lineage", "cache"] {
        std::fs::create_dir_all(home.join(d)).unwrap();
    }
    let mock = std::env::var_os("INIT_MOCKLLM_BIN").map(PathBuf::from).unwrap_or_else(|| repo.join("kernel/target/debug/init-mockllm"));
    assert!(mock.is_file(), "{} is missing: build init-mockllm first", mock.display());
    std::fs::copy(&mock, home.join("kernel/bin/init-mockllm")).unwrap();
    // The install's packaging cut, committed as the agent, as lineage `main`.
    let cut = home.join("cache/seedcut");
    line(Command::new("/usr/bin/python3").arg("-I").arg(repo.join("scripts/slice1/seedpack.py")).arg("--repo").arg(&repo).arg("--dst").arg(&cut).args(["--kind", "repo", "--rev", "worktree"]));
    ag(&cut, &home, &["init", "-q", "-b", "main"]);
    ag(&cut, &home, &["add", "-A"]);
    ag(&cut, &home, &["commit", "-q", "-m", "seed"]);
    let c1 = ag(&cut, &home, &["rev-parse", "HEAD"]);
    ag(&home.join("lineage"), &home, &["clone", "-q", "--bare", cut.to_str().unwrap(), "body.git"]);
    let p = paths();
    let zone = Zone::new(&home, &p);
    let sbx = SeatbeltSpawner::new(zone.clone());
    let mut cfg = kernel_supervise::config::defaults();
    cfg.sandbox = sandbox_cfg(&p);
    let sdk = line(Command::new("/usr/bin/xcrun").arg("--show-sdk-path"));
    cfg.sandbox.sdkroot = Some(PathBuf::from(sdk));
    let env = Env {
        home: home.clone(),
        cfg,
        gate: GateCfg::default(),
        spawner: sbx.clone(),
        g0: Arc::new(KernelZoneG0 { argv: zone.kernel_argv(), n: Mutex::new(0) }),
        proxy: Arc::new(promote::NoGateProxy),
        lineage: Arc::new(NoLineage),
        sup: Arc::new(NoSup),
        jobs: Arc::new(MemJobs::default()),
        upload_pack: "git-upload-pack".into(),
        level: 0,
        kernel: KernelInfo::default(),
        model: "deepseek-flash".into(),
        pipeline_busy: AtomicBool::new(false),
        g4_running: Mutex::new(None),
        gates: Default::default(),
        g4_attempts: Mutex::new(Default::default()),
        fault: None,
        slot_lock: Mutex::new(()),
        sealed: None,
        extend: None,
    };
    let run = |job: &str, sha: &str| -> Value {
        let t = Instant::now();
        let r = promote::stages::build_and_g0(&env, job, sha, &c1, &home.join("cache/gate").join(job), false);
        let wall = t.elapsed().as_secs_f64();
        let rec = match r {
            Ok(b) => b.record,
            Err(f) => panic!("{job}: {} {}\n{}", f.code, f.detail, f.record),
        };
        let s = |ms: &Value| ms.as_u64().unwrap_or(0) as f64 / 1000.0;
        eprintln!(
            "merge-t2 {job}: deps {} (prep {:.1} s), candidate build {:.1} s, build stage {:.1} s, G0 {:.1} s, total {wall:.1} s; steps {}",
            rec["deps"]["mode"], s(&rec["deps"]["ms"]), s(&rec["deps"]["build_ms"]), s(&rec["build_ms"]), s(&rec["wall_ms"]), rec["steps"]
        );
        rec
    };
    let r1 = run("j-000001", &c1);
    assert_eq!((r1["deps"]["mode"].as_str(), r1["deps"]["published"].as_bool()), (Some("stub"), Some(true)), "{r1}");
    // A second promote of a small change.
    let main_rs = cut.join("src/main.rs");
    let mut text = std::fs::read_to_string(&main_rs).unwrap();
    text.push_str("// merge-t2: a no-op comment\n");
    std::fs::write(&main_rs, text).unwrap();
    ag(&cut, &home, &["commit", "-q", "-am", "refactor: a no-op comment"]);
    let c2 = ag(&cut, &home, &["rev-parse", "HEAD"]);
    ag(&home.join("lineage/body.git"), &home, &["fetch", "-q", cut.to_str().unwrap(), "+refs/heads/main:refs/agent/main"]);
    let r2 = run("j-000002", &c2);
    assert_eq!((r2["deps"]["mode"].as_str(), r2["cold_target"].as_bool()), (Some("hit"), Some(false)), "{r2}");
    // Only the body compiles: no vendored crate (wasmtime's build script tracks SOURCE_DATE_EPOCH).
    let log2 = std::fs::read_to_string(home.join("state/kernel/gate/j-000002/build.log")).unwrap();
    let compiled: Vec<&str> = log2.lines().filter(|l| l.trim_start().starts_with("Compiling")).collect();
    assert!(compiled.len() == 1 && compiled[0].contains("init-body"), "{compiled:?}");
    let refused: Vec<Launch> = sbx.launches().into_iter().filter(|l| l.refused.is_some()).collect();
    assert!(refused.is_empty(), "the Root would refuse: {refused:?}");
    assert!(
        r2["build_ms"].as_u64().unwrap() * 3 < r1["build_ms"].as_u64().unwrap(),
        "the warm build stage is a fraction of the cold one: {} ms vs {} ms",
        r2["build_ms"],
        r1["build_ms"]
    );
    std::fs::remove_dir_all(&home).unwrap();
}
