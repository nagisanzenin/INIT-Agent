//! The stage drivers: fetch and checks (§3.6.1), export and build (§3.6.2), G0, G1 (selfmap in a
//! child, gate.md §3.4), G2 (two probes, gate.md §3.5) and G3 (the scripted self-edit, gate.md
//! §3.6). Each returns a [`StageOk`] or a [`StageFail`] with a `gate.<stage>.<reason>` code; the
//! rules themselves are kernel-gate's.

use super::proxy::{GateScope, LaneKind};
use super::Env;
use crate::build::{BuildSlot, DepsCache};
use crate::git::{CodedError, Git};
use crate::instance::{run_to_completion, SpawnRequest};
use crate::probe::{probe_scoped, Candidate, ProbeOpts, Purpose};
use crate::proberoot::SessionSrc;
use init_proto::files::{BootReason, InitMode};
use init_proto::initctl::Stage;
use init_proto::root_link::InstanceKind;
use kernel_gate::evo::{self, Validated};
use kernel_gate::g2::{ArmObs, Continuity, G2Input, G2Outcome, StartFail};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A failed stage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageFail {
    pub stage: Stage,
    /// `gate.<stage>.<reason>` (or an `E_*` code before the job).
    pub code: String,
    /// Agent-visible (≤ 4 KiB).
    pub detail: String,
    /// The stage's record object.
    pub record: Value,
}

impl StageFail {
    pub fn new(stage: Stage, code: impl Into<String>, detail: impl Into<String>) -> StageFail {
        StageFail { stage, code: code.into(), detail: kernel_gate::record::cap(&detail.into(), 4096), record: Value::Null }
    }
    pub fn reason(&self) -> &str {
        self.code.rsplit('.').next().unwrap_or(&self.code)
    }
}

/// `cache/gate/<job>/`.
pub fn job_cache(env: &Env, job: &str) -> PathBuf {
    env.home.join("cache/gate").join(job)
}

/// `vault/gate/jobs/<job>/`.
pub fn job_vault(env: &Env, job: &str) -> PathBuf {
    env.home.join("vault/gate/jobs").join(job)
}

// ---- fetch, kernel checks, validator (the synchronous part) ---------------------------------

/// What the synchronous part learned.
#[derive(Clone, Debug)]
pub struct Fetched {
    pub sha: String,
    pub running: String,
    pub branch: String,
    pub validated: Validated,
    pub scope: kernel_gate::scope::ScopeDecision,
    pub dep_change: bool,
}

/// `$G fetch` of `refs/heads/<branch>` from `repo` into `dst_ref` with the sandboxed upload-pack.
pub fn fetch(env: &Env, repo: &Path, branch: &str, dst_ref: &str) -> Result<String, CodedError> {
    let git = env.git();
    git.fetch(repo, &format!("refs/heads/{branch}"), dst_ref, &env.upload_pack).map_err(|e| CodedError::new("E_FETCH", format!("fetch {branch}: {e}")))
}

/// `gate --quick`'s fetch: `HEAD`, a commit id, a ref or a branch ([`crate::git::Git::fetch_rev`]).
pub fn fetch_rev(env: &Env, repo: &Path, rev: &str, dst_ref: &str) -> Result<String, CodedError> {
    env.git().fetch_rev(repo, rev, dst_ref, &env.upload_pack).map_err(|e| CodedError::new("E_FETCH", format!("fetch {rev}: {e}")))
}

/// The trailer parser in a kernel-owned empty directory.
pub fn trailer_parser(env: &Env) -> evo::trailers::GitTrailerParser {
    let empty = env.home.join("cache/evo-empty");
    let _ = std::fs::create_dir_all(&empty);
    evo::trailers::GitTrailerParser::new(&env.cfg.supervise.git, empty)
}

/// `evo::validate(parent, candidate)` on lineage.
pub fn validate(env: &Env, parent: &str, candidate: &str) -> Result<Validated, Vec<evo::ValidationError>> {
    // EV-TRACE: `trace:` refs resolve against the proxy's store (agent-visible requests only).
    let repo = evo::repo::GitCliRepo::new(&env.cfg.supervise.git, &env.home)
        .with_trace_lookup(Box::new(kernel_evo::trace::lookup(env.home.join("vault/traces/v1"))));
    evo::validate::validate(&repo, &trailer_parser(env), parent, candidate)
}

/// The scope decision (gate.md §3.1) from `$G diff --numstat` and both lockfiles.
pub fn scope(env: &Env, running: &str, sha: &str) -> Result<(kernel_gate::scope::ScopeDecision, bool), String> {
    let git = env.git();
    let out = git.run(&["diff", "--numstat", "--no-renames", running, sha], None, Duration::from_secs(60)).map_err(|e| e.to_string())?;
    let mut changes = Vec::new();
    for l in out.text().lines() {
        let mut it = l.splitn(3, '\t');
        let (a, d, p) = (it.next().unwrap_or("-"), it.next().unwrap_or("-"), it.next().unwrap_or(""));
        let old_lines = match git.path_id(running, p) {
            Ok(Some(oid)) => git.read_blob(&oid).map(|b| b.iter().filter(|c| **c == b'\n').count() as u64).unwrap_or(0),
            _ => 0,
        };
        changes.push(kernel_gate::scope::FileChange { path: p.to_string(), added: a.parse().ok(), deleted: d.parse().ok(), old_lines });
    }
    let blob = |sha: &str, p: &str| -> Option<String> { git.path_id(sha, p).ok().flatten().and_then(|o| git.read_blob(&o).ok()).map(|b| String::from_utf8_lossy(&b).into_owned()) };
    let (bp, bc, ep, ec) = (blob(running, "Cargo.lock"), blob(sha, "Cargo.lock"), blob(running, "ext/Cargo.lock"), blob(sha, "ext/Cargo.lock"));
    let locks = kernel_gate::scope::Lockfiles { body_parent: bp.as_deref(), body_candidate: bc.as_deref(), ext_parent: ep.as_deref(), ext_candidate: ec.as_deref() };
    kernel_gate::scope::classify(&changes, &locks).map_err(|e| e.0)
}

// ---- build + G0 -----------------------------------------------------------------------------

/// Outputs of build + G0: the candidate's binary and components under `cache/gate/<job>/`.
#[derive(Clone, Debug)]
pub struct Built {
    /// The directory that holds `bin/` and `ext/` (`cache/gate/<job>/`, or `cache/gate/<job>/g3/`
    /// for G3's inner gate). G1's `OUT` (kernel/sandbox.md §3.4).
    pub dir: PathBuf,
    pub binary: PathBuf,
    pub components: PathBuf,
    pub record: Value,
}

fn rustc_vv(env: &Env) -> (String, String) {
    let rustc = env.cfg.sandbox.toolchain.join("bin/rustc");
    let out = std::process::Command::new(&rustc).arg("-vV").env_clear().env("PATH", "/usr/bin:/bin").output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let host = out.lines().find_map(|l| l.strip_prefix("host: ")).unwrap_or("unknown").to_string();
    (out, host)
}

/// How the slot's target dir was prepared (supervise.md §3.6.2 "Dependency cache").
#[derive(Clone, Debug)]
pub struct DepsPrep {
    pub key: Option<String>,
    /// `hit` (cloned from `cache/deps/<key>`), `stub` (compiled from the stub workspace in this
    /// slot, staged as `<key>.tmp`), or `cold` (no usable key, or the stub build failed).
    pub mode: &'static str,
    /// Mode `stub`: the staged entry's digest, published once the candidate's G0 passed.
    pub staged: Option<String>,
    pub ms: u64,
    pub note: String,
}

impl DepsPrep {
    fn json(&self, build_ms: u64, published: Option<&Result<bool, String>>) -> Value {
        let mut j = json!({"mode": self.mode, "key": self.key, "ms": self.ms, "build_ms": build_ms});
        if let Some(p) = published {
            j["published"] = match p {
                Ok(b) => json!(b),
                Err(e) => json!(kernel_gate::record::cap(e, 512)),
            };
        }
        if !self.note.is_empty() {
            j["note"] = json!(kernel_gate::record::cap(&self.note, 1024));
        }
        j
    }
}

