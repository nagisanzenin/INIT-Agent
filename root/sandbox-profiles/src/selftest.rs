//! The A6 sandbox self-test (kernel/sandbox.md §8), the half that may run on any machine,
//! including the owner's. Lane R exposes it as `init-root doctor --sandbox-selftest`.
//!
//! What it does:
//! - builds a throwaway `INIT_HOME` (`i/`) and `ROOT_HOME` (`r/`) under `base/<run id>/`;
//! - plays the Root without a Root process: it validates each kernel-style parameter map (V1–V5,
//!   class rules), creates marks, composes each profile exactly as root/root.md §3.3.1 does
//!   (PRELUDE + SECTION + ROOT_ALLOWS + ZONE_TAIL) and launches `/usr/bin/sandbox-exec -p … -D… --`;
//! - binds stand-ins for the Root's `owner.sock` and `egress.sock` and the kernel's lane sockets;
//! - inside each instance a probe ([`crate::probe`]) performs one operation per launch and the
//!   runner compares the result with the expectation (deny = `EPERM`/`EACCES`, never `ENOENT`).
//!
//! Live-system safety (CONVENTIONS §1, D1): every process is spawned here and signalled, if at all,
//! only by the PID this runner holds (`Child::kill` after a timeout; `kill -0` targets are this
//! runner's own `sleep`s and instances). Network checks use Unix sockets; the IP checks only create
//! a socket or bind an ephemeral loopback port, so nothing is ever sent. Writes outside the repo
//! (`/private/tmp`, the owner's home) are *policy queries* (`sandbox_check`), never real writes.
//! Sweep and kill rules (S-M40, S-M41, S-M47, S-Z17) are reported as skipped: disposable-env only.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Read};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::ProfileId;
use crate::access::{self, AccessState, Grant};
use crate::params::{ValidationCtx, check_spawn_misc, validate_spawn_params};
use crate::seatbelt::{self, Rendered, ZoneLayout, create_mark, new_instance_id, path_str};
use crate::sys;

const OP_TIMEOUT: Duration = Duration::from_secs(30);
const BUILD_TIMEOUT: Duration = Duration::from_secs(300);
/// macOS `sun_path` is 104 bytes including the NUL.
const SUN_PATH_MAX: usize = 103;

/// Rows of §8 that are never run on the owner's machine (CONVENTIONS §1, D1).
pub const DISPOSABLE_ONLY: [(&str, &str); 10] = [
    ("S-M40", "kill -0 on a process outside the sandbox that the suite did not spawn"),
    ("S-M41", "kill -0 on pid 1"),
    ("S-M47a", "Root sweep of a setsid child, sibling decoy as exclusion path"),
    ("S-M47b", "Root sweep of a setsid child, Root canary as exclusion path"),
    ("S-Z17a", "SW-1 sweep over an instance and its setsid escapee"),
    ("S-Z17b", "SW-2 unsandboxed same-user process that can read the mark is not killed"),
    ("S-Z17c", "SW-3 permissive foreign sandbox reading mark and canary is not killed"),
    ("S-Z17d", "SW-4 decoy denied the canary but not reading the mark is not killed"),
    ("S-Z17e", "SW-5 full panic and rollback with decoys running"),
    ("S-Z17f", "SW-6 runner guard refuses S-Z17 on the owner's account"),
];

/// Where and with what the suite runs.
#[derive(Clone, Debug)]
pub struct SelftestConfig {
    /// Temporary roots are created under `base/<run id>/` (e.g. `root/target/test-homes/wp6a`).
    pub base: PathBuf,
    /// The binary that implements [`crate::probe::inner_main`]; copied into each instance's readable area.
    pub probe_exe: PathBuf,
    /// Arguments placed before the op, e.g. `["inner"]` or `["sandbox-selftest-inner"]`.
    pub probe_prefix: Vec<String>,
    pub toolchain: PathBuf,
    pub devdir: PathBuf,
    pub sdkroot: PathBuf,
    pub home_real: PathBuf,
    /// The real git binary (supervise.md §3.11.2 default: `$DEVDIR/usr/bin/git`).
    pub git: PathBuf,
    /// Run S-M46 and S-B6 (cargo builds inside the sandbox, a few seconds each).
    pub builds: bool,
    /// Keep `base/<run id>/` even when everything passed.
    pub keep_home: bool,
}

impl SelftestConfig {
    /// Resolve the toolchain (`rustc --print sysroot`), developer dir, SDK and home on this machine.
    pub fn detect(base: PathBuf, probe_exe: PathBuf, probe_prefix: Vec<String>) -> io::Result<Self> {
        let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
        let toolchain = canon(Path::new(&first_line_of(&rustc, &["--print", "sysroot"])?))?;
        let devdir = first_line_of("/usr/bin/xcode-select", &["-p"]).unwrap_or_else(|_| "/Library/Developer/CommandLineTools".into());
        let devdir = canon(Path::new(&devdir))?;
        let sdkroot = canon(Path::new(&first_line_of("/usr/bin/xcrun", &["--show-sdk-path"])?))?;
        let home = std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is not set"))?;
        let home_real = canon(Path::new(&home))?;
        let git = devdir.join("usr/bin/git");
        Ok(SelftestConfig { base, probe_exe, probe_prefix, toolchain, devdir, sdkroot, home_real, git, builds: true, keep_home: false })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Pass,
    Fail(String),
    Skipped(String),
}

#[derive(Clone, Debug)]
pub struct CheckResult {
    pub id: String,
    pub profile: String,
    pub what: String,
    pub status: Status,
}

#[derive(Debug)]
pub struct Report {
    pub results: Vec<CheckResult>,
    pub home: PathBuf,
    pub home_kept: bool,
    pub elapsed_ms: u128,
}

impl Report {
    fn count(&self, f: impl Fn(&Status) -> bool) -> usize {
        self.results.iter().filter(|r| f(&r.status)).count()
    }
    pub fn passed(&self) -> usize {
        self.count(|s| *s == Status::Pass)
    }
    pub fn failed(&self) -> usize {
        self.count(|s| matches!(s, Status::Fail(_)))
    }
    pub fn skipped(&self) -> usize {
        self.count(|s| matches!(s, Status::Skipped(_)))
    }
    /// One ICJ-style line per check (sorted keys; serde_json without `preserve_order`).
    pub fn json_lines(&self) -> Vec<String> {
        self.results
            .iter()
            .map(|r| {
                let (status, detail) = match &r.status {
                    Status::Pass => ("pass", String::new()),
                    Status::Fail(d) => ("fail", d.clone()),
                    Status::Skipped(d) => ("skipped", d.clone()),
                };
                json!({"id": r.id, "profile": r.profile, "what": r.what, "status": status, "detail": detail}).to_string()
            })
            .collect()
    }
    pub fn summary(&self) -> String {
        format!(
            "sandbox self-test: {} pass, {} fail, {} skipped, {} checks, {} ms; home {} ({})",
            self.passed(),
            self.failed(),
            self.skipped(),
            self.results.len(),
            self.elapsed_ms,
            self.home.display(),
            if self.home_kept { "kept" } else { "removed" }
        )
    }
}

/// Run the suite. Returns `Err` only when the environment cannot be set up at all.
pub fn run(cfg: &SelftestConfig) -> io::Result<Report> {
    let started = Instant::now();
    if !Path::new(seatbelt::SANDBOX_EXEC).exists() {
        return Err(io::Error::other("/usr/bin/sandbox-exec is missing (Halted{tooling})"));
    }
    let home = setup(cfg)?;
    let mut r = Runner::new(cfg, home);
    r.all();
    let Runner { results, h, listeners, tcp, udp, tcp6, .. } = r;
    drop((listeners, tcp, udp, tcp6));
    let mut report = Report { results, home: h.run.clone(), home_kept: true, elapsed_ms: 0 };
    if report.failed() == 0 && !cfg.keep_home {
        // Deleted by the exact path this run created; remove_dir_all never follows symlinks.
        fs::remove_dir_all(&h.run)?;
        report.home_kept = false;
    }
    report.elapsed_ms = started.elapsed().as_millis();
    Ok(report)
}

// ---------------------------------------------------------------------------------------------
// Layout

struct Home {
    run: PathBuf,
    i: PathBuf,
    r: PathBuf,
    svc: PathBuf,
    outside: PathBuf,
    extra: PathBuf,
    extra2: PathBuf,
}

impl Home {
    fn i(&self, rel: &str) -> PathBuf {
        self.i.join(rel)
    }
    fn r(&self, rel: &str) -> PathBuf {
        self.r.join(rel)
    }
}

const DIRS: &[&str] = &[
    "i/kernel/bin",
    "i/bin/ext",
    "i/body",
    "i/work/home",
    "i/work/tmp",
    "i/work/githome",
    "i/work/.sbx",
    "i/work/pm/mac/src",
    "i/work/pm/app/src",
    "i/work/r/.git",
    "i/state/kernel",
    "i/state/spill/s1",
    "i/lineage/kernel.git",
    "i/vault/holdout",
    "i/vault/canaries",
    "i/cache/git-home",
    "i/cache/tmp",
    "i/cache/build/slot/src/src",
    "i/cache/build/slot/out/home",
    "i/cache/build/slot/out/tmp",
    "i/cache/check/in",
    "i/cache/check/out",
    "i/probe/p1/body",
    "i/probe/p1/work/home",
    "i/probe/p1/work/tmp",
    "i/probe/p1/state/kernel",
    "i/probe/p1/kernel",
    "i/probe/p1/bin/ext",
    "i/probe/p1/run",
    "i/probe/p2/body",
    "i/probe/p2/work",
    "i/probe/p2/state/kernel",
    "i/probe/p2/bin",
    "i/probe/p2/run",
    "i/fleet/b1/body",
    "i/fleet/b1/work/home",
    "i/fleet/b1/work/tmp",
    "i/fleet/b1/state/kernel",
    "i/fleet/b1/kernel",
    "i/fleet/b1/bin",
    "i/fleet/b1/run",
    "i/run/mark",
    "i/run/fleet/b1",
    "i/root",
    "r/run",
    "r/sealed",
    "r/keys/providers",
    "svc",
    "outside",
    "extra",
    "extra2",
    // ADR-009: the run directory is the instances' fake HOME (Z_HOME); nothing in the real home is granted.
    "Documents",
    ".ssh",
    ".config/init-agent",
    "tax",
    "ws/src",
    "other",
    // ADR-009 §9.3 Q5: the auto-exec paths of the fake HOME and of the workspace's and another repository's .git
    ".config/fish",
    ".config/git",
    ".config/autostart",
    ".local/bin",
    "bin",
    "Library/LaunchAgents",
    "ws/.git/hooks",
    "ws/.git/objects",
    "ws/.git/modules/sub/hooks",
    "ws/.git/worktrees/wt",
    "ws/renamerepo/.git",
    "ws/gitdirfile",
    "other/.git/hooks",
    // v0.5.3 (lane H): the credential stores added to the deny list, and another INIT install's default homes and a
    // `.init-root*` one (access::other_installs)
    ".config/gh",
    ".config/cloudflare",
    ".cloudflared",
    ".cargo",
    ".init-root/keys",
    ".init-root/bin",
    ".init-root-demo/keys",
    ".init/kernel/bin",
    ".init-release",
];

const FILES: &[(&str, &str)] = &[
    ("i/kernel/CONSTITUTION.md", "# constitution (test copy)\n"),
    ("i/bin/init-body", "stand-in\n"),
    ("i/body/f", "body file\n"),
    ("i/state/x", "state\n"),
    ("i/state/kernel/lkg.json", "{}\n"),
    ("i/lineage/x", "lineage\n"),
    ("i/lineage/kernel.git/HEAD", "ref: refs/heads/main\n"),
    ("i/vault/secret", "vault secret\n"),
    ("i/vault/holdout/t", "holdout task\n"),
    ("i/vault/canaries/c", "canary\n"),
    ("i/cache/c", "cache\n"),
    ("i/cache/check/in/f", "check input\n"),
    ("i/probe/p1/work/task.txt", "probe task prompt\n"),
    ("i/probe/p1/kernel/CONSTITUTION.md", "# constitution (probe clone)\n"),
    ("i/probe/p1/bin/ext/x.wasm", "\0asm"),
    ("i/probe/p1/state/kernel/lkg.json", "{}\n"),
    ("i/fleet/b1/kernel/CONSTITUTION.md", "# constitution (member clone)\n"),
    ("i/root/ledger.json", "{\"level\":0}\n"),
    ("r/secret", "root secret\n"),
    ("r/sealed/x", "sealed task\n"),
    ("r/keys/providers/fake", "sk-wp6a-fake-not-a-key\n"),
    ("svc/dev.init.root.plist", "<plist/>\n"),
    ("outside/readme", "outside INIT_HOME, inside the repo\n"),
    ("extra/f", "extra_read\n"),
    ("extra2/f", "sibling of extra_read\n"),
    ("Documents/x", "a document in the fake home\n"),
    (".ssh/id", "fake key, not a real one\n"),
    (".config/init-agent/deepseek.key", "sk-adr009-fake-not-a-key\n"),
    ("tax/2025.txt", "a charter read_deny entry\n"),
    ("ws/src/a.txt", "workspace file\n"),
    ("other/readme", "elsewhere in the fake home\n"),
    (".git-credentials", "fake git credentials, not a real one\n"),
    (".config/gh/hosts.yml", "fake gh hosts file, not a real one\n"),
    (".cargo/credentials.toml", "fake cargo credentials, not a real one\n"),
    (".config/cloudflare/token", "fake cloudflare file, not a real one\n"),
    (".cloudflared/cert.pem", "fake cloudflared file, not a real one\n"),
    (".init-root/keys/lineage.key", "fake key of another install, not a real one\n"),
    (".init-root/bin/init-root", "#!/bin/sh\n"),
    (".init-root-demo/keys/lineage.key", "fake key of another install, not a real one\n"),
    (".init/kernel/bin/init-kernel", "#!/bin/sh\n"),
    (".init-release/release.key", "fake release signing key, not a real one\n"),
    (".zshrc", "# fake\n"),
    (".bash_profile", "# fake\n"),
    (".profile", "# fake\n"),
    (".config/fish/config.fish", "# fake\n"),
    (".gitconfig", "# fake\n"),
    (".config/git/config", "# fake\n"),
    (".local/bin/init-agent", "#!/bin/sh\n"),
    (".local/bin/init", "#!/bin/sh\n"),
    (".local/bin/init-tui", "#!/bin/sh\n"),
    ("bin/tool", "#!/bin/sh\n"),
    ("ws/.git/HEAD", "ref: refs/heads/main\n"),
    ("ws/.git/config", "[core]\n"),
    ("ws/.git/hooks/pre-commit.sample", "#!/bin/sh\n"),
    ("ws/.git/modules/sub/config", "[core]\n"),
    ("other/.git/config", "[core]\n"),
    ("i/work/pm/Cargo.toml", "[workspace]\nmembers = [\"mac\", \"app\"]\nresolver = \"3\"\n"),
    ("i/work/pm/mac/Cargo.toml", "[package]\nname = \"sbx-mac\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[lib]\nproc-macro = true\n"),
    (
        "i/work/pm/mac/src/lib.rs",
        "use proc_macro::TokenStream;\n\n#[proc_macro]\npub fn answer(_input: TokenStream) -> TokenStream {\n    \"42\".parse().unwrap()\n}\n",
    ),
    ("i/work/pm/app/Cargo.toml", "[package]\nname = \"sbx-app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nsbx-mac = { path = \"../mac\" }\n"),
    ("i/work/pm/app/src/main.rs", "fn main() {\n    assert_eq!(sbx_mac::answer!(), 42);\n}\n"),
    ("i/cache/build/slot/src/Cargo.toml", "[package]\nname = \"sbx-b6\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n"),
    (
        "i/cache/build/slot/src/Cargo.lock",
        "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n\n[[package]]\nname = \"sbx-b6\"\nversion = \"0.1.0\"\n",
    ),
    ("i/cache/build/slot/src/src/main.rs", "fn main() {}\n"),
];

/// Where the probe binary is copied: one place each profile can read and execute.
const PROBE_COPIES: &[&str] = &[
    "i/bin/sbx-probe",
    "i/kernel/bin/sbx-probe",
    "i/kernel/bin/initctl",
    "i/probe/p1/bin/sbx-probe",
    "i/probe/p2/bin/sbx-probe",
    "i/fleet/b1/bin/sbx-probe",
    "i/work/.sbx/sbx-probe",
];

fn setup(cfg: &SelftestConfig) -> io::Result<Home> {
    fs::create_dir_all(&cfg.base)?;
    let base = canon(&cfg.base)?;
    let run = loop {
        let id = new_instance_id()?[..6].to_owned();
        let p = base.join(id);
        match fs::create_dir(&p) {
            Ok(()) => break p,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    };
    let h = Home {
        i: run.join("i"),
        r: run.join("r"),
        svc: run.join("svc/dev.init.root.plist"),
        outside: run.join("outside"),
        extra: run.join("extra"),
        extra2: run.join("extra2"),
        run,
    };
    let longest = h.i("probe/p1/run/kernel.sock");
    if path_str(&longest).len() > SUN_PATH_MAX {
        return Err(io::Error::other(format!("base too long: {} exceeds sun_path ({SUN_PATH_MAX} bytes)", longest.display())));
    }
    for d in DIRS {
        fs::create_dir_all(h.run.join(d))?;
    }
    // The Root's install creates INIT_HOME/run with mode 0700 (and Z4 then freezes it).
    fs::set_permissions(h.i("run"), fs::Permissions::from_mode(0o700))?;
    for (f, body) in FILES {
        fs::write(h.run.join(f), body)?;
    }
    for c in PROBE_COPIES {
        let dst = h.run.join(c);
        fs::copy(&cfg.probe_exe, &dst)?;
        fs::set_permissions(&dst, fs::Permissions::from_mode(0o755))?;
    }
    // v0.2 link layout for RR-4b: a symlink inside the probe root naming the real kernel/.
    std::os::unix::fs::symlink(h.i("kernel"), h.i("probe/p2/kernel"))?;
    // The Root's marks: the decoy and the zone mark (INIT_HOME), the canary (ROOT_HOME).
    create_mark(&h.i("run/mark/decoy"))?;
    create_mark(&h.i("run/zone-mark"))?;
    create_mark(&h.r("run/canary"))?;
    Ok(h)
}

// ---------------------------------------------------------------------------------------------
// Instances and probes

#[derive(Clone, Debug)]
enum Expect {
    Allow,
    /// `EPERM` or `EACCES`; anything else (e.g. `ENOENT`) is inconclusive and fails.
    Deny,
    /// The probe's child exited with this code (S-M42/S-Z12: nested sandbox-exec → 71).
    Code(i32),
    Fds(Vec<i64>),
    /// A `sandbox_check` policy query: true = allowed (rc 0).
    Query(bool),
    /// Allowed, and the probe's child died of this signal.
    Signal(i64),
}

struct Instance {
    label: String,
    rendered: Rendered,
    exe: PathBuf,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
    mark: PathBuf,
}

struct ProbeOut {
    code: Option<i32>,
    json: Option<Value>,
    stderr: String,
}

struct Running {
    child: Child,
    lines: mpsc::Receiver<String>,
    stdin: Option<ChildStdin>,
}

struct Runner<'a> {
    cfg: &'a SelftestConfig,
    h: Home,
    ctx: ValidationCtx,
    layout: ZoneLayout,
    results: Vec<CheckResult>,
    listeners: Vec<UnixListener>,
    /// The runner's own loopback endpoints (S-M35, S-M37, S-Z7, RR-1): nothing else listens there.
    tcp: Option<std::net::TcpListener>,
    udp: Option<std::net::UdpSocket>,
    /// The runner's own IPv6 loopback listener (S-B4m).
    tcp6: Option<std::net::TcpListener>,
}

fn s(p: &Path) -> String {
    path_str(p)
}

impl<'a> Runner<'a> {
    fn new(cfg: &'a SelftestConfig, h: Home) -> Self {
        let ctx = ValidationCtx {
            init_home: h.i.clone(),
            toolchain: cfg.toolchain.clone(),
            devdir: cfg.devdir.clone(),
            home_real: cfg.home_real.clone(),
            extra_read: vec![],
        };
        // ADR-009: the run directory stands in for the owner's home, with `tax/` as a charter read_deny entry; v0.5.3: the
        // other INIT installs come between, as the Root's `access::deny` puts them.
        let read_deny = access::read_deny(&h.run, &[access::other_installs(&h.run, &h.r, &h.i), vec![h.run.join("tax")]].concat());
        let write_deny = access::write_deny(&h.run, &[]);
        let layout = ZoneLayout {
            root_home: h.r.clone(),
            init_home: h.i.clone(),
            root_svc: h.svc.clone(),
            extra_read: vec![],
            home: h.run.clone(),
            read_deny,
            write_deny,
        };
        let tcp = std::net::TcpListener::bind("127.0.0.1:0").ok();
        let udp = std::net::UdpSocket::bind("127.0.0.1:0").ok();
        let tcp6 = std::net::TcpListener::bind("[::1]:0").ok();
        Runner { cfg, h, ctx, layout, results: vec![], listeners: vec![], tcp, udp, tcp6 }
    }

