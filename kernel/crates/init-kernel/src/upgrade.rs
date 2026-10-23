//! Free kernel promotion: isolated import/build, Root admission, then Root-owned swap.
//! No candidate binary runs in the supervisor. Root's smoke, snapshot and probation decide.
use init_proto::envelope::ErrorBody;
use init_proto::errors::ErrorCode;
use init_proto::initctl::{JobState, Stage};
use init_proto::root_link::{ChangeVetoArgs, InstanceKind, KernelRequest, KillMode, StoreKind, StorePutArgs, SwapData, SystemSwapArgs};
use init_proto::serde_json::{self, json, Value};
use kernel_serve::{JobCtl, JobStore};
use kernel_supervise::git::Git;
use kernel_supervise::instance::{run_to_completion, SpawnRequest, Spawner, Spawned, SpawnError, ExitInfo};
use kernel_supervise::kupgrade::candidate::{self, Artifact};
use kernel_supervise::promote::Env;
use kernel_supervise::supervisor::Services;
use std::collections::BTreeMap;
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::atomic::Ordering;
use std::time::Duration;

fn error(message: impl Into<String>) -> ErrorBody { ErrorBody::new(ErrorCode::Internal, message.into()) }
fn cancelled(job: &JobCtl) -> Result<(), ErrorBody> {
    if job.is_cancelled() { Err(error("kernel promotion cancelled")) } else { Ok(()) }
}
fn call(sv: &Services, req: KernelRequest, timeout: Duration) -> Result<Value, ErrorBody> {
    sv.link.call(&req, timeout).map_err(|e| match e {
        kernel_supervise::link::LinkError::Root(e) => e,
        other => error(format!("Root: {other}")),
    })
}

