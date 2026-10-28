//! The two homes and `install` (root/root.md §3.2, §4.3): ROOT_HOME and INIT_HOME layouts, the
//! placement rules (§3.2.1) and the layout check (R-F5), keys, the charter, the published view,
//! the service file. WP23 adds the kernel-source import; WP21b the seed bundle (step 7), the kernel
//! `config.toml` template and the install-mode kernel that forms the genesis tuple (step 8, sup.rs).

use crate::charter::{self, Ctx, Loaded};
use crate::lineage::{self, Lineage};
use crate::util::{self, bad, hex, icj, write_atomic};
use crate::{ledger, sys};
use serde_json::{Value, json};
use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const RH_DIRS: [&str; 17] = ["", "bin", "keys", "keys/providers", "keys/actions", "egress", "lineage", "store",
    "store/blobs", "store/tuples", "state", "state/proposals", "sealed", "cache", "cache/git-home", "logs", "run"];
/// kernel/supervise.md §3.1 with root.md §3.2.2's Root-written additions.
const IH_DIRS: [&str; 19] = ["", "kernel", "kernel/bin", "kernel/src", "kernel/seed", "bin", "lineage", "body", "work", "work/body", "work/kernel",
    "state", "state/kernel", "probe", "cache", "vault", "run", "run/mark", "root"];
/// §3.2.1: 64 bytes so the Root's sockets fit `sun_path`; test builds allow 80 for homes under `target/test-homes/`.
const MAX_HOME: usize = if cfg!(feature = "test-fixtures") { 80 } else { 64 };
const READ_ROOTS: [&str; 9] = ["/System", "/usr", "/bin", "/sbin", "/Library", "/Applications", "/opt/homebrew", "/etc", "/private/etc"];

#[derive(Clone, Debug)]
pub struct Paths {
    pub rh: PathBuf,
    pub ih: PathBuf,
    pub home: PathBuf,
}

impl Paths {
    /// ROOT_HOME: `--root-home`, then `INIT_ROOT_HOME`, then `~/.init-root`. INIT_HOME: `--home`, then
    /// `install.json`, then `INIT_HOME`, then `~/.init`.
    pub fn resolve(root_home: Option<String>, init_home: Option<String>) -> io::Result<Paths> {
        let home = PathBuf::from(std::env::var_os("HOME").ok_or_else(|| bad("HOME is not set"))?);
        let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
        let rh = root_home.map(PathBuf::from).or_else(|| env("INIT_ROOT_HOME")).unwrap_or_else(|| home.join(".init-root"));
        let recorded = fs::read_to_string(rh.join("install.json")).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok());
        let recorded = recorded.and_then(|v| v["init_home"].as_str().map(PathBuf::from));
        let ih = init_home.map(PathBuf::from).or(recorded).or_else(|| env("INIT_HOME")).unwrap_or_else(|| home.join(".init"));
        Ok(Paths { rh, ih, home })
    }
    pub fn r(&self, rel: &str) -> PathBuf {
        self.rh.join(rel)
    }
    pub fn i(&self, rel: &str) -> PathBuf {
        self.ih.join(rel)
    }
    pub fn ctx(&self) -> Ctx<'_> {
        Ctx { root_home: &self.rh, init_home: &self.ih, home: &self.home }
    }
}

/// Absolute with no symlink component (an existing path canonicalizes to itself; a missing one's parent does).
fn canonical(x: &Path) -> bool {
    x.is_absolute()
        && match fs::canonicalize(x) {
            Ok(c) => c == x,
            Err(_) => fs::symlink_metadata(x).is_err() && x.parent().is_some_and(canonical),
        }
}

/// §3.2.1 placement rules.
pub fn check_paths(p: &Paths, extra_read: &[String]) -> Result<(), String> {
    if !canonical(&p.rh) || p.rh.as_os_str().len() > MAX_HOME {
        return Err(format!("ROOT_HOME {} must be absolute, symlink-free and at most {MAX_HOME} bytes", p.rh.display()));
    }
    if !canonical(&p.ih) {
        return Err(format!("INIT_HOME {} must be absolute and symlink-free", p.ih.display()));
    }
    if p.rh.starts_with(&p.ih) || p.ih.starts_with(&p.rh) {
        return Err("ROOT_HOME and INIT_HOME must not contain each other".into());
    }
    let reads = READ_ROOTS.iter().map(PathBuf::from).chain(extra_read.iter().map(PathBuf::from));
    match reads.into_iter().find(|r| p.rh.starts_with(r)) {
        Some(r) => Err(format!("ROOT_HOME must lie outside the zone's read root {}", r.display())),
        None => Ok(()),
    }
}