    fn push(&mut self, id: &str, profile: &str, what: &str, status: Status) {
        self.results.push(CheckResult { id: id.into(), profile: profile.into(), what: what.into(), status });
    }

    fn base_env(&self, home: &Path, tmp: &Path) -> BTreeMap<String, String> {
        let mut e: BTreeMap<String, String> = [
            ("PATH", "/usr/bin:/bin:/usr/sbin:/sbin"),
            ("LC_ALL", "C"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_ATTR_NOSYSTEM", "1"),
            ("GIT_TERMINAL_PROMPT", "0"),
            ("GIT_NO_REPLACE_OBJECTS", "1"),
            ("GIT_NO_LAZY_FETCH", "1"),
            ("INIT_SBX_SELFTEST", "1"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        e.insert("SDKROOT".into(), s(&self.cfg.sdkroot));
        e.insert("DEVELOPER_DIR".into(), s(&self.cfg.devdir));
        e.insert("HOME".into(), s(home));
        e.insert("TMPDIR".into(), s(tmp));
        e
    }

    #[allow(clippy::too_many_arguments)]
    fn instance(
        &mut self,
        label: &str,
        kind_profile: Option<(&str, ProfileId)>,
        inner: Option<&str>,
        params: BTreeMap<String, String>,
        layout: &ZoneLayout,
        exe: PathBuf,
        cwd: PathBuf,
        env: BTreeMap<String, String>,
    ) -> Option<Instance> {
        self.instance_with(label, kind_profile, inner, params, layout, exe, cwd, env, &Grant::default())
    }

    /// Validate (for kernel-suppliable profiles), create a mark, compose with the ADR-009 `grant`. `None`
    /// after recording a failure when the parameter map is refused.
    #[allow(clippy::too_many_arguments)]
    fn instance_with(
        &mut self,
        label: &str,
        kind_profile: Option<(&str, ProfileId)>,
        inner: Option<&str>,
        params: BTreeMap<String, String>,
        layout: &ZoneLayout,
        exe: PathBuf,
        cwd: PathBuf,
        env: BTreeMap<String, String>,
        grant: &Grant,
    ) -> Option<Instance> {
        let id = format!("V.{label}");
        if let Some((kind, profile)) = kind_profile {
            let ctx = ValidationCtx { extra_read: layout.extra_read.clone(), ..self.ctx.clone() };
            let checked =
                validate_spawn_params(kind, profile, &params, &ctx).and_then(|()| check_spawn_misc(&s(&exe), &s(&cwd), &env, &self.cfg.probe_prefix, &ctx));
            let what = "the runner's parameter map passes V1–V5 and the class rules";
            match checked {
                Ok(()) => self.push(&id, label, what, Status::Pass),
                Err(e) => {
                    self.push(&id, label, what, Status::Fail(e.to_string()));
                    return None;
                }
            }
        }
        let mark = match new_instance_id().and_then(|n| {
            let m = layout.mark_path(&n);
            create_mark(&m).map(|()| m)
        }) {
            Ok(m) => m,
            Err(e) => {
                self.push(&id, label, "create the instance mark", Status::Fail(e.to_string()));
                return None;
            }
        };
        let composed = match (kind_profile, inner) {
            (_, Some(text)) => seatbelt::compose_over(text, &params, layout, &mark, grant),
            (Some((_, p)), None) => seatbelt::compose(p, &params, layout, &mark, grant),
            (None, None) => seatbelt::compose(ProfileId::Kernel, &params, layout, &mark, grant),
        };
        match composed {
            Ok(rendered) => Some(Instance { label: label.into(), rendered, exe, cwd, env, mark }),
            Err(e) => {
                self.push(&id, label, "compose the profile", Status::Fail(e.to_string()));
                None
            }
        }
    }

    fn command(&self, inst: &Instance, program: &Path, args: &[String], extra_env: &[(String, String)]) -> Command {
        let argv = inst.rendered.sandbox_exec_argv();
        let mut c = Command::new(&argv[0]);
        c.args(&argv[1..]).arg("--").arg(program).args(args);
        c.env_clear().envs(&inst.env).envs(extra_env.iter().cloned()).current_dir(&inst.cwd);
        sys::set_exec_clean(&mut c);
        c
    }

    fn tcp_port(&self) -> String {
        self.tcp.as_ref().and_then(|l| l.local_addr().ok()).map_or("0".into(), |a| a.port().to_string())
    }

    fn tcp6_port(&self) -> String {
        self.tcp6.as_ref().and_then(|l| l.local_addr().ok()).map_or("0".into(), |a| a.port().to_string())
    }

    fn udp_port(&self) -> String {
        self.udp.as_ref().and_then(|u| u.local_addr().ok()).map_or("0".into(), |a| a.port().to_string())
    }

    fn probe_args(&self, op: &[String]) -> Vec<String> {
        self.cfg.probe_prefix.iter().cloned().chain(op.iter().cloned()).collect()
    }

    fn probe(&self, inst: &Instance, op: &[String], extra_env: &[(String, String)], timeout: Duration) -> ProbeOut {
        let cmd = self.command(inst, &inst.exe, &self.probe_args(op), extra_env);
        match run_child(cmd, timeout) {
            Ok((status, out, err)) => ProbeOut { code: status.and_then(|s| s.code()), json: last_result(&out), stderr: err },
            Err(e) => ProbeOut { code: None, json: None, stderr: format!("spawn: {e}") },
        }
    }

    fn check(&mut self, id: &str, inst: &Instance, what: &str, op: &[String], expect: Expect) {
        self.check_env(id, inst, what, op, expect, &[], OP_TIMEOUT);
    }

    #[allow(clippy::too_many_arguments)]
    fn check_env(&mut self, id: &str, inst: &Instance, what: &str, op: &[String], expect: Expect, extra_env: &[(String, String)], timeout: Duration) {
        let out = self.probe(inst, op, extra_env, timeout);
        let status = evaluate(&expect, &out);
        self.push(id, &inst.label, what, status);
    }

    /// S-M0: the composed profile parses and runs `/usr/bin/true`.
    fn parses(&mut self, id: &str, inst: &Instance) {
        let cmd = self.command(inst, Path::new("/usr/bin/true"), &[], &[]);
        let status = match run_child(cmd, OP_TIMEOUT) {
            Ok((Some(st), _, _)) if st.success() => Status::Pass,
            Ok((st, _, err)) => Status::Fail(format!("exit {st:?}: {}", tail(&err, 400))),
            Err(e) => Status::Fail(e.to_string()),
        };
        self.push(id, &inst.label, "the composed profile parses (sandbox-exec … /usr/bin/true exits 0)", status);
    }

    fn start(&self, inst: &Instance, op: &[String], stdin_piped: bool) -> io::Result<Running> {
        let mut cmd = self.command(inst, &inst.exe, &self.probe_args(op), &[]);
        cmd.stdin(if stdin_piped { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take();
        let out = child.stdout.take().ok_or_else(|| io::Error::other("no stdout"))?;
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Running { child, lines: rx, stdin })
    }

    // -----------------------------------------------------------------------------------------
    // Parameter maps

    fn body_params(&self, r: &Path, run: &Path, kernel: &Path, bin: &Path, link_kernel: &Path) -> BTreeMap<String, String> {
        let i = &self.h.i;
        let m: [(&str, PathBuf); 20] = [
            ("INIT_HOME", r.to_path_buf()),
            ("BODY", r.join("body")),
            ("WORK", r.join("work")),
            ("STATE", r.join("state")),
            ("STATE_KERNEL", r.join("state/kernel")),
            ("KERNEL", kernel.to_path_buf()),
            ("BIN", bin.to_path_buf()),
            ("LINEAGE", i.join("lineage")),
            ("RUN", run.to_path_buf()),
            ("KERNEL_SOCK", run.join("kernel.sock")),
            ("PROXY_SOCK", run.join("proxy.sock")),
            ("VAULT", i.join("vault")),
            ("CACHE", i.join("cache")),
            ("PUB", i.join("root")),
            ("LINK_KERNEL", link_kernel.to_path_buf()),
            ("LINK_LINEAGE", i.join("lineage")),
            ("LINK_BIN", bin.to_path_buf()),
            ("TOOLCHAIN", self.cfg.toolchain.clone()),
            ("DEVDIR", self.cfg.devdir.clone()),
            ("HOME_REAL", self.cfg.home_real.clone()),
        ];
        m.into_iter().map(|(k, v)| (k.to_owned(), s(&v))).collect()
    }

    fn build_params(&self, src: &Path, out: &Path) -> BTreeMap<String, String> {
        let i = &self.h.i;
        [
            ("INIT_HOME", i.clone()),
            ("SRC", src.to_path_buf()),
            ("OUT", out.to_path_buf()),
            ("KERNEL", i.join("kernel")),
            ("VAULT", i.join("vault")),
            ("TOOLCHAIN", self.cfg.toolchain.clone()),
            ("DEVDIR", self.cfg.devdir.clone()),
            ("HOME_REAL", self.cfg.home_real.clone()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), s(&v)))
        .collect()
    }

    fn git_read_params(&self) -> BTreeMap<String, String> {
        let i = &self.h.i;
        [
            ("INIT_HOME", i.clone()),
            ("BODY", i.join("body")),
            ("WORK", i.join("work")),
            ("VAULT", i.join("vault")),
            ("CACHE", i.join("cache")),
            ("TOOLCHAIN", self.cfg.toolchain.clone()),
            ("DEVDIR", self.cfg.devdir.clone()),
            ("HOME_REAL", self.cfg.home_real.clone()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), s(&v)))
        .collect()
    }

    fn live_body(&mut self, label: &str, layout: &ZoneLayout) -> Option<Instance> {
        let i = self.h.i.clone();
        let params = self.body_params(&i, &i.join("run"), &i.join("kernel"), &i.join("bin"), &i.join("kernel"));
        let env = self.base_env(&i.join("work/home"), &i.join("work/tmp"));
        self.instance(label, Some(("body", ProfileId::Body)), None, params, layout, i.join("bin/sbx-probe"), i.join("work"), env)
    }

    fn kernel_instance(&mut self, label: &str, layout: &ZoneLayout) -> Option<Instance> {
        let i = self.h.i.clone();
        let params = layout.kernel_profile_params(&self.cfg.toolchain, &self.cfg.devdir, &self.cfg.home_real);
        let env = self.base_env(&i.join("cache/git-home"), &i.join("cache/tmp"));
        self.instance(label, None, None, params, layout, i.join("kernel/bin/sbx-probe"), i.clone(), env)
    }

    // -----------------------------------------------------------------------------------------
    // The suite

    fn all(&mut self) {
        let resolvable =
            if sys::sbcheck::available() { Status::Pass } else { Status::Fail("dlsym(RTLD_DEFAULT, \"sandbox_check\") is null: sweep degraded".into()) };
        self.push("SYS-1", "-", "sandbox_check resolves through dlsym (sandbox.md §3.8)", resolvable);
        for (id, what) in DISPOSABLE_ONLY {
            self.push(id, "-", what, Status::Skipped("disposable-env only (CONVENTIONS §1, D1)".into()));
        }
        self.push("S-M38", "body", "DNS lookup", Status::Skipped("no DNS traffic on the owner's machine; the resolver socket is S-M34".into()));
        // Stand-ins for the Root's sockets and the probe and member lanes (no Root process).
        let h = &self.h;
        let socks = [
            h.r("run/owner.sock"),
            h.r("run/egress.sock"),
            h.i("probe/p1/run/kernel.sock"),
            h.i("probe/p1/run/proxy.sock"),
            h.i("run/fleet/b1/kernel.sock"),
            h.i("run/fleet/b1/proxy.sock"),
            // S-B4f, S-Z8r: next to the build slot's OUT, with OUT's path as a string prefix
            h.i("cache/build/slot/outl"),
            // S-B4i, S-Z8s: outside INIT_HOME (stands for the owner's other sockets)
            h.outside.join("l.sock"),
        ];
        for p in socks {
            match UnixListener::bind(&p) {
                Ok(l) => self.listeners.push(l),
                Err(e) => self.push("SETUP", "-", &format!("bind {}", p.display()), Status::Fail(e.to_string())),
            }
        }
        self.controls();
        let layout = self.layout.clone();
        let live = self.live_body("body", &layout);
        let kern = self.kernel_instance("kernel", &layout);
        if let (Some(live), Some(kern)) = (&live, &kern) {
            self.zone_cross(live, kern);
            self.lane_socket_checks(live, kern);
        }
        for p in [self.h.i("run/kernel.sock"), self.h.i("run/proxy.sock")] {
            match UnixListener::bind(&p) {
                Ok(l) => self.listeners.push(l),
                Err(e) => self.push("SETUP", "-", &format!("bind {}", p.display()), Status::Fail(e.to_string())),
            }
        }
        if let Some(live) = &live {
            self.parses("S-M0.body", live);
            self.live_checks(live);
        }
        self.probe_checks();
        self.member_checks();
        self.access_checks();
        self.link_probe_checks();
        self.extra_read_body();
        self.build_checks();
        self.git_read_checks();
        if let Some(kern) = &kern {
            self.parses("S-M0.kernel", kern);
            self.kernel_checks(kern);
        }
        self.extra_read_kernel();
        self.tail_checks();
        if let Some(live) = &live {
            self.live_last(live);
        }
        if let Some(kern) = &kern {
            self.kernel_last(kern);
        }
    }

    /// Negative controls: the same probe under `(version 1)(allow default)` with no tail is allowed
    /// what the suite expects the profiles to refuse, so a refusal is the profile's doing.
    fn controls(&mut self) {
        let i = self.h.i.clone();
        let inst = Instance {
            label: "control/allow-default".into(),
            rendered: Rendered { text: "(version 1)\n(allow default)\n".into(), params: BTreeMap::new() },
            exe: i.join("kernel/bin/sbx-probe"),
            cwd: i.join("work"),
            env: self.base_env(&i.join("work/home"), &i.join("work/tmp")),
            mark: PathBuf::new(),
        };
        let r = |x: &str| s(&self.h.r(x));
        let rows = vec![
            ("CTRL-1", "no profile: write kernel/", ops(&["write", &s(&i.join("kernel/ctrl"))])),
            ("CTRL-2", "no profile: read ROOT_HOME", ops(&["read", &r("secret")])),
            ("CTRL-3", "no profile: connect the owner socket", ops(&["connect", &r("run/owner.sock")])),
            ("CTRL-4", "no profile: read vault/", ops(&["read", &s(&i.join("vault/secret"))])),
        ];
        for (id, what, op) in rows {
            self.check(id, &inst, what, &op, Expect::Allow);
        }
        let _ = fs::remove_file(i.join("kernel/ctrl"));
    }

    /// S-Z8b, S-Z10, S-Z11: two instances at once.
    fn zone_cross(&mut self, live: &Instance, kern: &Instance) {
        let ks = self.h.i("run/kernel.sock");
        // S-Z8b: the kernel instance serves its KERNEL_SOCK; a body instance connects.
        let what = "a body instance connects to a socket the kernel instance bound at KERNEL_SOCK; the kernel accepts";
        let status = self.serve_and_connect(kern, live, "serve-once", &ks);
        self.push("S-Z8b", "kernel+body", what, status);
        if ks.exists() {
            // The socket file the kernel instance created; removed by its exact path.
            let _ = fs::remove_file(&ks);
        }

        // S-Z10, S-Z11 against a live body instance that waits on stdin.
        let canary = self.layout.canary();
        let decoy = self.layout.decoy();
        match self.start(live, &ops(&["wait-stdin"]), true) {
            Ok(tgt) => {
                let ready = tgt.lines.recv_timeout(Duration::from_secs(10)).ok().and_then(|l| serde_json::from_str::<Value>(&l).ok());
                let pid = ready.as_ref().and_then(|v| v["pid"].as_i64());
                if let Some(pid) = pid.filter(|p| *p == tgt.child.id() as i64) {
                    let out = self.probe(kern, &ops(&["sbcheck", &pid.to_string(), &s(&live.mark), &s(&canary), &s(&decoy)]), &[], OP_TIMEOUT);
                    let st = match &out.json {
                        Some(v) => {
                            let sb = v["data"]["sandboxed"].as_i64();
                            let p: Vec<Option<i64>> = v["data"]["paths"].as_array().map(|a| a.iter().map(Value::as_i64).collect()).unwrap_or_default();
                            let exact = sb == Some(1) && p.len() == 3 && p[0] == Some(0) && p[1].is_some_and(|x| x != 0) && p[2].is_some_and(|x| x != 0);
                            if exact { Status::Pass } else { Status::Fail(format!("sandboxed {sb:?}, mark/canary/decoy {p:?}")) }
                        }
                        None => Status::Fail(format!("no result; stderr {}", tail(&out.stderr, 300))),
                    };
                    self.push("S-Z10", "kernel", "sandbox_check with the path filter on a body pid: sandboxed=1, own mark 0, canary and decoy denied", st);
                    self.check("S-Z11", kern, "kill -0 on a body instance (spawned by this suite)", &ops(&["kill0", &pid.to_string()]), Expect::Deny);
                } else {
                    self.push("S-Z10", "kernel", "sandbox_check on a body pid", Status::Fail(format!("target not ready: {ready:?}")));
                }
                let end = finish(tgt, Duration::from_secs(10));
                if !end.as_ref().is_some_and(|v| v["ok"] == true) {
                    self.push("S-Z10.end", "body", "the wait-stdin target exits on EOF", Status::Fail(format!("{end:?}")));
                }
            }
            Err(e) => self.push("S-Z10", "kernel", "sandbox_check on a body pid", Status::Fail(format!("spawn: {e}"))),
        }
    }

    /// The kernel instance runs `serve_op` (`serve-once` or `serve-lane`) on `sock`; once it is
    /// ready, the body instance connects and sends one line, which the kernel must receive.
    fn serve_and_connect(&self, kern: &Instance, body: &Instance, serve_op: &str, sock: &Path) -> Status {
        match self.start(kern, &ops(&[serve_op, &s(sock)]), false) {
            Ok(srv) => {
                let ready = srv.lines.recv_timeout(Duration::from_secs(10)).ok().and_then(|l| serde_json::from_str::<Value>(&l).ok());
                let client = if ready.as_ref().is_some_and(|v| v["ready"] == true) {
                    let out = self.probe(body, &ops(&["connect-send", &s(sock)]), &[], OP_TIMEOUT);
                    evaluate(&Expect::Allow, &out)
                } else {
                    Status::Fail(format!("server not ready: {ready:?}"))
                };
                let served = finish(srv, Duration::from_secs(10));
                match (client, served) {
                    (Status::Pass, Some(v)) if v["ok"] == true && v["data"]["line"] == "hello from body" => Status::Pass,
                    (c, v) => Status::Fail(format!("client {c:?}; server {v:?}")),
                }
            }
            Err(e) => Status::Fail(format!("spawn: {e}")),
        }
    }

    /// FIX1 (build/status/S1.md gap 3): the kernel-owned socket paths of kernel/initctl.md §3.3,
    /// kernel/proxy.md §3.4 and kernel/supervise.md §3.14 under the composed `kernel` profile, with
    /// the kernel's own lane sequence; and the `body` profile refused on every one of them.
    /// Runs while `run/kernel.sock` and `run/proxy.sock` are absent; every socket file it creates is
    /// removed by its exact path.
    fn lane_socket_checks(&mut self, live: &Instance, kern: &Instance) {
        let (ih, outside) = (self.h.i.clone(), self.h.outside.clone());
        let i = |x: &str| ih.join(x);
        // S-Z8f: the S1 failure. Z4 refuses a chmod of run/ itself, even to its current 0700, so a
        // lane server must leave run/ alone (kernel-serve add_lane chmods only when the mode differs).
        let chmod_run = ops(&["chmod", "700", &s(&i("run"))]);
        self.check("S-Z8f", kern, "kernel: chmod run/ itself, even to its current 0700 (Z4; the S1 gap-3 EPERM)", &chmod_run, Expect::Deny);

        // S-Z8e: run/kernel.sock with the kernel's lane sequence over a stale socket file.
        let ks = i("run/kernel.sock");
        let stale = UnixListener::bind(&ks).map(drop); // the runner leaves a stale socket file behind
        let what = "kernel serves run/kernel.sock (lane sequence: stale socket unlinked, bind, 0600, accept); a body instance connects";
        let status = match stale {
            Ok(()) => self.serve_and_connect(kern, live, "serve-lane", &ks),
            Err(e) => Status::Fail(format!("setup: stale socket: {e}")),
        };
        self.push("S-Z8e", "kernel+body", what, status);

        // S-Z8g–l: every other kernel-owned socket path, with the same sequence (it creates
        // run/fleet/b2/ and chmods probe/p2/run/, which only run/ itself may not have).
        let lanes = [
            ("S-Z8g", "run/proxy.sock"),
            ("S-Z8h", "run/spawn.sock"),
            ("S-Z8i", "run/fleet/b2/kernel.sock"),
            ("S-Z8j", "run/fleet/b2/proxy.sock"),
            ("S-Z8k", "probe/p2/run/kernel.sock"),
            ("S-Z8l", "probe/p2/run/proxy.sock"),
        ];
        for (id, rel) in lanes {
            let what = format!("kernel: lane sequence on {rel}");
            self.check(id, kern, &what, &ops(&["lane-bind", &s(&i(rel))]), Expect::Allow);
        }
        // Nothing broader than run/ and probe/ (section 6), and never into run/mark/ (Z4) or
        // outside INIT_HOME (Z6).
        let narrow = [
            ("S-Z8m", "kernel: bind in run/mark/ (Z4)", i("run/mark/k.sock")),
            ("S-Z8n", "kernel: bind in work/ (not under RUN or PROBE)", i("work/k.sock")),
            ("S-Z8o", "kernel: bind outside INIT_HOME (Z6)", outside.join("k.sock")),
        ];
        for (id, what, p) in narrow {
            self.check(id, kern, what, &ops(&["bind", &s(&p)]), Expect::Deny);
        }

        // S-M39b–h: a body may bind none of them. Each path is absent when it is tried (a bind on
        // an existing file fails with EADDRINUSE before the sandbox decides), and S-M33 shows a body
        // cannot unlink the kernel's socket to make room.
        let mut paths = vec![ks];
        paths.extend(lanes.iter().map(|(_, rel)| i(rel)));
        for p in &paths {
            if fs::symlink_metadata(p).is_ok() {
                let _ = fs::remove_file(p); // a socket file the kernel instance created; exact path
            }
        }
        let ids = ["S-M39b", "S-M39c", "S-M39d", "S-M39e", "S-M39f", "S-M39g", "S-M39h"];
        for (id, p) in ids.iter().zip(&paths) {
            let rel = p.strip_prefix(&ih).map_or_else(|_| s(p), s);
            let what = format!("body: bind the kernel-owned {rel}");
            self.check(id, live, &what, &ops(&["bind", &s(p)]), Expect::Deny);
        }
        for p in &paths {
            if fs::symlink_metadata(p).is_ok() {
                let _ = fs::remove_file(p); // only if a body bind unexpectedly succeeded; exact path
            }
        }
    }

    fn live_checks(&mut self, b: &Instance) {
        let h = &self.h;
        let i = |x: &str| s(&h.i(x));
        let r = |x: &str| s(&h.r(x));
        let home = |x: &str| s(&self.cfg.home_real.join(x));
        let tc = s(&self.cfg.toolchain.join("bin/cargo"));
        let (tp, up) = (self.tcp_port(), self.udp_port());
        let rows: Vec<(&str, &str, Vec<String>, Expect)> = vec![
            ("S-M1", "write a file in body/", ops(&["write", &i("body/x")]), Expect::Allow),
            ("S-M2", "write a file in work/", ops(&["write", &i("work/x")]), Expect::Allow),
            ("S-M3", "write a file in state/", ops(&["write", &i("state/y")]), Expect::Allow),
            ("S-M4", "write state/kernel/x", ops(&["write", &i("state/kernel/x")]), Expect::Deny),
            ("S-M5", "overwrite state/kernel/lkg.json", ops(&["write", &i("state/kernel/lkg.json")]), Expect::Deny),
            ("S-M7", "unlink state/kernel/lkg.json", ops(&["unlink", &i("state/kernel/lkg.json")]), Expect::Deny),
            ("S-M8", "symlink work/kl -> kernel/ and write through it", ops(&["symlink-write", &i("work/kl"), &i("kernel"), &i("work/kl/x")]), Expect::Deny),
            ("S-M9", "hard-link kernel/CONSTITUTION.md into work/", ops(&["hardlink", &i("kernel/CONSTITUTION.md"), &i("work/c")]), Expect::Deny),
            ("S-M10", "write in kernel/", ops(&["write", &i("kernel/x")]), Expect::Deny),
            ("S-M11", "write in bin/", ops(&["write", &i("bin/x")]), Expect::Deny),
            ("S-M12", "write in lineage/", ops(&["write", &i("lineage/y")]), Expect::Deny),
            ("S-M13", "write in INIT_HOME/", ops(&["write", &i("x")]), Expect::Deny),
            ("CAL-1", "policy query: write work/ (calibrates S-M16/17)", ops(&["policy-write", &i("work")]), Expect::Query(true)),
            ("CAL-2", "policy query: write kernel/ (calibrates S-M16/17)", ops(&["policy-write", &i("kernel")]), Expect::Query(false)),
            ("S-M16", "write under /private/tmp (policy query, nothing written)", ops(&["policy-write", "/private/tmp"]), Expect::Query(false)),
            (
                "S-M17",
                "write under the owner's real home (policy query, nothing written)",
                ops(&["policy-write", &s(&self.cfg.home_real)]),
                Expect::Query(false),
            ),
            ("S-M17r", "write outside INIT_HOME (real write, inside the test dir)", ops(&["write", &s(&self.h.outside.join("x"))]), Expect::Deny),
            ("S-M18", "read kernel/", ops(&["read", &i("kernel/CONSTITUTION.md")]), Expect::Allow),
            ("S-M19", "read bin/", ops(&["read", &i("bin/init-body")]), Expect::Allow),
            ("S-M20", "read lineage/", ops(&["read", &i("lineage/x")]), Expect::Allow),
            ("S-M21", "read state/kernel/", ops(&["read", &i("state/kernel/lkg.json")]), Expect::Allow),
            ("S-M22", "read vault/", ops(&["read", &i("vault/secret")]), Expect::Deny),
            ("S-M23", "list vault/", ops(&["list", &i("vault")]), Expect::Deny),
            ("S-M24", "stat in vault/", ops(&["stat", &i("vault/secret")]), Expect::Deny),
            ("S-M25", "read cache/", ops(&["read", &i("cache/c")]), Expect::Deny),
            ("S-M26", "list ~/.ssh", ops(&["list", &home(".ssh")]), Expect::Deny),
            ("S-M27", "list ~/Documents", ops(&["list", &home("Documents")]), Expect::Deny),
            ("S-M28", "list /opt/homebrew/var", ops(&["list", "/opt/homebrew/var"]), Expect::Deny),
            ("S-M29", "connect run/kernel.sock", ops(&["connect", &i("run/kernel.sock")]), Expect::Allow),
            ("S-M30", "connect run/proxy.sock", ops(&["connect", &i("run/proxy.sock")]), Expect::Allow),
            ("S-M31", "connect ROOT_HOME/run/owner.sock", ops(&["connect", &r("run/owner.sock")]), Expect::Deny),
            ("S-M32", "connect ROOT_HOME/run/egress.sock", ops(&["connect", &r("run/egress.sock")]), Expect::Deny),
            (
                "S-M32b",
                "connect the owner socket through a symlink created in work/",
                ops(&["symlink-connect", &i("work/os"), &r("run/owner.sock")]),
                Expect::Deny,
            ),
            ("S-M33", "unlink run/kernel.sock", ops(&["unlink", &i("run/kernel.sock")]), Expect::Deny),
            ("S-M34", "connect /private/var/run/mDNSResponder (the resolver)", ops(&["connect", "/private/var/run/mDNSResponder"]), Expect::Deny),
            ("S-M35", "TCP connect to 127.0.0.1 (the runner's own listener)", ops(&["tcp-connect", &tp]), Expect::Deny),
            ("S-M36", "TCP bind on loopback", ops(&["tcp-bind-loopback"]), Expect::Deny),
            ("S-M37", "UDP send to 127.0.0.1 (the runner's own socket)", ops(&["udp-send", &up]), Expect::Deny),
            ("S-M37b", "UDP bind on loopback", ops(&["udp-bind-loopback"]), Expect::Deny),
            ("S-M39", "bind a Unix socket in work/", ops(&["bind", &i("work/s.sock")]), Expect::Allow),
            (
                "S-M42",
                "nested sandbox-exec, even (allow default)",
                ops(&["exec", "/usr/bin/sandbox-exec", "-p", "(version 1)(allow default)", "/usr/bin/true"]),
                Expect::Code(71),
            ),
            ("S-M43", "exec git", ops(&["exec", "/usr/bin/git", "--version"]), Expect::Allow),
            ("S-M44", "exec cargo", ops(&["exec", &tc, "--version"]), Expect::Allow),
            ("S-M48", "open fds in the body: only 0, 1, 2", ops(&["devfd"]), Expect::Fds(vec![0, 1, 2])),
            ("S-M50a", "list INIT_HOME/probe/", ops(&["list", &i("probe")]), Expect::Deny),
            ("S-M50b", "read a task file inside a probe root", ops(&["read", &i("probe/p1/work/task.txt")]), Expect::Deny),
            ("S-M51", "kill -TERM a child the body itself started (same sandbox)", ops(&["spawn-term-child"]), Expect::Signal(15)),
            ("S-M52a", "exec /bin/kill (kill -l; no signal sent)", ops(&["exec", "/bin/kill", "-l"]), Expect::Allow),
            ("S-M52b", "create a file under state/spill/<sid>/", ops(&["write", &i("state/spill/s1/x")]), Expect::Allow),
            ("S-M53", "read a file in ROOT_HOME", ops(&["read", &r("secret")]), Expect::Deny),
            ("S-M54", "list ROOT_HOME", ops(&["list", &s(&self.h.r)]), Expect::Deny),
            ("S-M55", "list INIT_HOME/fleet/", ops(&["list", &i("fleet")]), Expect::Deny),
            ("S-M56", "list /opt/homebrew/bin (ESC-2)", ops(&["list", "/opt/homebrew/bin"]), Expect::Allow),
            ("S-M58", "read INIT_HOME/root/ledger.json (PUB)", ops(&["read", &i("root/ledger.json")]), Expect::Allow),
            ("S-M59", "write into INIT_HOME/root/", ops(&["write", &i("root/x")]), Expect::Deny),
            ("S-M61", "read the Root canary", ops(&["read", &r("run/canary")]), Expect::Deny),
            ("S-X3a", "read vault/holdout/", ops(&["read", &i("vault/holdout/t")]), Expect::Deny),
            ("S-X3b", "read vault/canaries/", ops(&["read", &i("vault/canaries/c")]), Expect::Deny),
            ("S-X3c", "read ROOT_HOME/sealed/", ops(&["read", &r("sealed/x")]), Expect::Deny),
            ("S-X1k", "read the provider key store", ops(&["read", &r("keys/providers/fake")]), Expect::Deny),
        ];
        for (id, what, op, expect) in rows {
            self.check(id, b, what, &op, expect);
        }

        // S-M45: exec initctl. The real binary is WP1's; a copy of the probe stands in at kernel/bin/initctl.
        let initctl = self.h.i("kernel/bin/initctl");
        let mut op = vec!["exec".to_owned(), s(&initctl)];
        op.extend(self.cfg.probe_prefix.iter().cloned());
        op.push("noop".into());
        self.check("S-M45", b, "exec kernel/bin/initctl (stand-in binary until WP1)", &op, Expect::Allow);

        // S-M60: another instance's mark.
        let other = self.layout.mark_path("0000feedf00d");
        let st = create_mark(&other);
        match st {
            Ok(()) => self.check("S-M60", b, "read another instance's mark", &ops(&["read", &s(&other)]), Expect::Deny),
            Err(e) => self.push("S-M60", "body", "read another instance's mark", Status::Fail(format!("setup: {e}"))),
        }

        // S-M40b: kill -0 on an unsandboxed process this suite spawned, by its PID.
        self.kill0_own_sleep("S-M40b", b);

        // S-M46: cargo build of a crate with a proc-macro dependency in work/.
        if self.cfg.builds {
            let w = self.h.i("work");
            let env = self.cargo_env(&w.join("cargo-home"), &w.join("pm/target"), &w.join("home"), &w.join("tmp"));
            let op = ops(&["exec", &tc, "build", "--offline", "--manifest-path", &s(&w.join("pm/Cargo.toml"))]);
            self.check_env("S-M46", b, "cargo build of a crate with a proc-macro dependency in work/", &op, Expect::Allow, &env, BUILD_TIMEOUT);
        } else {
            self.push("S-M46", "body", "cargo build with a proc-macro dependency", Status::Skipped("--no-builds".into()));
        }
    }

    /// Checks that would break later rows if the sandbox wrongly allowed them; run last.
    fn live_last(&mut self, b: &Instance) {
        let h = &self.h;
        let rows = vec![
            ("S-M6", "rename state/kernel", ops(&["rename", &s(&h.i("state/kernel")), &s(&h.i("state/k2"))])),
            ("S-M14", "rmdir body/", ops(&["rmdir", &s(&h.i("body"))])),
            ("S-M15", "mv work work2", ops(&["rename", &s(&h.i("work")), &s(&h.i("work2"))])),
        ];
        for (id, what, op) in rows {
            self.check(id, b, what, &op, Expect::Deny);
        }
    }

    fn kill0_own_sleep(&mut self, id: &str, inst: &Instance) {
        let what = "kill -0 on an unsandboxed process this suite spawned, by its PID";
        match Command::new("/bin/sleep").arg("30").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            Ok(mut sleeper) => {
                let pid = sleeper.id().to_string();
                self.check(id, inst, what, &ops(&["kill0", &pid]), Expect::Deny);
                // The one process this signals: the sleep spawned just above, by its PID.
                let _ = sleeper.kill();
                let _ = sleeper.wait();
            }
            Err(e) => self.push(id, &inst.label, what, Status::Fail(format!("setup: {e}"))),
        }
    }

    fn cargo_env(&self, cargo_home: &Path, target: &Path, home: &Path, tmp: &Path) -> Vec<(String, String)> {
        let bin = self.cfg.toolchain.join("bin");
        vec![
            ("CARGO_HOME".into(), s(cargo_home)),
            ("CARGO_TARGET_DIR".into(), s(target)),
            ("RUSTC".into(), s(&bin.join("rustc"))),
            ("PATH".into(), format!("{}:/usr/bin:/bin", s(&bin))),
            ("HOME".into(), s(home)),
            ("TMPDIR".into(), s(tmp)),
        ]
    }

    fn probe_checks(&mut self) {
        let i = self.h.i.clone();
        let root = i.join("probe/p1");
        let params = self.body_params(&root, &root.join("run"), &root.join("kernel"), &root.join("bin"), &root.join("kernel"));
        let env = self.base_env(&root.join("work/home"), &root.join("work/tmp"));
        let layout = self.layout.clone();
        let Some(p) = self.instance("body/probe", Some(("probe", ProfileId::Body)), None, params, &layout, root.join("bin/sbx-probe"), root.join("work"), env)
        else {
            return;
        };
        self.parses("S-M0.probe", &p);
        let q = |x: &str| s(&root.join(x));
        let li = |x: &str| s(&i.join(x));
        let r = |x: &str| s(&self.h.r(x));
        let rows: Vec<(&str, &str, Vec<String>, Expect)> = vec![
            ("S-M49a", "probe: read <probe>/kernel/CONSTITUTION.md (clone)", ops(&["read", &q("kernel/CONSTITUTION.md")]), Expect::Allow),
            ("S-M49b", "probe: read <probe>/bin/ext/x.wasm (clone)", ops(&["read", &q("bin/ext/x.wasm")]), Expect::Allow),
            ("S-M49c", "probe: read the real lineage/x", ops(&["read", &li("lineage/x")]), Expect::Allow),
            ("S-M49d", "probe: write into <probe>/kernel/", ops(&["write", &q("kernel/x")]), Expect::Deny),
            ("S-M49e", "probe: write the live body/", ops(&["write", &li("body/p")]), Expect::Deny),
            ("S-M49f", "probe: read the live state/", ops(&["read", &li("state/x")]), Expect::Deny),
            ("S-M49g", "probe: connect its own lane kernel.sock", ops(&["connect", &q("run/kernel.sock")]), Expect::Allow),
            ("S-M49h", "probe: connect the live run/kernel.sock", ops(&["connect", &li("run/kernel.sock")]), Expect::Deny),
            ("S-M49i", "probe: connect the Root owner socket", ops(&["connect", &r("run/owner.sock")]), Expect::Deny),
            ("S-M49j", "probe: bind in its own lane directory <probe>/run/ (kernel-owned)", ops(&["bind", &q("run/x.sock")]), Expect::Deny),
            ("S-X3d", "probe: read vault/holdout/", ops(&["read", &li("vault/holdout/t")]), Expect::Deny),
            ("S-X3e", "probe: read ROOT_HOME/sealed/", ops(&["read", &r("sealed/x")]), Expect::Deny),
        ];
        for (id, what, op, expect) in rows {
            self.check(id, &p, what, &op, expect);
        }
    }

    fn member_checks(&mut self) {
        let i = self.h.i.clone();
        let root = i.join("fleet/b1");
        let run = i.join("run/fleet/b1");
        let params = self.body_params(&root, &run, &root.join("kernel"), &root.join("bin"), &root.join("kernel"));
        let env = self.base_env(&root.join("work/home"), &root.join("work/tmp"));
        let layout = self.layout.clone();
        let Some(m) = self.instance("body/member", Some(("body", ProfileId::Body)), None, params, &layout, root.join("bin/sbx-probe"), root.join("work"), env)
        else {
            return;
        };
        self.parses("S-M0.member", &m);
        let q = |x: &str| s(&root.join(x));
        let li = |x: &str| s(&i.join(x));
        let rows: Vec<(&str, &str, Vec<String>, Expect)> = vec![
            ("S-M57a", "member: write its own work/", ops(&["write", &q("work/x")]), Expect::Allow),
            ("S-M57b", "member: write the live body/", ops(&["write", &li("body/m")]), Expect::Deny),
            ("S-M57c", "member: write its own state/kernel/", ops(&["write", &q("state/kernel/x")]), Expect::Deny),
            ("S-M57d", "member: connect its own lane run/fleet/b1/kernel.sock", ops(&["connect", &li("run/fleet/b1/kernel.sock")]), Expect::Allow),
            ("S-M57e", "member: connect the live run/kernel.sock", ops(&["connect", &li("run/kernel.sock")]), Expect::Deny),
            ("S-M57f", "member: read kernel/ (its clone)", ops(&["read", &q("kernel/CONSTITUTION.md")]), Expect::Allow),
            ("S-M57g", "member: bind in its own lane directory run/fleet/b1/ (kernel-owned)", ops(&["bind", &li("run/fleet/b1/x.sock")]), Expect::Deny),
        ];
        for (id, what, op, expect) in rows {
            self.check(id, &m, what, &op, expect);
        }
    }

    /// The layout a grant is composed with: for a full grant (ADR-012) the full deny lists, as the Root's
    /// `zone::launcher` picks them (with `tax/` as the charter's read_deny entry and the fake home's `.local/bin`
    /// as INIT's bin directory).
    fn layout_for(&self, g: &Grant) -> ZoneLayout {
        let mut layout = self.layout.clone();
        if g.full {
            layout.read_deny = access::full_read_deny(&self.h.run, &[access::other_installs(&self.h.run, &self.h.r, &self.h.i), vec![self.h.run.join("tax")]].concat());
            layout.write_deny = access::full_write_deny(&self.h.run, None);
        }
        layout
    }

    /// A live body with the grant `access::grant_for` gives kind `body` on the live lane for `st`.
    fn live_body_granted(&mut self, label: &str, st: &AccessState) -> Option<Instance> {
        let i = self.h.i.clone();
        let g = access::grant_for("body", true, st, &self.layout.home);
        let layout = self.layout_for(&g.grant);
        let params = self.body_params(&i, &i.join("run"), &i.join("kernel"), &i.join("bin"), &i.join("kernel"));
        let env = self.base_env(&i.join("work/home"), &i.join("work/tmp"));
        let (exe, cwd) = (i.join("bin/sbx-probe"), i.join("work"));
        self.instance_with(label, Some(("body", ProfileId::Body)), None, params, &layout, exe, cwd, env, &g.grant)
    }

    /// S-A27's directory: a fresh `init-sbx-full-<run id>` in the per-user temp directory (`/var/folders/…`, outside
    /// the home and `/tmp`), created here, unsandboxed; removed by its exact path after the full rows.
    fn full_tmp_dir(&self) -> Option<PathBuf> {
        let base = sys::darwin_user_temp_dir().or_else(|| Some(std::env::temp_dir())).and_then(|b| canon(&b).ok())?;
        let name = self.h.run.file_name()?.to_string_lossy().into_owned();
        let d = base.join(format!("init-sbx-full-{name}"));
        fs::create_dir(&d).ok().map(|()| d)
    }

    /// ADR-009 (build/specs/ADR-009-spec.md, Tests): the read grant, the workspace and full mode, against the
    /// run directory as a fake HOME (`Z_HOME`), with `tax/` as a charter read_deny entry. Every write outside
    /// the run directory is a policy query; the real home is never granted.
    fn access_checks(&mut self) {
        let fake = self.h.run.clone();
        let hm = |x: &str| s(&fake.join(x));
        let (ih, rh) = (self.h.i.clone(), self.h.r.clone());
        let li = |x: &str| s(&ih.join(x));
        let r = |x: &str| s(&rh.join(x));
        let ws = fake.join("ws");
        let read_only = AccessState { read_home: true, workspace: None, full: false };
        let with_ws = AccessState { workspace: Some(ws.clone()), ..read_only.clone() };
        let full = AccessState { full: true, ..with_ws.clone() };
        let home_real = s(&self.cfg.home_real);
        let svc = s(&self.h.svc);
        let real_ssh = s(&self.cfg.home_real.join(".ssh"));
        let tp = self.tcp_port();

        if let Some(b) = self.live_body_granted("body/read_home", &read_only) {
            self.parses("S-M0.read_home", &b);
            let rows: Vec<(&str, &str, Vec<String>, Expect)> = vec![
                ("S-A1", "read_home: read ~/Documents/x", ops(&["read", &hm("Documents/x")]), Expect::Allow),
                ("S-A1b", "read_home: list ~/Documents", ops(&["list", &hm("Documents")]), Expect::Allow),
                ("S-A2", "read_home: read the fake ~/.ssh/id (deny list)", ops(&["read", &hm(".ssh/id")]), Expect::Deny),
                ("S-A2b", "read_home: list the fake ~/.ssh (deny list)", ops(&["list", &hm(".ssh")]), Expect::Deny),
                ("S-A2c", "read_home: read ~/.config/init-agent/ (the key dir, deny list)", ops(&["read", &hm(".config/init-agent/deepseek.key")]), Expect::Deny),
                ("S-A2d", "read_home: read a charter read_deny path", ops(&["read", &hm("tax/2025.txt")]), Expect::Deny),
                // v0.5.3 (lane H): credential stores the v0.5.2 list missed, and another INIT install's homes
                ("S-A2e", "read_home: read the fake ~/.git-credentials (deny list, v0.5.3)", ops(&["read", &hm(".git-credentials")]), Expect::Deny),
                ("S-A2f", "read_home: read the fake ~/.config/gh/hosts.yml (deny list, v0.5.3)", ops(&["read", &hm(".config/gh/hosts.yml")]), Expect::Deny),
                ("S-A2g", "read_home: read the fake ~/.cargo/credentials.toml (deny list, v0.5.3)", ops(&["read", &hm(".cargo/credentials.toml")]), Expect::Deny),
                ("S-A2h", "read_home: read the fake ~/.config/cloudflare/token (deny list, v0.5.3)", ops(&["read", &hm(".config/cloudflare/token")]), Expect::Deny),
                ("S-A2m", "read_home: read the fake ~/.cloudflared/cert.pem (deny list, v0.5.3)", ops(&["read", &hm(".cloudflared/cert.pem")]), Expect::Deny),
                ("S-A2i", "read_home: read another install's ~/.init-root/keys/ (other_installs, v0.5.3)", ops(&["read", &hm(".init-root/keys/lineage.key")]), Expect::Deny),
                ("S-A2j", "read_home: list another install's ~/.init-root-demo/keys (other_installs, v0.5.3)", ops(&["list", &hm(".init-root-demo/keys")]), Expect::Deny),
                ("S-A2k", "read_home: read another install's ~/.init/kernel/ (other_installs, v0.5.3)", ops(&["read", &hm(".init/kernel/bin/init-kernel")]), Expect::Deny),
                ("S-A2l", "read_home: read the fake release signing key ~/.init-release/ (deny list, v0.5.3)", ops(&["read", &hm(".init-release/release.key")]), Expect::Deny),
                ("S-A3", "read_home: write ~/Documents (no write grant)", ops(&["write", &hm("Documents/w")]), Expect::Deny),
                ("S-A4", "read_home: read ROOT_HOME (Z1)", ops(&["read", &r("secret")]), Expect::Deny),
                ("S-A5", "read_home: read vault/ (INIT_HOME keeps its own rules)", ops(&["read", &li("vault/secret")]), Expect::Deny),
                ("S-A5b", "read_home: read cache/", ops(&["read", &li("cache/c")]), Expect::Deny),
                ("S-A5c", "read_home: read a probe root's task", ops(&["read", &li("probe/p1/work/task.txt")]), Expect::Deny),
                ("S-A6", "read_home: read the Root's service file (Z2)", ops(&["read", &svc]), Expect::Deny),
                ("S-A6b", "read_home: list the real home's ~/.ssh (outside the fake HOME)", ops(&["list", &real_ssh]), Expect::Deny),
            ];
            for (id, what, op, expect) in rows {
                self.check(id, &b, what, &op, expect);
            }
        }

        // Real repositories in the workspace for the git regression rows, built unsandboxed. ws/repo exercises commits
        // and a branch named modules/x; ws/super/.git/modules/sub is a submodule-style gitdir, to prove commits still
        // write under .git/modules/<m>/. The Z3b file and admin-directory denies are exercised against the fake ws/.git
        // tree, ws/renamerepo and these repos.
        let repo = ws.join("repo");
        let repo_s = s(&repo);
        let git = s(&self.cfg.git);
        let repo_ready = fs::create_dir_all(&repo).is_ok()
            && self.git_ok(&["-C", &repo_s, "init", "-q"]).is_ok()
            && fs::write(repo.join("tracked.txt"), "initial\n").is_ok();
        let subwt = ws.join("super/sub");
        let subwt_s = s(&subwt);
        let subgit = ws.join("super/.git/modules/sub");
        let sub_ready = fs::create_dir_all(&subwt).is_ok()
            && fs::create_dir_all(ws.join("super/.git/modules")).is_ok()
            && self.git_ok(&["-C", &subwt_s, "init", "-q", "--separate-git-dir", &s(&subgit)]).is_ok()
            && fs::write(subwt.join("s.txt"), "sub\n").is_ok();
        // Agent-built directories to rename onto a gitdir admin entry (the rename-swap bypass), and a parent the nested
        // rename target needs.
        for d in ["ev1", "ev2", "ev3", "ev4"] {
            let _ = fs::create_dir_all(ws.join(d));
        }
        let _ = fs::create_dir_all(ws.join(".git/modules/sub/modules"));

        if let Some(b) = self.live_body_granted("body/workspace", &with_ws) {
            self.parses("S-M0.workspace", &b);
            // git runs inside the sandbox under the workspace profile; identity is passed on the command line.
            let gcc = |dir: &str, sub: &[&str]| -> Vec<String> {
                let mut v = ops(&["exec", &git, "-C", dir, "-c", "user.name=wp6a", "-c", "user.email=wp6a@init.invalid", "-c", "commit.gpgsign=false"]);
                v.extend(sub.iter().map(|x| (*x).to_owned()));
                v
            };
            let mut rows: Vec<(&str, &str, Vec<String>, Expect)> = vec![
                ("S-A7", "workspace: write a new file in the workspace", ops(&["write", &hm("ws/new.txt")]), Expect::Allow),
                ("S-A7b", "workspace: overwrite a file deep in the workspace", ops(&["write", &hm("ws/src/a.txt")]), Expect::Allow),
                ("S-A7c", "workspace: read ~/Documents/x (read grant)", ops(&["read", &hm("Documents/x")]), Expect::Allow),
                ("S-A8", "workspace: write elsewhere in the home", ops(&["write", &hm("other/x")]), Expect::Deny),
                ("S-A8b", "workspace: write ~/Documents", ops(&["write", &hm("Documents/y")]), Expect::Deny),
                ("S-A8c", "workspace: write in the home itself", ops(&["write", &hm("top.txt")]), Expect::Deny),
                ("S-A9", "workspace: write ROOT_HOME (Z1)", ops(&["write", &r("w.txt")]), Expect::Deny),
                ("S-A9b", "workspace: write INIT_HOME/kernel/ (Z4)", ops(&["write", &li("kernel/w")]), Expect::Deny),
                ("S-A9c", "workspace: write work/ (the SECTION's own rule)", ops(&["write", &li("work/w")]), Expect::Allow),
                ("S-A9d", "workspace: write vault/", ops(&["write", &li("vault/w")]), Expect::Deny),
                ("S-A9e", "workspace: write under /private/tmp (policy query)", ops(&["policy-write", "/private/tmp"]), Expect::Query(false)),
                ("S-A9f", "workspace: write the real home (policy query)", ops(&["policy-write", &home_real]), Expect::Query(false)),
                ("S-A20", "workspace: add a git hook (Z3b)", ops(&["write", &hm("ws/.git/hooks/pre-commit")]), Expect::Deny),
                ("S-A20b", "workspace: write .git/config (Z3b)", ops(&["write", &hm("ws/.git/config")]), Expect::Deny),
                ("S-A20c", "workspace: add a submodule's hook (Z3b)", ops(&["write", &hm("ws/.git/modules/sub/hooks/post-checkout")]), Expect::Deny),
                ("S-A20d", "workspace: write a submodule's config (Z3b)", ops(&["write", &hm("ws/.git/modules/sub/config")]), Expect::Deny),
                ("S-A20e", "workspace: write a git object (the rest of .git stays writable)", ops(&["write", &hm("ws/.git/objects/ab")]), Expect::Allow),
                ("S-A20f", "workspace: write .git/HEAD", ops(&["write", &hm("ws/.git/HEAD")]), Expect::Allow),
                ("S-A20g", "workspace: write .git/config.lock (git's lock file; the rename onto config is denied)", ops(&["write", &hm("ws/.git/config.lock")]), Expect::Allow),
                ("S-A20h", "workspace: write ~/.zshrc (Z3, Z3a)", ops(&["write", &hm(".zshrc")]), Expect::Deny),
                ("S-A20i", "workspace: a git repo inside INIT_HOME keeps a writable .git/config (the kernel's clones)", ops(&["write", &li("work/r/.git/config")]), Expect::Allow),
                // v0.5.1 Z3b: the config/command sources git reads, and the .git entry itself, are unwritable outside INIT_HOME.
                ("S-A20l", "workspace: write .git/commondir (redirect the common dir at an agent-written config; Z3b bypass a)", ops(&["write", &hm("ws/.git/commondir")]), Expect::Deny),
                ("S-A20m", "workspace: write .git/config.worktree (worktreeConfig; Z3b bypass c)", ops(&["write", &hm("ws/.git/config.worktree")]), Expect::Deny),
                ("S-A20n", "workspace: write .git/worktrees/wt/commondir (Z3b bypass c)", ops(&["write", &hm("ws/.git/worktrees/wt/commondir")]), Expect::Deny),
                ("S-A20o", "workspace: write .git/worktrees/wt/config.worktree (Z3b bypass c)", ops(&["write", &hm("ws/.git/worktrees/wt/config.worktree")]), Expect::Deny),
                ("S-A20p", "workspace: write .git/worktrees/wt/gitdir (Z3b bypass c)", ops(&["write", &hm("ws/.git/worktrees/wt/gitdir")]), Expect::Deny),
                ("S-A20q", "workspace: write .git/modules/sub/config.worktree (Z3b bypass c)", ops(&["write", &hm("ws/.git/modules/sub/config.worktree")]), Expect::Deny),
                ("S-A20r", "workspace: write .git/modules/sub/commondir (Z3b bypass c)", ops(&["write", &hm("ws/.git/modules/sub/commondir")]), Expect::Deny),
                ("S-A20s", "workspace: create a .git gitdir: file redirecting the git dir (Z3b bypass b)", ops(&["write", &hm("ws/gitdirfile/.git")]), Expect::Deny),
                ("S-A20t", "workspace: rename a repo's .git directory aside (Z3b bypass b)", ops(&["rename", &hm("ws/renamerepo/.git"), &hm("ws/renamerepo/gitmoved")]), Expect::Deny),
            ];
            // Committing still works: in a repo, on a branch named modules/x (a ref, not a gitdir), and in a submodule.
            if repo_ready {
                rows.push(("S-A20j", "workspace: git add -A in a real repo (the rest of .git stays writable)", gcc(&repo_s, &["add", "-A"]), Expect::Allow));
                rows.push(("S-A20k", "workspace: git commit in a real repo (objects, refs, index, HEAD, logs writable)", gcc(&repo_s, &["commit", "-q", "-m", "wp6a"]), Expect::Allow));
                rows.push(("S-A20A", "workspace: git checkout -b modules/x (a branch named modules/* is a writable ref)", gcc(&repo_s, &["checkout", "-b", "modules/x"]), Expect::Allow));
                rows.push(("S-A20B", "workspace: git commit on branch modules/x (refs/heads/modules/x, logs writable)", gcc(&repo_s, &["commit", "--allow-empty", "-q", "-m", "y"]), Expect::Allow));
            } else {
                self.push("S-A20j", "body/workspace", "git add+commit in a real workspace repo", Status::Fail("could not initialise the test repository".into()));
            }
            if sub_ready {
                rows.push(("S-A20C", "workspace: git add -A inside a submodule (writes .git/modules/sub/objects, index)", gcc(&subwt_s, &["add", "-A"]), Expect::Allow));
                rows.push(("S-A20D", "workspace: git commit inside a submodule (writes .git/modules/sub/refs, logs)", gcc(&subwt_s, &["commit", "-q", "-m", "sub"]), Expect::Allow));
            } else {
                self.push("S-A20C", "body/workspace", "git add+commit inside a submodule gitdir", Status::Fail("could not initialise the submodule repository".into()));
            }
            // v0.5.1 Z3b (cont.): a whole gitdir cannot be swapped in by renaming a directory onto a modules/worktrees
            // admin entry, at any depth; renaming such an admin directory or a gitdir aside is denied too.
            rows.push(("S-A20u", "workspace: rename an agent dir onto .git/modules/<m> (swap a submodule gitdir)", ops(&["rename", &hm("ws/ev1"), &hm("ws/.git/modules/newmod")]), Expect::Deny));
            rows.push(("S-A20v", "workspace: rename an agent dir onto .git/modules/sub/modules/<b> (nested)", ops(&["rename", &hm("ws/ev2"), &hm("ws/.git/modules/sub/modules/nested")]), Expect::Deny));
            rows.push(("S-A20w", "workspace: rename an agent dir onto .git/worktrees/<w> (swap a linked-worktree gitdir)", ops(&["rename", &hm("ws/ev3"), &hm("ws/.git/worktrees/w2")]), Expect::Deny));
            rows.push(("S-A20x", "workspace: rename an agent dir onto a repo's .git/modules admin dir", ops(&["rename", &hm("ws/ev4"), &hm("ws/renamerepo/.git/modules")]), Expect::Deny));
            rows.push(("S-A20y", "workspace: move an existing submodule gitdir .git/modules/sub aside", ops(&["rename", &hm("ws/.git/modules/sub"), &hm("ws/.git/modules/sub_bak")]), Expect::Deny));
            rows.push(("S-A20z", "workspace: move the whole .git/modules admin dir aside", ops(&["rename", &hm("ws/.git/modules"), &hm("ws/.git/modules_bak")]), Expect::Deny));
            for (id, what, op, expect) in rows {
                self.check(id, &b, what, &op, expect);
            }
        }

        // ADR-012 full access (the v0.3.1 default): the live body reads, writes and runs anything outside INIT_HOME and
        // uses the IP network, except the Root-protected set. Z_FULL is "/", so every write row targets a path this
        // run created (the fake home, or the temp directory below); the real home only gets policy queries.
        let tmp = self.full_tmp_dir();
        if let Some(b) = self.live_body_granted("body/full", &full) {
            self.parses("S-M0.full", &b);
            let up = self.udp_port();
            let tmp_x = tmp.as_ref().map(|d| s(&d.join("x")));
            let exe = fake.join("other/sbx-probe-copy");
            let exe_op: Vec<String> = [s(&exe)].into_iter().chain(self.cfg.probe_prefix.iter().cloned()).chain(["noop".to_string()]).collect();
            let mut exec = ops(&["exec"]);
            exec.extend(exe_op);
            let rows: Vec<(&str, &str, Vec<String>, Expect)> = vec![
                ("S-A10", "full: write elsewhere in the home", ops(&["write", &hm("other/x")]), Expect::Allow),
                ("S-A10b", "full: write ~/Documents", ops(&["write", &hm("Documents/z")]), Expect::Allow),
                ("S-A10c", "full: write the workspace", ops(&["write", &hm("ws/full.txt")]), Expect::Allow),
                ("S-A11", "full: write ROOT_HOME (Z1)", ops(&["write", &r("w.txt")]), Expect::Deny),
                ("S-A11b", "full: read ROOT_HOME (Z1)", ops(&["read", &r("secret")]), Expect::Deny),
                ("S-A11c", "full: list ROOT_HOME's keys (Z1)", ops(&["list", &r("keys/providers")]), Expect::Deny),
                ("S-A12", "full: write the fake ~/.ssh (ADR-012: the secrets list is workspace mode's)", ops(&["write", &hm(".ssh/authorized_keys")]), Expect::Allow),
                ("S-A12b", "full: read the fake ~/.ssh/id (ADR-012: git push over ssh works)", ops(&["read", &hm(".ssh/id")]), Expect::Allow),
                ("S-A12c", "full: write ~/.config/init-agent/ (INIT's key dir, still denied)", ops(&["write", &hm(".config/init-agent/k")]), Expect::Deny),
                ("S-A12d", "full: write a charter read_deny path (still honoured)", ops(&["write", &hm("tax/x")]), Expect::Deny),
                ("S-A12e", "full: read ~/.config/init-agent/deepseek.key (INIT's key dir)", ops(&["read", &hm(".config/init-agent/deepseek.key")]), Expect::Deny),
                ("S-A12f", "full: read a charter read_deny path", ops(&["read", &hm("tax/2025.txt")]), Expect::Deny),
                // v0.5.3 (lane H): another INIT install's homes are the Root-protected set too; the secrets list stays
                // workspace mode's
                ("S-A12g", "full: read another install's ~/.init-root/keys/ (other_installs, v0.5.3)", ops(&["read", &hm(".init-root/keys/lineage.key")]), Expect::Deny),
                ("S-A12h", "full: replace another install's Root binary ~/.init-root/bin/init-root (v0.5.3)", ops(&["write", &hm(".init-root/bin/init-root")]), Expect::Deny),
                ("S-A12i", "full: read another install's ~/.init-root-demo/keys/ (v0.5.3)", ops(&["read", &hm(".init-root-demo/keys/lineage.key")]), Expect::Deny),
                ("S-A12j", "full: write another install's ~/.init/kernel/ (v0.5.3)", ops(&["write", &hm(".init/kernel/bin/init-kernel")]), Expect::Deny),
                ("S-A12k", "full: read the fake ~/.git-credentials (ADR-012: the secrets list is workspace mode's)", ops(&["read", &hm(".git-credentials")]), Expect::Allow),
                ("S-A12l", "full: read the fake release signing key ~/.init-release/ (FULL_READ_DENY, v0.5.3)", ops(&["read", &hm(".init-release/release.key")]), Expect::Deny),
                ("S-A12m", "full: list ~/.init-release (FULL_READ_DENY, v0.5.3)", ops(&["list", &hm(".init-release")]), Expect::Deny),
                ("S-A12n", "full: write into ~/.init-release (FULL_READ_DENY, v0.5.3)", ops(&["write", &hm(".init-release/x")]), Expect::Deny),
                ("S-A13", "full: write INIT_HOME/kernel/ (Z4)", ops(&["write", &li("kernel/f")]), Expect::Deny),
                ("S-A13b", "full: write vault/", ops(&["write", &li("vault/f")]), Expect::Deny),
                ("S-A13c", "full: write state/kernel/", ops(&["write", &li("state/kernel/f")]), Expect::Deny),
                ("S-A13d", "full: write run/mark/ (Z4)", ops(&["write", &li("run/mark/f")]), Expect::Deny),
                ("S-A13e", "full: write lineage/", ops(&["write", &li("lineage/f")]), Expect::Deny),
                ("S-A13f", "full: read vault/ (INIT_HOME keeps the SECTION's rules)", ops(&["read", &li("vault/secret")]), Expect::Deny),
                ("S-A14", "full: add a file next to the Root's service file (Z2)", ops(&["write", &hm("svc/evil.plist")]), Expect::Deny),
                ("S-A14b", "full: overwrite the Root's service file (Z2)", ops(&["write", &svc]), Expect::Deny),
                ("S-A14c", "full: read the Root's service file (Z2)", ops(&["read", &svc]), Expect::Deny),
                ("S-A15", "full: connect the owner socket in ROOT_HOME (Z1)", ops(&["connect", &r("run/owner.sock")]), Expect::Deny),
                ("S-A15c", "full: connect the egress socket in ROOT_HOME (Z1)", ops(&["connect", &r("run/egress.sock")]), Expect::Deny),
                ("S-A15b", "full: TCP connect to the suite's 127.0.0.1 listener (ADR-012)", ops(&["tcp-connect", &tp]), Expect::Allow),
                ("S-A15d", "full: bind a loopback TCP listener (ADR-012)", ops(&["tcp-bind-loopback"]), Expect::Allow),
                ("S-A15e", "full: UDP send to the suite's 127.0.0.1 socket (ADR-012)", ops(&["udp-send", &up]), Expect::Allow),
                ("S-A16", "full: write the real home (policy query; ADR-012)", ops(&["policy-write", &home_real]), Expect::Query(true)),
                ("S-A16b", "full: write under /private/tmp (policy query; ADR-012)", ops(&["policy-write", "/private/tmp"]), Expect::Query(true)),
                ("S-A16c", "full: run a program copied into the home (map-executable outside INIT_HOME)", exec, Expect::Allow),
                ("S-A21", "full: overwrite ~/.zshrc (ADR-012: deferred execution is a residual)", ops(&["write", &hm(".zshrc")]), Expect::Allow),
                ("S-A21b", "full: create ~/.zshenv (ADR-012)", ops(&["write", &hm(".zshenv")]), Expect::Allow),
                ("S-A21c", "full: overwrite ~/.bash_profile (ADR-012)", ops(&["write", &hm(".bash_profile")]), Expect::Allow),
                ("S-A21d", "full: overwrite ~/.profile (ADR-012)", ops(&["write", &hm(".profile")]), Expect::Allow),
                ("S-A21e", "full: write ~/.config/fish/ (ADR-012)", ops(&["write", &hm(".config/fish/config.fish")]), Expect::Allow),
                ("S-A22", "full: overwrite ~/.gitconfig (ADR-012)", ops(&["write", &hm(".gitconfig")]), Expect::Allow),
                ("S-A22b", "full: write ~/.config/git/ (ADR-012)", ops(&["write", &hm(".config/git/config")]), Expect::Allow),
                ("S-A23", "full: replace <bin_dir>/init (Z3a, INIT's launcher's old name; the macOS shim)", ops(&["write", &hm(".local/bin/init")]), Expect::Deny),
                ("S-A23a", "full: replace <bin_dir>/init-agent (Z3a, INIT's launcher)", ops(&["write", &hm(".local/bin/init-agent")]), Expect::Deny),
                ("S-A23b", "full: replace <bin_dir>/init-tui (Z3a, INIT's launcher)", ops(&["write", &hm(".local/bin/init-tui")]), Expect::Deny),
                ("S-A23c", "full: add a command to ~/.local/bin (ADR-012: only the launchers are denied)", ops(&["write", &hm(".local/bin/git")]), Expect::Allow),
                ("S-A23d", "full: add a command to ~/bin (ADR-012)", ops(&["write", &hm("bin/ls")]), Expect::Allow),
                ("S-A24", "full: add a login item in ~/Library/LaunchAgents (Z3a, the service directory)", ops(&["write", &hm("Library/LaunchAgents/x.plist")]), Expect::Deny),
                ("S-A24b", "full: add an autostart entry in ~/.config/autostart (ADR-012)", ops(&["write", &hm(".config/autostart/x.desktop")]), Expect::Allow),
                ("S-A25", "full: add a hook to another repository in the home (ADR-012)", ops(&["write", &hm("other/.git/hooks/pre-commit")]), Expect::Allow),
                ("S-A25b", "full: write another repository's .git/config (ADR-012)", ops(&["write", &hm("other/.git/config")]), Expect::Allow),
                ("S-A25c", "full: add a git hook in the workspace (ADR-012)", ops(&["write", &hm("ws/.git/hooks/post-merge")]), Expect::Allow),
                ("S-A25d", "full: write a git object in the workspace", ops(&["write", &hm("ws/.git/objects/cd")]), Expect::Allow),
            ];
            if let Err(e) = fs::copy(&self.cfg.probe_exe, &exe).and_then(|_| fs::set_permissions(&exe, fs::Permissions::from_mode(0o755))) {
                self.push("SETUP", "body/full", "copy the probe into the fake home", Status::Fail(e.to_string()));
            }
            for (id, what, op, expect) in rows {
                self.check(id, &b, what, &op, expect);
            }
            let what = "full: write a new temp directory outside the home and /tmp (ADR-012)";
            match &tmp_x {
                Some(x) => self.check("S-A27", &b, what, &ops(&["write", x]), Expect::Allow),
                None => self.push("S-A27", "body/full", what, Status::Fail("setup: no per-user temp directory".into())),
            }
            self.kill0_own_sleep("S-A28", &b); // the Root stand-in: a process outside the sandbox this suite spawned
            let last = vec![
                ("S-A17", "full: rename the fake ~/.ssh away (ADR-012)", ops(&["rename", &hm(".ssh"), &hm("ssh2")]), Expect::Allow),
                ("S-A17b", "full: rename the Root's service directory (Z2)", ops(&["rename", &hm("svc"), &hm("svc2")]), Expect::Deny),
                ("S-A26", "full: rename ~/.local/bin away (ADR-012: the launchers are file denies)", ops(&["rename", &hm(".local/bin"), &hm(".local/bin2")]), Expect::Allow),
                ("S-A26b", "full: rename the workspace's .git/hooks away (ADR-012)", ops(&["rename", &hm("ws/.git/hooks"), &hm("ws/.git/h2")]), Expect::Allow),
            ];
            for (id, what, op, expect) in last {
                self.check(id, &b, what, &op, expect);
                // An allowed rename is put back (unsandboxed, by the exact paths): later rows (RR-1x2, RR-2j) use them.
                if let [_, from, to] = op.as_slice()
                    && !Path::new(from).exists()
                    && Path::new(to).exists()
                    && let Err(e) = fs::rename(to, from)
                {
                    self.push("SETUP", "body/full", &format!("rename {to} back"), Status::Fail(e.to_string()));
                }
            }
        }
        if let Some(d) = &tmp {
            // Removed by the exact paths this run created (the file only if the row wrote it).
            let _ = fs::remove_file(d.join("x"));
            let _ = fs::remove_dir(d);
        }

        // A fleet member gets the read grant only; a probe gets nothing (gate evaluation stays hermetic).
        let i = self.h.i.clone();
        let layout = self.layout.clone();
        let root = i.join("fleet/b1");
        let g = access::grant_for("body", false, &full, &layout.home);
        let never_full = |g: &Grant| if g.full { Status::Fail(format!("{g:?}")) } else { Status::Pass };
        self.push("S-A18e", "body/member+grant", "member: grant_for never gives full access, with full on for the live body", never_full(&g.grant));
        let params = self.body_params(&root, &i.join("run/fleet/b1"), &root.join("kernel"), &root.join("bin"), &root.join("kernel"));
        let env = self.base_env(&root.join("work/home"), &root.join("work/tmp"));
        let (exe, cwd) = (root.join("bin/sbx-probe"), root.join("work"));
        if let Some(m) = self.instance_with("body/member+grant", Some(("body", ProfileId::Body)), None, params, &layout, exe, cwd, env, &g.grant) {
            self.check("S-A18", &m, "member: read ~/Documents/x (read grant)", &ops(&["read", &hm("Documents/x")]), Expect::Allow);
            self.check("S-A18b", &m, "member: write the workspace (no write grant)", &ops(&["write", &hm("ws/m.txt")]), Expect::Deny);
            self.check("S-A18c", &m, "member: write elsewhere in the home (never full)", &ops(&["write", &hm("other/m.txt")]), Expect::Deny);
            self.check("S-A18d", &m, "member: TCP connect to the suite's 127.0.0.1 listener (never full: Z6)", &ops(&["tcp-connect", &tp]), Expect::Deny);
        }
        let root = i.join("probe/p1");
        let g = access::grant_for("probe", true, &full, &layout.home);
        self.push("S-A19d", "body/probe+grant", "probe: grant_for never gives full access, with full on for the live body", never_full(&g.grant));
        let params = self.body_params(&root, &root.join("run"), &root.join("kernel"), &root.join("bin"), &root.join("kernel"));
        let env = self.base_env(&root.join("work/home"), &root.join("work/tmp"));
        let (exe, cwd) = (root.join("bin/sbx-probe"), root.join("work"));
        if let Some(p) = self.instance_with("body/probe+grant", Some(("probe", ProfileId::Body)), None, params, &layout, exe, cwd, env, &g.grant) {
            self.check("S-A19", &p, "probe: read ~/Documents/x (no grant)", &ops(&["read", &hm("Documents/x")]), Expect::Deny);
            self.check("S-A19b", &p, "probe: write the workspace (no grant)", &ops(&["write", &hm("ws/p.txt")]), Expect::Deny);
            self.check("S-A19c", &p, "probe: TCP connect to the suite's 127.0.0.1 listener (never full: Z6)", &ops(&["tcp-connect", &tp]), Expect::Deny);
        }
    }

    /// RR-4b: a `LINK_*` naming a real symlink inside a probe root (D8 exemption) is accepted and
    /// the probe can traverse it.
    fn link_probe_checks(&mut self) {
        let i = self.h.i.clone();
        let root = i.join("probe/p2");
        let params = self.body_params(&root, &root.join("run"), &i.join("kernel"), &root.join("bin"), &root.join("kernel"));
        let env = self.base_env(&root.join("work"), &root.join("work"));
        let layout = self.layout.clone();
        let Some(p) =
            self.instance("body/probe-link", Some(("probe", ProfileId::Body)), None, params, &layout, root.join("bin/sbx-probe"), root.join("work"), env)
        else {
            return;
        };
        let op = ops(&["read", &s(&root.join("kernel/CONSTITUTION.md"))]);
        self.check("RR-4b", &p, "probe: read kernel/ through a LINK_* symlink in the probe root", &op, Expect::Allow);
    }

    /// D9: a charter `extra_read` path is readable by a body; its sibling is not.
    fn extra_read_body(&mut self) {
        let mut layout = self.layout.clone();
        layout.extra_read = vec![self.h.extra.clone()];
        let Some(b) = self.live_body("body/extra_read", &layout) else {
            return;
        };
        let (e, e2) = (s(&self.h.extra.join("f")), s(&self.h.extra2.join("f")));
        self.check("S-D9a", &b, "body: read a charter extra_read path", &ops(&["read", &e]), Expect::Allow);
        self.check("S-D9b", &b, "body: read the sibling of an extra_read path", &ops(&["read", &e2]), Expect::Deny);
    }

    fn build_checks(&mut self) {
        let i = self.h.i.clone();
        let layout = self.layout.clone();
        let (src, out) = (i.join("cache/build/slot/src"), i.join("cache/build/slot/out"));
        let env = self.base_env(&out.join("home"), &out.join("tmp"));
        let params = self.build_params(&src, &out);
        if let Some(b) = self.instance("build", Some(("build", ProfileId::Build)), None, params, &layout, i.join("kernel/bin/sbx-probe"), out.clone(), env) {
            self.parses("S-M0.build", &b);
            // S-B4a: G0's tests serve Unix sockets under OUT (init-mockllm, in-test stubs; G0 sets
            // TMPDIR = OUT/tmp) and connect to them from another process of the same build. Socket
            // paths here are relative to the instance's cwd (OUT), as the seed's tests use them when an
            // absolute path would exceed sun_path; Seatbelt checks the resolved path either way.
            let what = "serve a Unix socket at OUT/tmp/a.sock (bind, accept); a second build instance connects and its line arrives";
            let status = self.serve_and_connect(&b, &b, "serve-once", Path::new("tmp/a.sock"));
            self.push("S-B4a", "build", what, status);
            let li = |x: &str| s(&i.join(x));
            let slot = |x: &str| s(&i.join("cache/build/slot").join(x));
            let (tp, up, t6) = (self.tcp_port(), self.udp_port(), self.tcp6_port());
            let r = |x: &str| s(&self.h.r(x));
            let rows: Vec<(&str, &str, Vec<String>, Expect)> = vec![
                ("S-B1", "write OUT", ops(&["write", &s(&out.join("x"))]), Expect::Allow),
                ("S-B2", "write SRC", ops(&["write", &s(&src.join("x"))]), Expect::Deny),
                ("S-B3", "read the live body/", ops(&["read", &li("body/f")]), Expect::Deny),
                ("S-B4a2", "bind a Unix socket at OUT/home/b.sock", ops(&["bind", "home/b.sock"]), Expect::Allow),
                ("S-B4b", "connect run/kernel.sock (a live listener outside OUT)", ops(&["connect", &li("run/kernel.sock")]), Expect::Deny),
                ("S-B4c", "TCP connect to 127.0.0.1 (the runner's own listener)", ops(&["tcp-connect", &tp]), Expect::Deny),
                ("S-B4d", "bind a Unix socket in SRC (outside OUT)", ops(&["bind", "../src/b.sock"]), Expect::Deny),
                ("S-B4e", "bind slot/outb (OUT's path is a string prefix of it; outside OUT)", ops(&["bind", &slot("outb")]), Expect::Deny),
                ("S-B4f", "connect a live listener at slot/outl (next to OUT)", ops(&["connect", &slot("outl")]), Expect::Deny),
                ("S-B4g", "connect the Root egress socket (the tail leaves it open; the build SECTION does not)", ops(&["connect", &r("run/egress.sock")]), Expect::Deny),
                ("S-B4h", "connect the Root owner socket", ops(&["connect", &r("run/owner.sock")]), Expect::Deny),
                (
                    "S-B4i",
                    "connect a live listener outside INIT_HOME (stands for docker.sock or another ~/.init*)",
                    ops(&["connect", &s(&self.h.outside.join("l.sock"))]),
                    Expect::Deny,
                ),
                (
                    "S-B4j",
                    "connect run/kernel.sock through a symlink created in OUT (resolved path)",
                    ops(&["symlink-connect", "kl.sock", &li("run/kernel.sock")]),
                    Expect::Deny,
                ),
                ("S-B4k", "UDP send to 127.0.0.1 (the runner's own socket)", ops(&["udp-send", &up]), Expect::Deny),
                ("S-B4l", "TCP bind on loopback", ops(&["tcp-bind-loopback"]), Expect::Deny),
                ("S-B4m", "TCP connect to [::1] (the runner's own IPv6 listener)", ops(&["tcp6-connect", &t6]), Expect::Deny),
                ("S-B5", "read vault/", ops(&["read", &li("vault/secret")]), Expect::Deny),
                ("S-B7", "read ROOT_HOME", ops(&["read", &s(&self.h.r("secret"))]), Expect::Deny),
            ];
            for (id, what, op, expect) in rows {
                self.check(id, &b, what, &op, expect);
            }
            if self.cfg.builds {
                let env = self.cargo_env(&out.join("cargo-home"), &out.join("target"), &out.join("home"), &out.join("tmp"));
                let tc = s(&self.cfg.toolchain.join("bin/cargo"));
                let op = ops(&["exec", &tc, "build", "--frozen", "--manifest-path", &s(&src.join("Cargo.toml"))]);
                self.check_env("S-B6", &b, "cargo build --frozen of SRC with output in OUT", &op, Expect::Allow, &env, BUILD_TIMEOUT);
            } else {
                self.push("S-B6", "build", "cargo build --frozen", Status::Skipped("--no-builds".into()));
            }
        }
        // `check`: the build text with acceptance-check parameters.
        let (src, out) = (i.join("cache/check/in"), i.join("cache/check/out"));
        let env = self.base_env(&out, &out);
        let params = self.build_params(&src, &out);
        if let Some(c) = self.instance("check", Some(("check", ProfileId::Check)), None, params, &layout, i.join("kernel/bin/sbx-probe"), out.clone(), env) {
            self.parses("S-M0.check", &c);
            self.check("S-B1c", &c, "check: write OUT", &ops(&["write", &s(&out.join("x"))]), Expect::Allow);
            self.check("S-B2c", &c, "check: write SRC", &ops(&["write", &s(&src.join("y"))]), Expect::Deny);
            self.check("S-B5c", &c, "check: read vault/", &ops(&["read", &s(&i.join("vault/secret"))]), Expect::Deny);
            self.check("S-B4ac", &c, "check: bind a Unix socket in OUT", &ops(&["bind", &s(&out.join("c.sock"))]), Expect::Allow);
            self.check("S-B4bc", &c, "check: connect run/kernel.sock", &ops(&["connect", &s(&i.join("run/kernel.sock"))]), Expect::Deny);
        }
    }

    fn git(&self, args: &[&str]) -> io::Result<Output> {
        let mut c = Command::new(&self.cfg.git);
        c.args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.attributesFile=/dev/null",
            "-c",
            "protocol.allow=never",
            "-c",
            "protocol.file.allow=always",
            "-c",
            "transfer.fsckObjects=true",
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "-c",
            "user.useConfigOnly=true",
            "-c",
            "user.name=wp6a-selftest",
            "-c",
            "user.email=wp6a@init.invalid",
            "-c",
            "init.defaultBranch=main",
        ]);
        c.args(args);
        c.env_clear().envs(self.base_env(&self.h.i("work/githome"), &self.h.i("work/tmp")));
        c.stdin(Stdio::null()).output()
    }