/// G0's environment for this slot (gate.md §3.3).
fn g0_env_spec(env: &Env, slot: &BuildSlot, epoch: i64) -> kernel_gate::g0::EnvSpec {
    let out = slot.out();
    kernel_gate::g0::EnvSpec {
        toolchain_bin: env.cfg.sandbox.toolchain.join("bin"),
        home: out.join("home"),
        tmpdir: out.join("tmp"),
        cargo_home: out.join("cargo-home"),
        target_dir: slot.target(),
        source_date_epoch: epoch.max(0) as u64,
        init_home: env.home.clone(),
        sdkroot: env.cfg.sandbox.sdkroot.clone(),
        developer_dir: (!env.cfg.sandbox.devdir.as_os_str().is_empty() && env.cfg.sandbox.devdir.is_dir()).then(|| env.cfg.sandbox.devdir.clone()),
    }
}

/// The compiling commands of G0 (gate.md §3.3), for the stub: `check`, `clippy`, the `ext` step and
/// `test` with `--no-run` (the same units; the stub has no tests to run). `true` marks the
/// extension workspace's commands.
pub fn stub_g0_commands(has_ext: bool) -> Vec<(Vec<String>, bool)> {
    let mut v = Vec::new();
    for step in kernel_gate::g0::steps(has_ext) {
        if let kernel_gate::g0::Action::Commands { cmds } = step.action {
            for c in cmds {
                if c.argv.get(1).is_some_and(|s| s == "metadata") {
                    continue;
                }
                let ext = c.argv.iter().any(|a| a == "--manifest-path");
                let mut argv = c.argv;
                if argv.get(1).is_some_and(|s| s == "test") {
                    argv.push("--no-run".into());
                }
                v.push((argv, ext));
            }
        }
    }
    v
}