/// Creates missing directories with mode 0700; an existing entry must be a real directory.
pub fn make_dirs(base: &Path, dirs: &[&str]) -> io::Result<()> {
    for d in dirs {
        let x = base.join(d);
        match fs::symlink_metadata(&x) {
            Ok(m) if !m.is_dir() => return Err(bad(format!("{} exists and is not a directory", x.display()))),
            Ok(_) => {}
            Err(_) => fs::DirBuilder::new().recursive(true).mode(0o700).create(&x)?,
        }
        fs::set_permissions(&x, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Both install-mode and live kernels use these HOME/TMPDIR directories. Check
/// the cache anchor as well, so a substituted symlink cannot redirect Root writes.
pub fn prepare_kernel_scratch(p: &Paths) -> io::Result<()> {
    make_dirs(&p.ih, &["cache", "cache/kernel-home", "cache/tmp"])
}

/// R-F5, at every start.
pub fn check_layout(p: &Paths, extra_read: &[String]) -> Result<(), String> {
    check_paths(p, extra_read)?;
    for d in RH_DIRS {
        let ok = fs::symlink_metadata(p.r(d)).is_ok_and(|m| m.is_dir() && m.uid() == sys::euid() && m.mode() & 0o777 == 0o700);
        if !ok {
            return Err(format!("ROOT_HOME/{d} must be a 0700 directory owned by you"));
        }
    }
    let top = IH_DIRS.iter().filter(|d| !d.contains('/')).copied().chain(["lineage/kernel.git"]);
    match top.into_iter().find(|d| !fs::symlink_metadata(p.i(d)).is_ok_and(|m| m.is_dir())) {
        Some(d) => Err(format!("INIT_HOME/{d} must be a real directory")),
        None => Ok(()),
    }
}

/// Platform tools (§3.6.5 step 2, §4.3 step 1).
pub fn check_tools(git: &str) -> Result<(), String> {
    let launcher = if cfg!(target_os = "macos") { "/usr/bin/sandbox-exec" } else { "/usr/bin/bwrap" };
    if !Path::new(launcher).exists() {
        return Err(format!("{launcher} is missing"));
    }
    #[cfg(target_os = "linux")]
    {
        if !Path::new("/usr/bin/unshare").exists() { return Err("/usr/bin/unshare is required for the Linux network namespace".into()); }
        // The binaries the zone will run: the owner's root-owned copies under /usr/local/lib/init-agent when present
        // (AppArmor-restricted Ubuntu; init_sandbox_profiles::entry), else /usr/bin's. Both must pass the same checks.
        init_sandbox_profiles::entry::zone_entry("unshare")?;
        for bwrap in [Path::new(launcher).to_path_buf(), init_sandbox_profiles::entry::zone_entry("bwrap")?] {
            let out = Command::new(&bwrap).arg("--version").env_clear().output().map_err(|e| format!("{}: {e}", bwrap.display()))?;
            let version = String::from_utf8_lossy(&out.stdout);
            let parts: Vec<u32> = version.split_whitespace().last().unwrap_or("").split('.').take(3).map(|s| s.parse().unwrap_or(0)).collect();
            if !out.status.success() || parts < vec![0, 13, 0] {
                return Err(format!("bubblewrap >= 0.13.0 with verified fd-bind mounts is required ({}: {})", bwrap.display(), version.trim()));
            }
        }
    }
    let out = Command::new(git).arg("--version").env_clear().output().map_err(|e| format!("{git}: {e}"))?;
    let v = String::from_utf8_lossy(&out.stdout);
    let n: Vec<u32> = v.split_whitespace().nth(2).unwrap_or("").split('.').take(3).map(|x| x.parse().unwrap_or(0)).collect();
    if n < vec![2, 39, 4] {
        return Err(format!("git ≥ 2.39.4 is required ({})", v.trim()));
    }
    Ok(())
}

/// The `charter` record (§3.10.5) for a newly applied charter; returns the record.
pub fn charter_record(lin: &mut Lineage, l: &Loaded, old: Option<&Value>, source: &str) -> io::Result<Value> {
    let changed: Vec<String> = charter::diff(old, &charter::effective(&l.ch)).into_iter().map(|d| d.0).collect();
    let money = |s: &str| charter::musd(s).unwrap_or(0);
    // ADR-011 (X7): the `[extend]` digest and the mode, which R7's fold reads (ledger.rs)
    let data = json!({"changed": changed, "digest": l.digest, "extend": {"mode": l.ch.extend.mode, "sha256": charter::extend_sha256(&l.ch)},
        "ladder": l.ch.ladder, "max_level": l.ch.autonomy.max_level,
        "money": {"day_musd": money(&l.ch.money.day_usd), "total_musd": money(&l.ch.money.total_usd)}, "source": source});
    lin.append("root", "charter", data)
}

/// §3.10.6: rewrites `INIT_HOME/root/` (charter, ledger, status, key, lineage copy). Ceilings: WP22.
pub fn publish(p: &Paths, l: Option<&Loaded>, level: &ledger::Level, status: &Value) -> io::Result<()> {
    let dir = p.i("root");
    if !dir.is_dir() {
        return Ok(());
    }
    let line = |v: &Value| icj(v).map(|s| format!("{s}\n"));
    if let Some(l) = l {
        write_atomic(&dir.join("charter.json"), line(&charter::view(&l.ch, &l.digest))?.as_bytes(), 0o644)?;
        write_atomic(&dir.join("ledger.json"), line(&level.json(l.ch.autonomy.max_level))?.as_bytes(), 0o644)?;
    }
    let mut status = status.clone();
    if status.is_object() {
        status["evolution"] = evolution(p); // what the agent can change in itself here (T0 vs T1/T2)
    }
    write_atomic(&dir.join("status.json"), line(&status)?.as_bytes(), 0o644)?;
    for (from, to) in [("keys/lineage.pub", "lineage.pub")] { // lineage.jsonl: the lineage keeps its own view (WP23)
        if let Ok(b) = fs::read(p.r(from))
            && fs::read(dir.join(to)).ok().as_ref() != Some(&b)
        {
            write_atomic(&dir.join(to), &b, 0o644)?;
        }
    }
    Ok(())
}

/// `status.json` of the view (§4.5); the daemon adds the tuples and the kernel build (link.rs `publish`).
pub fn status_view(slot: &str, mode: &str, panicked: bool, level: i64, digest: Option<&str>) -> Value {
    json!({"audited_good": {"body_sha": null, "tuple": null}, "charter_sha256": digest, "kernel_build": null,
        "level": level, "mode": mode, "panicked": panicked, "slot": slot,
        "tuples": {"audited": null, "current": null, "lkg": null}, "v": 1})
}

pub struct Opts {
    pub release: Option<PathBuf>,
    pub charter: Option<PathBuf>,
    pub git: String,
    pub with_testkit: bool,
    /// The Rust toolchain the zone may read (`TOOLCHAIN`); default `rustup which rustc`, two levels up.
    pub toolchain: Option<PathBuf>,
    /// §4.3 step 7: the seed, a local git repository with branch `seed` and tags `seed-v*`, or a git bundle
    /// of them. None: no seed (the first start forms a kernel-only genesis tuple; tests).
    pub seed_source: Option<PathBuf>,
    /// The seed tag to install; default the highest-semver `seed-v*` tag.
    pub seed_tag: Option<String>,
    /// Write no service file (step 9): disposable homes and harnesses that run `install` with the owner's
    /// real HOME, which the zone's class rules need (`HOME_REAL`), but must not touch `~/Library`.
    pub no_service: bool,
    /// Where install.sh puts the `init-agent` launcher and `init-tui` (SBX-009): recorded as `install.json` `bin_dir`,
    /// write-denied to the zone in every mode (tail Z3a). Must be absolute.
    pub bin_dir: Option<PathBuf>,
}

/// Bundle-install options (lane UX-BUNDLE), kept apart from [`Opts`] so existing callers stay unchanged.
#[derive(Clone, Debug, Default)]
pub struct BundleOpts {
    /// `--toolchain none`: install without a Rust toolchain even when rustup has one (install.json records
    /// none). Chat, tools and T0 self-extension work; T1/T2 (rebuilding the body) wait for `init-root toolchain --set`.
    pub no_toolchain: bool,
    /// A release bundle's prebuilt body (docs/ux-v1.md §3.5.1): `--prebuilt-body FILE --prebuilt-sha256 HEX
    /// [--prebuilt-commit SHA]`. The install-mode kernel installs it instead of building the seed, so the install
    /// needs no toolchain; the genesis tuple's body blob must equal HEX bit for bit.
    pub prebuilt: Option<Prebuilt>,
}

#[derive(Clone, Debug)]
pub struct Prebuilt {
    pub body: PathBuf,
    pub sha256: String,
    pub commit: Option<String>,
}

/// Verifies the prebuilt body against its expected sha256 and puts it, with its record, in
/// `INIT_HOME/kernel/prebuilt/` (Root-written; the zone only reads `kernel/`), where the install-mode kernel finds it.
fn stage_prebuilt(p: &Paths, b: &Prebuilt) -> Result<(), String> {
    let hex_ok = |h: &str, n: usize| h.len() == n && h.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c));
    if !hex_ok(&b.sha256, 64) {
        return Err("--prebuilt-sha256 must be 64 lowercase hex digits".into());
    }
    if b.commit.as_deref().is_some_and(|c| !hex_ok(c, 40)) {
        return Err("--prebuilt-commit must be a 40-digit lowercase hex commit".into());
    }
    let bytes = fs::read(&b.body).map_err(|e| format!("--prebuilt-body {}: {e}", b.body.display()))?;
    let got = util::sha256_hex(&bytes);
    if got != b.sha256 {
        return Err(format!("--prebuilt-body {} has sha256 {got}, not {}", b.body.display(), b.sha256));
    }
    let dir = p.i("kernel/prebuilt");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    write_atomic(&dir.join("init-body"), &bytes, 0o755).map_err(|e| e.to_string())?;
    let rec = json!({"commit": b.commit, "sha256": b.sha256, "v": 1});
    write_atomic(&dir.join("init-body.json"), format!("{}\n", icj(&rec).map_err(|e| e.to_string())?).as_bytes(), 0o644).map_err(|e| e.to_string())
}

/// Removes `kernel/prebuilt/` by exact paths once the body is in the store (or the install failed).
fn drop_prebuilt(p: &Paths) {
    let dir = p.i("kernel/prebuilt");
    for f in ["init-body", "init-body.json"] {
        let _ = fs::remove_file(dir.join(f));
    }
    let _ = fs::remove_dir(&dir);
}

/// The genesis tuple's body is the prebuilt one, bit for bit (and from the recorded commit).
fn check_prebuilt_tuple(p: &Paths, id: &str, b: &Prebuilt) -> Result<(), String> {
    let t = crate::tuple::load(&p.rh, id).ok_or_else(|| format!("genesis tuple {id} is unreadable"))?;
    if t["body"]["binary"].as_str() != Some(b.sha256.as_str()) {
        return Err(format!("genesis tuple {id} holds body {}, not the prebuilt {}", t["body"]["binary"], b.sha256));
    }
    if let Some(c) = &b.commit
        && t["body"]["sha"].as_str() != Some(c.as_str())
    {
        return Err(format!("genesis tuple {id} holds body commit {}, not the prebuilt's {c}", t["body"]["sha"]));
    }
    Ok(())
}

/// The command that installs Rust 1.97.1 with the Wasm target for this user (rustup's own installer, no sudo).
pub const RUSTUP_CMD: &str = "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.97.1 --target wasm32-wasip2";

/// What the agent can do to itself with this install's toolchain (docs/ux-v1.md §3.4.9): T0 (skills, prompts,
/// extension source) always; T1/T2 (rebuilding the body or the kernel) need `install.json` `toolchain` with
/// cargo and rustc in it. Read by `init-root status` and published in `INIT_HOME/root/status.json`.
pub fn evolution(p: &Paths) -> Value {
    let info = fs::read_to_string(p.r("install.json")).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or(Value::Null);
    let tc = info["toolchain"].as_str().map(PathBuf::from);
    let ok = tc.as_ref().is_some_and(|t| ["bin/cargo", "bin/rustc"].iter().all(|b| t.join(b).is_file()));
    if ok {
        return json!({"tiers": ["T0", "T1", "T2"], "toolchain": tc, "hint": null});
    }
    json!({"tiers": ["T0"], "toolchain": tc,
        "hint": format!("Self-evolution is limited to T0 (skills, prompts, extension source): no Rust toolchain is configured. \
            T1/T2 (rebuilding itself) need Rust 1.97.1. The owner runs: {RUSTUP_CMD}; then: init-root stop && init-root toolchain --set && init-root start")})
}

/// `init-root toolchain [--set [DIR]]`: show the toolchain, or record one (default: `~/.cargo/bin/rustup which rustc`,
/// two levels up) in `install.json` and the kernel config's `[sandbox]` together, which the broker compares. The
/// Root must be stopped (it reads install.json at boot).
pub fn toolchain_cmd(p: &Paths, set: bool, dir: Option<PathBuf>) -> Result<(), (u8, String)> {
    if !set {
        let v = evolution(p);
        println!("toolchain {} · self-evolution {}", v["toolchain"].as_str().unwrap_or("none"),
            v["tiers"].as_array().map(|t| t.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("+")).unwrap_or_default());
        if let Some(h) = v["hint"].as_str() {
            println!("{h}");
        }
        return Ok(());
    }
    let _lock = crate::cli::take_lock(p).map_err(|e| (1, format!("run/root.lock: {e}")))?
        .ok_or((1u8, "the Root is running: init-root stop first, then init-root start after".to_string()))?;
    let o = Opts { release: None, charter: None, git: String::new(), with_testkit: false, toolchain: dir, seed_source: None, seed_tag: None,
        no_service: true, bin_dir: None };
    let tc = toolchain(p, &o).ok_or((1u8, format!("no Rust toolchain found (rustup not installed?). Install it: {RUSTUP_CMD}")))?;
    for b in ["bin/cargo", "bin/rustc"] {
        if !tc.join(b).is_file() {
            return Err((1, format!("{} has no {b}", tc.display())));
        }
    }
    let v = Command::new(tc.join("bin/rustc")).arg("-V").env_clear().output().map_err(|e| (1u8, e.to_string()))?;
    let v = String::from_utf8_lossy(&v.stdout).trim().to_string();
    if !v.starts_with("rustc 1.97.") {
        return Err((1, format!("{} is {v}; INIT needs rustc 1.97.x (rustup toolchain install 1.97.1)", tc.display())));
    }
    if !tc.join("lib/rustlib/wasm32-wasip2").is_dir() {
        return Err((1, format!("{} lacks the wasm32-wasip2 target: rustup target add --toolchain 1.97.1 wasm32-wasip2", tc.display())));
    }
    let ip = p.r("install.json");
    let mut info: Value = fs::read_to_string(&ip).ok().and_then(|s| serde_json::from_str(&s).ok()).ok_or((1u8, format!("{} is unreadable: install first", ip.display())))?;
    info["toolchain"] = json!(tc);
    let cp = p.i("kernel/config.toml");
    let text = fs::read_to_string(&cp).map_err(|e| (1u8, format!("{}: {e}", cp.display())))?;
    let text = set_sandbox_toolchain(&text, &tc).map_err(|m| (1u8, format!("{}: {m}", cp.display())))?;
    write_atomic(&cp, text.as_bytes(), 0o644).map_err(|e| (1u8, e.to_string()))?;
    write_atomic(&ip, format!("{}\n", icj(&info).map_err(|e| (1u8, e.to_string()))?).as_bytes(), 0o600).map_err(|e| (1u8, e.to_string()))?;
    println!("toolchain {} ({v}) recorded in install.json and kernel/config.toml [sandbox]; self-evolution T0+T1+T2", tc.display());
    Ok(())
}

/// Sets `toolchain` in the `[sandbox]` table of a kernel config, replacing the line or adding it after the header.
fn set_sandbox_toolchain(text: &str, tc: &Path) -> Result<String, String> {
    let line = format!("toolchain = {}", serde_json::to_string(&tc.display().to_string()).map_err(|e| e.to_string())?);
    let mut out: Vec<String> = Vec::new();
    let (mut in_sb, mut seen, mut done) = (false, false, false);
    for l in text.lines() {
        let t = l.trim_start();
        if t.starts_with('[') {
            in_sb = t.starts_with("[sandbox]");
            if in_sb {
                seen = true;
                out.push(l.to_string());
                out.push(line.clone());
                done = true;
                continue;
            }
        } else if in_sb && (t.starts_with("toolchain ") || t.starts_with("toolchain=")) {
            continue;
        }
        out.push(l.to_string());
    }
    if !seen || !done {
        return Err("no [sandbox] table".into());
    }
    Ok(out.join("\n") + "\n")
}

/// `install` (first time), §4.3 steps 1–5, 6 (bare repo), 8 (records), 9 and 10. Exit 1 error, 2 invalid charter.
#[cfg_attr(not(test), allow(dead_code))] // the CLI calls install_with; the tests call this
pub fn install(p: &Paths, o: &Opts) -> Result<(), (u8, String)> {
    install_with(p, o, &BundleOpts::default())
}

/// [`install`] with a release bundle's options: no toolchain, a prebuilt body.
pub fn install_with(p: &Paths, o: &Opts, bo: &BundleOpts) -> Result<(), (u8, String)> {
    let e1 = |e: io::Error| (1u8, e.to_string());
    if p.r("install.json").exists() {
        return Err((1, "already installed (install.json exists): upgrade it instead, which keeps the evolved body, the lineage, \
            the keys and the charter (init-agent upgrade, or scripts/install.sh --upgrade from the new release)".into()));
    }
    check_paths(p, &[]).map_err(|m| (1, m))?;
    check_tools(&o.git).map_err(|m| (1, m))?;
    // 3. ROOT_HOME, the canary, keys
    make_dirs(&p.rh, &RH_DIRS).map_err(e1)?;
    let new_file = |f: PathBuf, mode| if fs::symlink_metadata(&f).is_err() { write_atomic(&f, b"", mode) } else { Ok(()) };
    new_file(p.r("run/canary"), 0o400).map_err(e1)?;
    new_file(p.r("keys/known_hosts"), 0o600).map_err(e1)?;
    let vk = lineage::ensure_key(&p.rh).map_err(e1)?;
    let mk = p.r("keys/mirror_ed25519");
    if fs::symlink_metadata(&mk).is_err() && Path::new("/usr/bin/ssh-keygen").exists() {
        let st = Command::new("/usr/bin/ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-C", "init-root", "-f"]).arg(&mk).env_clear().status();
        if !st.is_ok_and(|s| s.success()) {
            return Err((1, "ssh-keygen failed to create the mirror deploy key".into()));
        }
    }
    // 4. the charter
    let cp = p.r("charter.toml");
    let src = match &o.charter {
        Some(f) => Some(fs::read(f).map_err(|e| (1, format!("{}: {e}", f.display())))?),
        None => fs::symlink_metadata(&cp).is_err().then(|| charter::template().into_bytes()),
    };
    if let Some(src) = src {
        write_atomic(&cp, &src, 0o600).map_err(e1)?;
    }
    let l = charter::load(&cp, &p.ctx()).map_err(|fs| {
        let lines: Vec<String> = fs.iter().map(ToString::to_string).collect();
        (2, format!("{}\nedit {}, then run install again", lines.join("\n"), cp.display()))
    })?;
    l.warnings.iter().for_each(|w| eprintln!("{w}"));
    // 2. disk (a warning only)
    let probe = p.ih.ancestors().find(|a| a.exists()).unwrap_or(Path::new("/"));
    if sys::free_bytes(probe).is_ok_and(|b| b < (l.ch.quotas.disk_min_free_gib as u64) << 30) {
        eprintln!("warning: less than {} GiB free on {}", l.ch.quotas.disk_min_free_gib, probe.display());
    }
    // 5. the INIT_HOME skeleton, only into an empty or Root-made INIT_HOME (R-F1)
    if p.i("").is_dir() && !p.i("root").is_dir() && fs::read_dir(&p.ih).map_err(e1)?.next().is_some() {
        return Err((1, format!("INIT_HOME {} exists and is not empty", p.ih.display())));
    }
    make_dirs(&p.ih, &IH_DIRS).map_err(e1)?;
    for f in ["run/mark/decoy", "run/zone-mark"] {
        new_file(p.i(f), 0o644).map_err(e1)?;
    }
    write_atomic(&p.i("kernel/CONSTITUTION.md"), l.ch.constitution.text.as_bytes(), 0o644).map_err(e1)?;
    if let Some(rel) = &o.release {
        copy_release(rel, p, o.with_testkit, l.ch.root.test_mode).map_err(e1)?;
    }
    // 6. lineage/kernel.git (bare, Root-written); the release source import is WP21/WP23
    let kgit = p.i("lineage/kernel.git");
    if !kgit.is_dir() {
        let st = git(p, &o.git).args(["init", "--bare", "-q", "--template="]).arg(&kgit).status();
        if !st.is_ok_and(|s| s.success()) {
            return Err((1, "git init --bare lineage/kernel.git failed".into()));
        }
    }
    crate::lineage::kgit::import_release(p, &o.git).map_err(|m| (1, m))?; // kernel/src/<v>.bundle → refs/upstream/<v> (D7)
    // 8. genesis and charter records (a test ledger may already hold genesis), the active charter
    let mut lin = Lineage::open(&p.rh, Some(p.i("root/lineage.jsonl"))).map_err(e1)?;
    if lin.head.is_none() {
        let id = hex(&util::random::<8>().map_err(e1)?);
        lin.append("root", "genesis", json!({"install_id": id, "pubkey": hex(&vk.to_bytes()), "root_version": VERSION})).map_err(e1)?;
    }
    charter_record(&mut lin, &l, None, "install").map_err(e1)?;
    drop(lin); // the install-mode Root (step 8) opens the lineage itself
    write_atomic(&p.r("state/charter.active.toml"), l.src.as_bytes(), 0o600).map_err(e1)?;
    // 9 (early). the binary (a new inode): the install-mode kernel is spawned through `exec-clean`
    let exe = std::env::current_exe().and_then(fs::read).map_err(e1)?;
    write_atomic(&p.r("bin/init-root"), &exe, 0o755).map_err(e1)?;
    // 10 (early). install.json: the broker validates the install-mode kernel's build against it
    let devdir = platform_tool_prefix().map_err(e1)?;
    let sdkroot = if cfg!(target_os = "macos") {
        devdir.as_ref().map(|d| d.join("SDKs/MacOSX.sdk")).filter(|s| s.is_dir())
    } else { None };
    let info = json!({"devdir": devdir, "git": o.git, "init_home": p.ih, "platform": std::env::consts::OS, "root_home": p.rh,
        "sdkroot": sdkroot, "toolchain": if bo.no_toolchain { None } else { toolchain(p, o) }, "v": 1});
    let mut info = info;
    if let Some(b) = o.bin_dir.as_ref().filter(|b| b.is_absolute()) {
        info["bin_dir"] = json!(init_sandbox_profiles::access::resolve(b));
    }
    if let Some(rel) = &o.release {
        info["release"] = crate::upgrade::release_id(rel); // what `upgrade` compares a new release with (no downgrade)
    }
    // 5 (end). the kernel config template: `[sandbox]` equal to install.json (the class rules compare them)
    if !p.i("kernel/config.toml").exists() {
        write_atomic(&p.i("kernel/config.toml"), config_template(&info).as_bytes(), 0o644).map_err(e1)?;
    }
    write_atomic(&p.r("install.json"), format!("{}\n", icj(&info).map_err(e1)?).as_bytes(), 0o600).map_err(e1)?;
    // 7–8. the seed bundle and the install-mode kernel (genesis tuple). On failure install.json goes again
    // (exact path), so `install` can be re-run: every earlier step is idempotent.
    if bo.prebuilt.is_some() && o.seed_source.is_none() {
        let _ = fs::remove_file(p.r("install.json"));
        return Err((64, "--prebuilt-body needs --seed-source (the body's own source)".into()));
    }
    let seeded = match &o.seed_source {
        Some(src) => {
            let staged = bo.prebuilt.as_ref().map_or(Ok(()), |b| stage_prebuilt(p, b));
            let r = staged.and_then(|()| seed_bundle(p, &o.git, src, o.seed_tag.as_deref()))
                .and_then(|(b, tag)| install_seed(p, &b, &tag).map(|id| (id, tag)))
                .and_then(|(id, tag)| bo.prebuilt.as_ref().map_or(Ok(()), |b| check_prebuilt_tuple(p, &id, b)).map(|()| (id, tag)));
            drop_prebuilt(p);
            match r {
                Ok(x) => Some(x),
                Err(m) => {
                    let _ = fs::remove_file(p.r("install.json"));
                    return Err((1, format!("seed install failed: {m}\nfix it, then run install again")));
                }
            }
        }
        None => None,
    };
    // 9. the service file (written, never loaded: that is the owner's step)
    let svc = if o.no_service { None } else { Some(service_file(p).map_err(e1)?) };
    if seeded.is_none() { // with a seed, the install-mode Root has published the view with its tuples
        let level = ledger::Level::fold(&lineage::records(&p.rh).map_err(e1)?);
        publish(p, Some(&l), &level, &status_view("stopped", "normal", false, level.level, Some(&l.digest))).map_err(e1)?;
    }
    if !l.ch.root.test_mode {
        let _ = Command::new("/usr/bin/tmutil").arg("addexclusion").arg(p.r("keys")).env_clear().status();
    }
    let fp = &util::sha256_hex(&vk.to_bytes())[..32];
    println!("init-root installed.\n  ROOT_HOME {}\n  INIT_HOME {}\n  lineage key fingerprint {fp}", p.rh.display(), p.ih.display());
    println!("  mirror deploy key {}.pub (add it to the mirror repository as a deploy key with write access)", mk.display());
    match svc {
        Some(f) => println!("  service file {} (load it to run the Root at login)", f.display()),
        None => println!("  no service file (--no-service)"),
    }
    if let Some((id, tag)) = seeded {
        let how = if bo.prebuilt.is_some() { "prebuilt body, verified bit for bit" } else { "built from source" };
        println!("  seed {tag} installed ({how}): genesis tuple {id}");
    }
    let ev = evolution(p);
    match ev["hint"].as_str() {
        Some(h) => println!("  self-evolution: T0 only. {h}"),
        None => println!("  self-evolution: T0+T1+T2 (toolchain {})", ev["toolchain"].as_str().unwrap_or("")),
    }
    Ok(())
}