    fn git_ok(&self, args: &[&str]) -> Result<String, String> {
        match self.git(args) {
            Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).trim().to_owned()),
            Ok(o) => Err(format!("git {args:?}: {}", tail(&String::from_utf8_lossy(&o.stderr), 300))),
            Err(e) => Err(format!("git {args:?}: {e}")),
        }
    }

    fn git_read_checks(&mut self) {
        let i = self.h.i.clone();
        let layout = self.layout.clone();
        let env = self.base_env(&i.join("work/githome"), &i.join("work/tmp"));
        let params = self.git_read_params();
        let Some(g) =
            self.instance("git-read", Some(("git_read", ProfileId::GitRead)), None, params, &layout, i.join("work/.sbx/sbx-probe"), i.join("body"), env)
        else {
            return;
        };
        self.parses("S-M0.git-read", &g);
        self.check("S-G3a", &g, "git-read: write body/", &ops(&["write", &s(&i.join("body/g"))]), Expect::Deny);
        self.check("S-G3b", &g, "git-read: write work/", &ops(&["write", &s(&i.join("work/g"))]), Expect::Deny);
        self.check("S-G4", &g, "git-read: read ROOT_HOME", &ops(&["read", &s(&self.h.r("secret"))]), Expect::Deny);

        // The upload-pack shim: the instance's exact argv, with git upload-pack as the program.
        let wrapper = self.h.run.join("upload-pack.sh");
        let mut script = String::from("#!/bin/sh\nexec");
        for a in g.rendered.sandbox_exec_argv() {
            script.push(' ');
            script.push_str(&sh_quote(&a.to_string_lossy()));
        }
        script.push_str(&format!(" -- {} upload-pack \"$@\"\n", sh_quote(&s(&self.cfg.git))));
        let upload_pack = format!("--upload-pack={}", sh_quote(&s(&wrapper)));
        let written = fs::write(&wrapper, script).and_then(|()| fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)));
        if let Err(e) = written {
            self.push("S-G1", "git-read", "fetch from body/", Status::Fail(format!("setup: {e}")));
            return;
        }
        let (body, lin) = (s(&i.join("body")), i.join("lineage"));

        // S-G1: a normal fetch from body/ into a bare repo succeeds.
        let g1 = (|| -> Result<(), String> {
            self.git_ok(&["init", "-q", &body])?;
            self.git_ok(&["-C", &body, "add", "f"])?;
            self.git_ok(&["-C", &body, "commit", "-q", "-m", "selftest"])?;
            let head = self.git_ok(&["-C", &body, "rev-parse", "HEAD"])?;
            let dest = s(&lin.join("body.git"));
            self.git_ok(&["init", "-q", "--bare", &dest])?;
            self.git_ok(&["--git-dir", &dest, "fetch", "-q", &upload_pack, &body, "+refs/heads/main:refs/heads/main"])?;
            let got = self.git_ok(&["--git-dir", &dest, "rev-parse", "refs/heads/main"])?;
            if got == head { Ok(()) } else { Err(format!("fetched {got}, want {head}")) }
        })();
        self.push("S-G1", "git-read", "fetch from body/ through a sandboxed upload-pack succeeds", g1.map_or_else(Status::Fail, |()| Status::Pass));

        // S-G2: alternates pointing into vault/ plus a ref naming a vault commit.
        let alternates = i.join("body/.git/objects/info/alternates");
        let g2 = (|| -> Result<(), String> {
            let v = s(&i.join("vault/v"));
            self.git_ok(&["init", "-q", &v])?;
            fs::write(i.join("vault/v/secret.txt"), "vault object\n").map_err(|e| e.to_string())?;
            self.git_ok(&["-C", &v, "add", "secret.txt"])?;
            self.git_ok(&["-C", &v, "commit", "-q", "-m", "vault"])?;
            let c = self.git_ok(&["-C", &v, "rev-parse", "HEAD"])?;
            fs::write(&alternates, format!("{}\n", s(&i.join("vault/v/.git/objects")))).map_err(|e| e.to_string())?;
            self.git_ok(&["-C", &body, "update-ref", "refs/heads/evil", &c])?;
            let (dest, control) = (s(&lin.join("g2.git")), s(&lin.join("g2c.git")));
            self.git_ok(&["init", "-q", "--bare", &dest])?;
            self.git_ok(&["init", "-q", "--bare", &control])?;
            let sandboxed = self.git_ok(&["--git-dir", &dest, "fetch", "-q", &upload_pack, &body, "+refs/heads/evil:refs/heads/evil"]);
            let leaked = self.git_ok(&["--git-dir", &dest, "cat-file", "-e", &c]).is_ok();
            // Control: the same fetch without the sandbox copies the vault object.
            let ctl = self.git_ok(&["--git-dir", &control, "fetch", "-q", &body, "+refs/heads/evil:refs/heads/evil"]);
            let ctl_has = self.git_ok(&["--git-dir", &control, "cat-file", "-e", &c]).is_ok();
            match (sandboxed.is_err(), leaked, ctl.is_ok() && ctl_has) {
                (true, false, true) => Ok(()),
                (_, _, false) => Err(format!("control fetch did not copy the object ({ctl:?}); the check proves nothing")),
                (s_failed, leak, _) => Err(format!("sandboxed fetch failed={s_failed}, vault object present={leak}")),
            }
        })();
        // Undo the S-G2 setup by exact paths.
        let _ = fs::remove_file(&alternates);
        let _ = self.git_ok(&["-C", &body, "update-ref", "-d", "refs/heads/evil"]);
        self.push(
            "S-G2",
            "git-read",
            "alternates into vault/: the fetch fails and the object never reaches lineage",
            g2.map_or_else(Status::Fail, |()| Status::Pass),
        );
    }

    fn kernel_checks(&mut self, k: &Instance) {
        let h = &self.h;
        let i = |x: &str| s(&h.i(x));
        let r = |x: &str| s(&h.r(x));
        let tc = s(&self.cfg.toolchain.join("bin/cargo"));
        let git = s(&self.cfg.git);
        let (tp, up) = (self.tcp_port(), self.udp_port());
        let rows: Vec<(&str, &str, Vec<String>, Expect)> = vec![
            ("S-Z1", "kernel: write state/kernel/", ops(&["write", &i("state/kernel/zk")]), Expect::Allow),
            ("S-Z2a.bin", "kernel: write bin/", ops(&["write", &i("bin/zk")]), Expect::Allow),
            ("S-Z2a.lineage", "kernel: write lineage/", ops(&["write", &i("lineage/zk")]), Expect::Allow),
            ("S-Z2b.cache", "kernel: write cache/", ops(&["write", &i("cache/zk")]), Expect::Allow),
            ("S-Z2b.vault", "kernel: write vault/", ops(&["write", &i("vault/zk")]), Expect::Allow),
            ("S-Z2c.probe", "kernel: write probe/", ops(&["write", &i("probe/zk")]), Expect::Allow),
            ("S-Z2c.fleet", "kernel: write fleet/", ops(&["write", &i("fleet/zk")]), Expect::Allow),
            ("S-Z2c.run", "kernel: write run/", ops(&["write", &i("run/zk")]), Expect::Allow),
            ("S-Z3a", "kernel: write kernel/", ops(&["write", &i("kernel/zk")]), Expect::Deny),
            ("S-Z3d", "kernel: write root/ (Z4)", ops(&["write", &i("root/zk")]), Expect::Deny),
            ("S-Z3e", "kernel: write run/mark/ (Z4)", ops(&["write", &i("run/mark/zk")]), Expect::Deny),
            ("S-Z3f", "kernel: write lineage/kernel.git/ (Z4)", ops(&["write", &i("lineage/kernel.git/zk")]), Expect::Deny),
            ("S-Z3g", "kernel: overwrite run/zone-mark (Z4)", ops(&["write", &i("run/zone-mark")]), Expect::Deny),
            ("S-Z4a", "kernel: read ROOT_HOME (Z1)", ops(&["read", &r("secret")]), Expect::Deny),
            ("S-Z4b", "kernel: list ROOT_HOME (Z1)", ops(&["list", &s(&h.r)]), Expect::Deny),
            ("S-Z4c", "kernel: read the Root's service file (Z2)", ops(&["read", &s(&h.svc)]), Expect::Deny),
            ("S-Z5", "kernel: connect the Root owner socket", ops(&["connect", &r("run/owner.sock")]), Expect::Deny),
            ("S-Z6", "kernel: connect the Root egress socket", ops(&["connect", &r("run/egress.sock")]), Expect::Allow),
            ("S-Z7a", "kernel: TCP connect to 127.0.0.1 (the runner's own listener)", ops(&["tcp-connect", &tp]), Expect::Deny),
            ("S-Z7b", "kernel: UDP send to 127.0.0.1 (the runner's own socket)", ops(&["udp-send", &up]), Expect::Deny),
            ("S-Z7c", "kernel: connect the resolver socket (DNS path)", ops(&["connect", "/private/var/run/mDNSResponder"]), Expect::Deny),
            ("S-Z7d", "kernel: TCP bind on loopback", ops(&["tcp-bind-loopback"]), Expect::Deny),
            ("S-Z8a", "kernel: bind a Unix socket under run/", ops(&["bind", &i("run/zs.sock")]), Expect::Allow),
            ("S-Z8c", "kernel: bind under run/fleet/b1/", ops(&["bind", &i("run/fleet/b1/zs.sock")]), Expect::Allow),
            ("S-Z8d", "kernel: bind under fleet/b1/run/", ops(&["bind", &i("fleet/b1/run/zs.sock")]), Expect::Deny),
            (
                "S-Z9",
                "kernel: git writes an object into lineage/body.git",
                ops(&["exec", &git, "-c", "core.hooksPath=/dev/null", "--git-dir", &i("lineage/body.git"), "hash-object", "-w", &i("lineage/x")]),
                Expect::Allow,
            ),
            (
                "S-Z12",
                "kernel: nested sandbox-exec, even (allow default)",
                ops(&["exec", "/usr/bin/sandbox-exec", "-p", "(version 1)(allow default)", "/usr/bin/true"]),
                Expect::Code(71),
            ),
            ("S-Z13a", "kernel: list ~/Documents", ops(&["list", &s(&self.cfg.home_real.join("Documents"))]), Expect::Deny),
            ("S-Z13b", "kernel: write outside INIT_HOME (Z3)", ops(&["write", &s(&h.outside.join("zk"))]), Expect::Deny),
            ("S-Z14a", "kernel: list /opt/homebrew/bin", ops(&["list", "/opt/homebrew/bin"]), Expect::Allow),
            ("S-Z14b", "kernel: list /opt/homebrew/var", ops(&["list", "/opt/homebrew/var"]), Expect::Deny),
            ("S-Z15a", "kernel: exec cargo", ops(&["exec", &tc, "--version"]), Expect::Allow),
            ("S-Z15b", "kernel: exec tar", ops(&["exec", "/usr/bin/tar", "--version"]), Expect::Allow),
            ("S-Z16", "kernel: read INIT_HOME/root/ledger.json", ops(&["read", &i("root/ledger.json")]), Expect::Allow),
            ("RR-2a", "kernel: read its own mark", ops(&["read", &s(&k.mark)]), Expect::Allow),
            ("RR-2b", "kernel: read the decoy (Z5)", ops(&["read", &i("run/mark/decoy")]), Expect::Deny),
            ("RR-2c", "kernel: read another mark (Z5)", ops(&["read", &i("run/mark/0000feedf00d")]), Expect::Deny),
            ("RR-2d", "kernel: read the zone mark", ops(&["read", &i("run/zone-mark")]), Expect::Allow),
            ("RR-2e", "kernel: list ~/.ssh", ops(&["list", &s(&self.cfg.home_real.join(".ssh"))]), Expect::Deny),
            ("RR-2f", "kernel: read the canary (Z1)", ops(&["read", &r("run/canary")]), Expect::Deny),
            ("RR-2i", "kernel: read the fake ~/Documents/x (ADR-009 grants are a body's)", ops(&["read", &s(&h.run.join("Documents/x"))]), Expect::Deny),
            ("RR-2j", "kernel: read the fake ~/.ssh/id", ops(&["read", &s(&h.run.join(".ssh/id"))]), Expect::Deny),
        ];
        for (id, what, op, expect) in rows {
            self.check(id, k, what, &op, expect);
        }
        self.slot_out_checks(k);
    }

    /// root/root.md §3.3.3 section 6a: G0's commands run as the kernel's children, so the `kernel`
    /// profile binds and connects Unix sockets under the build slot's OUT, and nowhere else new.
    /// Paths are relative to the kernel instance's cwd (`INIT_HOME`), so they fit `sun_path`.
    fn slot_out_checks(&mut self, k: &Instance) {
        let what = "kernel (a G0 child): serve a Unix socket at slot/out/tmp/p.sock; a second kernel instance connects";
        let status = self.serve_and_connect(k, k, "serve-once", Path::new("cache/build/slot/out/tmp/p.sock"));
        self.push("S-Z8p", "kernel", what, status);
        let (owner, outside) = (s(&self.h.r("run/owner.sock")), s(&self.h.outside.join("l.sock")));
        let sl = |x: &str| format!("cache/build/slot/{x}");
        let rows: Vec<(&str, &str, Vec<String>, Expect)> = vec![
            ("S-Z8q", "kernel: bind in slot/src (writable, but not under SLOT_OUT)", ops(&["bind", &sl("src/q.sock")]), Expect::Deny),
            ("S-Z8r", "kernel: connect a live listener at slot/outl (next to SLOT_OUT)", ops(&["connect", &sl("outl")]), Expect::Deny),
            ("S-Z8s", "kernel: connect a live listener outside INIT_HOME", ops(&["connect", &outside]), Expect::Deny),
            (
                "S-Z8t",
                "kernel: connect the Root owner socket through a symlink created in slot/out",
                ops(&["symlink-connect", &sl("out/t.sock"), &owner]),
                Expect::Deny,
            ),
        ];
        for (id, what, op, expect) in rows {
            self.check(id, k, what, &op, expect);
        }
    }

    fn kernel_last(&mut self, k: &Instance) {
        let h = &self.h;
        let rows = vec![
            ("S-Z3b", "kernel: rename kernel/", ops(&["rename", &s(&h.i("kernel")), &s(&h.i("kernel2"))])),
            ("S-Z3c", "kernel: rename INIT_HOME", ops(&["rename", &s(&h.i), &s(&h.run.join("i2"))])),
        ];
        for (id, what, op) in rows {
            self.check(id, k, what, &op, Expect::Deny);
        }
    }

    /// RR-2: a charter `extra_read` path is readable by the kernel; its sibling is not.
    fn extra_read_kernel(&mut self) {
        let mut layout = self.layout.clone();
        layout.extra_read = vec![self.h.extra.clone()];
        let Some(k) = self.kernel_instance("kernel/extra_read", &layout) else {
            return;
        };
        let (e, e2) = (s(&self.h.extra.join("f")), s(&self.h.extra2.join("f")));
        self.check("RR-2g", &k, "kernel: read a charter extra_read path", &ops(&["read", &e]), Expect::Allow);
        self.check("RR-2h", &k, "kernel: read the sibling of an extra_read path", &ops(&["read", &e2]), Expect::Deny);
    }

    /// RR-1: the zone tail over `(version 1)(allow default)`, the worst possible inner profile.
    fn tail_checks(&mut self) {
        let i = self.h.i.clone();
        let layout = self.layout.clone();
        let env = self.base_env(&i.join("work/home"), &i.join("work/tmp"));
        let Some(t) = self.instance(
            "tail/allow-default",
            None,
            Some("(version 1)\n(allow default)\n"),
            BTreeMap::new(),
            &layout,
            i.join("kernel/bin/sbx-probe"),
            i.join("work"),
            env,
        ) else {
            return;
        };
        self.parses("S-M0.tail", &t);
        let (tp, up) = (self.tcp_port(), self.udp_port());
        let h = &self.h;
        let li = |x: &str| s(&h.i(x));
        let r = |x: &str| s(&h.r(x));
        let rows: Vec<(&str, &str, Vec<String>, Expect)> = vec![
            ("RR-1a", "tail: read ROOT_HOME (Z1)", ops(&["read", &r("secret")]), Expect::Deny),
            ("RR-1b", "tail: list ROOT_HOME (Z1)", ops(&["list", &s(&h.r)]), Expect::Deny),
            ("RR-1c", "tail: read the service file (Z2)", ops(&["read", &s(&h.svc)]), Expect::Deny),
            (
                "RR-1d",
                "tail: read a non-Root path outside INIT_HOME (D9: reads are the inner profile's)",
                ops(&["read", &s(&h.outside.join("readme"))]),
                Expect::Allow,
            ),
            ("RR-1e", "tail: write outside INIT_HOME (Z3)", ops(&["write", &s(&h.outside.join("t1"))]), Expect::Deny),
            ("RR-1f", "tail: write INIT_HOME/work", ops(&["write", &li("work/t1")]), Expect::Allow),
            ("RR-1g", "tail: write kernel/ (Z4)", ops(&["write", &li("kernel/t1")]), Expect::Deny),
            ("RR-1h", "tail: write root/ (Z4)", ops(&["write", &li("root/t1")]), Expect::Deny),
            ("RR-1i", "tail: write run/mark/ (Z4)", ops(&["write", &li("run/mark/t1")]), Expect::Deny),
            ("RR-1j", "tail: overwrite run/zone-mark (Z4)", ops(&["write", &li("run/zone-mark")]), Expect::Deny),
            ("RR-1k", "tail: write lineage/kernel.git/ (Z4)", ops(&["write", &li("lineage/kernel.git/t1")]), Expect::Deny),
            ("RR-1n", "tail: read its own mark", ops(&["read", &s(&t.mark)]), Expect::Allow),
            ("RR-1o", "tail: read the decoy (Z5)", ops(&["read", &li("run/mark/decoy")]), Expect::Deny),
            ("RR-1p", "tail: read another mark (Z5)", ops(&["read", &li("run/mark/0000feedf00d")]), Expect::Deny),
            ("RR-1q", "tail: read the zone mark", ops(&["read", &li("run/zone-mark")]), Expect::Allow),
            ("RR-1r", "tail: connect the egress socket (Z6 leaves it open)", ops(&["connect", &r("run/egress.sock")]), Expect::Allow),
            ("RR-1s", "tail: connect the owner socket (Z6)", ops(&["connect", &r("run/owner.sock")]), Expect::Deny),
            ("RR-1t", "tail: bind a Unix socket inside INIT_HOME", ops(&["bind", &li("work/t.sock")]), Expect::Allow),
            ("RR-1u", "tail: bind a Unix socket outside INIT_HOME (Z6)", ops(&["bind", &s(&h.outside.join("t.sock"))]), Expect::Deny),
            ("RR-1v", "tail: TCP bind on loopback (Z6)", ops(&["tcp-bind-loopback"]), Expect::Deny),
            ("RR-1v2", "tail: UDP bind on loopback (Z6)", ops(&["udp-bind-loopback"]), Expect::Deny),
            ("RR-1v3", "tail: TCP connect to 127.0.0.1, the runner's own listener (Z6)", ops(&["tcp-connect", &tp]), Expect::Deny),
            ("RR-1v4", "tail: UDP send to 127.0.0.1, the runner's own socket (Z6)", ops(&["udp-send", &up]), Expect::Deny),
            ("RR-1x", "tail: read /etc/hosts", ops(&["read", "/etc/hosts"]), Expect::Allow),
            ("RR-1x2", "tail: read the fake ~/.ssh/id (Z2 deny list, ADR-009)", ops(&["read", &s(&h.run.join(".ssh/id"))]), Expect::Deny),
            ("RR-1x3", "tail: read a charter read_deny path (Z2)", ops(&["read", &s(&h.run.join("tax/2025.txt"))]), Expect::Deny),
            ("RR-1x4", "tail: write next to the Root's service file (Z2)", ops(&["write", &s(&h.run.join("svc/t1"))]), Expect::Deny),
            (
                "RR-1y",
                "tail: nested sandbox-exec",
                ops(&["exec", "/usr/bin/sandbox-exec", "-p", "(version 1)(allow default)", "/usr/bin/true"]),
                Expect::Code(71),
            ),
            ("RR-1z0", "tail: policy query, write INIT_HOME/work (calibrates RR-1z)", ops(&["policy-write", &li("work")]), Expect::Query(true)),
            ("RR-1z", "tail: write /private/tmp (policy query, nothing written; Z3)", ops(&["policy-write", "/private/tmp"]), Expect::Query(false)),
        ];
        let last = vec![
            ("RR-1l", "tail: rename kernel/ (Z4)", ops(&["rename", &li("kernel"), &li("kernel3")])),
            ("RR-1m", "tail: rename run/ (Z4)", ops(&["rename", &li("run"), &li("run3")])),
        ];
        for (id, what, op, expect) in rows {
            self.check(id, &t, what, &op, expect);
        }
        self.kill0_own_sleep("RR-1w", &t);
        for (id, what, op) in last {
            self.check(id, &t, what, &op, Expect::Deny);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Helpers

fn ops(a: &[&str]) -> Vec<String> {
    a.iter().map(|x| (*x).to_owned()).collect()
}

fn sh_quote(x: &str) -> String {
    format!("'{}'", x.replace('\'', "'\\''"))
}

fn canon(p: &Path) -> io::Result<PathBuf> {
    fs::canonicalize(p)
}

fn first_line_of(program: &str, args: &[&str]) -> io::Result<String> {
    let out = Command::new(program).args(args).stdin(Stdio::null()).output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!("{program} {args:?} failed")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or("").trim().to_owned())
}

fn tail(x: &str, n: usize) -> String {
    let start = x.len().saturating_sub(n);
    let start = (start..x.len()).find(|i| x.is_char_boundary(*i)).unwrap_or(x.len());
    x[start..].replace('\n', " | ")
}

fn last_result(stdout: &str) -> Option<Value> {
    stdout.lines().rev().filter_map(|l| serde_json::from_str::<Value>(l).ok()).find(|v| v.get("op").is_some())
}

/// Spawn, collect stdout and stderr, wait at most `timeout`; on timeout kill the child by the PID
/// this function holds (`Child::kill`) and reap it.
fn run_child(mut cmd: Command, timeout: Duration) -> io::Result<(Option<ExitStatus>, String, String)> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let rx_out = drain(child.stdout.take());
    let rx_err = drain(child.stderr.take());
    let status = wait_timeout(&mut child, timeout)?;
    let out = rx_out.recv_timeout(Duration::from_secs(5)).unwrap_or_default();
    let err = rx_err.recv_timeout(Duration::from_secs(5)).unwrap_or_default();
    Ok((status, out, err))
}