/// One G0-style command through G0's spawner, output appended to `log`, killed (its own child
/// handle) at `deadline`.
fn run_g0_command(sp: &dyn kernel_gate::g0::Spawner, tool: &Path, argv: &[String], envv: &[(String, std::ffi::OsString)], cwd: &Path, log: &Path, deadline: Instant) -> Result<(), String> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let _ = writeln!(f, "$ {}", argv.join(" "));
    let (o, e) = (f.try_clone().map_err(|e| e.to_string())?, f.try_clone().map_err(|e| e.to_string())?);
    let req = kernel_gate::g0::SpawnRequest { program: tool.join(&argv[0]), args: argv[1..].to_vec(), env: envv.to_vec(), cwd: cwd.to_path_buf(), stdout: o.into(), stderr: e.into() };
    let mut child = sp.spawn(req).map_err(|e| format!("spawn cargo {}: {e}", argv[1]))?;
    loop {
        match child.try_wait() {
            Ok(Some(st)) if st.success() => return Ok(()),
            Ok(Some(st)) => return Err(format!("cargo {} exited {st}", argv[1])),
            Ok(None) => {}
            Err(e) => return Err(format!("cargo {}: {e}", argv[1])),
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("cargo {}: no result before the cold build bound", argv[1]));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The stub workspace's dependency build, in this slot (supervise.md §3.6.2 "Miss"): the export
/// moves aside to `slot/cand` (outside the build profile's `SRC` and `OUT`), the stub is written to
/// `slot/src`, built with the build command (build profile, cold bound) and G0's compiling commands
/// ([`stub_g0_commands`], G0's spawner and environment), and its first-party artifacts are pruned
/// from `slot/out/target`. Then the export moves back and `out/{tmp,home,cargo-home}` start empty.
/// `Err` only when the export could not be restored; the inner result says whether the stub built
/// (its `Ok` carries a note, e.g. a failed extension part, which leaves only the body warm).
fn stub_build(env: &Env, slot: &BuildSlot, job: &str, epoch: i64) -> Result<Result<String, String>, String> {
    let (src, cand) = (slot.src(), slot.dir().join("cand"));
    crate::build::remove_exact(&cand).map_err(|e| format!("slot/cand: {e}"))?;
    std::fs::rename(&src, &cand).map_err(|e| format!("move the export aside: {e}"))?;
    let res = (|| -> Result<String, String> {
        let names = crate::build::make_stub_workspace(&cand, &src).map_err(|e| format!("stub workspace: {e}"))?;
        let _ = crate::build::pin_vendor_mtimes(&src);
        let deadline = Instant::now() + slot.timeouts.cold;
        let log = env.home.join("state/kernel/gate").join(job).join("deps-stub.log");
        let b = slot.run_build(env.spawner.as_ref(), &slot.build_request(epoch, &env.cfg.sandbox), false, &log);
        if !b.ok {
            return Err(format!("{}: {}", b.reason.unwrap_or("build_failed"), b.message));
        }
        let envv = kernel_gate::g0::env(&g0_env_spec(env, slot, epoch));
        let tool = env.cfg.sandbox.toolchain.join("bin");
        let mut note = String::new();
        for (argv, ext) in stub_g0_commands(src.join("ext/Cargo.toml").is_file()) {
            match run_g0_command(env.g0.as_ref(), &tool, &argv, &envv, &src, &log, deadline) {
                Ok(()) => {}
                Err(e) if ext => note.push_str(&format!("ext stub: {e}; ")),
                Err(e) => return Err(e),
            }
        }
        crate::build::prune_first_party(&slot.target(), &names).map_err(|e| format!("prune: {e}"))?;
        Ok(note)
    })();
    let restore = crate::build::remove_exact(&src).and_then(|()| std::fs::rename(&cand, &src));
    for d in ["tmp", "home", "cargo-home"] {
        let p = slot.out().join(d);
        let _ = crate::build::remove_exact(&p);
        let _ = std::fs::create_dir_all(&p);
    }
    restore.map_err(|e| format!("restore the export: {e}"))?;
    Ok(res)
}

/// Prepare `slot/out/target` for the candidate (supervise.md §3.6.2 "Dependency cache"): pin the
/// export's `vendor/` mtimes, then on a verified hit clone `cache/deps/<key>/target`; on a miss
/// (or a corrupt entry, which is deleted) build the stub in the slot and stage its target as
/// `<key>.tmp`. Anything that goes wrong leaves an empty target: a cold build. `Err` only when the
/// export could not be restored.
/// v0.5.4 (lane K): the gate's step within `build` and `G0`, for the job's progress (`stage`, `pct`, `msg`), so the job
/// record says what runs: the dependency build, the body build, G0's tests. A no-op where nothing tracks it.
pub type Steps<'a> = &'a dyn Fn(Stage, u8, &str);

/// What the job's progress says while the dependencies build (a cache miss): the TUI's `deps+G0`.
pub const STEP_DEPS: &str = "deps+G0: building the dependencies (not cached yet: 15–21 min on a busy M2, once)";
/// The body build, after the dependencies (built now or from the cache).
pub const STEP_BODY: &str = "deps+G0: building the body";
/// G0's own checks.
pub const STEP_G0: &str = "G0: the body's tests and clippy";

pub fn prepare_deps(env: &Env, slot: &BuildSlot, git: &Git, sha: &str, job: &str, epoch: i64, step: Steps<'_>) -> Result<DepsPrep, String> {
    let t0 = Instant::now();
    let cache = DepsCache::new(&env.home);
    let mut note = String::new();
    if let Err(e) = crate::build::pin_vendor_mtimes(&slot.src()) {
        note.push_str(&format!("vendor mtimes: {e}; "));
    }
    let (vv, host) = rustc_vv(env);
    let key = match crate::build::deps_key(git, sha, &vv, &host) {
        Ok(k) => k,
        Err(e) => return Ok(DepsPrep { key: None, mode: "cold", staged: None, ms: 0, note: format!("{note}no deps key: {e}") }),
    };
    let empty_target = || crate::build::remove_exact(&slot.target()).and_then(|()| std::fs::create_dir_all(slot.target()));
    let prep = |mode, staged, note: String| DepsPrep { key: Some(key.clone()), mode, staged, ms: t0.elapsed().as_millis() as u64, note };
    if cache.lookup(&key).is_some() {
        if cache.verify(&key) {
            match cache.clone_into(&key, &slot.target()) {
                Ok(()) => return Ok(prep("hit", None, note)),
                Err(e) => note.push_str(&format!("clone deps {key}: {e}; ")),
            }
        } else {
            note.push_str(&format!("deps {key} failed verification and was deleted; "));
            let _ = cache.remove(&key);
        }
        if let Err(e) = empty_target() {
            note.push_str(&format!("empty target: {e}"));
            return Ok(prep("cold", None, note));
        }
    }
    // v0.5.2 (R-TUI): the owner's gate track shows the dependency build as its own step, with an
    // estimate, while it runs (a cold one took 15-21 min on a busy M2, docs/guide.md "Time"; it read
    // as G0 "waited" before)
    env.sup.owner_event("gate_deps", json!({"gate_id": job, "sha": sha}));
    step(Stage::Build, 10, STEP_DEPS);
    match stub_build(env, slot, job, epoch)? {
        Ok(n) => note.push_str(&n),
        Err(e) => {
            note.push_str(&format!("stub build: {e}"));
            let _ = empty_target();
            return Ok(prep("cold", None, note));
        }
    }
    match cache.stage(&key, &slot.target()) {
        Ok(d) => Ok(prep("stub", d, note)),
        Err(e) => {
            note.push_str(&format!("stage deps {key}: {e}"));
            Ok(prep("stub", None, note))
        }
    }
}

/// Export `sha` into the slot, warm it from the deps cache ([`prepare_deps`]), build `init-body`
/// (§3.6.2), then run G0 in the same slot (gate.md §3.3), then copy the artifacts into `out_dir`
/// (`bin/`, `ext/`). A staged deps entry is published only when G0 passed, and discarded otherwise.
/// `advisory`: ADR-011 A1 on the free path, clippy and the body's tests do not fail G0; their
/// failures are in the record's `advisory` list and steps (A3 reads the `test` step).
pub fn build_and_g0(env: &Env, job: &str, sha: &str, parent: &str, out_dir: &Path, advisory: bool) -> Result<Built, StageFail> {
    build_and_g0_until(env, job, sha, parent, out_dir, advisory, &AtomicBool::new(false), &|_, _, _| {})
}

/// [`build_and_g0`] that reports its steps ([`Steps`]): the promote pipeline's job.
pub fn build_and_g0_steps(env: &Env, job: &str, sha: &str, parent: &str, out_dir: &Path, advisory: bool, step: Steps<'_>) -> Result<Built, StageFail> {
    build_and_g0_until(env, job, sha, parent, out_dir, advisory, &AtomicBool::new(false), step)
}

/// [`build_and_g0`] that stops when `stop` is set: the body build is killed through the Root
/// ([`StopSpawner`]) and G0 runs no further command ([`StopG0`]). A3 uses it (F7).
#[allow(clippy::too_many_arguments)]
fn build_and_g0_until(env: &Env, job: &str, sha: &str, parent: &str, out_dir: &Path, advisory: bool, stop: &AtomicBool, step: Steps<'_>) -> Result<Built, StageFail> {
    let git = env.git();
    let slot = BuildSlot::configured(&env.home, &env.cfg.supervise);
    let t0 = Instant::now();
    let fail = |code: &str, d: String| StageFail::new(Stage::Build, code, d);
    slot.reset().map_err(|e| fail("gate.build.failed", format!("slot reset: {e}")))?;
    slot.export(&git, sha).map_err(|e| fail("gate.build.failed", format!("export: {e}")))?;
    // A fixed SOURCE_DATE_EPOCH, not the commit's time (build::SLOT_EPOCH_S): cc's build scripts
    // track it, so a per-commit value would rebuild wasmtime on every promote.
    let epoch = crate::build::SLOT_EPOCH_S;
    let deps = prepare_deps(env, &slot, &git, sha, job, epoch, step).map_err(|e| fail("gate.build.failed", e))?;
    step(Stage::Build, 20, STEP_BODY);
    let warm = deps.mode != "cold";
    let cache = DepsCache::new(&env.home);
    let discard = || {
        if let (Some(k), Some(_)) = (&deps.key, &deps.staged) {
            let _ = cache.discard(k);
        }
    };
    let req = slot.build_request(epoch, &env.cfg.sandbox);
    let log = env.home.join("state/kernel/gate").join(job).join("build.log");
    let b = slot.run_build(&StopSpawner { inner: env.spawner.as_ref(), stop }, &req, warm, &log);
    if !b.ok {
        discard();
        let code = if b.timed_out { "gate.build.timeout" } else { "gate.build.failed" };
        let tail = std::fs::read_to_string(&log).map(|t| kernel_gate::record::cap(&t[t.len().saturating_sub(4096)..], 4096)).unwrap_or_default();
        let mut f = fail(code, format!("{}\n{tail}", b.message));
        f.record = json!({"pass": false, "warm": warm, "wall_ms": b.wall_ms, "reason": f.reason().to_string(), "deps": deps.json(b.wall_ms, None)});
        return Err(f);
    }
    let build_ms = t0.elapsed().as_millis() as u64;
    // G0.
    step(Stage::G0, 30, STEP_G0);
    let spec_env = g0_env_spec(env, &slot, epoch);
    let mut spec = kernel_gate::g0::G0Spec::new(slot.src(), spec_env, job_vault(env, job).join("g0"));
    spec.total_timeout = Duration::from_secs(env.gate.g0_timeout_s);
    spec.advisory = advisory;
    spec.parent_cargo_toml = git.path_id(parent, "Cargo.toml").ok().flatten().and_then(|o| git.read_blob(&o).ok()).map(|b| String::from_utf8_lossy(&b).into_owned());
    let rep = kernel_gate::g0::run(&spec, &StopG0 { inner: env.g0.as_ref(), stop });
    let mut record = json!({
        "pass": rep.pass, "wall_ms": rep.wall_ms, "build_ms": build_ms, "cold_target": deps.mode != "hit",
        "steps": rep.steps.iter().map(|s| {
            let mut j = json!({"name": s.name, "exit": s.exit, "wall_ms": s.wall_ms});
            if s.advisory {
                j["pass"] = json!(false);
                j["advisory"] = json!(true);
                j["code"] = json!(s.code);
                j["timed_out"] = json!(s.timed_out);
            }
            j
        }).collect::<Vec<_>>(),
        "build_config_present": rep.build_config_present, "lints_changed": rep.lints_changed, "warnings": [],
    });
    if advisory {
        record["advisory"] = json!(rep.advisory.iter().map(|a| json!({"step": a.step, "code": a.code, "exit": a.exit, "timed_out": a.timed_out,
            "diagnostics": kernel_gate::record::cap(&a.diagnostics, 1024)})).collect::<Vec<_>>());
    }
    if !rep.pass {
        discard();
        record["deps"] = deps.json(b.wall_ms, None);
        let code = rep.code.unwrap_or("gate.g0.check");
        let mut f = StageFail::new(Stage::G0, code, format!("G0 step `{}` failed:\n{}", rep.step.unwrap_or("?"), rep.diagnostics));
        f.record = record;
        f.record["reason"] = json!(f.reason());
        return Err(f);
    }
    // G0 passed: publish the staged deps entry (taken before any candidate code ran) and keep the
    // `deps_cache_keys` most recently used keys.
    let published = match (&deps.key, &deps.staged) {
        (Some(k), Some(d)) => {
            let p = cache.publish(k, d).map_err(|e| e.to_string());
            if p.is_ok() {
                let _ = cache.evict_lru(env.cfg.supervise.deps_cache_keys.max(1) as usize);
            }
            Some(p)
        }
        _ => None,
    };
    record["deps"] = deps.json(b.wall_ms, published.as_ref());
    // Copy the artifacts (APFS clones) before the slot is reused.
    let bin = out_dir.join("bin/init-body");
    let ext = out_dir.join("ext");
    let _ = std::fs::create_dir_all(bin.parent().unwrap());
    let _ = std::fs::create_dir_all(&ext);
    let _ = std::fs::remove_file(&bin);
    crate::sys::clone_path(&slot.built_binary(), &bin).or_else(|_| std::fs::copy(slot.built_binary(), &bin).map(drop)).map_err(|e| StageFail::new(Stage::G0, "gate.build.failed", format!("copy init-body: {e}")))?;
    if slot.src().join("ext/Cargo.toml").is_file() {
        if let Ok(rd) = std::fs::read_dir(slot.src().join("ext")) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if e.path().join("Cargo.toml").is_file() {
                    let wasm = slot.target().join("wasm32-wasip2/debug").join(format!("{}.wasm", name.replace('-', "_")));
                    let _ = std::fs::copy(&wasm, ext.join(format!("{name}.wasm")));
                }
            }
        }
    }
    Ok(Built { dir: out_dir.to_path_buf(), binary: bin, components: ext, record })
}