/// `kernel/config.toml` when the release ships none (§4.3 step 5; kernel/config.md §4.9): `[sandbox]` from
/// install.json, everything else the kernel's defaults. The owner edits this file directly.
pub fn config_template(info: &Value) -> String {
    // a TOML basic string. With no toolchain (install.json `toolchain: null`) the broker validates TOOLCHAIN against
    // INIT_HOME/kernel (zone.rs `vctx`), so the kernel gets that same harmless readable dir: chat and T0 work, and a
    // build finds no cargo there and fails cleanly until `init-root toolchain --set`.
    let kdir = info["init_home"].as_str().map(|h| format!("{h}/kernel"));
    let q = |toml: &str, v: Option<&str>| v.and_then(|v| serde_json::to_string(v).ok()).map(|v| format!("{toml} = {v}\n")).unwrap_or_default();
    let tc = info["toolchain"].as_str().map(String::from).or(kdir.clone());
    let dev = info["devdir"].as_str().map(String::from).or(kdir); // the same fallback for DEVDIR
    format!("# INIT kernel config (kernel/config.md), written by `init-root install`. Edit freely; [sandbox] must\n\
             # stay equal to ROOT_HOME/install.json, because the Root's broker compares them.\n\
             schema_version = 2\n\n[sandbox]\n{}{}", q("toolchain", tc.as_deref()), q("developer_dir", dev.as_deref()))
}

