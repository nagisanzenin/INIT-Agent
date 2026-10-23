//! The seed passes its own G0 (lane G0-SEED; kernel/gate.md §3.3). $0: no model, no network.
//!
//! S-REAL found on a real install (2026-10-20) that G0's `test` step failed the seed, so no T2
//! promotion could pass. Every piece of that failure was outside any test the repo ran: the body
//! checkout's layout, G0's environment and the sandbox. This check runs exactly those:
//!
//! 1. **The body checkout.** The current seed tree is cut the way install packaging cuts it
//!    (`scripts/slice1/seedpack.py --kind repo`: `seed/`, its own `vendor/` and a
//!    `.cargo/config.toml` naming `vendor`) into `ih/cache/build/slot/src` of a stand-in INIT_HOME.
//!    `ih/kernel/bin/init-mockllm` is a copy of the kernel workspace's mock, as the install ships it.
//! 2. **The build stage** (supervise.md §3.6.2): `BUILD_ARGS` with the build slot's environment, in
//!    the `build` profile, as the Root's broker runs it.
//! 3. **G0 itself:** the real runner, `kernel_gate::g0::run` (the call `promote::stages` makes),
//!    with every command under `/usr/bin/sandbox-exec` and the real profile texts read from
//!    `root/sandbox-profiles/profiles/`, composed the way the Root composes them
//!    (PRELUDE + SECTION + ROOT_ALLOWS + ZONE_TAIL, root/root.md §3.3.1). Profiles:
//!    - `kernel` (the default): where G0 runs, as children of the kernel inside its own zone
//!      profile (`init-kernel/src/serve.rs`, WP8b deviation 1: G0's spawner is `DirectSpawner`;
//!      section 6a lets them use Unix sockets under the slot's OUT, G0-ROOT d05c1e3);
//!    - `build` (opt-in): gate.md §3.3's text (SRC read-only, OUT the only writable tree), for a
//!      broker-side G0 runner that does not exist yet.
//! 4. **G1** (lane SELFJSON; gate.md §3.4): the seed ships its committed `self.json` (`seed/self.json`,
//!    `body/self.json` once installed). S-REAL found that without it `initctl gate --quick` on an
//!    unchanged, freshly installed body fails `gate.g1.self_json_missing`. This step runs G1 as
//!    `promote::stages::g1` does: the shipped `ih/kernel/bin/init-kernel selfmap --root <export>
//!    --binary … --components … --out …` in the `build` profile (`SRC` = the export, `OUT` =
//!    `ih/cache/gate/seed-g1`), then `g1::compare_self_json` against the export's `self.json` and
//!    `g1::check_wit_copy` against the repo's `wit/`. A seed change that does not regenerate
//!    `seed/self.json` fails here with the command that regenerates it ([`REGEN`]).
//!
//! **Regenerating `seed/self.json`** (`INIT_SEED_G1_BLESS=1`): after the build stage, the cut is made
//! a git checkout (`git init`, as the installed `body/` is one) and the body's own T2 procedure runs
//! on it, `init-kernel selfmap --root <checkout> --write` (selfmap.md §3.1 write mode, G0's
//! environment, the warm target). The result is copied to `seed/self.json`, and G1 then checks it in
//! gate mode, so a bless run also proves that write mode and gate mode agree. A bless run skips G0
//! unless `INIT_SEED_G0_PROFILES` is set.
//!
//! The target dir `ih/cache/build/slot/out/target` is kept between runs (warm; about 1.3 GB). The
//! seed's own crates rebuild whenever their sources change, and the tests always run.
//! `INIT_SEED_G0_COLD=1` removes it first, as the production slot does on every build.
//!
//! Run: `cargo test --offline -p kernel-gate --test seed_g0 -- --ignored` (ci/local.sh step
//! "seed G0"). Knobs: `INIT_SEED_G0_DIR` (default `seed/target/g0`), `INIT_SEED_G0_REV` (a git rev
//! for seedpack, default `worktree`), `INIT_SEED_G0_PROFILES` (default `kernel`; `kernel,build`),
//! `INIT_SEED_G0_COLD=1`, `INIT_MOCKLLM_BIN` (default `kernel/target/debug/init-mockllm`),
//! `INIT_KERNEL_BIN` (default `kernel/target/debug/init-kernel`), `INIT_SEED_G1_BLESS=1`.
//! `CARGO_BUILD_JOBS` / `RUST_TEST_THREADS`, when set (ci/env.sh), are added to G0's environment.
//! Logs: `ih/vault/gate/jobs/seed-g0-<profile>/g0/<step>.log`, `ih/state/kernel/gate/build.log`,
//! `ih/state/kernel/gate/{bless,g1-ext}.log` and `ih/cache/gate/seed-g1/g1/selfmap.log`.