// ---- G1 ---------------------------------------------------------------------------------------

/// G1's outputs for G2 and the accept rule.
#[derive(Clone, Debug, Default)]
pub struct G1Out {
    pub tools_sha256: Option<String>,
    pub tools: Vec<(String, String)>,
    pub body_tokens: u64,
    pub record: Value,
}

const G1_CODES: [&str; 11] = ["self_json_missing", "generator_changed", "self_json_drift", "tag_marker_mismatch", "wit_copy_drift", "manifest_tools_hash", "manifest_error", "ext_contract", "marker_grammar", "marker_conflict", "selfmap_error"];

/// G1's child: `init-kernel selfmap` in the `build` profile with `SRC` = the export and `OUT` =
/// [`Built::dir`], which holds the candidate binary and components (kernel/sandbox.md §3.4 and
/// §3.9, gate.md §3.4). Its `HOME`, `TMPDIR` and log are in `OUT/g1/`; the output is `OUT/self.json`.
pub struct G1Child {
    pub req: SpawnRequest,
    pub out_file: PathBuf,
    pub log: PathBuf,
}

pub fn g1_child(env: &Env, src: &Path, built: &Built) -> G1Child {
    let out = built.dir.clone();
    let scratch = out.join("g1");
    let _ = std::fs::create_dir_all(scratch.join("home"));
    let _ = std::fs::create_dir_all(scratch.join("tmp"));
    let out_file = out.join("self.json");
    let _ = std::fs::remove_file(&out_file);
    let s = |p: &Path| p.to_string_lossy().into_owned();
    let env_map = BTreeMap::from([
        ("PATH".to_string(), format!("{}:/usr/bin:/bin", env.cfg.sandbox.toolchain.join("bin").display())),
        ("HOME".to_string(), s(&scratch.join("home"))),
        ("TMPDIR".to_string(), format!("{}/", scratch.join("tmp").display())),
        ("LANG".to_string(), "C".to_string()),
    ]);
    let req = SpawnRequest {
        kind: InstanceKind::Build,
        profile: "build".into(),
        params: crate::spawnspec::build_params(&env.home, src, &out, &env.cfg.sandbox),
        program: env.home.join("kernel/bin/init-kernel"),
        args: vec!["selfmap".into(), "--root".into(), s(src), "--binary".into(), s(&built.binary), "--components".into(), s(&built.components), "--out".into(), s(&out_file)],
        env: env_map,
        cwd: scratch.clone(),
        lane: None,
        replaces: None,
    };
    G1Child { req, out_file, log: scratch.join("selfmap.log") }
}