/// The highest-semver `seed-v<a>.<b>.<c>` tag.
fn semver(tag: &str) -> Option<(u64, u64, u64)> {
    let mut v = tag.strip_prefix("seed-v")?.split('.').map(|x| x.parse::<u64>().ok());
    let r = (v.next()??, v.next()??, v.next()??);
    v.next().is_none().then_some(r)
}

/// §4.3 step 7: `INIT_HOME/kernel/seed/<tag>.bundle` from `src`. A directory is the owner's own repository
/// (its `.git`, or itself when bare), bundled with the hardened prefix (`--branches=seed --tags=seed-v*`); a
/// file must already be such a bundle and is copied. Returns the bundle path and the tag to install.
/// The branch is named in full: `--branches=seed` would mean `refs/heads/seed/*` (a pattern without a glob
/// character gets `/*` appended, git-rev-list(1)), which leaves the branch out.
pub fn seed_bundle(p: &Paths, git_bin: &str, src: &Path, want: Option<&str>) -> Result<(PathBuf, String), String> {
    let src = fs::canonicalize(src).map_err(|e| format!("--seed-source {}: {e}", src.display()))?;
    let dir = p.i("kernel/seed");
    let tmp = dir.join(format!(".new-{}.bundle", hex(&util::random::<6>().map_err(|e| e.to_string())?)));
    let run = |c: &mut Command| -> Result<String, String> {
        let o = c.output().map_err(|e| format!("{git_bin}: {e}"))?;
        let err = String::from_utf8_lossy(&o.stderr).trim().chars().take(400).collect::<String>();
        o.status.success().then(|| String::from_utf8_lossy(&o.stdout).into_owned()).ok_or(err)
    };
    let made = if src.is_dir() {
        let gd = if src.join(".git").exists() { src.join(".git") } else { src.clone() };
        run(git(p, git_bin).arg(format!("--git-dir={}", gd.display())).args(["bundle", "create", "-q"]).arg(&tmp)
            .args(["refs/heads/seed", "--tags=seed-v*"])).map(drop).map_err(|e| format!("git bundle create (branch seed, tags seed-v*): {e}"))
    } else {
        fs::copy(&src, &tmp).map(drop).map_err(|e| format!("copy {}: {e}", src.display()))
    };
    let heads = made.and_then(|()| run(git(p, git_bin).args(["bundle", "list-heads"]).arg(&tmp)).map_err(|e| format!("not a git bundle: {e}")));
    let res = heads.and_then(|h| {
        let refs: Vec<&str> = h.lines().filter_map(|l| l.split_whitespace().nth(1)).collect();
        if !refs.contains(&"refs/heads/seed") {
            return Err("the seed has no branch `seed`".into());
        }
        let tags: Vec<&str> = refs.iter().filter_map(|r| r.strip_prefix("refs/tags/")).filter(|t| semver(t).is_some()).collect();
        let tag = match want {
            Some(t) if tags.contains(&t) => t.to_string(),
            Some(t) => return Err(format!("the seed has no tag {t} (it has: {})", tags.join(", "))),
            None => tags.iter().max_by_key(|t| semver(t)).ok_or("the seed has no seed-v<x.y.z> tag")?.to_string(),
        };
        let out = dir.join(format!("{tag}.bundle"));
        fs::rename(&tmp, &out).map_err(|e| e.to_string())?;
        fs::set_permissions(&out, fs::Permissions::from_mode(0o644)).map_err(|e| e.to_string())?;
        Ok((out, tag))
    });
    if res.is_err() {
        let _ = fs::remove_file(&tmp); // exact path, made above
    }
    res
}