#![forbid(unsafe_code)]
// It composes the macOS Seatbelt profiles (sandbox-exec); on Linux this test crate is empty.
#![cfg(target_os = "macos")]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use kernel_gate::g0::{self, EnvSpec, G0Report, G0Spec, SpawnRequest, Spawner};

/// supervise.md §3.6.2's build command (kernel-supervise `build::BUILD_ARGS`).
const BUILD_ARGS: [&str; 10] = [
    "build",
    "--frozen",
    "--bin",
    "init-body",
    "--profile",
    "dev",
    "--config",
    "source.crates-io.replace-with=\"vendored-sources\"",
    "--config",
    "source.vendored-sources.directory=\"vendor\"",
];
/// The build stage's bound: production's cold limit is 900 s, but it starts from a warm deps cache;
/// the first run here is fully cold under a loaded machine.
const BUILD_TIMEOUT: Duration = Duration::from_secs(1800);
/// The watchdog bound (K-FLAKE): a cold build plus two G0 runs (`kernel,build`) at their 900 s budget,
/// plus G1 (its 300 s bound and the ext build).
const WD: Duration = Duration::from_secs(4800);
/// G1's bound on the selfmap child (`promote::stages::g1`).
const G1_TIMEOUT: Duration = Duration::from_secs(300);
/// The command that regenerates `seed/self.json` (from the repo root).
const REGEN: &str = "source ci/env.sh && (cd kernel && INIT_SEED_G1_BLESS=1 ../ci/slot.sh cargo test --offline \
-p kernel-gate --test seed_g0 -- --ignored --nocapture), then commit seed/self.json";

fn repo() -> PathBuf {
    let m = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    m.ancestors().nth(3).expect("repo root").to_path_buf()
}

fn line(cmd: &mut Command) -> String {
    let out = init_testkit::watchdog::output(cmd).unwrap_or_else(|e| panic!("{cmd:?}: {e}"));
    assert!(
        out.status.success(),
        "{cmd:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// Remove `p` (a directory or file) by its exact path, if present.
fn remove(p: &Path) {
    match fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => {
            fs::remove_dir_all(p).unwrap_or_else(|e| panic!("rm {}: {e}", p.display()))
        }
        Ok(_) => fs::remove_file(p).unwrap_or_else(|e| panic!("rm {}: {e}", p.display())),
        Err(_) => {}
    }
}

/// `<crate dir> -> .cargo-checksum.json "package"` (the sha256 of the published crate) of a vendor tree.
fn vendor_ids(dir: &Path) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let ck = fs::read(e.path().join(".cargo-checksum.json")).unwrap_or_default();
        let pkg = serde_json::from_slice::<serde_json::Value>(&ck)
            .ok()
            .and_then(|v| v["package"].as_str().map(str::to_string))
            .unwrap_or_default();
        m.insert(e.file_name().to_string_lossy().into_owned(), pkg);
    }
    m
}

/// Put the previous run's `vendor/` back when the fresh cut names exactly the same crates
/// (directory names and package checksums equal). A fresh cut gives every directory a new mtime, and
/// build scripts with a directory `rerun-if-changed` (cranelift-isle) then rebuild cranelift and
/// wasmtime on every run (about 3 min); file contents and file mtimes are clones either way.
fn reuse_vendor(fresh: &Path, kept: &Path) -> &'static str {
    if !kept.is_dir() {
        return "vendor: fresh cut";
    }
    if vendor_ids(fresh) != vendor_ids(kept) {
        remove(kept);
        return "vendor: changed, fresh cut";
    }
    remove(fresh);
    fs::rename(kept, fresh).unwrap();
    "vendor: same crates, previous tree kept"
}

/// The stand-in INIT_HOME and the Root-side paths the profiles name.
struct Layout {
    dir: PathBuf,
    ih: PathBuf,
    toolchain: PathBuf,
    devdir: Option<PathBuf>,
    sdkroot: Option<PathBuf>,
    home_real: PathBuf,
}