/// gate.md §3.4: `init-kernel selfmap` in a child (build profile, [`g1_child`]), then the drift,
/// marker and WIT checks against the export in the slot.
pub fn g1(env: &Env, job: &str, src: &Path, built: &Built, v: &Validated, parent: &str) -> Result<G1Out, StageFail> {
    let t0 = Instant::now();
    let G1Child { req, out_file, log } = g1_child(env, src, built);
    let _ = std::fs::write(&log, b"");
    let fail = |code: &str, d: String| StageFail::new(Stage::G1, code, d);
    let (exit, timed_out) = run_to_completion(env.spawner.as_ref(), &req, Duration::from_secs(300), Some(&log), 1 << 20).map_err(|e| fail("gate.g1.selfmap_error", format!("selfmap: {e}")))?;
    let regenerated = std::fs::read(&out_file).ok();
    if timed_out || !exit.success() || regenerated.is_none() {
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        let code = text
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .find_map(|d| d.get("code").and_then(Value::as_str).map(str::to_string))
            .filter(|c| G1_CODES.contains(&c.as_str()))
            .unwrap_or_else(|| "selfmap_error".into());
        return Err(fail(&format!("gate.g1.{code}"), format!("selfmap exited {:?}:\n{}", exit.code, kernel_gate::record::cap(&text, 3800))));
    }
    let regenerated = regenerated.unwrap();
    let committed = std::fs::read(src.join("self.json")).ok();
    let warnings = kernel_gate::g1::compare_self_json(committed.as_deref(), &regenerated).map_err(|f| fail(f.code, f.detail))?;
    let r: Value = serde_json::from_slice(&regenerated).unwrap_or(Value::Null);
    let git = env.git();
    let p: Value = git.path_id(parent, "self.json").ok().flatten().and_then(|o| git.read_blob(&o).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(json!({}));
    kernel_gate::g1::check_tag_markers(v.change_type, &v.trailers.compensates_for.tag, &r, &p).map_err(|f| fail(f.code, f.detail))?;
    kernel_gate::g1::check_wit_copy(src, &env.home.join("kernel/wit")).map_err(|f| fail(f.code, f.detail))?;
    let vault = job_vault(env, job);
    let _ = std::fs::create_dir_all(&vault);
    let _ = std::fs::write(vault.join("self.json"), &regenerated);
    let tools = r["tools"]["list"].as_array().map(|a| a.iter().map(|t| (t["name"].as_str().unwrap_or("").to_string(), t["schema_sha256"].as_str().unwrap_or("").to_string())).collect()).unwrap_or_default();
    let body_tokens = r["budget"]["body_tokens"].as_u64().or_else(|| r["budget"]["tokens"].as_u64()).or_else(|| r["tokens"]["body"].as_u64()).unwrap_or(0);
    let tools_sha256 = r["tools"]["sha256"].as_str().map(str::to_string);
    let record = json!({"pass": true, "wall_ms": t0.elapsed().as_millis() as u64, "body_tokens": body_tokens,
        "self_json_sha256": crate::fsx::hex(&<sha2::Sha256 as sha2::Digest>::digest(&regenerated)), "tools_sha256": tools_sha256,
        "warnings": warnings.iter().map(|w| serde_json::to_value(w).unwrap_or(Value::Null)).collect::<Vec<_>>()});
    Ok(G1Out { tools_sha256, tools, body_tokens, record })
}

// ---- G2 ---------------------------------------------------------------------------------------

/// The live session to copy (`state/session`), if its log exists.
fn live_session(env: &Env) -> Option<(PathBuf, String)> {
    let sid = crate::layout::read_session(&env.home)?;
    let log = env.home.join("state/sessions").join(format!("{sid}.jsonl"));
    log.is_file().then_some((log, sid))
}

/// One G2 arm: probe, ready, window, stop.
fn g2_arm(env: &Env, job: &str, arm: &str, c: &Candidate, session: SessionSrc, gate_job: &str) -> ArmObs {
    let scope = GateScope::new(env.proxy.clone(), &format!("{job}/probe-{arm}"), LaneKind::Probe, gate_job, false, env.gate.probe_musd, &c.sha, "promote");
    let opts = ProbeOpts { session, mode: InitMode::Probe, boot_reason: BootReason::Promote, purpose: Purpose::Probe, job: gate_job.into(), extra_files: vec![], cwd: None, keep_on_stop: vec![] };
    let mut o = ArmObs { sha: c.sha.clone(), ..Default::default() };
    let h = match probe_scoped(env.spawner.clone(), &env.home, &env.cfg, c, opts, Some(scope.clone() as Arc<dyn crate::probe::ProbeScope>)) {
        Ok(h) => h,
        Err(e) => {
            o.start_fail = Some(StartFail::Infra(e.to_string()));
            return o;
        }
    };
    env.point("gate.g2.probe_running");
    match h.wait_ready(Duration::from_secs(env.gate.g2_ready_timeout_s)) {
        Ok(_) => {
            o.window_ok = h.window(Duration::from_secs(env.gate.g2_window_s));
            o.window_exit = !o.window_ok && h.exited(Duration::from_millis(10)).is_some();
        }
        Err(crate::probe::ProbeFail::Exit(e)) => o.start_fail = Some(StartFail::Exit { code: e.code, signal: e.signal }),
        Err(crate::probe::ProbeFail::Timeout) => o.start_fail = Some(StartFail::Timeout),
        Err(e) => o.start_fail = Some(StartFail::Infra(e.to_string())),
    }
    let out = h.stop();
    o.ready_ms = out.ready_at.map(|d| d.as_millis() as u64);
    o.stop_code = out.exit.as_ref().and_then(|e| e.code);
    o.scope = out.scope;
    o.last_boot_event = out.last_boot_event;
    o
}

/// The message lists of the candidate's requests for `sid` (first request, then the last one).
fn arm_requests(env: &Env, scope: &Value, sid: &str) -> Vec<Vec<Vec<u8>>> {
    let mut ids = Vec::new();
    if scope["first"]["session"].as_str() == Some(sid) {
        if let Some(id) = scope["first"]["req_id"].as_str() {
            ids.push(id.to_string());
        }
    }
    if let Some(id) = scope["last_req_by_session"][sid].as_str() {
        if !ids.iter().any(|x| x == id) {
            ids.push(id.to_string());
        }
    }
    ids.iter().filter_map(|id| env.proxy.request(id)).map(|r| r.elements).collect()
}

/// G2 (gate.md §3.5): candidate and parent probed in parallel on copies of the live log, then the
/// criteria. Arm p failing is infra: one retry after `g2_parent_retry_s`.
#[allow(clippy::too_many_arguments)]
pub fn g2(env: &Env, job: &str, gate_job: &str, cand: &Candidate, parent: &Candidate, g1: &G1Out, prefix_change: bool) -> G2Outcome {
    g2_inner(env, job, gate_job, cand, parent, g1, prefix_change, prefix_change)
}

/// [`g2`] with the continuity waiver separate from `prefix_change`. G3's inner gate needs both: its
/// own commit is `Prefix-change: no` (the inner prefix must equal the candidate's), but its
/// continuity reference is the live body, which the candidate replaces; when the candidate itself
/// declared `Prefix-change: yes` (waived in the outer G2), that reference differs by design (REL-G0:
/// every prefix-changing T2 candidate failed `gate.g3.inner_g2` continuity at element 0).
#[allow(clippy::too_many_arguments)]
fn g2_inner(env: &Env, job: &str, gate_job: &str, cand: &Candidate, parent: &Candidate, g1: &G1Out, prefix_change: bool, waive_continuity: bool) -> G2Outcome {
    let mut attempt = 0;
    loop {
        let live = live_session(env);
        let session = || match &live {
            Some((log, sid)) => SessionSrc::Copy { live_log: log.clone(), sid: sid.clone() },
            None => SessionSrc::Fresh,
        };
        let (c, p) = std::thread::scope(|s| {
            let hc = s.spawn(|| g2_arm(env, job, "c", cand, session(), gate_job));
            let hp = s.spawn(|| g2_arm(env, job, "p", parent, session(), gate_job));
            (hc.join().unwrap_or_default(), hp.join().unwrap_or_default())
        });
        let continuity = match &live {
            _ if waive_continuity => Some(Continuity::Waived),
            None => Some(Continuity::NotApplicable),
            Some((_, sid)) => {
                let rp = env.proxy.scope_summary("live")["last_req_by_session"][sid.as_str()].as_str().and_then(|id| env.proxy.request(id)).map(|r| r.elements);
                Some(kernel_gate::g2::continuity(rp.as_deref(), &arm_requests(env, &c.scope, sid)))
            }
        };
        let heads = match (c.scope["first"]["req_id"].as_str(), p.scope["first"]["req_id"].as_str()) {
            (Some(a), Some(b)) => match (env.proxy.request(a).and_then(|r| r.head), env.proxy.request(b).and_then(|r| r.head)) {
                (Some(x), Some(y)) => Some((x, y)),
                _ => None,
            },
            _ => None,
        };
        let input = G2Input { c, p, self_tools_sha256: g1.tools_sha256.clone(), self_tools: g1.tools.clone(), prefix_change, continuity, heads };
        let out = kernel_gate::g2::evaluate(&input);
        if out.parent_failed && attempt == 0 {
            attempt += 1;
            std::thread::sleep(Duration::from_secs(env.gate.g2_parent_retry_s));
            continue;
        }
        return out;
    }
}

// ---- G3 ---------------------------------------------------------------------------------------

/// G3's result: the commit, the inner gate's outcome, the record.
#[derive(Clone, Debug)]
pub struct G3Out {
    pub commit: String,
    pub record: Value,
}

/// The agent part's end rule (gate.md §3.6), on the caller's clock (`now` = time since any fixed
/// start: the real clock in the gate, a fake one in tests): done at a new `stop` finish with nothing
/// in flight for `quiet`; timed out at `limit` ([`super::config::G3Bounds::agent`], X0c).
#[derive(Clone, Debug)]
pub struct AgentWait {
    deadline: Duration,
    quiet: Duration,
    base_requests: u64,
    since: Option<Duration>,
}

/// One [`AgentWait::step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wait {
    Pending,
    Done,
    TimedOut,
}

impl AgentWait {
    pub fn new(now: Duration, limit: Duration, quiet: Duration, base_requests: u64) -> AgentWait {
        AgentWait { deadline: now + limit, quiet, base_requests, since: None }
    }

    /// Feed the probe lane's scope summary at `now`.
    pub fn step(&mut self, now: Duration, s: &Value) -> Wait {
        if now >= self.deadline {
            return Wait::TimedOut;
        }
        let req = s["requests"].as_u64().unwrap_or(0);
        let idle = s["inflight"].as_u64().unwrap_or(0) == 0 && s["last_finish"]["reason"].as_str() == Some("stop") && req > self.base_requests;
        match (idle, self.since) {
            (true, None) => self.since = Some(now),
            (true, Some(t)) if now.saturating_sub(t) >= self.quiet => return Wait::Done,
            (false, _) => self.since = None,
            _ => {}
        }
        Wait::Pending
    }
}

/// Wait for the agent part to end: a new `stop` finish with nothing in flight for `quiet`, or the
/// timeout, or the body exiting.
fn wait_agent(h: &crate::probe::ProbeHandle, base_requests: u64, quiet: Duration, timeout: Duration, stop: &AtomicBool) -> bool {
    let t0 = Instant::now();
    let mut w = AgentWait::new(Duration::ZERO, timeout, quiet, base_requests);
    loop {
        if h.exited(Duration::from_millis(200)).is_some() || stop.load(Ordering::SeqCst) {
            return false;
        }
        match w.step(t0.elapsed(), &h.scope_summary()) {
            Wait::Pending => {}
            Wait::Done => return true,
            Wait::TimedOut => return false,
        }
    }
}

/// G3 (gate.md §3.6): a third probe of the candidate gets the scripted instruction; the result is
/// fetched (sandboxed) into `refs/init/g3/<job>`, verified with `$G`, validated, and gated once
/// more (G0, G1, G2 with the candidate as parent; no recursion).
#[allow(clippy::too_many_arguments)]
/// `cand_prefix_change`: the candidate's own `Prefix-change: yes` (a promote's G3); false for A3,
/// where the candidate is already the live body.
pub fn g3(env: &Env, job: &str, cand: &Candidate, cand_built: &Built, cand_prefix_change: bool) -> Result<G3Out, StageFail> {
    g3_until(env, job, cand, cand_built, cand_prefix_change, &AtomicBool::new(false))
}