/// §4.3 step 8 under `run/root.lock`: a Root (cold start, no daemon, no egress) serves the install-mode
/// kernel until it exits (sup.rs `install_seed`).
fn install_seed(p: &Paths, bundle: &Path, tag: &str) -> Result<String, String> {
    let _lock = crate::cli::take_lock(p).map_err(|e| format!("run/root.lock: {e}"))?.ok_or("a Root is running (run/root.lock is held)")?;
    let root = crate::link::Root::boot(p.clone()).map_err(|e| format!("cold start: {e}"))?;
    println!("installing seed {tag}: the kernel imports and builds it (a cold build takes minutes)");
    crate::sup::install_seed(&root, bundle, tag, std::time::Duration::from_secs(crate::sup::INSTALL_TIMEOUT_S))
}

/// The hardened Root git prefix (§3.11): no owner config, no hooks, UTC.
pub fn git(p: &Paths, git: &str) -> Command {
    let mut c = Command::new(git);
    c.env_clear().env("PATH", "/usr/bin:/bin").env("HOME", p.r("cache/git-home")).env("LC_ALL", "C").env("TZ", "UTC")
        .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_TERMINAL_PROMPT", "0")
        .args(["-c", "core.hooksPath=/dev/null", "-c", "user.useConfigOnly=true", "-c", "user.name=init-root", "-c", "user.email=root@init.invalid"]);
    c
}