impl Layout {
    fn src(&self) -> PathBuf {
        self.ih.join("cache/build/slot/src")
    }
    fn out(&self) -> PathBuf {
        self.ih.join("cache/build/slot/out")
    }
    /// G1's `OUT` (`cache/gate/<job>/`, which holds the candidate binary and components).
    fn g1_out(&self) -> PathBuf {
        self.ih.join("cache/gate/seed-g1")
    }
    /// The Root's home: never created; the zone tail denies it (Z1).
    fn root_home(&self) -> PathBuf {
        self.dir.join("rh")
    }
    fn mark(&self) -> PathBuf {
        self.ih.join("run/mark/5eedc0ffee00")
    }

    /// The parameters any of the composed texts may reference (root/root.md §3.3.2,
    /// kernel-supervise `spawnspec::build_params`, sandbox-profiles `kernel_profile_params`).
    fn params(&self) -> BTreeMap<String, String> {
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let i = &self.ih;
        let rh = self.root_home();
        [
            ("TOOLCHAIN", self.toolchain.clone()),
            ("DEVDIR", self.devdir.clone().unwrap_or_default()),
            ("HOME_REAL", self.home_real.clone()),
            ("INIT_HOME", i.clone()),
            // build / check
            ("KERNEL", i.join("kernel")),
            ("SRC", self.src()),
            ("OUT", self.out()),
            ("VAULT", i.join("vault")),
            ("MARK", self.mark()),
            // kernel
            ("RUN", i.join("run")),
            ("PROBE", i.join("probe")),
            ("MARKS", i.join("run/mark")),
            ("EGRESS_SOCK", rh.join("run/egress.sock")),
            ("SLOT_OUT", self.out()),
            // ROOT_ALLOWS and the zone tail
            ("Z_ROOT_HOME", rh.clone()),
            ("Z_ROOT_SVC", self.dir.join("dev.init.root.plist")),
            ("Z_ROOT_SVC_DIR", rh.join("LaunchAgents")), // write-denied (Z2); never INIT_HOME's parent
            ("Z_WRITE", i.clone()), // ADR-009: no write grant, so INIT_HOME (as the Root sets it)
            ("Z_INIT_HOME", i.clone()),
            ("Z_RUN", i.join("run")),
            ("Z_KERNEL", i.join("kernel")),
            ("Z_PUB", i.join("root")),
            ("Z_KGIT", i.join("lineage/kernel.git")),
            ("Z_MARKS", i.join("run/mark")),
            ("Z_MARK", self.mark()),
            ("Z_ZONE_MARK", i.join("run/zone-mark")),
            ("Z_EGRESS_SOCK", rh.join("run/egress.sock")),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), s(&v)))
        .collect()
    }
}

/// Every `(param "NAME")` a text references, ignoring `;` comments (sandbox-profiles
/// `seatbelt::referenced_params`).
fn referenced(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for l in text.lines() {
        let mut rest = l.split(';').next().unwrap_or("");
        while let Some(at) = rest.find("(param \"") {
            rest = &rest[at + 8..];
            if let Some(end) = rest.find('"') {
                out.insert(rest[..end].to_string());
                rest = &rest[end..];
            }
        }
    }
    out
}

/// One composed profile and its `-D` parameters: PRELUDE + SECTION(id) + ROOT_ALLOWS (no
/// `extra_read`) + ZONE_TAIL, from the texts `init-root` compiles in (`seatbelt::render`).
struct Sandbox {
    name: String,
    text: String,
    params: Vec<(String, String)>,
}

impl Sandbox {
    fn new(name: &str, l: &Layout) -> Sandbox {
        Sandbox::with_out(name, l, &l.out())
    }