/// [`g3`] that stops when `stop` is set (A3 after its change was rolled back, F7): the probe and
/// its agent's own builds end within about 200 ms, nothing is fetched, the inner gate's build is
/// killed and no later inner stage starts. The result is then [`G3_STOPPED`].
fn g3_until(env: &Env, job: &str, cand: &Candidate, cand_built: &Built, cand_prefix_change: bool, stop: &AtomicBool) -> Result<G3Out, StageFail> {
    let t0 = Instant::now();
    let n = kernel_gate::g3::nonce(job);
    let fail = |reason: &str, d: String| StageFail::new(Stage::G3, format!("gate.g3.{reason}"), d);
    let scope = GateScope::new(env.proxy.clone(), &format!("{job}/g3"), LaneKind::G3, job, false, env.gate.probe_musd, &cand.sha, "cold");
    let opts = ProbeOpts { session: SessionSrc::Fresh, mode: InitMode::Probe, boot_reason: BootReason::Cold, purpose: Purpose::Probe, job: job.into(), extra_files: vec![], cwd: None, keep_on_stop: vec![] };
    let h = probe_scoped(env.spawner.clone(), &env.home, &env.cfg, cand, opts, Some(scope.clone() as Arc<dyn crate::probe::ProbeScope>)).map_err(|e| fail("timeout", format!("the G3 probe did not start: {e}")))?;
    if let Err(e) = h.wait_ready(Duration::from_secs(env.gate.g2_ready_timeout_s)) {
        let _ = h.stop();
        return Err(fail("timeout", format!("the G3 probe did not become ready: {e}")));
    }
    let root = h.root().to_path_buf();
    let base = h.scope_summary()["requests"].as_u64().unwrap_or(0);
    h.send_input(&kernel_gate::g3::instruction(&n, &root.to_string_lossy()));
    let bounds = env.gate.g3_bounds(&env.cfg.supervise);
    let finished = wait_agent(&h, base, Duration::from_millis(env.gate.quiet_ms), bounds.agent, stop);
    if stop.load(Ordering::SeqCst) {
        let out = h.stop();
        let mut f = fail("stopped", "the change was rolled back while its G3 ran: the probe was stopped".into());
        f.record = json!({"pass": false, "mode": env.gate.g3_mode, "wall_ms": t0.elapsed().as_millis() as u64,
                          "spent_musd": out.scope["spent_musd"].as_u64().unwrap_or(0), "reason": "stopped"});
        return Err(f);
    }
    // Fetch before stop(): stop deletes the root. The kernel never runs git inside the root.
    let dst = format!("refs/init/g3/{job}");
    let fetched = fetch(env, &root.join("body"), &kernel_gate::g3::branch(&n), &dst);
    let out = h.stop();
    let agent_ms = t0.elapsed().as_millis() as u64;
    let spent = out.scope["spent_musd"].as_u64().unwrap_or(0);
    let c3 = match fetched {
        Ok(s) => s,
        Err(e) => {
            let reason = if finished { "no_branch" } else { "timeout" };
            let mut f = fail(reason, format!("no branch evo/g3-{n} in the G3 probe ({e}); the agent part {}", if finished { "ended" } else { "timed out" }));
            f.record = json!({"pass": false, "mode": env.gate.g3_mode, "wall_ms": agent_ms, "spent_musd": spent, "reason": reason});
            return Err(f);
        }
    };
    let git = env.git();
    let res = (|| -> Result<(), StageFail> {
        let facts = g3_facts(&git, &cand.sha, &c3).map_err(|e| fail("shape", e))?;
        kernel_gate::g3::verify(&facts, &cand.sha, &n).map_err(|f| StageFail::new(Stage::G3, f.code, f.detail))?;
        let v = validate(env, &cand.sha, &c3).map_err(|errs| fail("trailers", errs.iter().map(|e| format!("{}: {}", e.code, e.detail)).collect::<Vec<_>>().join("\n")))?;
        // Inner gate, no recursion: export + build, G0, G1, G2 with the candidate as parent.
        let inner_job = format!("{job}-g3");
        let inner_out = job_cache(env, job).join("g3");
        let t_inner = Instant::now();
        let stopped = || if stop.load(Ordering::SeqCst) { Err(fail("stopped", "the change was rolled back while its G3 ran".into())) } else { Ok(()) };
        stopped()?;
        let built = build_and_g0_until(env, &inner_job, &c3, &cand.sha, &inner_out, false, stop, &|_, _, _| {}).map_err(|f| fail("inner_g0", format!("{}: {}", f.code, f.detail)))?;
        stopped()?;
        let build = Duration::from_millis(built.record["build_ms"].as_u64().unwrap_or(0));
        let g1o = g1(env, &inner_job, &BuildSlot::new(&env.home).src(), &built, &v, &cand.sha).map_err(|f| fail("inner_g1", format!("{}: {}", f.code, f.detail)))?;
        let inner_c = Candidate { sha: c3.clone(), binary: built.binary.clone(), ext_set_dir: None };
        let inner_p = Candidate { sha: cand.sha.clone(), binary: cand_built.binary.clone(), ext_set_dir: None };
        stopped()?;
        let o = g2_inner(env, &inner_job, job, &inner_c, &inner_p, &g1o, false, cand_prefix_change);
        if !o.pass {
            return Err(fail("inner_g2", format!("{}: {}", o.code().unwrap_or(""), o.detail())));
        }
        if !bounds.inner_in_time(t_inner.elapsed(), build) {
            return Err(fail("timeout", format!("the inner gate's G0-G2 exceeded {} s after its {} s build", bounds.inner_after_build.as_secs(), build.as_secs())));
        }
        Ok(())
    })();
    // A stop during an inner stage reads as that stage's failure; it is a stop.
    let res = match res {
        Err(_) if stop.load(Ordering::SeqCst) => Err(fail("stopped", "the change was rolled back while its G3 ran".into())),
        r => r,
    };
    let record = json!({"pass": res.is_ok(), "mode": env.gate.g3_mode, "wall_ms": t0.elapsed().as_millis() as u64, "spent_musd": spent, "commit": c3,
                        "reason": res.as_ref().err().map(|f| f.reason().to_string())});
    match res {
        Ok(()) => Ok(G3Out { commit: c3, record }),
        Err(mut f) => {
            f.record = record;
            Err(f)
        }
    }
}

fn g3_facts(git: &Git, cand: &str, c3: &str) -> Result<kernel_gate::g3::Facts, String> {
    let t = Duration::from_secs(60);
    let run = |a: &[&str]| git.run(a, None, t).map(|o| o.text()).map_err(|e| e.to_string());
    let parents: Vec<String> = run(&["rev-list", "--parents", "-n", "1", c3])?.split_whitespace().skip(1).map(str::to_string).collect();
    let who = run(&["log", "-1", "--format=%an%x00%ae%x00%cn%x00%ce", c3])?;
    let w: Vec<&str> = who.trim_end().split('\0').collect();
    let pair = |i: usize| (w.get(i).unwrap_or(&"").to_string(), w.get(i + 1).unwrap_or(&"").to_string());
    let changed = run(&["diff", "--name-only", "--no-renames", cand, c3])?.lines().map(str::to_string).collect();
    let diff = run(&["diff", "--no-renames", cand, c3, "--", "src/main.rs"])?;
    Ok(kernel_gate::g3::Facts { parents, author: pair(0), committer: pair(2), changed, main_rs_added: kernel_gate::g3::added_lines(&diff) })
}

// ---- A3: G3 and the body's tests after the change went live (ADR-011 §2.1; X0c) ---------------

/// A3's G3 was stopped because the Root rolled its change's group back while it ran (demo
/// 2026-10-25, F7: G3 kept building in `INIT_HOME/probe`, about 1.6 GB, after the owner's
/// `/rollback`). It vetoes nothing.
pub const G3_STOPPED: &str = "gate.g3.stopped";

/// A spawner whose `wait_exit` kills the instance (through the Root, which signals its process
/// group) as soon as `stop` is set; the build then fails as killed.
struct StopSpawner<'a> {
    inner: &'a dyn crate::instance::Spawner,
    stop: &'a AtomicBool,
}

