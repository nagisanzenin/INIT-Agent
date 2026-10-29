//! `init-root upgrade` (lane UPGRADE, v0.5.0; docs/guide.md "Upgrade", docs/ux-v1.md §3.7 as built): the owner's
//! stop-then-replace upgrade to a new release. It replaces the Root, the kernel and INIT's launchers and keeps what the
//! agent has become (its body, extensions, sessions, lineage) and the owner's keys and charter byte for byte.
//!
//! An owner action only. Every step runs from the owner's shell (`init-agent upgrade`, or get.sh `--upgrade`, run the
//! release's `scripts/install.sh --upgrade`), outside every sandbox, and writes only ROOT_HOME, INIT_HOME's Root-written
//! `kernel/` and INIT's launchers, which the zone can write in no access mode (tail Z1, Z3a, Z4; ADR-012). The verbs:
//!
//! - `upgrade --release DIR [--bin-dir DIR]` (run as `DIR/init-root`, the Root stopped, under `run/root.lock`): the
//!   refusals come first and change nothing: a running Root, a pending upgrade, the panic latch, an open change group,
//!   a downgrade, the same release again (exit 4), a body this release's kernel cannot run (below; exit 3). Then a
//!   snapshot of everything it replaces into `ROOT_HOME/state/upgrade/<id>/` (APFS clones; reflinks or copies on
//!   Linux), the journal `ROOT_HOME/state/upgrade.json`, and the replace: `kernel/bin`, `kernel/{docs,wit,src,
//!   kupgrade-smoke}`, the kernel source (`refs/upstream/<version>`, `refs/heads/main`), the tuple {new kernel, current
//!   body, current ext} (a `tuple` record moves `current`; `lkg` stays until `--commit`), `ROOT_HOME/bin/init-root`
//!   and `install.json` `release`. An error after the snapshot restores it, so the old install is left as it was.
//! - `upgrade --probe [--timeout S]`: starts the installed Root as this process's child, waits until the kernel is
//!   ready on the new tuple and the body has written `ready` since the start, checks it stays up, stops it (the zone,
//!   then SIGTERM to the child) and verifies the whole lineage. Exit 0 healthy, 1 not.
//! - `upgrade --commit`: `lkg` moves to the new tuple, the rollback anchors (`audited`, `settled`) and the twin take
//!   its kernel with their own body and ext (`tuple::reanchor`), goal readings on changes from before end
//!   (`goal::upgrade_committed`), then the journal says `committing` and the snapshot goes. Every step before
//!   `committing` is idempotent, so a commit re-run after a crash finishes it. `upgrade --rollback` (refused from
//!   `committing` on, and while the snapshot is incomplete): a `rollback` record back to the tuple from before,
//!   `refs/heads/main` back, the snapshot restored (Root binary, kernel files, launchers, install.json), checked
//!   against the recorded sha256 of the old Root binary, and what a commit that stopped part way moved (`lkg`, the
//!   anchors, the twin, `refs/heads/lkg`) back to the journal's `from`. `upgrade --status [--json]`.
//!
//! **The body check.** The evolved body is the agent's code, so the Root never runs it outside a sandbox: its
//! `--manifest` (body/boot.md §4.3: no env, no files, no sockets) runs in a sealed run root (root.md §3.3.6), and so
//! does the release's own body (the bundle's `body/init-body`, else the release's `init-body`), which was built and
//! gated with the release's kernel and so states what that kernel expects of a body. The control channel and the
//! manifest format must be equal and the WIT majors must overlap. Otherwise the upgrade is refused: INIT never migrates
//! a body and never guesses.
//!
//! **The signature (v0.5.3, docs/release-signing.md)** is checked before this runs, by the installed Root: get.sh
//! `--upgrade` asks `ROOT_HOME/bin/init-root release verify` (release.rs), with the key compiled into that binary,
//! before it unpacks the download. This verb runs as the new release's own binary, which cannot vouch for itself, so it
//! checks nothing about the signature; a local bundle (`--bundle FILE --sha256 HEX`) is the owner's own check.
//!
//! **Not here (deferred, docs/guide.md):** offering the release's new seed to the agent (its `seed/` bundle is not
//! imported); a live kernel swap (`system.swap`).

use crate::install::{self, Paths};
use crate::link::{self, Root, lock};
use crate::sealed::{Env, RunRoot};
use crate::tuple;
use crate::util::{self, icj, now_ms, rfc3339, write_atomic};
use serde_json::{Value, json};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const JOURNAL: &str = "state/upgrade.json";
const SNAPS: &str = "state/upgrade";
/// Refused: nothing changed (a version or body check, a precondition the owner can fix).
pub const REFUSED: u8 = 3;
/// The release is the one installed: nothing to do.
pub const CURRENT: u8 = 4;
const USAGE: &str = "usage: init-root upgrade --release DIR [--bin-dir DIR] [--by-installer] | --probe [--timeout S] | --commit | --rollback | --status [--json]";
/// Where the replace step keeps the release's get.sh, inside the upgrade's snapshot directory (not a snapshot item).
const GET_SH: &str = "release/get.sh";
/// The variable `upgrade --probe` sets on the Root it starts: the pending upgrade's id. That Root is the one `start`
/// that runs the zone on the upgrade's tuple before the commit ([`at_start`]).
const PROBE_ENV: &str = "INIT_UPGRADE_PROBE";
/// INIT's launchers in the bin dir (install.sh writes them; `init` is the pre-v0.4.1 name and the v0.4.1 macOS shim).
const LAUNCHERS: [&str; 3] = ["init-agent", "init-tui", "init"];

type Res<T> = Result<T, (u8, String)>;

fn err<E: std::fmt::Display>(e: E) -> (u8, String) {
    (1, e.to_string())
}

fn refuse(m: impl Into<String>) -> (u8, String) {
    (REFUSED, m.into())
}

pub fn cli(p: &Paths, mut a: Vec<String>) -> u8 {
    let opt = |a: &mut Vec<String>, k: &str| a.iter().position(|x| x == k).filter(|i| i + 1 < a.len()).map(|i| a.drain(i..=i + 1).nth(1).unwrap_or_default());
    let flag = |a: &mut Vec<String>, k: &str| { let n = a.len(); a.retain(|x| x != k); a.len() != n };
    let r = if let Some(rel) = opt(&mut a, "--release") {
        let bin = opt(&mut a, "--bin-dir").map(PathBuf::from);
        // R-UNDO (v0.5.2): install.sh runs --probe and --commit itself, so its replace step prints no "next" (the owner
        // read "… upgrade --probe, then --commit" after the installer had already run them)
        let by_installer = flag(&mut a, "--by-installer");
        upgrade(p, Path::new(&rel), bin).map(|mut v| {
            if by_installer && let Some(o) = v.as_object_mut() {
                o.remove("next");
            }
            v
        })
    } else if flag(&mut a, "--probe") {
        let t = opt(&mut a, "--timeout").and_then(|s| s.parse::<u64>().ok()).unwrap_or(120);
        probe(p, Duration::from_secs(t.clamp(10, 1800)))
    } else if flag(&mut a, "--commit") {
        commit(p)
    } else if flag(&mut a, "--rollback") {
        rollback(p)
    } else if flag(&mut a, "--status") {
        let j = journal(p).unwrap_or(Value::Null);
        if flag(&mut a, "--json") {
            Ok(j)
        } else {
            println!("{}", describe(&j));
            return 0;
        }
    } else {
        Err((64, USAGE.into()))
    };
    match r {
        Ok(v) => {
            println!("{}", icj(&v).unwrap_or_default());
            0
        }
        Err((c, m)) => {
            eprintln!("init-root upgrade: {m}");
            c
        }
    }
}