    /// The same profile with `OUT` = `out` (G1's `OUT` is the job dir, not the build slot's).
    fn with_out(name: &str, l: &Layout, out: &Path) -> Sandbox {
        let dir = repo().join("root/sandbox-profiles/profiles");
        let read = |f: &str| fs::read_to_string(dir.join(f)).unwrap_or_else(|e| panic!("{f}: {e}"));
        let read_tail = read("zone-tail.sb");
        let section = match name {
            "kernel" => "kernel.sb",
            "build" => "build.sb",
            other => panic!("INIT_SEED_G0_PROFILES: unknown profile {other:?} (kernel, build)"),
        };
        let allows: String = read("root-allows.template")
            .lines()
            .take(2)
            .map(|x| format!("{x}\n"))
            .collect();
        // The tail with no ADR-009 deny entry: each per-entry group (its comment lines and its `<i>` line) left
        // out, as the Root renders it for empty lists (sandbox-profiles `seatbelt::zone_tail(0, 0)`).
        let lines: Vec<&str> = read_tail.lines().collect();
        let mut keep = vec![true; lines.len()];
        for (n, x) in lines.iter().enumerate() {
            if x.contains("<i>") {
                keep[n] = false;
                let mut k = n;
                while k > 0 && lines[k - 1].starts_with(';') {
                    k -= 1;
                    keep[k] = false;
                }
            }
        }
        let tail: String = lines.iter().zip(&keep).filter(|(_, k)| **k).map(|(x, _)| format!("{x}\n")).collect();
        let text = [read("prelude.sb"), read(section), allows, tail].concat();
        let mut all = l.params();
        all.insert("OUT".into(), out.to_string_lossy().into_owned());
        let params = referenced(&text)
            .into_iter()
            .map(|k| {
                let v = all.get(&k).unwrap_or_else(|| {
                    panic!("profile {name} uses parameter {k}, unknown to this check")
                });
                (k, v.clone())
            })
            .collect();
        Sandbox {
            name: name.to_string(),
            text,
            params,
        }
    }

    fn command(&self, program: &Path) -> Command {
        let mut c = Command::new("/usr/bin/sandbox-exec");
        c.arg("-p").arg(&self.text);
        for (k, v) in &self.params {
            c.arg(format!("-D{k}={v}"));
        }
        c.arg("--").arg(program);
        c
    }
}

/// The caller's CPU caps (`ci/env.sh`: `CARGO_BUILD_JOBS`, `RUST_TEST_THREADS`), added to G0's
/// exact environment when set, so the check keeps the lanes' CPU budget. They change parallelism
/// only, never what is built or which tests run; unset (a plain `ci/local.sh`), G0 runs as in
/// production.
fn cpu_caps() -> Vec<(String, std::ffi::OsString)> {
    ["CARGO_BUILD_JOBS", "RUST_TEST_THREADS"]
        .into_iter()
        .filter_map(|k| std::env::var_os(k).map(|v| (k.to_string(), v)))
        .collect()
}

impl Spawner for Sandbox {
    fn spawn(&self, req: SpawnRequest) -> io::Result<Child> {
        let mut c = self.command(&req.program);
        c.args(&req.args)
            .env_clear()
            .envs(req.env.iter().map(|(k, v)| (k, v)))
            .envs(cpu_caps())
            .current_dir(&req.cwd)
            .stdin(Stdio::null())
            .stdout(req.stdout)
            .stderr(req.stderr)
            .process_group(0);
        // sandbox-exec applies the profile and execs cargo in the same process: this is the child
        // whose death ends the command, and the watchdog's handle on its group.
        let child = c.spawn()?;
        init_testkit::watchdog::track(child.id());
        Ok(child)
    }
}