#[derive(Default)]
struct State { receipt: Option<Value>, job: Option<JobCtl> }
pub struct Upgrade { home: PathBuf, state: Mutex<State> }
impl Upgrade {
    pub fn new(home: &Path) -> Arc<Self> { Arc::new(Self { home: home.into(), state: Mutex::new(State::default()) }) }
    fn path(&self) -> PathBuf { self.home.join("state/kernel/kernel-upgrade.json") }
    fn save(&self, value: &Value) -> Result<(), ErrorBody> {
        let bytes = serde_json::to_vec(value).map_err(|e| error(e.to_string()))?;
        kernel_supervise::fsx::write_atomic(&self.path(), &bytes).map_err(|e| error(e.to_string()))
    }
    pub fn active(&self) -> Option<String> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.receipt.as_ref().filter(|r| !matches!(r["phase"].as_str(), Some("failed" | "probation_pass" | "rolled_back")))
            .and_then(|r| r["job"].as_str().map(str::to_string))
    }
    pub fn candidate(&self) -> Option<init_proto::initctl::KernelCandidate> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let rec = state.job.as_ref()?.record()?;
        if rec.state.is_terminal() { return None; }
        Some(init_proto::initctl::KernelCandidate { job: rec.id, root_job: rec.root_job,
            sha: rec.sha.unwrap_or_default(), stages: rec.stages, state: rec.state })
    }
    fn update(&self, f: impl FnOnce(&mut Value)) -> Result<(), ErrorBody> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let r = state.receipt.as_mut().ok_or_else(|| error("missing upgrade receipt"))?;
        f(r); self.save(r)
    }
    pub fn recover(&self, sv: &Services, jobs: &JobStore) {
        let Ok(meta) = std::fs::metadata(self.path()) else { return };
        if meta.len() > 65536 { return; }
        let Some(mut r) = std::fs::read(self.path()).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { return };
        if r["v"] != 1 || matches!(r["phase"].as_str(), Some("failed" | "probation_pass" | "rolled_back")) { return; }
        let Ok(tuple) = call(sv, KernelRequest::TupleGet, sv.link.request_timeout) else { return };
        let installed = r["init_kernel"].as_str().is_some_and(|h| h.len() == 64 && tuple["current"]["tuple"]["kernel"]["init_kernel"] == h)
            && tuple["current"]["tuple"]["kernel"]["initctl"] == r["initctl"];
        if installed {
            // A swap may start before the old kernel receives its RPC response. Bind the
            // restored receipt from Root's boot cause in that case, never an arbitrary event.
            if sv.boot.boot.reason.as_str() == "kernel_upgrade" {
                if let Some(cause) = &sv.boot.boot.cause {
                    if cause["to"] == tuple["current"]["id"] && cause["job"].is_string() {
                        if r["root_job"].is_null() { r["root_job"] = cause["job"].clone(); }
                        else if r["root_job"] != cause["job"] { return; }
                    }
                }
            }
            if r["root_job"].as_str().is_none() { return; }
            if let (Some(id), Some(sha)) = (r["job"].as_str(), r["sha"].as_str()) {
                if let Some(job) = jobs.resume_kernel_swap(id, sha, r["root_job"].as_str()) {
                    r["phase"] = json!("installed");
                    let _ = self.save(&r);
                    let root_job = r["root_job"].as_str().unwrap().to_string();
                    let group = r["group"].clone();
                    *self.state.lock().unwrap_or_else(|e| e.into_inner()) = State { receipt: Some(r), job: Some(job.clone()) };
                    // Replay only the exact job's durable Root verdict. Matching installed
                    // bytes alone never means probation passed. Hook is installed before recovery.
                    let verdict = call(sv, KernelRequest::SystemSwapStatus(init_proto::root_link::SystemSwapStatusArgs { job: root_job.clone() }), sv.link.request_timeout);
                    match verdict {
                        Ok(v) if v["job"] == root_job && v["to"] == tuple["current"]["id"] && v["group"] == group && v["stage"].is_string() => {
                            self.on_swap(&SwapData { job: root_job, stage: v["stage"].as_str().unwrap().into(), msg: "replayed durable Root swap record".into() });
                        }
                        _ => {
                            // Do not strand an active candidate or infer a terminal success if
                            // Root cannot authenticate the interrupted swap's persisted record.
                            if !job.is_finished() {
                                let _ = self.update(|r| { r["phase"] = json!("failed"); r["recovery"] = json!("Root swap status unavailable or mismatched"); });
                                job.fail(error("interrupted kernel swap could not be authenticated at Root"));
                            }
                        }
                    }
                    return;
                }
            }
        }
        // Unknown/interrupted/rolled-back upgrades remain failed in the ordinary job store.
        r["phase"] = json!("rolled_back");
        r["recovery"] = json!("Root current tuple does not confirm this candidate; no promotion success inferred");
        let _ = self.save(&r);
    }
    pub fn on_swap(&self, event: &SwapData) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(r) = state.receipt.as_mut() else { return };
        if matches!(r["phase"].as_str(), Some("failed" | "probation_pass" | "rolled_back")) { return; }
        if let Some(id) = r["root_job"].as_str() {
            if id != event.job { return; }
        } else if event.stage != "requested" || !matches!(r["phase"].as_str(), Some("submitting" | "swap_unknown")) { return; }
        r["root_job"] = json!(event.job);
        r["phase"] = json!(event.stage);
        let _ = self.save(r);
        let result = r.clone();
        let Some(job) = &state.job else { return };
        job.set_root_job(&event.job);
        match event.stage.as_str() {
            "probation_pass" => { job.succeed(&result); }
            "gog_fail" | "boot_fail" | "probation_fail" | "aborted" | "prepare_fail" => {
                job.fail(error(format!("Root rejected kernel swap: {}", event.stage)));
                if let Some(r) = state.receipt.as_mut() { r["phase"] = json!("failed"); let _ = self.save(r); }
            }
            _ => { let _ = job.observe_kernel_swap(); job.progress(JobState::AwaitingSwap, Some(Stage::Prepare), 90, None, &format!("Root kernel swap: {}", event.stage)); }
        }
    }
    pub fn start(self: &Arc<Self>, env: Arc<Env>, sv: Services, branch: String, job: JobCtl) -> Result<(), ErrorBody> {
        if !sv.extend_free.load(Ordering::SeqCst) { return Err(error("gated kernel K0–K3 is not implemented; free mode is required")); }
        let id = branch.strip_prefix("kevo/").filter(|id| kernel_supervise::git::evo_id_syntax_ok(id))
            .ok_or_else(|| error("kernel branch must be kevo/<id>"))?;
        let _ = id;
        if env.pipeline_busy.swap(true, Ordering::SeqCst) { return Err(error("another promotion pipeline is running")); }
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.receipt.as_ref().is_some_and(|r| !matches!(r["phase"].as_str(), Some("failed" | "probation_pass" | "rolled_back"))) {
                env.pipeline_busy.store(false, Ordering::SeqCst);
                return Err(error("kernel candidate is already in flight"));
            }
            let r = json!({"v":1,"job":job.id(),"branch":branch,"phase":"import","root_job":null});
            if let Err(e) = self.save(&r) { env.pipeline_busy.store(false, Ordering::SeqCst); return Err(e); }
            *state = State { receipt: Some(r), job: Some(job.clone()) };
        }
        let this = self.clone();
        let cleanup = env.clone();
        std::thread::Builder::new().name(format!("kernel-promote-{}", job.id())).spawn(move || {
            let outcome = this.run(&sv, &branch, &job);
            env.pipeline_busy.store(false, Ordering::SeqCst);
            if let Err(e) = outcome {
                // If an open group exists but swap never started, close it; Root owns rollback.
                let group = this.state.lock().unwrap_or_else(|e| e.into_inner()).receipt.as_ref().and_then(|r| {
                    if matches!(r["phase"].as_str(), Some("admitted")) { r["group"].as_u64() } else { None }
                });
                if let Some(group) = group { let _ = call(&sv, KernelRequest::ChangeVeto(ChangeVetoArgs { group, reason: "kernel promotion aborted before swap".into() }), sv.link.request_timeout); }
                let _ = this.update(|r| { r["phase"] = json!("failed"); r["error"] = json!(e.to_string()); });
                job.fail(e);
            }
        }).map_err(|e| { cleanup.pipeline_busy.store(false, Ordering::SeqCst); let _ = self.update(|r| r["phase"] = json!("failed")); error(e.to_string()) })?;
        Ok(())
    }

    fn run(&self, sv: &Services, branch: &str, job: &JobCtl) -> Result<(), ErrorBody> {
        cancelled(job)?;
        job.progress(JobState::Running, Some(Stage::Fetch), 5, None, "isolating kernel source import");
        let current = call(sv, KernelRequest::TupleGet, sv.link.request_timeout)?;
        let parent = current["current"]["tuple"]["kernel"]["built_from"].as_str().unwrap_or("");
        let root_git = Git::with_dir(&sv.cfg.supervise.git, &sv.home, &sv.home.join("lineage/kernel.git"));
        let parent = if kernel_supervise::git::is_oid(parent) { parent.to_string() } else {
            root_git.rev_parse_commit("refs/heads/main").ok_or_else(|| error("Root kernel lineage has no pinned parent commit"))?
        };
        let git = Git::with_dir(&sv.cfg.supervise.git, &sv.home, &sv.home.join("lineage/kernel-cand.git"));
        if !git.git_dir.exists() { git.init_bare(&git.git_dir).map_err(|e| error(e.to_string()))?; }
        // Parent source is Root-written, never an agent-controlled upload-pack configuration.
        if git.rev_parse_commit(&parent).as_deref() != Some(&parent) {
            git.fetch_rev(&root_git.git_dir, &parent, &format!("refs/init/kparents/{}", job.id()), "git-upload-pack").map_err(|e| error(e.to_string()))?;
        }
        let import = sv.home.join("cache/kernel-imports").join(job.id());
        std::fs::create_dir_all(import.parent().unwrap()).map_err(|e| error(e.to_string()))?;
        std::fs::create_dir(&import).map_err(|e| error(e.to_string()))?;
        let bundle = import.join("candidate.bundle");
        let source = sv.home.join("work/kernel");
        let mut req = request(&sv.home, &sv.cfg, &source, &import, 0)?;
        req.kind = InstanceKind::GitRead;
        req.profile = "git-read".into();
        req.params = kernel_supervise::spawnspec::git_read_params(&sv.home, &sv.home, &sv.cfg.sandbox);
        // The sandbox cannot write cache. Only the supervisor owns the bundle sink.
        req.env.insert("HOME".into(), "/nonexistent".into());
        req.env.insert("TMPDIR".into(), "/nonexistent".into());
        req.program = sv.cfg.supervise.git.clone();
        req.args = vec!["-c".into(), "core.hooksPath=/dev/null".into(), "-c".into(), "protocol.allow=never".into(),
            "-c".into(), "maintenance.auto=false".into(), "bundle".into(), "create".into(), "-".into(), format!("refs/heads/{branch}")];
        capture_bundle(sv, &req, job, &bundle, &import.join("import.log"))?;
        let bundle_meta = std::fs::symlink_metadata(&bundle).map_err(|e| error(e.to_string()))?;
        if !bundle_meta.file_type().is_file() || bundle_meta.len() == 0 || bundle_meta.len() > 512 * 1024 * 1024 {
            return Err(error("kernel source bundle must be a nonempty regular file <=512MiB"));
        }
        let sha = git.fetch(&bundle, &format!("refs/heads/{branch}"), &format!("refs/init/kcandidates/{}", job.id()), "git-upload-pack")
            .map_err(|e| error(e.to_string()))?;
        job.set_sha(&sha);
        self.update(|r| { r["sha"] = json!(sha); r["parent_sha"] = json!(parent); r["phase"] = json!("build"); })?;
        let prepared = candidate::prepare(&git, &parent, &sha, job.id()).map_err(|e| error(e.to_string()))?;
        cancelled(job)?;
        job.progress(JobState::Gating, Some(Stage::K0), 20, None, "building kernel and initctl in Root sandbox");
        let out = prepared.target.parent().unwrap();
        let mut req = request(&sv.home, &sv.cfg, &prepared.source, out, git.commit_time(&sha).map_err(|e| error(e.to_string()))?)?;
        req.args = candidate::BUILD_ARGS.iter().map(|s| s.to_string()).collect();
        run(sv, &req, job, Duration::from_secs(1800), &out.join("build.log"))?;
        let (kernel, ctl) = prepared.artifacts().map_err(|e| error(e.to_string()))?;
        cancelled(job)?;
        job.progress(JobState::Gating, Some(Stage::Store), 65, None, "storing pinned kernel artifact descriptors at Root");
        store(sv, StoreKind::InitKernel, &kernel)?; store(sv, StoreKind::Initctl, &ctl)?;
        let tuple = call(sv, KernelRequest::TupleGet, sv.link.request_timeout)?;
        let tuple = &tuple["current"]["tuple"];
        if tuple["kernel"] != current["current"]["tuple"]["kernel"] { return Err(error("running kernel tuple changed during candidate build")); }
        self.update(|r| { r["init_kernel"] = json!(kernel.sha256); r["initctl"] = json!(ctl.sha256); r["phase"] = json!("admitting"); })?;
        job.progress(JobState::Gating, Some(Stage::Admission), 75, None, "Root conformance probe and system snapshot");
        cancelled(job)?;
        let admitted = call(sv, KernelRequest::ChangeOpen(json!({"sha":sha,"from_sha":parent,"scope":"k","caps":[],"prefix_change":false,
            "candidate":{"body":tuple["body"],"ext":tuple["ext"],"kernel":{"built_from":sha,"init_kernel":kernel.sha256,"initctl":ctl.sha256,"source":"agent","version":"agent"}}})), Duration::from_secs(600))?;
        let group = admitted["group"].as_u64().ok_or_else(|| error("Root admission omitted group"))?;
        self.update(|r| { r["group"] = json!(group); r["phase"] = json!("admitted"); })?;
        cancelled(job)?;
        // Persist hashes/group BEFORE the call: Root may ask prepare_swap before its reply is handled.
        job.awaiting_root(None).map_err(|_| error("upgrade job disappeared before swap"))?;
        self.update(|r| r["phase"] = json!("submitting"))?;
        let result = sv.link.call(&KernelRequest::SystemSwap(SystemSwapArgs { group: Some(group), version: "agent".into(), source_repo:"lineage/kernel-cand.git".into(), source_sha:sha,
            init_kernel:kernel.sha256, initctl:ctl.sha256, report:json!({"mode":"free","build":"passed","root_admission":admitted,"K0_K3":"not_claimed"}) }), Duration::from_secs(60));
        let result = match result {
            Ok(result) => result,
            Err(kernel_supervise::link::LinkError::Root(e)) => {
                self.update(|r| r["phase"] = json!("admitted"))?;
                return Err(e);
            }
            Err(e) => {
                // The swap may already be running: never retry it or veto an unknown outcome.
                self.update(|r| { if r["root_job"].is_null() { r["phase"] = json!("swap_unknown"); } r["link_error"] = json!(e.to_string()); })?;
                return Ok(());
            }
        };
        let root_job = result["job"].as_str().ok_or_else(|| error("Root swap response omitted job"))?;
        job.set_root_job(root_job);
        self.update(|r| { r["root_job"] = json!(root_job); if r["phase"] == "submitting" { r["phase"] = json!("awaiting_swap"); } })?;
        let _ = job.observe_kernel_swap();
        job.progress(JobState::AwaitingSwap, Some(Stage::Prepare), 90, None, "Root accepted swap; waiting for Root probation verdict");
        Ok(())
    }
}