/// Release binaries, docs, WIT, upgrade fixtures and kernel-source bundles into `INIT_HOME/kernel/`.
/// `kernel/bin` in every install. `init-mockllm` is the $0 local mock model (no network call): G0 runs the
/// body's provider tests against it (`INIT_MOCKLLM_BIN`, kernel/gate.md §3.3). Like all of `kernel/` it is
/// Root-written and read-only to the zone (Z4). It is not in the tuple, so a rollback leaves it in place.
pub const KERNEL_BINS: [&str; 3] = ["init-kernel", "initctl", "init-mockllm"];
/// Only with `--with-testkit`.
pub const TESTKIT_BINS: [&str; 1] = ["init-fakebody"];

/// A kernel release without `init-mockllm` fails every T2 gate at G0 `test`: refused, or a warning in test mode.
fn copy_release(rel: &Path, p: &Paths, testkit: bool, test_mode: bool) -> io::Result<()> {
    if rel.join("init-kernel").is_file() && !rel.join("init-mockllm").is_file() {
        let m = format!("{}/init-mockllm is missing; every install ships it (G0 runs the body's tests against it)", rel.display());
        if !test_mode {
            return Err(io::Error::other(m));
        }
        eprintln!("warning: {m}");
    }
    let bins = KERNEL_BINS.into_iter().chain(testkit.then_some(TESTKIT_BINS).into_iter().flatten());
    for b in bins {
        let src = rel.join(b);
        if src.is_file() {
            util::copy_atomic(&src, &p.i("kernel/bin").join(b), 0o755)?;
        }
    }
    for d in ["docs", "wit", "kupgrade-smoke", "src"] {
        copy_tree(&rel.join(d), &p.i("kernel").join(d))?;
    }
    if rel.join("config.toml").is_file() && !p.i("kernel/config.toml").exists() {
        write_atomic(&p.i("kernel/config.toml"), &fs::read(rel.join("config.toml"))?, 0o644)?;
    }
    Ok(())
}

pub(crate) fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    if !from.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)? {
        let (e, t) = { let e = e?; let t = to.join(e.file_name()); (e, t) };
        match e.file_type()? {
            ft if ft.is_dir() => copy_tree(&e.path(), &t)?,
            ft if ft.is_file() => write_atomic(&t, &fs::read(e.path())?, 0o644)?,
            _ => {} // symlinks are not copied
        }
    }
    Ok(())
}

/// `--toolchain`, else the toolchain of `~/.cargo/bin/rustup which rustc`, canonical; None if neither.
fn toolchain(p: &Paths, o: &Opts) -> Option<PathBuf> {
    let found = || {
        let out = Command::new(p.home.join(".cargo/bin/rustup")).args(["which", "rustc"]).env_clear().env("HOME", &p.home).output().ok()?;
        Some(PathBuf::from(String::from_utf8(out.stdout).ok()?.trim()).parent()?.parent()?.to_path_buf())
    };
    o.toolchain.clone().or_else(found).and_then(|t| fs::canonicalize(t).ok())
}

/// Absolute `DEVDIR` remains part of V2/V3 on every platform. Linux uses its
/// existing read-only system tool prefix; an empty path or `/` is never a fallback.
fn platform_tool_prefix() -> io::Result<Option<PathBuf>> {
    if cfg!(target_os = "linux") {
        let prefix = fs::canonicalize("/usr")?;
        if prefix != Path::new("/usr") || !prefix.is_dir() {
            return Err(bad("Linux system tool prefix /usr must be a canonical directory"));
        }
        Ok(Some(prefix))
    } else {
        Ok(["/Library/Developer/CommandLineTools", "/Applications/Xcode.app/Contents/Developer"]
            .into_iter().find(|d| Path::new(d).is_dir()).map(PathBuf::from))
    }
}