fn describe(j: &Value) -> String {
    if j.is_null() {
        return "no upgrade has run on this install".into();
    }
    let v = |s: &str| j[s]["version"].as_str().unwrap_or("?").to_string();
    format!("upgrade {} {} → {}: {} at {}", j["id"].as_str().unwrap_or("?"), v("from"), v("to"), j["phase"].as_str().unwrap_or("?"),
        j["at"].as_str().unwrap_or("?"))
}

// ------------------------------------------------------------------------------------------------ the journal

fn journal(p: &Paths) -> Option<Value> {
    fs::read_to_string(p.r(JOURNAL)).ok().and_then(|s| serde_json::from_str(&s).ok())
}

fn save(p: &Paths, j: &Value) -> Res<()> {
    write_atomic(&p.r(JOURNAL), format!("{}\n", icj(j).map_err(err)?).as_bytes(), 0o600).map_err(err)
}

fn pending(j: &Value) -> bool {
    matches!(j["phase"].as_str(), Some("replacing" | "replaced" | "committing"))
}

/// The snapshot directory a journal names (always under `ROOT_HOME/state/upgrade/`).
fn snap_dir(p: &Paths, j: &Value) -> Option<PathBuf> {
    let id = j["id"].as_str().filter(|i| i.len() <= 40 && i.starts_with("u-") && i.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'))?;
    Some(p.r(SNAPS).join(id))
}

// ------------------------------------------------------------------------------------------------ the release

/// What a release directory provides.
struct Release {
    dir: PathBuf,
    /// The bundle's `VERSION` (its label included, e.g. `0.5.0-preview`), else MANIFEST.json's version.
    version: String,
    commit: String,
    /// The kernel source bundle `src/<version>.bundle`, if the release has one.
    src: Option<PathBuf>,
    /// The release's own body: what its kernel was built and gated with.
    body: Option<PathBuf>,
}

pub(crate) fn plain(v: &str) -> bool {
    (1..=64).contains(&v.len()) && !v.starts_with('.') && !v.contains("..") && v.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-+".contains(&c))
}

/// A release directory's identity, `{version, commit}`: the bundle's `VERSION` (its label included, e.g.
/// `0.5.0-preview`), else MANIFEST.json's version, else this binary's; the commit from MANIFEST.json ("" if none).
/// `install` records it in install.json `release`, which `upgrade` compares a new release with.
pub fn release_id(dir: &Path) -> Value {
    let m: Value = fs::read_to_string(dir.join("MANIFEST.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
    let label = fs::read_to_string(dir.join("VERSION")).ok().map(|s| s.trim().to_string()).filter(|s| plain(s));
    let version = label.or_else(|| m["version"].as_str().map(String::from)).unwrap_or_else(|| install::VERSION.to_string());
    json!({"commit": m["commit"].as_str().unwrap_or(""), "version": version})
}

fn release(dir: &Path, test_mode: bool) -> Res<Release> {
    let dir = fs::canonicalize(dir).map_err(|e| (1, format!("--release {}: {e}", dir.display())))?;
    let id = release_id(&dir);
    let version = id["version"].as_str().unwrap_or("").to_string();
    if !plain(&version) {
        return Err((1, format!("the release's version {version:?} is not a plain version string")));
    }
    for b in ["init-root", "init-kernel"] {
        if !dir.join(b).is_file() {
            return Err((1, format!("{} has no {b}: not a release directory", dir.display())));
        }
    }
    if !dir.join("init-mockllm").is_file() && !test_mode {
        return Err((1, format!("{}/init-mockllm is missing; every install ships it (G0 runs the body's tests against it)", dir.display())));
    }
    let mut srcs: Vec<PathBuf> = fs::read_dir(dir.join("src")).into_iter().flatten().flatten().map(|e| e.path())
        .filter(|x| x.extension().is_some_and(|e| e == "bundle")).collect();
    let src = match (srcs.len(), test_mode) {
        (1, _) => srcs.pop(),
        (0, true) => None,
        (n, _) => return Err((1, format!("{}/src holds {n} kernel source bundles; a release holds exactly one", dir.display()))),
    };
    let body = [dir.join("body/init-body"), dir.join("init-body")].into_iter().find(|b| b.is_file());
    Ok(Release { commit: id["commit"].as_str().unwrap_or("").to_string(), dir, version, src, body })
}

/// The `MAJOR.MINOR.PATCH` of a version string (`0.5.0-preview` → (0, 5, 0)).
pub(crate) fn core(v: &str) -> Option<(u64, u64, u64)> {
    let c = v.split(['-', '+']).next()?;
    let mut it = c.split('.').map(|x| x.parse::<u64>().ok());
    let r = (it.next()??, it.next()??, it.next()??);
    it.next().is_none().then_some(r)
}

/// V4 (v0.5.3): SemVer 2.0 precedence (§11) of two versions with a [`core`]: the core, then a version with a pre-release
/// label below the same core without one, then the labels field by field (numeric fields numerically and below
/// alphanumeric ones, which compare in ASCII; a longer label wins a tie); build metadata (`+…`) is ignored. None when
/// either has no core. Equal precedence is not the same release: only the same version string is (release.rs `check`).
/// get.sh's `ver_older` is the same rule.
pub(crate) fn precedence(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    fn label(v: &str) -> Option<Vec<(bool, usize, &str)>> {
        let (_, l) = v.split('+').next()?.split_once('-')?;
        Some(l.split('.').map(|x| if !x.is_empty() && x.bytes().all(|c| c.is_ascii_digit()) { (false, x.len(), x) } else { (true, 0, x) }).collect())
    }
    let (ka, kb) = ((core(a)?, label(a).is_none()), (core(b)?, label(b).is_none()));
    Some(ka.cmp(&kb).then_with(|| label(a).cmp(&label(b))))
}

// ------------------------------------------------------------------------------------------------ the body check

/// `init-body --manifest` of `bin` in a sealed run root: no network, no ROOT_HOME, no INIT_HOME, 30 s.
fn manifest(env: &Env, bin: &Path) -> Result<Value, String> {
    let run = RunRoot::new(&env.rh)?;
    let b = run.dir.join("bin/init-body");
    fs::copy(bin, &b).and_then(|_| fs::set_permissions(&b, std::os::unix::fs::PermissionsExt::from_mode(0o500)))
        .map_err(|e| format!("{}: {e}", bin.display()))?;
    let out = run.dir.join("out/manifest.json");
    let f = fs::File::create(&out).map_err(|e| e.to_string())?;
    let mut c = crate::sealed::sandboxed(env, &run, None, &[b.clone().into(), "--manifest".into()]);
    c.stdout(Stdio::from(f));
    let lim = crate::memwatch::Limits::of(&env.ch);
    let (code, why, _) = crate::sealed::supervise(c, &run, &lim, Duration::from_secs(30), None, || None)?;
    let mut text = String::new();
    fs::File::open(&out).and_then(|f| f.take(1 << 20).read_to_string(&mut text)).map_err(|e| e.to_string())?;
    if code != Some(0) {
        return Err(format!("`--manifest` ended {why} (exit {code:?})"));
    }
    let v: Value = serde_json::from_str(text.lines().next().unwrap_or("")).map_err(|e| format!("`--manifest` printed no JSON: {e}"))?;
    if v["control_channel"].is_string() { Ok(v) } else { Err("`--manifest` names no control_channel".into()) }
}

/// The installed body (`have`) against the release's (`want`): the same control channel and manifest format, and a
/// WIT major in common.
fn compatible(have: &Value, want: &Value) -> Result<(), String> {
    let s = |v: &Value, k: &str| v[k].as_str().map(String::from).unwrap_or_else(|| v[k].to_string());
    if have["control_channel"] != want["control_channel"] {
        return Err(format!("the installed body speaks {}, and this release's kernel speaks {}", s(have, "control_channel"), s(want, "control_channel")));
    }
    if have["manifest"] != want["manifest"] {
        return Err(format!("the installed body's manifest format is {}, and this release reads format {}", s(have, "manifest"), s(want, "manifest")));
    }
    let majors = |v: &Value| v["ext_api_majors"].as_array().cloned().unwrap_or_default();
    if !majors(have).iter().any(|m| majors(want).contains(m)) {
        return Err(format!("the installed body hosts extension WIT majors {}, and this release's {}", s(have, "ext_api_majors"), s(want, "ext_api_majors")));
    }
    Ok(())
}

/// The body check (exit 3 on a mismatch). `body` is the current tuple's body binary blob; "" (a seedless test home)
/// has nothing to check.
fn check_body(p: &Paths, rel: &Release, body: &str) -> Res<Value> {
    if body.is_empty() {
        return Ok(json!({"body": null}));
    }
    let blob = tuple::blob(&p.rh, body).filter(|b| b.is_file()).ok_or_else(|| (1, format!("the current body {body} is not in the store")))?;
    let reference = rel.body.as_ref().ok_or_else(|| refuse(format!(
        "{} has no body (body/init-body or init-body) to tell what its kernel expects; nothing changed", rel.dir.display())))?;
    let mut env = Env::load(p).map_err(err)?;
    if env.info["toolchain"].is_null() {
        env.info["toolchain"] = json!(p.i("kernel")); // the broker's harmless fallback (install.rs config_template)
    }
    let have = manifest(&env, &blob).map_err(|m| refuse(format!("the installed body's manifest could not be read in the sandbox ({m}); nothing changed")))?;
    let want = manifest(&env, reference).map_err(|m| (1, format!("the release's body manifest: {m}")))?;
    compatible(&have, &want).map_err(|m| refuse(format!(
        "{m}. INIT never migrates an evolved body: this release cannot run it. Nothing changed; keep this version, or \
         uninstall and install fresh (the evolved body goes to the Trash with INIT_HOME).")))?;
    Ok(json!({"body": body, "control_channel": have["control_channel"], "manifest": have["manifest"]}))
}

// ------------------------------------------------------------------------------------------------ the replace step

/// One item the upgrade may replace, as (key in the snapshot, live path).
fn items(p: &Paths, bin: &Path) -> Vec<(String, PathBuf)> {
    let mut v: Vec<(String, PathBuf)> = vec![("rh/bin/init-root".into(), p.r("bin/init-root")), ("rh/install.json".into(), p.r("install.json"))];
    for d in ["bin", "docs", "wit", "src", "kupgrade-smoke"] {
        v.push((format!("ih/kernel/{d}"), p.i("kernel").join(d)));
    }
    v.extend(LAUNCHERS.iter().map(|n| (format!("bin/{n}"), bin.join(n))));
    v
}

fn upgrade(p: &Paths, dir: &Path, bin_dir: Option<PathBuf>) -> Res<Value> {
    let info: Value = fs::read_to_string(p.r("install.json")).ok().and_then(|s| serde_json::from_str(&s).ok())
        .ok_or_else(|| (1, format!("{} is missing: INIT is not installed here (install it with scripts/install.sh)", p.r("install.json").display())))?;
    if let Some(j) = journal(p).filter(pending) {
        if j["phase"] == "committing" {
            return Err((1, format!("{}; finish it first: {}", describe(&j), p.cmd("upgrade --commit"))));
        }
        return Err((1, format!("{}; finish it first: {} (or {} after a passing --probe)", describe(&j), p.cmd("upgrade --rollback"), p.cmd("upgrade --commit"))));
    }
    let _lock = crate::cli::take_lock(p).map_err(|e| (1, format!("run/root.lock: {e}")))?
        .ok_or_else(|| (1, format!("the Root is running: stop it first ({} and install.sh --upgrade stop it for you)", p.agent_cmd("upgrade"))))?;
    let active = crate::charter::load(&p.r("state/charter.active.toml"), &p.ctx()).map_err(|_| (1, "the active charter is invalid".to_string()))?;
    let rel = release(dir, active.ch.root.test_mode)?;
    let me = std::env::current_exe().and_then(fs::read).map_err(err)?;
    if fs::read(rel.dir.join("init-root")).map_err(err)? != me {
        return Err((1, format!("run the release's own {}/init-root: the new Root performs its own upgrade", rel.dir.display())));
    }
    if fs::symlink_metadata(crate::panic::latch(&p.rh)).is_ok() {
        return Err(refuse(format!("INIT is in panic (the latch is set); the upgrade needs a system that runs. Resume it first: start the Root, type /resume in init-agent (or run {}), then run the upgrade again.", p.cmd("resume"))));
    }
    install::check_layout(p, &active.ch.sandbox.extra_read).map_err(|m| (1, format!("layout: {m}")))?;
    if crate::lineage::verify::audit_verify(p, true, false) != 0 {
        return Err((1, "the lineage does not verify; nothing changed".into()));
    }
    let recs = crate::lineage::records(&p.rh).map_err(err)?;
    let mut ptrs = tuple::Ptrs::default();
    recs.iter().for_each(|r| ptrs.apply(r));
    let open: Vec<u64> = {
        let mut o = std::collections::BTreeSet::new();
        for r in &recs {
            match (r["kind"].as_str(), r["data"]["group"].as_u64()) {
                (Some("change_open"), Some(g)) => drop(o.insert(g)),
                (Some("change_close"), Some(g)) => drop(o.remove(&g)),
                _ => {}
            }
        }
        o.into_iter().collect()
    };
    if !open.is_empty() {
        return Err(refuse(format!("self-change group(s) {open:?} are still in their watch window; upgrade when they have closed \
            (or roll them back), so a later rollback never returns to a tuple of the old kernel")));
    }
    let cur_id = ptrs.current.clone().ok_or_else(|| (1, "no current tuple: the install is incomplete".to_string()))?;
    let cur = tuple::load(&p.rh, &cur_id).filter(|t| tuple::verify(&p.rh, t))
        .ok_or_else(|| (1, format!("the current tuple {cur_id} does not verify; nothing changed")))?;
    let git = info["git"].as_str().unwrap_or("/usr/bin/git").to_string();
    let main = crate::lineage::kgit::get(p, &git, "refs/heads/main");
    // the version: never down, and the same release again is a no-op
    let old_version = info["release"]["version"].as_str().or(cur["kernel"]["version"].as_str()).unwrap_or("0.0.0").to_string();
    // V4: by SemVer precedence, so a lower pre-release label of the same core is a downgrade too (release.rs `check`)
    if precedence(&rel.version, &old_version) == Some(std::cmp::Ordering::Less) {
        return Err(refuse(format!("{} is older than the installed {old_version}: INIT does not downgrade", rel.version)));
    }
    let src_sha = match &rel.src {
        Some(s) => Some(src_head(p, &git, s)?),
        None => None,
    };
    let root_sha = tuple::sha_of(&p.r("bin/init-root")).unwrap_or_default();
    let kernel_same = tuple::sha_of(&rel.dir.join("init-kernel")) == tuple::sha_of(&p.i("kernel/bin/init-kernel"));
    if util::sha256_hex(&me) == root_sha && kernel_same && (src_sha.is_none() || src_sha == main) {
        return Err((CURRENT, format!("INIT already runs this release ({}); nothing to do", rel.version)));
    }
    let body = check_body(p, &rel, cur["body"]["binary"].as_str().unwrap_or(""))?;
    // disk: the new files on top of the charter's floor
    let size = |x: &Path| fs::metadata(x).map(|m| m.len()).unwrap_or(0);
    let need: u64 = ["init-root", "init-kernel", "initctl", "init-mockllm"].iter().map(|b| size(&rel.dir.join(b))).sum::<u64>()
        + rel.src.as_deref().map_or(0, size) + (64 << 20);
    let floor = (active.ch.quotas.disk_min_free_gib.max(0) as u64) << 30;
    let free = crate::sys::free_bytes(&p.rh).map_err(err)?;
    if free < floor + need {
        return Err(refuse(format!("{} MiB free; the upgrade needs {} MiB above the charter's floor of {} GiB", free >> 20, need >> 20, floor >> 30)));
    }
    let bin = bin_dir.or_else(|| info["bin_dir"].as_str().map(PathBuf::from)).unwrap_or_else(|| p.home.join(".local/bin"));
    if !bin.is_absolute() {
        return Err((1, "--bin-dir must be absolute".into()));
    }
    // the snapshot, then the journal (`replacing`): from here every error restores the snapshot
    let id = format!("u-{}-{}", rfc3339(now_ms()).replace([':', '.'], "").replace('T', "-").trim_end_matches('Z'), util::hex(&util::random::<3>().map_err(err)?));
    let snap = p.r(SNAPS).join(&id);
    install::make_dirs(&p.rh, &[SNAPS]).map_err(err)?;
    let mut list = Vec::new();
    for (k, live) in items(p, &bin) {
        let m = fs::symlink_metadata(&live).ok();
        let kind = match &m {
            Some(m) if m.is_dir() => "dir",
            Some(m) if m.is_file() => "file",
            Some(_) => "other", // a link or a special file: never followed, never replaced, never restored
            None => "none",
        };
        if matches!(kind, "dir" | "file") {
            let dst = snap.join(&k);
            fs::DirBuilder::new().recursive(true).create(dst.parent().unwrap_or(&snap)).map_err(err)?;
            if let Err(e) = crate::sys::clonefile(&live, &dst) {
                let _ = crate::snapshot::remove(&snap); // exact path, created above
                return Err((1, format!("snapshot of {}: {e}; nothing changed", live.display())));
            }
        }
        list.push(json!({"key": k, "kind": kind, "path": live}));
    }
    // R-UNDO (v0.5.2): the release's get.sh, kept beside the snapshot (not one of its items, so a rollback never
    // touches cache/get.sh) until `--commit` installs it; the release directory may be gone by then
    let get_sh = rel.dir.join("scripts/get.sh");
    if get_sh.is_file() {
        let staged = snap.join(GET_SH);
        let r = fs::DirBuilder::new().recursive(true).create(staged.parent().unwrap_or(&snap)).and_then(|()| util::copy_atomic(&get_sh, &staged, 0o755));
        if let Err(e) = r {
            let _ = crate::snapshot::remove(&snap); // exact path, created above
            return Err((1, format!("{}: {e}; nothing changed", get_sh.display())));
        }
    }
    // `from`: what a rollback restores, the pointers a commit moves included (`undo`)
    let kernel_lkg = crate::lineage::kgit::get(p, &git, "refs/heads/lkg");
    let mut j = json!({"v": 1, "id": id, "at": rfc3339(now_ms()), "phase": "replacing", "bin_dir": bin, "items": list, "body": body,
        "from": {"version": old_version, "tuple": cur_id, "lkg": ptrs.lkg, "audited": ptrs.audited, "settled": ptrs.settled, "twin": ptrs.twin,
            "kernel_lkg": kernel_lkg, "main": main, "root_sha256": root_sha},
        "to": {"version": rel.version, "commit": rel.commit, "main": src_sha, "root_sha256": util::sha256_hex(&me), "tuple": null}});
    save(p, &j)?;
    match replace(p, &rel, &git, &me, &cur, &cur_id, main.as_deref(), &mut j) {
        Ok(()) => {
            j["phase"] = json!("replaced");
            save(p, &j)?;
            Ok(json!({"upgrade": j["id"], "from": j["from"]["version"], "to": j["to"]["version"], "tuple": {"from": cur_id, "to": j["to"]["tuple"]},
                "body": j["body"], "next": format!("{} --probe, then --commit (or --rollback)", p.cmd("upgrade"))}))
        }
        Err(m) => {
            let back = undo(p, &j, &git);
            j["phase"] = json!("failed");
            j["error"] = json!(m);
            let _ = save(p, &j);
            Err((1, match back {
                Ok(()) => format!("{m}; the snapshot was restored and the old install is as it was"),
                Err(b) => format!("{m}; restoring the snapshot failed too ({b}): run {}", p.cmd("upgrade --rollback")),
            }))
        }
    }
}

/// The commit a kernel source bundle names (`refs/heads/release`).
fn src_head(p: &Paths, git: &str, src: &Path) -> Res<String> {
    let o = install::git(p, git).args(["bundle", "list-heads"]).arg(src).output().map_err(err)?;
    let s = String::from_utf8_lossy(&o.stdout);
    s.lines().filter_map(|l| l.split_once(' ')).find(|(_, r)| *r == "refs/heads/release").map(|(sha, _)| sha.to_string())
        .filter(|_| o.status.success()).ok_or_else(|| (1, format!("{}: not a kernel source bundle (refs/heads/release)", src.display())))
}

#[allow(clippy::too_many_arguments)]
fn replace(p: &Paths, rel: &Release, git: &str, me: &[u8], cur: &Value, cur_id: &str, main: Option<&str>, j: &mut Value) -> Result<(), String> {
    let s = |e: std::io::Error| e.to_string();
    let kbin = p.i("kernel/bin");
    let testkit = kbin.join("init-fakebody").is_file();
    for b in install::KERNEL_BINS.iter().chain(testkit.then_some(&install::TESTKIT_BINS[0])) {
        let src = rel.dir.join(b);
        if src.is_file() {
            util::copy_atomic(&src, &kbin.join(b), 0o755).map_err(|e| format!("kernel/bin/{b}: {e}"))?;
        }
    }
    for d in ["docs", "wit", "kupgrade-smoke"] {
        if rel.dir.join(d).is_dir() {
            crate::snapshot::remove(&p.i("kernel").join(d)).map_err(s)?;
            install::copy_tree(&rel.dir.join(d), &p.i("kernel").join(d)).map_err(s)?;
        }
    }
    if let Some(src) = &rel.src {
        let dst = p.i("kernel/src");
        crate::snapshot::remove(&dst).map_err(s)?;
        fs::create_dir_all(&dst).map_err(s)?;
        let file = dst.join(format!("{}.bundle", rel.version));
        util::copy_atomic(src, &file, 0o644).map_err(s)?;
        let sha = crate::lineage::kgit::import(p, git, &file, &rel.version)?;
        if main != Some(sha.as_str()) {
            crate::lineage::kgit::update(p, git, "refs/heads/main", &sha, main)?;
        }
    }
    // the tuple: the new kernel with the current body and ext (and everything else the current tuple carries)
    let root = Root::boot(p.clone()).map_err(s)?;
    if lock(&root.lin).is_none() {
        return Err("the lineage is unavailable".into());
    }
    let kernel = tuple::kernel_part(&root)?.ok_or("no kernel/bin/init-kernel after the copy")?;
    let same = ["init_kernel", "initctl", "built_from"].iter().all(|k| kernel[*k] == cur["kernel"][*k]);
    let to = if same {
        cur_id.to_string()
    } else {
        let mut t = cur.clone();
        t["kernel"] = kernel;
        let id = tuple::save(&p.rh, &t).map_err(s)?;
        root.append_by("root", "tuple", json!({"id": id, "tuple": t, "upgrade": {"from": j["from"]["version"], "id": j["id"], "to": j["to"]["version"]}}))
            .map_err(|e| format!("lineage: {e}"))?;
        id
    };
    j["to"]["tuple"] = json!(to);
    drop(root);
    write_atomic(&p.r("bin/init-root"), me, 0o755).map_err(|e| format!("bin/init-root: {e}"))?;
    let ip = p.r("install.json");
    let mut info: Value = fs::read_to_string(&ip).ok().and_then(|x| serde_json::from_str(&x).ok()).ok_or("install.json is unreadable")?;
    info["release"] = json!({"commit": rel.commit, "version": rel.version});
    write_atomic(&ip, format!("{}\n", icj(&info).map_err(s)?).as_bytes(), 0o600).map_err(s)
}

/// The snapshot items a journal names that its snapshot no longer holds (with their kind): a commit removes the
/// snapshot, and an incomplete one cannot put the old install back.
fn snapshot_missing(p: &Paths, j: &Value) -> Vec<String> {
    let Some(snap) = snap_dir(p, j) else { return vec!["the snapshot".into()] };
    j["items"].as_array().into_iter().flatten().filter_map(|it| {
        let (k, kind) = (it["key"].as_str()?, it["kind"].as_str()?);
        let m = fs::symlink_metadata(snap.join(k)).ok();
        let held = match kind {
            "dir" => m.is_some_and(|m| m.is_dir()),
            "file" => m.is_some_and(|m| m.is_file()),
            _ => true,
        };
        (!held).then(|| k.to_string())
    }).collect()
}

/// Puts the snapshot back (exact paths): each item that existed is cloned back; each that did not is removed; links
/// and special files are left alone. Nothing is removed unless the snapshot holds every item. Then `main` and the
/// tuple go back (the latter with a `rollback` record), and so do `lkg`, `audited`, `settled`, the twin and
/// `refs/heads/lkg` when a commit that stopped part way moved them.
fn undo(p: &Paths, j: &Value, git: &str) -> Result<(), String> {
    let snap = snap_dir(p, j).ok_or("the journal names no snapshot")?;
    let missing = snapshot_missing(p, j);
    if !missing.is_empty() {
        return Err(format!("the snapshot {} no longer holds {}; nothing was restored", snap.display(), missing.join(", ")));
    }
    let mut errs = Vec::new();
    for it in j["items"].as_array().into_iter().flatten() {
        let (Some(k), Some(live)) = (it["key"].as_str(), it["path"].as_str().map(PathBuf::from)) else { continue };
        let back = match it["kind"].as_str() {
            Some("dir" | "file") => crate::snapshot::remove(&live).and_then(|()| crate::sys::clonefile(&snap.join(k), &live)),
            Some("none") => crate::snapshot::remove(&live), // made by the upgrade (or by install.sh after it)
            _ => Ok(()),
        };
        if let Err(e) = back {
            errs.push(format!("{}: {e}", live.display()));
        }
    }
    let from = j["from"]["main"].as_str();
    if let Some(m) = from
        && crate::lineage::kgit::get(p, git, "refs/heads/main").as_deref() != Some(m)
        && let Err(e) = crate::lineage::kgit::rollback(p, git, m)
    {
        errs.push(format!("kernel.git main: {e}"));
    }
    let old = j["from"]["tuple"].as_str().unwrap_or("");
    let recs = crate::lineage::records(&p.rh).map_err(|e| e.to_string())?;
    let mut ptrs = tuple::Ptrs::default();
    recs.iter().for_each(|r| ptrs.apply(r));
    let mut root: Option<std::sync::Arc<Root>> = None;
    let boot = || -> Result<std::sync::Arc<Root>, String> { Root::boot(p.clone()).map_err(|e| e.to_string()) };
    if !old.is_empty() && ptrs.current.as_deref() != Some(old) {
        match tuple::load(&p.rh, old).filter(|t| tuple::verify(&p.rh, t)) {
            Some(t) => {
                let r = root.insert(boot()?);
                tuple::install_kernel(p, &t).map_err(|e| format!("install {old}: {e}"))?;
                r.append_by("root", "rollback", json!({"from": ptrs.current, "reason": "upgrade_failed", "target": "pre_upgrade", "to": old,
                    "upgrade": j["id"]})).map_err(|e| format!("lineage: {e}"))?;
            }
            None => errs.push(format!("the tuple from before ({old}) does not verify")),
        }
    }
    // a commit that stopped part way: what it moved goes back, so the old Root's demotion, panic and crash-loop paths
    // never install the release's kernel under it. Each record carries `upgrade` (for `lkg`, snapshot::settled_of's
    // boundary: no tuple of the upgrade settles later) and `rolled_back`. A pre-v0.5.1 journal records `lkg` only.
    let f = &j["from"];
    for (name, have) in [("lkg", ptrs.lkg), ("audited", ptrs.audited), ("settled", ptrs.settled), ("twin", ptrs.twin)] {
        let Some(want) = f.get(name).filter(|w| **w != json!(have)) else { continue };
        let mut d = json!({"from": have, "rolled_back": true, "tuple": want, "upgrade": j["id"]});
        if name == "twin" && let Some(r) = recs.iter().rev().find(|r| r["kind"] == "twin" && r["data"]["tuple"] == *want && r["data"]["rolled_back"] != true) {
            // the twin as it was frozen (twin.rs `pin_of`)
            (d["snapshot"], d["reason"]) = (r["data"]["snapshot"].clone(), r["data"]["reason"].clone());
            d["frozen_at"] = if r["data"]["frozen_at"].is_string() { r["data"]["frozen_at"].clone() } else { r["at"].clone() };
        }
        let r = match root.take() {
            Some(r) => r,
            None => boot()?,
        };
        if let Err(e) = r.append_by("root", name, d) {
            errs.push(format!("{name}: lineage: {e}"));
        }
        root = Some(r);
    }
    let have = crate::lineage::kgit::get(p, git, "refs/heads/lkg");
    if let Some(want) = f["kernel_lkg"].as_str()
        && have.as_deref() != Some(want)
        && let Err(e) = crate::lineage::kgit::update(p, git, "refs/heads/lkg", want, have.as_deref())
    {
        errs.push(format!("kernel.git lkg: {e}"));
    }
    drop(root);
    let want = j["from"]["root_sha256"].as_str().unwrap_or("");
    if !want.is_empty() && tuple::sha_of(&p.r("bin/init-root")).as_deref() != Some(want) {
        errs.push("ROOT_HOME/bin/init-root is not the old Root binary".into());
    }
    if errs.is_empty() { Ok(()) } else { Err(errs.join("; ")) }
}

fn locked_pending(p: &Paths) -> Res<(fs::File, Value, String)> {
    let lock = crate::cli::take_lock(p).map_err(|e| (1, format!("run/root.lock: {e}")))?.ok_or((1u8, "the Root is running: stop it first".to_string()))?;
    let j = journal(p).filter(pending).ok_or((1u8, "no upgrade is pending".to_string()))?;
    let info: Value = fs::read_to_string(p.r("install.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    Ok((lock, j, info["git"].as_str().unwrap_or("/usr/bin/git").to_string()))
}

fn rollback(p: &Paths) -> Res<Value> {
    let (_lock, mut j, git) = locked_pending(p)?;
    let missing = snapshot_missing(p, &j);
    if j["phase"] == "committing" || !missing.is_empty() {
        let why = if missing.is_empty() { "its commit has begun removing the snapshot".to_string() }
            else { format!("its snapshot no longer holds {}", missing.join(", ")) };
        return Err((1, format!("upgrade {} cannot be rolled back: {why}, so the install from before cannot be put back. Nothing was changed. \
            Finish the upgrade instead: {}", j["id"].as_str().unwrap_or("?"), p.cmd("upgrade --commit"))));
    }
    undo(p, &j, &git).map_err(|m| (1, format!("rollback incomplete: {m}; the snapshot is kept in {}", snap_dir(p, &j).unwrap_or_default().display())))?;
    if let Some(d) = snap_dir(p, &j) {
        crate::snapshot::remove(&d).map_err(err)?; // exact path, made by this upgrade
    }
    j["phase"] = json!("rolled_back");
    j["ended"] = json!(rfc3339(now_ms()));
    save(p, &j)?;
    Ok(json!({"upgrade": j["id"], "rolled_back": true, "version": j["from"]["version"], "tuple": j["from"]["tuple"]}))
}

fn commit(p: &Paths) -> Res<Value> {
    let (_lock, mut j, git) = locked_pending(p)?;
    if !matches!(j["phase"].as_str(), Some("replaced" | "committing")) {
        return Err((1, format!("the replace did not finish: {}", p.cmd("upgrade --rollback"))));
    }
    let (to, from) = (j["to"]["tuple"].as_str().unwrap_or("").to_string(), j["from"]["tuple"].as_str().unwrap_or("").to_string());
    let root = Root::boot(p.clone()).map_err(err)?;
    let ptrs = lock(&root.sup.ptrs).clone();
    if ptrs.current.as_deref() != Some(to.as_str()) {
        return Err((1, format!("the current tuple is {:?}, not the upgrade's {to}: roll back instead", ptrs.current)));
    }
    // 1. the lineage. Each step is idempotent, so a commit re-run after a crash finishes it; until `committing` below,
    //    `--rollback` stays open and `undo` puts these moves back too.
    if to != from && ptrs.lkg.as_deref() != Some(to.as_str()) {
        root.append_by("root", "lkg", json!({"tuple": to, "upgrade": j["id"]})).map_err(err)?; // `upgrade`: snapshot::settled_of
    }
    // the rollback anchors and the twin take the release's kernel, so no demotion, panic resume or /rollback twin ever
    // reinstalls the old one; each is compared with the release's kernel, also when this upgrade kept the kernel
    let anchors = tuple::reanchor(&root, &to, &j["id"]).map_err(err)?;
    // goal readings on changes from before end (root.md §3.7.2): they would count the old kernel's turns, or roll back to it
    let cancelled = crate::goal::upgrade_committed(&root, &j["id"]).map_err(err)?;
    if let (Some(m), Some(old)) = (j["to"]["main"].as_str(), crate::lineage::kgit::get(p, &git, "refs/heads/lkg"))
        && old != m
    {
        crate::lineage::kgit::update(p, &git, "refs/heads/lkg", m, Some(old.as_str())).map_err(err)?;
    }
    crate::lineage::env_fault("root.upgrade.after_anchors").map_err(err)?;
    // 2. the report, from the records: a re-run reports what an earlier run recorded
    for a in anchors.iter().filter(|a| a["skipped"].is_string()) { // cleared, never skipped silently (root.md §3.7.2)
        let (name, from) = (a["name"].as_str().unwrap_or(""), a["from"].as_str().unwrap_or("?"));
        let (what, then) = match name {
            "audited" => ("audited-good", "A panic resume or a demotion now rolls back to the last-known-good tuple, on the new kernel; a sealed audit sets a new audited-good tuple."),
            "settled" => ("settled", "A panic resume or a demotion now rolls back to the last-known-good tuple, on the new kernel; a new settled tuple forms 7 days after this upgrade."),
            _ => ("twin", "/rollback twin has no target now; on the free path a new twin is frozen from the last-known-good tuple when the Root next runs."),
        };
        let msg = format!("the {what} tuple {from} could not be read ({}), so the upgrade cleared it instead of moving it to the new kernel. {then}",
            a["skipped"].as_str().unwrap_or(""));
        root.alert("kernel_upgrade", &format!("[root] {msg}"));
        let run = if name == "audited" { format!(" Run: INIT_ROOT_HOME=\"{}\" \"{}\" audit run", p.rh.display(), p.r("bin/init-root").display()) } else { String::new() };
        eprintln!("init-root upgrade: {msg}{run}");
    }
    if !cancelled.is_empty() {
        let list: Vec<String> = cancelled.iter().map(|c| format!("{} (card {} on change {})", c["measure"].as_str().unwrap_or("?"), c["card"].as_str().unwrap_or("?"),
            c["sha"].as_str().map_or("?", |s| &s[..s.len().min(12)]))).collect();
        let msg = format!("{} goal reading(s) in flight on changes from before the upgrade were cancelled: {}. Their turns ran on the old \
            kernel; readings of changes made from now on are unaffected.", cancelled.len(), list.join(", "));
        root.alert("kernel_upgrade", &format!("[root] {msg}"));
        eprintln!("init-root upgrade: {msg}");
    }
    drop(root);
    // R-UNDO (v0.5.2): the release's get.sh, staged by the replace step, becomes what `init-agent upgrade` runs, also
    // when the owner commits by hand after install.sh's own commit failed (install.sh's keep_get runs only after it)
    if let Some(staged) = snap_dir(p, &j).map(|d| d.join(GET_SH)).filter(|f| f.is_file()) {
        install::make_dirs(&p.rh, &["cache"]).map_err(err)?;
        util::copy_atomic(&staged, &p.r("cache/get.sh"), 0o755).map_err(|e| (1, format!("cache/get.sh: {e}")))?;
    }
    // 3. `committing` before the snapshot goes: from here only `--commit` finishes the upgrade, because a partly
    //    removed snapshot can no longer put the old install back (`--rollback` refuses)
    j["to"]["anchors"] = json!(anchors);
    j["to"]["goal_readings_cancelled"] = json!(cancelled.iter().map(|c| c["measure"].clone()).collect::<Vec<_>>());
    j["phase"] = json!("committing");
    save(p, &j)?;
    if let Some(d) = snap_dir(p, &j) {
        crate::snapshot::remove(&d).map_err(err)?; // exact path, made by this upgrade
    }
    crate::lineage::env_fault("root.upgrade.after_snapshot_remove").map_err(err)?;
    j["phase"] = json!("committed");
    j["ended"] = json!(rfc3339(now_ms()));
    save(p, &j)?;
    Ok(json!({"upgrade": j["id"], "committed": true, "version": j["to"]["version"], "tuple": to, "anchors": anchors,
        "goal_readings_cancelled": j["to"]["goal_readings_cancelled"]}))
}

// ------------------------------------------------------------------------------------------------ a start mid-upgrade

/// R-UNDO (v0.5.2, the post-release review of v0.5.1): `start` while an upgrade is pending (root.md §4.3). install.sh
/// unloads the service but leaves its file, so an upgrade cut off after its replace step (a terminal closed during the
/// probe, a commit or rollback that failed) and then a login or reboot started the NEW Root on the new tuple while
/// `lkg`, the rollback anchors and the twin still named the old kernel; a crash loop, a demotion or a panic resume then
/// reinstalled the old kernel under the new Root. Decision: such a start never runs the zone on that mixed state. It
/// finishes the upgrade the one way that is sound: `committing` (the probe passed and the snapshot is going) is
/// committed, and the start goes on; `replacing` or `replaced` (no passing probe on record) is rolled back to the
/// install from before, and this process (the new binary) exits so the service starts the old one. If that step
/// fails, it says what to run: `start --foreground` (`service` false, the owner at a terminal) then exits 1. v0.5.3
/// (REL-052 §7.5): the service's `start` instead waits ([`wait_for_owner`]), because launchd's `KeepAlive` and
/// systemd's `Restart=always` start an exited Root again, and v0.5.2's exit 1 repeated the failed step, its alert and
/// its log line every 10 s for as long as the service was loaded. The probe's own Root (`PROBE_ENV` = the upgrade's
/// id) and a Root that already holds `run/root.lock` are left alone. Some(code): `start` exits with it.
pub fn at_start(p: &Paths, service: bool) -> Option<u8> {
    let j = journal(p).filter(pending)?;
    if std::env::var(PROBE_ENV).is_ok_and(|v| j["id"] == v.as_str()) {
        return None;
    }
    match crate::cli::take_lock(p) {
        Ok(Some(l)) => drop(l),
        _ => return None, // a Root or an upgrade step runs: `start`'s own path answers
    }
    let (what, finish) = (describe(&j), j["phase"] == "committing");
    let r = if finish { commit(p) } else { rollback(p) };
    let v = |s: &str| j[s]["version"].as_str().unwrap_or("?").to_string();
    let has_service = install::service_path(p).is_file();
    let msg = match &r {
        Ok(_) if finish => format!("{what}: its commit had begun, so this start finished it; INIT runs {} with its anchors on the new kernel.", v("to")),
        Ok(_) => format!("{what}: it was cut off after its replace step, before its health probe and commit, so this start rolled it back \
            and INIT is {} again, as before the upgrade. {} To upgrade, run the upgrade again: {}.", v("from"),
            if has_service { "Its service starts the Root again by itself.".to_string() } else { format!("Start the Root again: {}.", install::start_hint(p)) },
            p.agent_cmd("upgrade")),
        Err((_, m)) => {
            let step = if finish { "finish" } else { "roll back" };
            let run = format!("{} (or {} after a passing {})", p.cmd("upgrade --rollback"), p.cmd("upgrade --commit"), p.cmd("upgrade --probe"));
            if service {
                format!("{what} is not finished, and this start could not {step} it ({m}). The agent is not started on a half-upgraded install. \
                    A service starts an exited Root again, so this start waits instead of exiting: run {run} in a terminal, and it then exits \
                    by itself{}.", if has_service { " and its service starts the Root again".to_string() }
                    else { format!("; then start the Root again: {}", install::start_hint(p)) })
            } else {
                format!("{what} is not finished, and this start could not {step} it ({m}). The agent is not started on a half-upgraded \
                    install. With the Root stopped ({}), run {run}, then start the Root: {}.", install::stop_hint(p), install::start_hint(p))
            }
        }
    };
    util::log(&p.rh, if r.is_ok() { "warn" } else { "error" }, &msg);
    let _ = util::append_line(&p.r("alerts.jsonl"), &json!({"code": "kernel_upgrade", "message": msg, "source": "root", "ts": rfc3339(now_ms()), "v": 1}));
    eprintln!("init-root: {msg}");
    match r {
        Ok(_) if finish => None,
        Ok(_) => Some(75), // bin/init-root is the old Root again; this process is the new one
        Err(_) if service => Some(wait_for_owner(p)),
        Err(_) => Some(1),
    }
}

/// v0.5.3 (REL-052 §7.5): the service's `start` after it could not finish a pending upgrade ([`at_start`]). It holds no
/// lock (the owner's `upgrade --rollback`, `--probe` and `--commit` take `run/root.lock`) and runs no zone. It waits
/// until the journal shows the upgrade finished, then exits 75, so the service starts the Root again on what the
/// owner chose. A stop of the service (SIGTERM, whose default action ends it) ends it before that.
fn wait_for_owner(p: &Paths) -> u8 {
    while journal(p).filter(pending).is_some() {
        std::thread::sleep(Duration::from_secs(1));
    }
    util::log(&p.rh, "info", "the pending upgrade was finished by hand: this start exits, so its service starts the Root again");
    75
}

// ------------------------------------------------------------------------------------------------ the health probe

/// Signals and reaps the probe's Root (our own child, by its handle).
struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let pid = self.0.id() as i32;
        crate::sys::signal(pid, libc::SIGTERM, false);
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(20) {
            if matches!(self.0.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn probe(p: &Paths, wait: Duration) -> Res<Value> {
    drop(crate::cli::take_lock(p).map_err(|e| (1, format!("run/root.lock: {e}")))?.ok_or((1u8, "a Root is running: stop it first".to_string()))?);
    let j = journal(p).unwrap_or(Value::Null);
    let recs = crate::lineage::records(&p.rh).map_err(err)?;
    let mut ptrs = tuple::Ptrs::default();
    recs.iter().for_each(|r| ptrs.apply(r));
    let want = j["to"]["tuple"].as_str().filter(|_| pending(&j)).map(String::from).or(ptrs.current.clone()).unwrap_or_default();
    let has_body = tuple::load(&p.rh, &want).is_some_and(|t| t["body"]["binary"].as_str().is_some_and(|b| !b.is_empty()));
    let test = crate::charter::load(&p.r("state/charter.active.toml"), &p.ctx()).is_ok_and(|l| l.ch.root.test_mode);
    let log = fs::OpenOptions::new().create(true).append(true).open(p.r("logs/upgrade-probe.log")).map_err(err)?;
    let mut c = Command::new(p.r("bin/init-root"));
    c.arg("start").env_clear().env("HOME", &p.home).env("INIT_ROOT_HOME", &p.rh).env("PATH", "/usr/bin:/bin")
        .env(PROBE_ENV, j["id"].as_str().filter(|_| pending(&j)).unwrap_or("")).stdin(Stdio::null()).stdout(log.try_clone().map_err(err)?).stderr(log).process_group(0);
    if test { // test homes only: the harness's fake kernel and clock reach the Root as they do through `start`
        c.envs(std::env::vars().filter(|(k, _)| k.starts_with("INIT_TEST") || k.starts_with("INIT_ROOT_TEST") || k == "INIT_FAULT"));
    }
    let t0 = Instant::now();
    let since = std::time::SystemTime::now();
    let mut d = Daemon(c.spawn().map_err(|e| (1, format!("start the Root: {e}")))?);
    let mut why = String::from("timeout");
    let mut seen = Value::Null;
    let ready = |v: &Value| v["slot"] == "running" && v["kernel"]["ready"] == true && v["tuples"]["current"] == want.as_str()
        && (!has_body || fs::metadata(p.i("state/run/ready")).and_then(|m| m.modified()).is_ok_and(|m| m >= since));
    let mut ok = false;
    while t0.elapsed() < wait {
        std::thread::sleep(Duration::from_millis(250));
        if matches!(d.0.try_wait(), Ok(Some(_))) {
            why = "the Root exited".into();
            break;
        }
        let Ok(v) = link::request(p, "status", json!({}), Duration::from_secs(5)) else { continue };
        if v["tuples"]["current"].as_str().is_some_and(|c| c != want) {
            why = format!("the Root rolled back from {want} to {} (its crash-loop rule: the new kernel or body does not run)", v["tuples"]["current"]);
            seen = v;
            break;
        }
        if matches!(v["slot"].as_str(), Some("halted" | "panicked")) {
            why = format!("the Root is {} ({})", v["slot"].as_str().unwrap_or(""), v["halted"]);
            seen = v;
            break;
        }
        if ready(&v) {
            ok = true;
            seen = v;
            break;
        }
        seen = v;
    }
    let ready_ms = t0.elapsed().as_millis() as u64;
    if ok { // it stays up: the same kernel, still ready, 3 s later
        std::thread::sleep(Duration::from_secs(3));
        match link::request(p, "status", json!({}), Duration::from_secs(5)) {
            Ok(v) if ready(&v) && v["kernel"]["pid"] == seen["kernel"]["pid"] => {}
            Ok(v) => (ok, why, seen) = (false, "the zone did not stay up".into(), v),
            Err(_) => (ok, why) = (false, "the Root stopped answering".into()),
        }
    }
    let _ = link::request(p, "stop", json!({}), Duration::from_secs(10));
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(15) {
        let n = link::request(p, "status", json!({}), Duration::from_secs(2)).ok().map(|v| v["instances"].as_array().map_or(0, Vec::len));
        if n.is_none_or(|n| n == 0) {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    drop(d);
    let verify = crate::lineage::verify::audit_verify(p, true, false);
    let version = seen["root"]["version"].clone();
    if ok && verify != 0 {
        (ok, why) = (false, "the lineage does not verify".into());
    }
    let r = json!({"ok": ok, "why": if ok { Value::Null } else { json!(why) }, "ready_ms": ready_ms, "tuple": want, "version": version,
        "kernel_pid": seen["kernel"]["pid"], "slot": seen["slot"], "verify": verify, "log": p.r("logs/upgrade-probe.log")});
    if ok { Ok(r) } else { Err((1, format!("the health probe failed: {why}; {}", icj(&r).unwrap_or_default()))) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_semver_precedence() {
        use std::cmp::Ordering::{Equal, Greater, Less};
        // V4 (fails on c4797cc, which compared the core alone): the label counts
        for (a, b) in [("0.5.3-preview", "0.5.3"), ("0.5.3-alpha", "0.5.3-preview"), ("0.5.3-preview", "0.5.3-preview.1"),
            ("0.5.3-preview.2", "0.5.3-preview.10"), ("0.5.3-1", "0.5.3-preview"), ("0.5.2", "0.5.3-preview"), ("0.9.12", "0.10.0-a")]
        {
            assert_eq!((precedence(a, b), precedence(b, a)), (Some(Less), Some(Greater)), "{a} < {b}");
        }
        assert_eq!(precedence("0.5.3-preview+a", "0.5.3-preview+b"), Some(Equal), "build metadata is ignored");
        assert_eq!(precedence("0.5.3", "x.y"), None);
    }

    #[test]
    fn versions_compare_by_their_core() {
        assert_eq!(core("0.5.0-preview"), Some((0, 5, 0)));
        assert_eq!(core("0.4.1"), Some((0, 4, 1)));
        assert_eq!(core("0.10.0+b1"), Some((0, 10, 0)));
        assert!(core("0.10.0") > core("0.9.12"), "numeric, not lexical");
        for bad in ["0.5", "0.5.0.1", "v0.5.0", "x.y.z", ""] {
            assert_eq!(core(bad), None, "{bad}");
        }
        assert!(plain("0.5.0-preview") && plain("0.4.1+upg") && !plain("../x") && !plain("a b") && !plain(".x"));
    }

    #[test]
    fn a_body_is_compatible_only_on_the_same_channel_and_format() {
        let m = |ch: &str, f: u64, majors: &[u64]| json!({"control_channel": ch, "manifest": f, "ext_api_majors": majors});
        assert_eq!(compatible(&m("stdio-jsonl/1", 1, &[1]), &m("stdio-jsonl/1", 1, &[1])), Ok(()));
        assert_eq!(compatible(&m("stdio-jsonl/1", 1, &[1]), &m("stdio-jsonl/1", 1, &[1, 2])), Ok(()), "an older major the new host still takes");
        let e = compatible(&m("stdio-jsonl/1", 1, &[1]), &m("stdio-jsonl/2", 1, &[1])).unwrap_err();
        assert!(e.contains("speaks stdio-jsonl/1") && e.contains("kernel speaks stdio-jsonl/2"), "{e}");
        assert!(compatible(&m("stdio-jsonl/1", 1, &[1]), &m("stdio-jsonl/1", 2, &[1])).unwrap_err().contains("manifest format"));
        assert!(compatible(&m("stdio-jsonl/1", 1, &[1]), &m("stdio-jsonl/1", 1, &[2])).unwrap_err().contains("WIT majors"));
        assert!(compatible(&json!({}), &m("stdio-jsonl/1", 1, &[1])).is_err(), "no channel: never guessed");
    }

    #[test]
    fn the_snapshot_covers_the_root_the_kernel_and_the_launchers_only() {
        let p = Paths { rh: "/h/.init-root".into(), ih: "/h/.init".into(), home: "/h".into() };
        let keys: Vec<String> = items(&p, Path::new("/h/.local/bin")).into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["rh/bin/init-root", "rh/install.json", "ih/kernel/bin", "ih/kernel/docs", "ih/kernel/wit", "ih/kernel/src",
            "ih/kernel/kupgrade-smoke", "bin/init-agent", "bin/init-tui", "bin/init"]);
        for (_, path) in items(&p, Path::new("/h/.local/bin")) {
            for kept in ["/h/.init/body", "/h/.init/bin", "/h/.init/state", "/h/.init/vault", "/h/.init/lineage", "/h/.init-root/keys",
                "/h/.init-root/charter.toml", "/h/.init-root/lineage", "/h/.init-root/store", "/h/.init/kernel/config.toml", "/h/.init/kernel/seed"] {
                assert!(!path.starts_with(kept) && !Path::new(kept).starts_with(&path), "{} would touch {kept}", path.display());
            }
        }
        assert!(snap_dir(&p, &json!({"id": "u-20261027-1200-ab12cd"})).is_some());
        assert!(snap_dir(&p, &json!({"id": "../keys"})).is_none() && snap_dir(&p, &json!({"id": "u-a/b"})).is_none());
    }

    /// The second review of v0.5.1: `undo` removed each live item, then cloned it back from the snapshot; with the
    /// snapshot partly gone it removed live files it could not restore. Now it removes nothing then.
    #[test]
    fn undo_restores_nothing_from_an_incomplete_snapshot() {
        let top = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/test-homes/undo-snap-{}", std::process::id()));
        let _ = fs::remove_dir_all(&top); // exact path, this test's own
        let p = Paths { rh: top.join("rh"), ih: top.join("ih"), home: top.clone() };
        let (a, b) = (p.r("install.json"), p.i("kernel/bin"));
        fs::create_dir_all(&b).unwrap();
        fs::create_dir_all(&p.rh).unwrap();
        fs::write(&a, "new").unwrap();
        let snap = p.r(SNAPS).join("u-1");
        fs::create_dir_all(snap.join("ih/kernel/bin")).unwrap(); // `rh/install.json` is missing from it
        let j = json!({"id": "u-1", "from": {}, "items": [{"key": "ih/kernel/bin", "kind": "dir", "path": b}, {"key": "rh/install.json", "kind": "file", "path": a}]});
        assert_eq!(snapshot_missing(&p, &j), ["rh/install.json"]);
        assert!(undo(&p, &j, "/usr/bin/git").unwrap_err().contains("no longer holds rh/install.json"));
        assert_eq!((fs::read_to_string(&a).unwrap().as_str(), b.is_dir()), ("new", true), "nothing was removed");
        fs::remove_dir_all(&top).unwrap(); // exact path, this test's own
    }

    /// `src_head` (`git bundle list-heads`, no `--git-dir`) and every other Root git call: from `/`, never the caller's
    /// cwd, so no repository the caller stands in is discovered; no owner or system config, no hooks, no fsmonitor.
    #[test]
    fn root_git_runs_from_the_filesystem_root_with_no_outside_config() {
        let p = Paths { rh: "/h/.init-root".into(), ih: "/h/.init".into(), home: "/h".into() };
        let c = install::git(&p, "/usr/bin/git");
        assert_eq!(c.get_current_dir(), Some(Path::new("/")));
        let args: Vec<String> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        for want in ["core.hooksPath=/dev/null", "core.fsmonitor=false"] {
            assert!(args.windows(2).any(|w| w[0] == "-c" && w[1] == want), "{want}: {args:?}");
        }
        let env: Vec<(String, String)> = c.get_envs().filter_map(|(k, v)| Some((k.to_string_lossy().into_owned(), v?.to_string_lossy().into_owned()))).collect();
        for (k, v) in [("GIT_CONFIG_NOSYSTEM", "1"), ("GIT_CONFIG_GLOBAL", "/dev/null"), ("PATH", "/usr/bin:/bin")] {
            assert!(env.contains(&(k.into(), v.into())), "{k}={v}: {env:?}");
        }
    }
}