impl crate::instance::Spawner for StopSpawner<'_> {
    fn spawn(&self, req: &SpawnRequest) -> Result<crate::instance::Spawned, crate::instance::SpawnError> {
        self.inner.spawn(req)
    }
    fn kill(&self, instance: &str, mode: init_proto::root_link::KillMode) -> Result<(), crate::instance::SpawnError> {
        self.inner.kill(instance, mode)
    }
    fn wait_exit(&self, instance: &str, timeout: Duration) -> Option<crate::instance::ExitInfo> {
        let until = Instant::now() + timeout;
        loop {
            if self.stop.load(Ordering::SeqCst) {
                let _ = self.inner.kill(instance, init_proto::root_link::KillMode::Kill);
                return self.inner.wait_exit(instance, Duration::from_secs(30));
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            if let Some(e) = self.inner.wait_exit(instance, left.min(Duration::from_millis(250))) {
                return Some(e);
            }
        }
    }
}

/// G0's spawner that starts no command once `stop` is set (the running one ends on its own bound).
struct StopG0<'a> {
    inner: &'a dyn kernel_gate::g0::Spawner,
    stop: &'a AtomicBool,
}

impl kernel_gate::g0::Spawner for StopG0<'_> {
    fn spawn(&self, req: kernel_gate::g0::SpawnRequest) -> std::io::Result<std::process::Child> {
        if self.stop.load(Ordering::SeqCst) {
            return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "stopped: the change was rolled back"));
        }
        self.inner.spawn(req)
    }
}

/// What A3 found. `veto` is the `change.veto` call's result when A3 failed.
#[derive(Clone, Debug)]
pub struct A3Out {
    pub pass: bool,
    /// `gate.g0.test` (the body's own tests) or `gate.g3.<reason>`.
    pub reason: Option<String>,
    pub record: Value,
    pub veto: Option<Result<(), String>>,
}

/// The rule of A3, apart from what runs it: the body's own tests first (advisory in A1, so read
/// from the G0 stage record's `test` step), then, for scope `t2` or `dep`, G3 (`g3`, called only
/// then). The first failure is a zone veto (H12) of the change's group through `veto`, which sends
/// `change.veto{group, reason}`; the Root always honours it. A3 never holds the change back: it
/// runs after the change went live.
pub fn a3_run(scope: &str, g0: &Value, g3: impl FnOnce() -> Result<Value, StageFail>, veto: impl FnOnce(&str) -> Result<(), String>) -> A3Out {
    let tests_failed = g0["steps"].as_array().is_some_and(|a| a.iter().any(|s| s["name"] == "test" && (s["pass"] == false || s["exit"].as_i64().is_some_and(|e| e != 0))));
    let (reason, g3_record) = if tests_failed {
        (Some("gate.g0.test".to_string()), Value::Null)
    } else if matches!(scope, "t2" | "dep") {
        match g3() {
            Ok(r) => (None, r),
            Err(f) => (Some(f.code.clone()), json!({"pass": false, "reason": f.reason(), "detail": f.detail, "record": f.record})),
        }
    } else {
        (None, Value::Null)
    };
    // A stopped G3 vetoes nothing: its group is already being rolled back (F7).
    let veto = reason.as_deref().filter(|r| *r != G3_STOPPED).map(veto);
    let record = json!({"pass": reason.is_none(), "reason": reason, "tests_failed": tests_failed, "G3": g3_record,
                        "vetoed": veto.as_ref().map(|v| v.is_ok())});
    A3Out { pass: reason.is_none(), reason, record, veto }
}

/// A3 on the running gate (K2's `admit::a3_after_live` calls it): `job` names the A3 run (its G3
/// probe, proxy gate and cache), `binary` is the live candidate's binary (it must still exist), `g0`
/// the change's G0 record. G3 takes the build slot like G0–G3 of a promote, and spends at most
/// `budget.gate_usd` on its own proxy gate. `stop`: set by the supervisor when the Root rolls the
/// change's group back ([`super::admit::stop_a3`]); G3 then stops ([`G3_STOPPED`]) and vetoes nothing.
#[allow(clippy::too_many_arguments)]
pub fn a3(env: &Env, job: &str, scope: &str, sha: &str, binary: &Path, g0: &Value, stop: &AtomicBool, veto: impl FnOnce(&str) -> Result<(), String>) -> A3Out {
    let g3 = || {
        let _slot = env.slot_lock.lock().unwrap_or_else(|e| e.into_inner());
        if stop.load(Ordering::SeqCst) {
            // Rolled back while A3 waited for the build slot: no probe starts.
            return Err(StageFail::new(Stage::G3, G3_STOPPED, "the change was rolled back before its G3 started"));
        }
        let dir = job_cache(env, job);
        let built = Built { dir: dir.clone(), binary: dir.join("bin/init-body"), components: dir.join("ext"), record: g0.clone() };
        let _ = std::fs::create_dir_all(dir.join("bin"));
        let _ = std::fs::create_dir_all(&built.components);
        let _ = std::fs::remove_file(&built.binary);
        if let Err(e) = crate::sys::clone_path(binary, &built.binary).or_else(|_| std::fs::copy(binary, &built.binary).map(drop)) {
            // Not the change's fault: G3 cannot run, so it cannot veto (logged by the caller's record).
            return Ok(json!({"pass": true, "skipped": format!("the candidate binary {}: {e}", binary.display())}));
        }
        let _ = env.proxy.open_gate(job, "a3", env.gate.gate_musd);
        let cand = Candidate { sha: sha.into(), binary: built.binary.clone(), ext_set_dir: None };
        let r = g3_until(env, job, &cand, &built, false, stop).map(|o| o.record);
        env.proxy.close_gate(job);
        let _ = env.git().run(&["update-ref", "-d", &format!("refs/init/g3/{job}")], None, Duration::from_secs(30));
        if dir.starts_with(env.home.join("cache/gate")) && dir != env.home.join("cache/gate") {
            let _ = std::fs::remove_dir_all(&dir);
        }
        r
    };
    a3_run(scope, g0, g3, veto)
}

#[cfg(test)]
mod x0c_tests {
    use super::super::config::GateCfg;
    use super::*;

    /// A fake clock: seconds since the stage started.
    struct FakeClock(Duration);
    impl FakeClock {
        fn at(&mut self, s: u64) -> Duration {
            self.0 = Duration::from_secs(s);
            self.0
        }
    }

    fn summary(requests: u64, inflight: u64, finish: &str) -> Value {
        json!({"requests": requests, "inflight": inflight, "last_finish": {"reason": finish}})
    }

    /// The G3 agent part of M7 D's j-000010 on the fake clock: the model starts, its `selfmap
    /// --write` runs a 280 s cold body build (a bash call: nothing in flight, last finish
    /// `tool_use`), then it commits and stops. `(at_s, summary)` steps; the end is `stop` + 5 s quiet.
    fn timeline() -> Vec<(u64, Value)> {
        vec![
            (0, summary(1, 1, "")),
            (5, summary(1, 0, "tool_use")),
            (150, summary(1, 0, "tool_use")),
            (285, summary(2, 1, "tool_use")),
            (300, summary(2, 0, "tool_use")),
            (305, summary(3, 1, "tool_use")),
            (310, summary(3, 0, "stop")),
            (316, summary(3, 0, "stop")),
        ]
    }

    fn run(limit: Duration) -> (Wait, u64) {
        let mut clock = FakeClock(Duration::ZERO);
        let mut w = AgentWait::new(clock.0, limit, Duration::from_secs(5), 0);
        for (at, s) in timeline() {
            let r = w.step(clock.at(at), &s);
            if r != Wait::Pending {
                return (r, at);
            }
        }
        (Wait::Pending, 0)
    }