fn request(home: &Path, cfg: &kernel_supervise::config::KernelCfg, source: &Path, out: &Path, epoch: i64) -> Result<SpawnRequest, ErrorBody> {
    for name in ["target", "tmp", "home", "cargo-home"] { std::fs::create_dir_all(out.join(name)).map_err(|e| error(e.to_string()))?; }
    let sp = &cfg.sandbox;
    let mut vars = BTreeMap::new();
    for (key, value) in [("HOME",out.join("home")),("TMPDIR",out.join("tmp")),("CARGO_HOME",out.join("cargo-home")),("CARGO_TARGET_DIR",out.join("target"))] {
        vars.insert(key.into(), value.to_string_lossy().into_owned());
    }
    vars.insert("PATH".into(), format!("{}:{}:/usr/bin:/bin", sp.toolchain.join("bin").display(), sp.devdir.join("usr/bin").display()));
    for (key,value) in [("CARGO_NET_OFFLINE","true"),("CARGO_INCREMENTAL","0"),("GIT_CONFIG_NOSYSTEM","1"),("GIT_CONFIG_GLOBAL","/dev/null"),("GIT_TERMINAL_PROMPT","0"),("GIT_NO_REPLACE_OBJECTS","1"),("GIT_NO_LAZY_FETCH","1")] { vars.insert(key.into(), value.into()); }
    vars.insert("CARGO_BUILD_JOBS".into(), kernel_supervise::build::SANDBOX_CARGO_JOBS.into());
    vars.insert("SOURCE_DATE_EPOCH".into(), epoch.to_string());
    if cfg!(target_os="macos") {
        if let Some(sdk) = &sp.sdkroot { vars.insert("SDKROOT".into(),sdk.to_string_lossy().into()); }
        vars.insert("DEVELOPER_DIR".into(),sp.devdir.to_string_lossy().into());
    }
    Ok(SpawnRequest { kind:InstanceKind::Build,profile:"build".into(),params:kernel_supervise::spawnspec::build_params(home,source,out,sp),program:sp.toolchain.join("bin/cargo"),args:vec![],env:vars,cwd:source.into(),lane:None,replaces:None })
}
fn run(sv: &Services, req: &SpawnRequest, job: &JobCtl, timeout: Duration, log: &Path) -> Result<(), ErrorBody> {
    cancelled(job)?;
    let cancellable = CancelSpawner { inner: &*sv.spawner, job };
    let (exit,timed_out) = run_to_completion(&cancellable,req,timeout,Some(log),4<<20).map_err(|e| error(e.to_string()))?;
    cancelled(job)?;
    if timed_out || !exit.success() { return Err(error(format!("sandboxed kernel preparation failed: exit={:?}, timeout={timed_out}",exit.code))); }
    Ok(())
}
/// Preserve stdout byte-for-byte; stderr is never mixed into a Git bundle. Both
/// sinks are supervisor-owned and bounded, while Root confines the source reader.
fn capture_bundle(sv: &Services, req: &SpawnRequest, job: &JobCtl, bundle: &Path, log: &Path) -> Result<(), ErrorBody> {
    use std::io::{Read, Write};
    let mut output = std::fs::OpenOptions::new().write(true).create_new(true).open(bundle).map_err(|e| error(e.to_string()))?;
    let mut diagnostic = std::fs::OpenOptions::new().write(true).create_new(true).open(log).map_err(|e| error(e.to_string()))?;
    cancelled(job)?;
    let sp = CancelSpawner { inner: &*sv.spawner, job };
    let child = sp.spawn(req).map_err(|e| error(e.to_string()))?;
    drop(child.stdin);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> std::io::Result<()> {
            let input = std::fs::File::from(child.stdout);
            let bytes = std::io::copy(&mut input.take((512 << 20) + 1), &mut output)?;
            if bytes == 0 || bytes > 512 << 20 { return Err(std::io::Error::other("kernel bundle outside 1..512MiB")); }
            output.sync_all()
        })();
        let _ = tx.send(result);
    });
    std::thread::spawn(move || {
        let mut input = std::fs::File::from(child.stderr);
        let mut buf = [0; 8192];
        let mut remaining = 4 << 20;
        while let Ok(n) = input.read(&mut buf) {
            if n == 0 { break; }
            let count = n.min(remaining);
            if count > 0 { let _ = diagnostic.write_all(&buf[..count]); remaining -= count; }
        }
    });
    let exit = sp.wait_exit(&child.instance, Duration::from_secs(120));
    let _ = sp.kill(&child.instance, KillMode::Kill);
    if exit.is_none() { let _ = sp.wait_exit(&child.instance, Duration::from_secs(30)); }
    cancelled(job)?;
    if !exit.is_some_and(|e| e.success()) { return Err(error("sandboxed kernel source import failed or timed out")); }
    rx.recv_timeout(Duration::from_secs(5)).map_err(|_| error("kernel bundle pipe did not close"))?
        .map_err(|e| error(format!("kernel bundle capture: {e}")))
}