fn drain<R: Read + Send + 'static>(r: Option<R>) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut sbuf = String::new();
        if let Some(mut r) = r {
            let mut b = Vec::new();
            let _ = r.read_to_end(&mut b);
            sbuf = String::from_utf8_lossy(&b).into_owned();
        }
        let _ = tx.send(sbuf);
    });
    rx
}

fn wait_timeout(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(st) = child.try_wait()? {
            return Ok(Some(st));
        }
        if Instant::now() >= deadline {
            let _ = child.kill(); // the PID spawned by this runner
            let _ = child.wait();
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(5));
    }
}

/// Close stdin, wait for the instance to exit (killing it by its held PID after `timeout`), and
/// return its last result line.
fn finish(mut r: Running, timeout: Duration) -> Option<Value> {
    drop(r.stdin.take());
    let _ = wait_timeout(&mut r.child, timeout);
    let mut last = None;
    while let Ok(line) = r.lines.recv_timeout(Duration::from_millis(500)) {
        if let Ok(v) = serde_json::from_str::<Value>(&line)
            && v.get("op").is_some()
        {
            last = Some(v);
        }
    }
    last
}

fn evaluate(expect: &Expect, p: &ProbeOut) -> Status {
    let Some(v) = &p.json else {
        return Status::Fail(format!("no result (exit {:?}): {}", p.code, tail(&p.stderr, 400)));
    };
    let ok = v["ok"] == true;
    let errno = v["errno"].as_i64();
    let msg = v["msg"].as_str().unwrap_or("");
    let refused = matches!(errno, Some(e) if e == libc::EPERM as i64 || e == libc::EACCES as i64);
    match expect {
        Expect::Allow if ok => Status::Pass,
        Expect::Allow => Status::Fail(format!("refused: errno {errno:?}: {}", tail(msg, 400))),
        Expect::Deny if ok => Status::Fail("allowed".into()),
        Expect::Deny if refused => Status::Pass,
        Expect::Deny => Status::Fail(format!("not a sandbox refusal: errno {errno:?}: {}", tail(msg, 300))),
        Expect::Code(c) => match v["data"]["code"].as_i64() {
            Some(x) if x == *c as i64 => Status::Pass,
            x => Status::Fail(format!("exit {x:?}, want {c}: {}", tail(msg, 300))),
        },
        Expect::Fds(want) => {
            let got: Vec<i64> = v["data"]["fds"].as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
            if &got == want { Status::Pass } else { Status::Fail(format!("fds {got:?}")) }
        }
        Expect::Query(allowed) => match v["data"]["rc"].as_i64() {
            Some(rc) if (rc == 0) == *allowed => Status::Pass,
            Some(rc) => Status::Fail(format!("sandbox_check rc {rc}")),
            None => Status::Fail("sandbox_check unavailable".into()),
        },
        Expect::Signal(sig) => {
            if ok && v["data"]["signal"].as_i64() == Some(*sig) {
                Status::Pass
            } else {
                Status::Fail(format!("ok {ok}, signal {:?}: {msg}", v["data"]["signal"]))
            }
        }
    }
}