fn setup() -> Layout {
    let repo = repo();
    let dir = std::env::var_os("INIT_SEED_G0_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo.join("seed/target/g0"));
    assert!(dir.is_absolute(), "INIT_SEED_G0_DIR must be absolute");
    let ih = dir.join("ih");
    let sysroot = line(Command::new("rustc").args(["--print", "sysroot"]));
    let (devdir, sdkroot) = if cfg!(target_os = "macos") {
        (
            Some(PathBuf::from(line(Command::new("xcode-select").arg("-p")))),
            Some(PathBuf::from(line(
                Command::new("xcrun").arg("--show-sdk-path"),
            ))),
        )
    } else {
        (None, None)
    };
    let l = Layout {
        dir,
        ih,
        toolchain: PathBuf::from(sysroot),
        devdir,
        sdkroot,
        home_real: PathBuf::from(std::env::var_os("HOME").expect("HOME")),
    };
    // The export: fresh every run, but the previous run's vendor/ is set aside (see `reuse_vendor`).
    // The slot's out/: fresh but for the warm target dir.
    let kept = l.dir.join("vendor.prev");
    remove(&kept);
    if l.src().join("vendor").is_dir() {
        fs::rename(l.src().join("vendor"), &kept).unwrap();
    }
    remove(&l.src());
    for d in ["home", "tmp", "cargo-home"] {
        remove(&l.out().join(d));
    }
    if std::env::var("INIT_SEED_G0_COLD").as_deref() == Ok("1") {
        remove(&l.out().join("target"));
    }
    for d in ["target", "home", "tmp", "cargo-home"] {
        fs::create_dir_all(l.out().join(d)).unwrap();
    }
    for d in [
        "kernel/bin",
        "run/mark",
        "probe",
        "state/kernel/gate",
        "vault/gate/jobs",
    ] {
        fs::create_dir_all(l.ih.join(d)).unwrap();
    }
    fs::create_dir_all(l.src().parent().unwrap()).unwrap();
    // The install's packaging cut (root.md §4.3 step 7).
    let rev = std::env::var("INIT_SEED_G0_REV").unwrap_or_else(|_| "worktree".into());
    let cut = line(
        Command::new("/usr/bin/python3")
            .arg("-I")
            .arg(repo.join("scripts/slice1/seedpack.py"))
            .arg("--repo")
            .arg(&repo)
            .arg("--dst")
            .arg(l.src())
            .args(["--kind", "repo", "--rev", &rev]),
    );
    eprintln!(
        "seed-g0: seedpack {rev}: {cut}; {}",
        reuse_vendor(&l.src().join("vendor"), &kept)
    );
    // The mock the install ships (gate.md §3.3 INIT_MOCKLLM_BIN = $INIT_HOME/kernel/bin/init-mockllm).
    let mock = std::env::var_os("INIT_MOCKLLM_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo.join("kernel/target/debug/init-mockllm"));
    assert!(
        mock.is_file(),
        "{} is missing: build the kernel workspace first (or set INIT_MOCKLLM_BIN)",
        mock.display()
    );
    let shipped = l.ih.join("kernel/bin/init-mockllm");
    remove(&shipped);
    fs::copy(&mock, &shipped).unwrap();
    // The kernel the install ships (G1 runs `$INIT_HOME/kernel/bin/init-kernel selfmap`). fs::copy
    // clones on APFS, so the 150 MB debug binary costs no disk.
    let kernel = std::env::var_os("INIT_KERNEL_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo.join("kernel/target/debug/init-kernel"));
    assert!(
        kernel.is_file(),
        "{} is missing: build the kernel workspace first (or set INIT_KERNEL_BIN)",
        kernel.display()
    );
    let shipped = l.ih.join("kernel/bin/init-kernel");
    remove(&shipped);
    fs::copy(&kernel, &shipped).unwrap();
    remove(&l.g1_out());
    l
}

/// A child under `sbx` (or none) with exactly `env` plus the CPU caps, output to `log_path`, bounded
/// by `timeout`; Ok(wall ms) on exit 0, else the log's tail.
fn run_logged(
    sbx: Option<&Sandbox>,
    program: &Path,
    args: &[String],
    env: &[(String, std::ffi::OsString)],
    cwd: &Path,
    log_path: &Path,
    timeout: Duration,
) -> Result<u64, String> {
    let log = fs::File::create(log_path).unwrap();
    let mut c = match sbx {
        Some(s) => s.command(program),
        None => Command::new(program),
    };
    c.args(args)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .envs(cpu_caps())
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .process_group(0);
    let t0 = Instant::now();
    let mut child = c
        .spawn()
        .map_err(|e| format!("spawn {}: {e}", program.display()))?;
    init_testkit::watchdog::track(child.id());
    let st = loop {
        if let Some(st) = child.try_wait().map_err(|e| e.to_string())? {
            break st;
        }
        if t0.elapsed() > timeout {
            let _ = child.kill(); // our own child, by its handle
            let _ = child.wait();
            return Err(format!(
                "no result within {} s ({})",
                timeout.as_secs(),
                log_path.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    if st.success() {
        Ok(t0.elapsed().as_millis() as u64)
    } else {
        let t = fs::read_to_string(log_path).unwrap_or_default();
        Err(format!(
            "{st} ({})\n{}",
            log_path.display(),
            &t[t.floor_char_boundary(t.len().saturating_sub(3000))..]
        ))
    }
}

/// G0's exact environment (`g0::env`) for this layout, with a fresh HOME, TMPDIR and CARGO_HOME.
fn g0_env(l: &Layout, epoch: u64) -> EnvSpec {
    for d in ["home", "tmp", "cargo-home"] {
        remove(&l.out().join(d));
        fs::create_dir_all(l.out().join(d)).unwrap();
    }
    EnvSpec {
        toolchain_bin: l.toolchain.join("bin"),
        home: l.out().join("home"),
        tmpdir: l.out().join("tmp"),
        cargo_home: l.out().join("cargo-home"),
        target_dir: l.out().join("target"),
        source_date_epoch: epoch,
        init_home: l.ih.clone(),
        sdkroot: l.sdkroot.clone(),
        developer_dir: l.devdir.clone().filter(|d| d.is_dir()),
    }
}

/// `INIT_SEED_G1_BLESS=1`: the body's T2 procedure on the cut, `init-kernel selfmap --root <checkout>
/// --write` (selfmap.md §3.1), in G0's environment and the warm target; the cut is made a git
/// checkout first, as the installed `body/` is (write mode lists files with `git ls-files`). The
/// result goes to the export (for G1) and to the repo's `seed/self.json`; the `.git` goes again.
fn bless(l: &Layout, epoch: u64) -> Result<(), String> {
    let src = l.src();
    let mut env = g0::env(&g0_env(l, epoch));
    // git reads no system or global config, so `--exclude-standard` means the tree's own ignores only.
    env.push(("GIT_CONFIG_NOSYSTEM".into(), "1".into()));
    env.push(("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()));
    let log = l.ih.join("state/kernel/gate/bless.log");
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let git = Path::new("/usr/bin/git");
    run_logged(None, git, &s(&["init", "-q"]), &env, &src, &log, G1_TIMEOUT)
        .map_err(|e| format!("git init: {e}"))?;
    let root = src.to_string_lossy().into_owned();
    let r = run_logged(
        None,
        &l.ih.join("kernel/bin/init-kernel"),
        &s(&["selfmap", "--root", &root, "--write"]),
        &env,
        &src,
        &log,
        BUILD_TIMEOUT,
    );
    remove(&src.join(".git"));
    let ms = r.map_err(|e| format!("init-kernel selfmap --write: {e}"))?;
    let dst = repo().join("seed/self.json");
    fs::copy(src.join("self.json"), &dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    eprintln!("seed-g1: blessed {} in {ms} ms", dst.display());
    Ok(())
}

/// G1 as `promote::stages::g1` runs it, on the export and what the build and G0 left in the target.
/// Returns the warnings, or `<code>: <detail>`.
fn g1(l: &Layout, epoch: u64) -> Result<Vec<String>, String> {
    let src = l.src();
    let out = l.g1_out();
    let target = l.out().join("target");
    remove(&out);
    for d in ["bin", "ext", "g1/home", "g1/tmp"] {
        fs::create_dir_all(out.join(d)).unwrap();
    }
    // The components: G0's `ext` step builds them (gate.md §3.3); built again here in G0's
    // environment, a no-op after G0, so that G1 also runs when G0 was skipped.
    let mut names = Vec::new();
    if src.join("ext/Cargo.toml").is_file() {
        let env = g0::env(&g0_env(l, epoch));
        let args: Vec<String> = [
            "build",
            "--workspace",
            "--frozen",
            "--target",
            "wasm32-wasip2",
        ]
        .map(String::from)
        .to_vec();
        run_logged(
            Some(&Sandbox::new("build", l)),
            &l.toolchain.join("bin/cargo"),
            &args,
            &env,
            &src.join("ext"),
            &l.ih.join("state/kernel/gate/g1-ext.log"),
            BUILD_TIMEOUT,
        )
        .map_err(|e| format!("gate.g1.selfmap_error: ext build: {e}"))?;
        for e in fs::read_dir(src.join("ext")).unwrap().flatten() {
            if e.path().join("Cargo.toml").is_file() {
                names.push(e.file_name().to_string_lossy().into_owned());
            }
        }
    }
    // The copies `promote::stages` makes after G0 (APFS clones).
    fs::copy(target.join("debug/init-body"), out.join("bin/init-body"))
        .map_err(|e| format!("gate.g1.selfmap_error: copy init-body: {e}"))?;
    for n in &names {
        let w = target.join(format!("wasm32-wasip2/debug/{}.wasm", n.replace('-', "_")));
        fs::copy(&w, out.join(format!("ext/{n}.wasm")))
            .map_err(|e| format!("gate.g1.selfmap_error: copy {}: {e}", w.display()))?;
    }
    let s = |p: PathBuf| p.to_string_lossy().into_owned();
    let path = format!("{}:/usr/bin:/bin", l.toolchain.join("bin").display());
    let env: Vec<(String, std::ffi::OsString)> = vec![
        ("PATH".into(), path.into()),
        ("HOME".into(), out.join("g1/home").into_os_string()),
        (
            "TMPDIR".into(),
            format!("{}/", out.join("g1/tmp").display()).into(),
        ),
        ("LANG".into(), "C".into()),
    ];
    let args = vec![
        "selfmap".to_string(),
        "--root".into(),
        s(src.clone()),
        "--binary".into(),
        s(out.join("bin/init-body")),
        "--components".into(),
        s(out.join("ext")),
        "--out".into(),
        s(out.join("self.json")),
    ];
    let log = out.join("g1/selfmap.log");
    let sbx = Sandbox::with_out("build", l, &out);
    if let Err(e) = run_logged(
        Some(&sbx),
        &l.ih.join("kernel/bin/init-kernel"),
        &args,
        &env,
        &out.join("g1"),
        &log,
        G1_TIMEOUT,
    ) {
        // gate.md §4.1: the first diagnostic's code names the failure.
        let code = fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .filter_map(|x| serde_json::from_str::<serde_json::Value>(x).ok())
            .find_map(|d| d["code"].as_str().map(str::to_string))
            .unwrap_or_else(|| "selfmap_error".into());
        return Err(format!("gate.g1.{code}: selfmap {e}"));
    }
    let regenerated = fs::read(out.join("self.json"))
        .map_err(|e| format!("gate.g1.selfmap_error: no output: {e}"))?;
    let committed = fs::read(src.join("self.json")).ok();
    let warnings = kernel_gate::g1::compare_self_json(committed.as_deref(), &regenerated)
        .map_err(|f| format!("{}: {}", f.code, f.detail))?;
    kernel_gate::g1::check_wit_copy(&src, &repo().join("wit"))
        .map_err(|f| format!("{}: {}", f.code, f.detail))?;
    Ok(warnings.iter().map(|w| format!("{w:?}")).collect())
}

/// The build stage: `BUILD_ARGS` in the `build` profile with the slot's env (kernel-supervise
/// `BuildSlot::build_request`), output to `ih/state/kernel/gate/build.log`.
fn build_stage(l: &Layout, sbx: &Sandbox, epoch: u64) -> Result<u64, String> {
    let s = |p: PathBuf| p.into_os_string();
    let tool = l.toolchain.join("bin");
    let mut path = tool.clone().into_os_string();
    if let Some(d) = &l.devdir {
        path.push(":");
        path.push(d.join("usr/bin"));
    }
    path.push(":/usr/bin:/bin");
    let log_path = l.ih.join("state/kernel/gate/build.log");
    let log = fs::File::create(&log_path).unwrap();
    let mut c = sbx.command(&tool.join("cargo"));
    c.args(BUILD_ARGS)
        .env_clear()
        .env("PATH", path)
        .env("HOME", s(l.out().join("home")))
        .env("TMPDIR", format!("{}/", l.out().join("tmp").display()))
        .env("CARGO_HOME", s(l.out().join("cargo-home")))
        .env("CARGO_TARGET_DIR", s(l.out().join("target")))
        .env("CARGO_NET_OFFLINE", "true")
        .env("CARGO_INCREMENTAL", "0")
        .env("SOURCE_DATE_EPOCH", epoch.to_string())
        .envs(cpu_caps())
        .current_dir(l.src())
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .process_group(0);
    if let Some(sdk) = &l.sdkroot {
        c.env("SDKROOT", sdk);
    }
    if let Some(d) = &l.devdir {
        c.env("DEVELOPER_DIR", d);
    }
    let t0 = Instant::now();
    let mut child = c.spawn().map_err(|e| format!("spawn: {e}"))?;
    init_testkit::watchdog::track(child.id());
    let st = loop {
        if let Some(st) = child.try_wait().map_err(|e| e.to_string())? {
            break st;
        }
        if t0.elapsed() > BUILD_TIMEOUT {
            let _ = child.kill(); // our own child, by its handle
            let _ = child.wait();
            return Err(format!(
                "build: no result within {} s ({})",
                BUILD_TIMEOUT.as_secs(),
                log_path.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let ms = t0.elapsed().as_millis() as u64;
    if st.success() && l.out().join("target/debug/init-body").is_file() {
        Ok(ms)
    } else {
        let t = fs::read_to_string(&log_path).unwrap_or_default();
        Err(format!(
            "build: {st} ({})\n{}",
            log_path.display(),
            &t[t.len().saturating_sub(3000)..]
        ))
    }
}

/// The failing tests and their panics, from a G0 `test.log`.
fn failures(log: &str) -> Vec<String> {
    log.lines()
        .filter(|x| {
            (x.starts_with("test ") && x.ends_with("FAILED"))
                || x.contains("panicked at")
                || x.starts_with("error: ")
                || x.starts_with("     Running ")
        })
        .map(str::to_string)
        .collect()
}

fn g0_under(l: &Layout, sbx: &Sandbox, epoch: u64) -> G0Report {
    // A fresh HOME, TMPDIR and CARGO_HOME per G0, as after a slot reset; the target stays.
    let env = g0_env(l, epoch);
    let log_dir =
        l.ih.join(format!("vault/gate/jobs/seed-g0-{}/g0", sbx.name));
    remove(&log_dir);
    let spec = G0Spec::new(l.src(), env, log_dir);
    g0::run(&spec, sbx)
}

#[test]
#[ignore = "G0 and G1 on the whole seed (about 1.3 GB warm target, minutes): ci/local.sh runs it with --ignored"]
fn seed_passes_its_own_g0() {
    let _wd = init_testkit::watchdog::arm("seed_passes_its_own_g0", WD);
    let l = setup();
    let epoch: u64 =
        line(
            Command::new("git")
                .arg("-C")
                .arg(repo())
                .args(["log", "-1", "--format=%ct"]),
        )
        .parse()
        .unwrap();
    let build = Sandbox::new("build", &l);
    let ms = build_stage(&l, &build, epoch).unwrap_or_else(|e| panic!("seed-g0: {e}"));
    eprintln!("seed-g0: build stage ok in {ms} ms");
    let bless_run = std::env::var("INIT_SEED_G1_BLESS").as_deref() == Ok("1");
    if bless_run {
        bless(&l, epoch).unwrap_or_else(|e| panic!("seed-g1: bless: {e}"));
    }

    // A bless run skips G0 unless the profiles are named.
    let names = std::env::var("INIT_SEED_G0_PROFILES").unwrap_or_else(|_| {
        if bless_run {
            String::new()
        } else {
            "kernel".into()
        }
    });
    let mut red = Vec::new();
    for name in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        let sbx = Sandbox::new(name, &l);
        let r = g0_under(&l, &sbx, epoch);
        let steps: Vec<String> = r
            .steps
            .iter()
            .map(|s| {
                format!(
                    "{}={}{} {}ms",
                    s.name,
                    if s.pass { "ok" } else { "FAIL" },
                    if s.timed_out { "(timeout)" } else { "" },
                    s.wall_ms
                )
            })
            .collect();
        eprintln!(
            "seed-g0 [{name}]: pass={} code={:?} wall={} ms; {}",
            r.pass,
            r.code,
            r.wall_ms,
            steps.join(", ")
        );
        if !r.pass {
            let dir = l.ih.join(format!("vault/gate/jobs/seed-g0-{name}/g0"));
            let step = r.step.unwrap_or("?");
            let log = fs::read_to_string(dir.join(format!("{step}.log"))).unwrap_or_default();
            let lines = if step == "test" {
                failures(&log)
            } else {
                vec![r.diagnostics.clone()]
            };
            eprintln!(
                "seed-g0 [{name}]: step `{step}` failed; log {}\n{}",
                dir.join(format!("{step}.log")).display(),
                lines.join("\n")
            );
            red.push(format!("G0 {name}: {:?} at `{step}`", r.code));
        }
    }
    // G1 (gate.md §3.4) on the same export, after G0 as in promote. It runs even when G0 failed, so
    // a stale seed/self.json is reported in the same run.
    match g1(&l, epoch) {
        Ok(w) => eprintln!("seed-g1: pass; warnings {w:?}"),
        Err(e) => {
            eprintln!("seed-g1: FAIL {e}\nregenerate seed/self.json: {REGEN}");
            let code = e.split(':').next().unwrap_or("?").to_string();
            red.push(format!("G1: {code} (regenerate: {REGEN})"));
        }
    }
    assert!(
        red.is_empty(),
        "the seed fails its own gate: {}",
        red.join("; ")
    );
    // Passed: the per-run dirs go. The warm target, the logs and the export (APFS clones, next to no
    // disk; its vendor/ is the next run's) stay.
    for d in ["home", "tmp", "cargo-home"] {
        remove(&l.out().join(d));
    }
}