fn store(sv: &Services, kind: StoreKind, artifact: &Artifact) -> Result<(), ErrorBody> {
    sv.link.call_with_fd(&KernelRequest::StorePut(StorePutArgs { kind,name:None,sha256:artifact.sha256.clone(),bytes:artifact.bytes,job:None }),artifact.file.as_fd(),kernel_supervise::link::STORE_TIMEOUT)
        .map(|_| ()).map_err(|e| error(format!("Root artifact store: {e}")))
}

/// Poll only this build instance, so cancellation stops its descendants through Root.
struct CancelSpawner<'a> { inner: &'a dyn Spawner, job: &'a JobCtl }
impl Spawner for CancelSpawner<'_> {
    fn spawn(&self, req: &SpawnRequest) -> Result<Spawned, SpawnError> { self.inner.spawn(req) }
    fn kill(&self, instance: &str, mode: KillMode) -> Result<(), SpawnError> { self.inner.kill(instance, mode) }
    fn wait_exit(&self, instance: &str, timeout: Duration) -> Option<ExitInfo> {
        let until = std::time::Instant::now() + timeout;
        loop {
            if self.job.is_cancelled() {
                let _ = self.inner.kill(instance, KillMode::Kill);
                return self.inner.wait_exit(instance, Duration::from_secs(30));
            }
            let remaining = until.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() { return None; }
            if let Some(exit) = self.inner.wait_exit(instance, remaining.min(Duration::from_millis(250))) {
                // Descendants must be quiescent before artifacts are inspected.
                let _ = self.inner.kill(instance, KillMode::Kill);
                return Some(exit);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel_serve::{JobConfig, LaneCtx};
    fn home(name: &str) -> PathBuf {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/kernel-upgrade");
        std::fs::create_dir_all(&base).unwrap();
        let path = base.canonicalize().unwrap().join(format!("{name}-{}", kernel_supervise::sys::random_hex(4)));
        std::fs::create_dir_all(path.join("state/kernel")).unwrap();
        path
    }
    fn upgrade(home: &Path) -> (Arc<Upgrade>, JobStore, JobCtl) {
        let (jobs, _) = JobStore::open(&home.join("state/kernel/jobs"), JobConfig::default()).unwrap();
        let job = jobs.create(&LaneCtx::live(), "kernel.promote", json!({"ref":"kevo/test"})).unwrap();
        job.set_sha(&"a".repeat(40));
        let upgrade = Upgrade::new(home);
        *upgrade.state.lock().unwrap() = State { job: Some(job.clone()), receipt: Some(json!({
            "v":1,"job":job.id(),"sha":"a".repeat(40),"phase":"submitting","group":7,"root_job":null
        })) };
        (upgrade, jobs, job)
    }
    #[test]
    fn kernel_upgrade_root_events_require_matching_job_and_probation() {
        let home = home("events");
        let (upgrade, _jobs, job) = upgrade(&home);
        upgrade.on_swap(&SwapData { job:"s-stale".into(),stage:"probation_pass".into(),msg:"".into() });
        assert!(!job.is_finished());
        upgrade.on_swap(&SwapData { job:"s-000001".into(),stage:"requested".into(),msg:"".into() });
        assert_eq!(job.record().unwrap().state, JobState::AwaitingSwap);
        upgrade.on_swap(&SwapData { job:"s-000002".into(),stage:"probation_pass".into(),msg:"".into() });
        assert!(!job.is_finished());
        upgrade.on_swap(&SwapData { job:"s-000001".into(),stage:"installed".into(),msg:"".into() });
        assert!(!job.is_finished());
        upgrade.on_swap(&SwapData { job:"s-000001".into(),stage:"probation_pass".into(),msg:"".into() });
        assert_eq!(job.record().unwrap().state, JobState::Succeeded);
        assert!(upgrade.active().is_none());
        std::fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn kernel_upgrade_root_abort_is_terminal_failure() {
        let home = home("abort");
        let (upgrade, _jobs, job) = upgrade(&home);
        upgrade.on_swap(&SwapData { job:"s-000001".into(),stage:"requested".into(),msg:"".into() });
        upgrade.on_swap(&SwapData { job:"s-000001".into(),stage:"aborted".into(),msg:"".into() });
        assert_eq!(job.record().unwrap().state, JobState::Failed);
        assert!(upgrade.active().is_none());
        std::fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn kernel_upgrade_build_request_keeps_source_outside_writable_output() {
        let home = home("request");
        let cfg = kernel_supervise::config::KernelCfg::default();
        let src = home.join("cache/kernel-candidates/j-000001/src");
        let out = home.join("cache/kernel-candidates/j-000001/out");
        let req = request(&home, &cfg, &src, &out, 42).unwrap();
        assert_eq!(req.profile, "build");
        assert_eq!(req.kind, InstanceKind::Build);
        assert!(!src.starts_with(&out));
        assert_eq!(req.env["CARGO_NET_OFFLINE"], "true");
        assert_eq!(req.env["CARGO_BUILD_JOBS"], "2");
        assert_eq!(req.env["CARGO_TARGET_DIR"], out.join("target").to_string_lossy());
        assert_eq!(req.params["SRC"], src.to_string_lossy());
        assert_eq!(req.params["OUT"], out.to_string_lossy());
        assert!(!req.env.contains_key("INIT_ROOT_HOME"));
        assert!(candidate::BUILD_ARGS.contains(&"--offline"));
        assert!(candidate::BUILD_ARGS.contains(&"initctl"));
        std::fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn kernel_upgrade_build_requests_do_not_inherit_host_job_count() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "upgrade::tests::kernel_upgrade_build_request_keeps_source_outside_writable_output", "--nocapture"])
            .env("CARGO_BUILD_JOBS", "128").output().unwrap();
        assert!(output.status.success(), "child environment qualification failed: {}", String::from_utf8_lossy(&output.stderr));
        assert!(String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed; 0 failed;"), "the exact request test must actually execute");
    }
}

#[cfg(test)]
mod pipeline_tests;