/// The Root's service file (tail Z2): the LaunchAgent plist, or the systemd user unit.
pub fn service_path(p: &Paths) -> PathBuf {
    p.home.join(if cfg!(target_os = "macos") { "Library/LaunchAgents/dev.init.root.plist" } else { ".config/systemd/user/init-root.service" })
}

fn service_file(p: &Paths) -> io::Result<PathBuf> {
    let x = |s: &Path| s.display().to_string().replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let (exe, rh) = (x(&p.r("bin/init-root")), x(&p.rh));
    let (f, text) = if cfg!(target_os = "macos") {
        let old = p.home.join("Library/LaunchAgents/dev.init.kernel.plist");
        if old.exists() {
            println!("a v0.2 kernel job exists; unload it with: launchctl bootout gui/$(id -u) {}", old.display());
        }
        (service_path(p), format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\"><dict>\n<key>Label</key><string>dev.init.root</string>\n\
             <key>ProgramArguments</key><array><string>{exe}</string><string>start</string></array>\n\
             <key>EnvironmentVariables</key><dict><key>INIT_ROOT_HOME</key><string>{rh}</string></dict>\n\
             <key>KeepAlive</key><true/>\n<key>ThrottleInterval</key><integer>10</integer>\n<key>ExitTimeOut</key><integer>60</integer>\n\
             <key>StandardOutPath</key><string>{rh}/logs/launchd.log</string>\n<key>StandardErrorPath</key><string>{rh}/logs/launchd.log</string>\n\
             </dict></plist>\n"))
    } else {
        // LINUX: systemd quoting, not XML (a literal `%` is `%%`; `"` and `\` are escaped inside the quotes).
        // Delegate=memory gives the Root its own cgroup v2 subtree for the per-workload memory.max (cgroup.rs).
        let q = |s: &Path| s.display().to_string().replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%");
        let (exe, rh) = (q(&p.r("bin/init-root")), q(&p.rh));
        (service_path(p), format!(
            "[Unit]\nDescription=INIT Root\n\n[Service]\nExecStart=\"{exe}\" start\nEnvironment=\"INIT_ROOT_HOME={rh}\"\n\
             Restart=always\nRestartSec=2\nKillMode=mixed\nTimeoutStopSec=60\nDelegate=memory\n\n[Install]\nWantedBy=default.target\n"))
    };
    fs::create_dir_all(f.parent().ok_or_else(|| bad("service dir"))?)?;
    write_atomic(&f, text.as_bytes(), 0o644)?;
    Ok(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_scratch_directories_exist_before_live_and_install_launch() {
        let top = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/test-homes/kernel-scratch-{}-{}", std::process::id(), hex(&util::random::<4>().unwrap())));
        let p = Paths { rh: top.join("rh"), ih: top.join("ih"), home: top.clone() };
        fs::create_dir_all(&p.ih).unwrap();
        prepare_kernel_scratch(&p).unwrap();
        for name in ["cache", "cache/kernel-home", "cache/tmp"] {
            let m = fs::symlink_metadata(p.i(name)).unwrap();
            assert!(m.file_type().is_dir(), "{name}");
            assert_eq!(m.mode() & 0o777, 0o700, "{name}");
        }
        fs::write(p.i("cache/kernel-home/retained"), b"keep").unwrap();
        prepare_kernel_scratch(&p).unwrap();
        assert_eq!(fs::read(p.i("cache/kernel-home/retained")).unwrap(), b"keep");
        fs::remove_dir_all(top).unwrap();
    }

    #[test]
    fn kernel_scratch_preparation_refuses_symlink_cache_and_children() {
        let top = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/test-homes/kernel-scratch-links-{}-{}", std::process::id(), hex(&util::random::<4>().unwrap())));
        let outside = top.join("outside"); fs::create_dir_all(&outside).unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(outside.join("retained"), b"private").unwrap();
        for (n, name) in ["cache", "cache/kernel-home", "cache/tmp"].into_iter().enumerate() {
            let p = Paths { rh: top.join("rh"), ih: top.join(format!("ih-{n}")), home: top.clone() };
            fs::create_dir_all(p.i(name).parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(&outside, p.i(name)).unwrap();
            assert!(prepare_kernel_scratch(&p).is_err(), "{name}");
            assert_eq!(fs::metadata(&outside).unwrap().mode() & 0o777, 0o755);
            assert!(!outside.join("kernel-home").exists());
            assert!(!outside.join("tmp").exists());
            assert_eq!(fs::read(outside.join("retained")).unwrap(), b"private");
        }
        fs::remove_dir_all(top).unwrap();
    }

    #[test]
    fn platform_tool_prefix_preserves_absolute_broker_contract() {
        let prefix = platform_tool_prefix().unwrap();
        #[cfg(target_os = "linux")]
        {
            assert_eq!(prefix.as_deref(), Some(Path::new("/usr")));
            assert_eq!(fs::canonicalize(prefix.as_ref().unwrap()).unwrap(), prefix.clone().unwrap());
            let info = json!({"toolchain": "/toolchain", "devdir": prefix});
            let config: toml::Table = toml::from_str(&config_template(&info)).unwrap();
            assert_eq!(config["sandbox"]["developer_dir"].as_str(), Some("/usr"));
        }
        #[cfg(target_os = "macos")]
        assert_eq!(prefix, ["/Library/Developer/CommandLineTools", "/Applications/Xcode.app/Contents/Developer"]
            .into_iter().find(|d| Path::new(d).is_dir()).map(PathBuf::from));
    }

    #[test]
    fn seed_tags_are_semver_and_the_template_matches_install_json() {
        assert_eq!(semver("seed-v0.10.0"), Some((0, 10, 0)));
        assert!(semver("seed-v0.10.0") > semver("seed-v0.9.12"), "numeric, not lexical");
        for bad in ["seed-v1.2", "seed-v1.2.3.4", "v1.2.3", "seed-v1.x.3", "seed-v"] {
            assert_eq!(semver(bad), None, "{bad}");
        }
        let info = json!({"toolchain": "/t/stable \"x\"", "devdir": "/Library/Developer/CommandLineTools"});
        let t: toml::Table = toml::from_str(&config_template(&info)).unwrap();
        assert_eq!(t["schema_version"].as_integer(), Some(2));
        assert_eq!(t["sandbox"]["toolchain"].as_str(), Some("/t/stable \"x\""), "a TOML basic string, escaped");
        assert_eq!(t["sandbox"]["developer_dir"].as_str(), Some("/Library/Developer/CommandLineTools"));
    }

    /// G0-ROOT: `init-mockllm` is in every install's `kernel/bin`; `init-fakebody` only with the testkit; a
    /// kernel release without the mock is refused outside test mode.
    #[test]
    fn kernel_bin_set_always_has_the_mock() {
        let top = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/test-homes/g0root/bins-{}", std::process::id()));
        let _ = fs::remove_dir_all(&top);
        let rel = top.join("rel");
        fs::create_dir_all(&rel).unwrap();
        for b in KERNEL_BINS.iter().chain(&TESTKIT_BINS) {
            fs::write(rel.join(b), format!("#!{b}\n")).unwrap();
        }
        let installed = |home: &str, testkit: bool, test_mode: bool| {
            let p = Paths { rh: top.join(format!("{home}-rh")), ih: top.join(home), home: top.clone() };
            fs::create_dir_all(p.i("kernel/bin")).unwrap();
            copy_release(&rel, &p, testkit, test_mode).map(|()| {
                let mut v: Vec<String> = fs::read_dir(p.i("kernel/bin")).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
                v.sort();
                v
            })
        };
        assert_eq!(installed("plain", false, false).unwrap(), ["init-kernel", "init-mockllm", "initctl"]);
        assert_eq!(installed("kit", true, false).unwrap(), ["init-fakebody", "init-kernel", "init-mockllm", "initctl"]);
        assert_eq!(fs::read(top.join("plain/kernel/bin/init-mockllm")).unwrap(), b"#!init-mockllm\n");
        fs::remove_file(rel.join("init-mockllm")).unwrap();
        assert!(installed("nomock", false, false).unwrap_err().to_string().contains("init-mockllm is missing"));
        assert_eq!(installed("nomock-test", false, true).unwrap(), ["init-kernel", "initctl"], "test mode: a warning only");
        fs::remove_dir_all(&top).unwrap(); // exact path, this test's own
    }

    #[test]
    fn no_toolchain_config_and_toolchain_set() {
        let t: toml::Table = toml::from_str(&config_template(&json!({"toolchain": null, "init_home": "/h/.init", "devdir": "/Library/Developer/CommandLineTools"}))).unwrap();
        assert_eq!(t["sandbox"]["toolchain"].as_str(), Some("/h/.init/kernel"), "the broker's fallback, never an empty path");
        assert_eq!(t["sandbox"]["developer_dir"].as_str(), Some("/Library/Developer/CommandLineTools"));
        let set = set_sandbox_toolchain(&config_template(&json!({"devdir": "/d"})), Path::new("/tc/x")).unwrap();
        let t: toml::Table = toml::from_str(&set).unwrap();
        assert_eq!((t["sandbox"]["toolchain"].as_str(), t["sandbox"]["developer_dir"].as_str()), (Some("/tc/x"), Some("/d")));
        let again = set_sandbox_toolchain(&format!("{set}\n[model]\nid = \"m\"\n"), Path::new("/tc/y")).unwrap();
        assert_eq!(again.matches("toolchain = ").count(), 1, "{again}");
        assert!(again.contains("toolchain = \"/tc/y\"") && again.contains("[model]"));
        assert!(set_sandbox_toolchain("schema_version = 2\n", Path::new("/tc")).is_err());
    }

    #[test]
    fn evolution_tiers_follow_the_recorded_toolchain() {
        let top = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/test-homes/evo-tiers-{}", std::process::id()));
        let _ = fs::remove_dir_all(&top);
        let p = Paths { rh: top.join("rh"), ih: top.join("ih"), home: top.clone() };
        fs::create_dir_all(&p.rh).unwrap();
        fs::write(p.r("install.json"), r#"{"toolchain":null}"#).unwrap();
        let v = evolution(&p);
        assert_eq!(v["tiers"], json!(["T0"]));
        assert!(v["hint"].as_str().unwrap().contains(RUSTUP_CMD) && v["hint"].as_str().unwrap().contains("init-root toolchain --set"));
        let tc = top.join("tc");
        fs::create_dir_all(tc.join("bin")).unwrap();
        fs::write(tc.join("bin/cargo"), b"").unwrap();
        fs::write(p.r("install.json"), json!({"toolchain": tc}).to_string()).unwrap();
        assert_eq!(evolution(&p)["tiers"], json!(["T0"]), "no rustc");
        fs::write(tc.join("bin/rustc"), b"").unwrap();
        assert_eq!(evolution(&p)["tiers"], json!(["T0", "T1", "T2"]));
        assert!(evolution(&p)["hint"].is_null());
        fs::remove_dir_all(&top).unwrap(); // exact path, this test's own
    }

    #[test]
    fn a_prebuilt_body_is_verified_before_it_is_staged() {
        let top = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/test-homes/prebuilt-root-{}", std::process::id()));
        let _ = fs::remove_dir_all(&top);
        let p = Paths { rh: top.join("rh"), ih: top.join("ih"), home: top.clone() };
        fs::create_dir_all(p.i("kernel")).unwrap();
        fs::write(top.join("body"), b"body").unwrap();
        let good = Prebuilt { body: top.join("body"), sha256: util::sha256_hex(b"body"), commit: Some("a".repeat(40)) };
        let wrong = Prebuilt { sha256: util::sha256_hex(b"other"), ..good.clone() };
        assert!(stage_prebuilt(&p, &wrong).unwrap_err().contains("has sha256"));
        assert!(!p.i("kernel/prebuilt/init-body").exists(), "nothing staged on a mismatch");
        assert!(stage_prebuilt(&p, &Prebuilt { commit: Some("xyz".into()), ..good.clone() }).is_err());
        stage_prebuilt(&p, &good).unwrap();
        assert_eq!(fs::read(p.i("kernel/prebuilt/init-body")).unwrap(), b"body");
        let rec: Value = serde_json::from_str(&fs::read_to_string(p.i("kernel/prebuilt/init-body.json")).unwrap()).unwrap();
        assert_eq!((rec["sha256"].as_str(), rec["commit"].as_str()), (Some(good.sha256.as_str()), Some("a".repeat(40).as_str())));
        drop_prebuilt(&p);
        assert!(!p.i("kernel/prebuilt").exists());
        fs::remove_dir_all(&top).unwrap(); // exact path, this test's own
    }

    #[test]
    fn placement_rules() {
        let base = PathBuf::from("/nonexistent-wp20"); // never created: the rules only read the filesystem
        let p = |rh: &str, ih: &str| Paths { rh: base.join(rh), ih: base.join(ih), home: base.clone() };
        assert_eq!(check_paths(&p("rh", "ih"), &[]), Ok(()));
        assert!(check_paths(&p("rh", "rh/ih"), &[]).is_err());
        assert!(check_paths(&p("ih/rh", "ih"), &[]).is_err());
        assert!(check_paths(&p(&"x".repeat(70), "ih"), &[]).is_err());
        assert!(check_paths(&p("rh", "ih"), &[base.display().to_string()]).is_err());
        let sys = Paths { rh: "/usr/local/r".into(), ih: base.join("ih"), home: base.clone() };
        assert!(check_paths(&sys, &[]).unwrap_err().contains("/usr"));
        let rel = Paths { rh: "rel/r".into(), ih: base.join("ih"), home: base.clone() };
        assert!(check_paths(&rel, &[]).is_err());
    }
}