    /// RF-03 (X0c, G3-STAGE-TIMEOUT): a 280 s cold-slot build passes G3 on the fake clock. The old
    /// fixed 300 s bounds time out both parts; the bounds derived from the build timeouts do not,
    /// while a stalled agent and a slow inner G0–G2 are still caught.
    #[test]
    fn rf03_cold_build_passes_on_the_fake_clock() {
        let gate = GateCfg::default();
        let sup = crate::config::defaults().supervise;
        let b = gate.g3_bounds(&sup);
        assert_eq!((gate.g3_agent_timeout_s, gate.g3_inner_timeout_s, sup.build_timeout_cold_s, sup.build_timeout_warm_s), (300, 300, 3600, 1800));
        assert_eq!(b.agent, Duration::from_secs(300 + 3600));
        // The agent part.
        assert_eq!(run(Duration::from_secs(gate.g3_agent_timeout_s)), (Wait::TimedOut, 300), "the old bound: the M7 failure");
        assert_eq!(run(b.agent), (Wait::Done, 316));
        // The agent never stops: still timed out, at the derived bound.
        let mut w = AgentWait::new(Duration::ZERO, b.agent, Duration::from_secs(5), 0);
        assert_eq!(w.step(Duration::from_secs(3899), &summary(1, 0, "tool_use")), Wait::Pending);
        assert_eq!(w.step(Duration::from_secs(3900), &summary(1, 0, "tool_use")), Wait::TimedOut);
        // The inner gate: a 280 s cold build, then G0 60 s, G1 10 s, G2 40 s = 390 s in all.
        let mut clock = FakeClock(Duration::ZERO);
        let start = clock.0;
        let build = clock.at(280) - start;
        let total = clock.at(390) - start;
        assert!(total > Duration::from_secs(gate.g3_inner_timeout_s), "the old bound fails it");
        assert!(b.inner_in_time(total, build));
        assert!(total <= b.inner_limit(true));
        // G0–G2 alone over inner_timeout_s after the build is still a timeout.
        assert!(!b.inner_in_time(Duration::from_secs(280 + 301), build));
        // Configured build timeouts flow through (M7's DV-5c ran with 1800 s).
        let sup = crate::config::parse("[supervise]\nbuild_timeout_cold_s = 3600\nbuild_timeout_warm_s = 1800\n").unwrap().supervise;
        let b = gate.g3_bounds(&sup);
        assert_eq!((b.agent.as_secs(), b.inner_limit(true).as_secs(), b.inner_limit(false).as_secs()), (3900, 3900, 2100));
    }

    /// RF-04 (X0c): a failing asynchronous G3 vetoes its group (`change.veto{group, reason}`), and so
    /// do the body's own failing tests; a passing A3, and t0/t1 changes (no G3), veto nothing.
    #[test]
    fn rf04_failing_async_g3_vetoes_its_group() {
        let vetoes = std::cell::RefCell::new(Vec::<(u64, String)>::new());
        let group = 42u64;
        let veto = |r: &str| {
            vetoes.borrow_mut().push((group, r.to_string()));
            Ok(())
        };
        let g0_ok = json!({"pass": true, "steps": [{"name": "check", "exit": 0}, {"name": "test", "exit": 0}]});
        let g3_fail = || {
            let mut f = StageFail::new(Stage::G3, "gate.g3.inner_g2", "the self-edited body did not boot");
            f.record = json!({"pass": false});
            Err(f)
        };
        let o = a3_run("t2", &g0_ok, g3_fail, veto);
        assert!(!o.pass);
        assert_eq!(o.reason.as_deref(), Some("gate.g3.inner_g2"));
        assert_eq!(o.record["vetoed"], true);
        assert_eq!(*vetoes.borrow(), [(42, "gate.g3.inner_g2".to_string())]);
        // dep runs G3 too.
        let o = a3_run("dep", &g0_ok, || Err(StageFail::new(Stage::G3, "gate.g3.timeout", "late")), veto);
        assert_eq!(o.reason.as_deref(), Some("gate.g3.timeout"));
        // Passing: no veto.
        let o = a3_run("t2", &g0_ok, || Ok(json!({"pass": true})), |_: &str| -> Result<(), String> { panic!("no veto on a pass") });
        assert!(o.pass && o.veto.is_none());
        // t1: no G3; failing body tests (advisory in A1) veto.
        let g0_tests = json!({"pass": true, "steps": [{"name": "test", "exit": 101}]});
        let o = a3_run("t1", &g0_tests, || -> Result<Value, StageFail> { panic!("no G3 for t1") }, veto);
        assert_eq!(o.reason.as_deref(), Some("gate.g0.test"));
        let o = a3_run("t0", &g0_ok, || -> Result<Value, StageFail> { panic!("no G3 for t0") }, |_: &str| -> Result<(), String> { panic!() });
        assert!(o.pass);
        // A veto the Root did not take is reported, not hidden.
        let o = a3_run("t2", &g0_ok, g3_fail, |_: &str| Err("control link down".into()));
        assert_eq!((o.record["vetoed"].clone(), o.veto.clone()), (json!(false), Some(Err("control link down".to_string()))));
        assert_eq!(vetoes.borrow().len(), 3);
    }

    /// F7: a G3 stopped because its group was rolled back vetoes nothing.
    #[test]
    fn f7_a_stopped_g3_vetoes_nothing() {
        let g0_ok = json!({"pass": true, "steps": [{"name": "test", "exit": 0}]});
        let o = a3_run("t2", &g0_ok, || Err(StageFail::new(Stage::G3, G3_STOPPED, "rolled back")), |_: &str| -> Result<(), String> { panic!("no veto when stopped") });
        assert!(!o.pass && o.veto.is_none(), "{o:?}");
        assert_eq!((o.reason.as_deref(), o.record["vetoed"].clone()), (Some(G3_STOPPED), Value::Null));
    }

    /// An instance that runs until it is killed (a cold build, as far as the waiter can tell).
    #[derive(Default)]
    struct Endless {
        killed: std::sync::Mutex<Option<init_proto::root_link::KillMode>>,
    }

    impl crate::instance::Spawner for Endless {
        fn spawn(&self, _: &SpawnRequest) -> Result<crate::instance::Spawned, crate::instance::SpawnError> {
            Err(crate::instance::SpawnError::Io("not used".into()))
        }
        fn kill(&self, _: &str, mode: init_proto::root_link::KillMode) -> Result<(), crate::instance::SpawnError> {
            *self.killed.lock().unwrap() = Some(mode);
            Ok(())
        }
        fn wait_exit(&self, _: &str, timeout: Duration) -> Option<crate::instance::ExitInfo> {
            let until = Instant::now() + timeout;
            while Instant::now() < until {
                if self.killed.lock().unwrap().is_some() {
                    return Some(crate::instance::ExitInfo { code: None, signal: Some(9), at: String::new() });
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            None
        }
    }

    /// F7: the inner gate's build is killed (through the Root) within about 250 ms of the stop,
    /// and G0 starts no further command; without a stop the waiter keeps its own bound.
    #[test]
    fn f7_stop_kills_the_build_and_g0_starts_nothing() {
        use crate::instance::Spawner;
        let inner = Endless::default();
        let stop = AtomicBool::new(false);
        let sp = StopSpawner { inner: &inner, stop: &stop };
        assert!(sp.wait_exit("i-1", Duration::from_millis(300)).is_none(), "no stop: the timeout");
        assert!(inner.killed.lock().unwrap().is_none());
        std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_millis(200));
                stop.store(true, Ordering::SeqCst);
            });
            let t0 = Instant::now();
            let e = sp.wait_exit("i-1", Duration::from_secs(600)).expect("killed, not timed out");
            assert_eq!(e.signal, Some(9));
            assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
        });
        assert_eq!(*inner.killed.lock().unwrap(), Some(init_proto::root_link::KillMode::Kill));
        let g0 = StopG0 { inner: &kernel_gate::g0::DirectSpawner, stop: &stop };
        let req = kernel_gate::g0::SpawnRequest {
            program: "/bin/echo".into(), args: vec![], env: vec![], cwd: "/".into(),
            stdout: std::process::Stdio::null(), stderr: std::process::Stdio::null(),
        };
        let err = kernel_gate::g0::Spawner::spawn(&g0, req).expect_err("stopped");
        assert_eq!(err.kind(), std::io::ErrorKind::Interrupted);
    }
}
