//! G4's process side (gate.md §3.7): the frozen task sets, one task run on supervise's `probe()`
//! (the task workspace placed before spawn, the prompt sent as an `input` frame, the end detected
//! from the proxy lane), the check protocol in the `check` profile on the kept workspace, and the
//! RunRecord from the lane's summary. Scheduling and statistics are `kernel_gate::g4`.

use super::proxy::{GateScope, LaneKind};
use super::stages::job_cache;
use super::Env;
use crate::instance::{SpawnRequest, Spawner};
use crate::probe::{probe_scoped, Candidate, ProbeOpts, Purpose};
use crate::proberoot::SessionSrc;
use init_proto::files::{BootReason, InitMode};
use init_proto::root_link::{InstanceKind, KillMode};
use kernel_gate::g4::{RunExecutor, RunRecord, RunSet, TaskDef};
use kernel_gate::stats::Arm;
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The frozen sets: `vault/gate/pool/`, `vault/holdout/`, `vault/canaries/`. Tasks that do not load
/// are skipped (and logged); `task_drift` against `state/tasks/` is reported by the caller.
pub fn load_set(env: &Env, rel: &str, set: RunSet) -> Vec<TaskDef> {
    let dir = env.home.join(rel);
    let mut v = Vec::new();
    let Ok(rd) = std::fs::read_dir(&dir) else { return v };
    let mut names: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    names.sort();
    for p in names {
        match kernel_gate::g4::load_task(&p) {
            Ok(t) => v.push(TaskDef { id: t.id.clone(), set, source: t.source.kind, labels: t.labels.clone(), family: None, dir: p, task: t }),
            Err(e) => crate::klog::log("gate", &format!("task {}: {e}", p.display())),
        }
    }
    v
}

/// Run an instance to completion, keeping stdout apart (the check's score line).
fn run_capture(sp: &dyn Spawner, req: &SpawnRequest, timeout: Duration) -> (Option<i32>, bool, Vec<u8>) {
    let Ok(s) = sp.spawn(req) else { return (None, false, Vec::new()) };
    drop(s.stdin);
    let out = std::thread::spawn(move || {
        let mut f = std::fs::File::from(s.stdout);
        let mut b = Vec::new();
        let _ = (&mut f).take(1 << 20).read_to_end(&mut b);
        let _ = std::io::copy(&mut f, &mut std::io::sink());
        b
    });
    let err = std::thread::spawn(move || {
        let mut f = std::fs::File::from(s.stderr);
        let _ = std::io::copy(&mut f, &mut std::io::sink());
    });
    let (code, timed_out) = match sp.wait_exit(&s.instance, timeout) {
        Some(e) => (e.code, false),
        None => {
            let _ = sp.kill(&s.instance, KillMode::Kill);
            let _ = sp.wait_exit(&s.instance, Duration::from_secs(30));
            (None, true)
        }
    };
    let stdout = out.join().unwrap_or_default();
    let _ = err.join();
    (code, timed_out, stdout)
}

/// One arm of a G4 job.
#[derive(Clone, Debug)]
pub struct ArmSpec {
    pub sha: String,
    pub binary: PathBuf,
}

/// [`RunExecutor`] on probes (gate.md §3.7.3).
pub struct ProbeRuns<'a> {
    pub env: &'a Env,
    pub job: String,
    pub c: ArmSpec,
    pub p: ArmSpec,
}

fn extract(tar: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let st = std::process::Command::new("/usr/bin/tar").arg("-x").arg("-f").arg(tar).arg("-C").arg(dest).env_clear().env("PATH", "/usr/bin:/bin").status().map_err(|e| e.to_string())?;
    if st.success() { Ok(()) } else { Err(format!("tar exited {st}")) }
}

fn remove_exact(p: &Path) {
    let _ = std::fs::remove_dir_all(p);
}

impl ProbeRuns<'_> {
    fn base(&self, arm: Arm, t: &TaskDef, r: u32, run_id: &str) -> RunRecord {
        let sha = if arm == Arm::C { &self.c.sha } else { &self.p.sha };
        RunRecord {
            v: kernel_gate::g4::RUN_RECORD_V,
            gate_id: self.job.clone(),
            run_id: run_id.into(),
            arm: if arm == Arm::C { "c".into() } else { "p".into() },
            sha: sha.clone(),
            level: self.env.level_str(),
            task: t.id.clone(),
            set: serde_json::to_value(t.set).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(),
            source: serde_json::to_value(t.source).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(),
            labels: t.labels.clone(),
            repeat: r,
            ..Default::default()
        }
    }

    fn check(&self, t: &TaskDef, run_id: &str, out: &Path) -> (kernel_gate::g4::CheckScore, u64) {
        let t0 = Instant::now();
        for d in ["tmp", "home"] {
            let _ = std::fs::create_dir_all(out.join(d));
        }
        if crate::sys::clone_path(&t.dir.join("check"), &out.join("check")).is_err() {
            return (kernel_gate::g4::CheckScore::default(), 0);
        }
        let env = kernel_gate::g4::check_env(out, &self.env.cfg.sandbox.toolchain.join("bin"), &t.id);
        let mut args: Vec<String> = t.task.check.argv[1..].to_vec();
        args.retain(|_| true);
        let req = SpawnRequest {
            kind: InstanceKind::Check,
            profile: "check".into(),
            params: crate::spawnspec::build_params(&self.env.home, &out.join("ws"), out, &self.env.cfg.sandbox),
            program: out.join("check/run"),
            args,
            env: env.into_iter().collect(),
            cwd: out.join("ws"),
            lane: None,
            replaces: None,
        };
        let (code, timed_out, stdout) = run_capture(self.env.spawner.as_ref(), &req, Duration::from_secs(t.task.check.timeout_s));
        let _ = run_id;
        (kernel_gate::g4::check_score(code, timed_out, &stdout), t0.elapsed().as_millis() as u64)
    }
}

impl RunExecutor for ProbeRuns<'_> {
    fn run(&self, arm: Arm, t: &TaskDef, r: u32, run_id: &str) -> RunRecord {
        let env = self.env;
        let mut rec = self.base(arm, t, r, run_id);
        let spec = if arm == Arm::C { &self.c } else { &self.p };
        let cache = job_cache(env, &self.job);
        let ws_src = cache.join("ws").join(run_id);
        let out = cache.join("check").join(run_id);
        // EV-TRACE: a pool run's session log is kept here until the score says whether to retain it.
        let sessions = cache.join("sessions").join(run_id);
        remove_exact(&ws_src);
        remove_exact(&out);
        remove_exact(&sessions);
        if let Err(e) = extract(&t.dir.join("workspace.tar"), &ws_src) {
            crate::klog::log("gate", &format!("{run_id}: workspace: {e}"));
            rec.outcome = "infra".into();
            return rec;
        }
        let kernel_only = matches!(t.set, RunSet::Holdout | RunSet::Canary);
        let scope = GateScope::new(env.proxy.clone(), &format!("{}/run/{run_id}", self.job), LaneKind::Run, &self.job, kernel_only, t.task.limits.max_cost_musd, &spec.sha, "cold");
        // Per-run caps on actuals (m7-design.md §3.2.8): the task's v3 limits, else the pool defaults.
        scope.set_caps(kernel_gate::caps::RunCaps::for_task(&t.task.limits));
        let mut keep = vec![(PathBuf::from("work/task"), PathBuf::from(format!("check/{run_id}/ws")))];
        if t.set == RunSet::Pool {
            keep.push((PathBuf::from("state/sessions"), PathBuf::from(format!("sessions/{run_id}"))));
        }
        let opts = ProbeOpts {
            session: SessionSrc::Fresh,
            mode: InitMode::Normal,
            boot_reason: BootReason::Cold,
            purpose: Purpose::Run,
            job: self.job.clone(),
            extra_files: vec![(ws_src.clone(), PathBuf::from("work/task"))],
            cwd: Some(PathBuf::from("work/task")),
            keep_on_stop: keep,
        };
        let c = Candidate { sha: spec.sha.clone(), binary: spec.binary.clone(), ext_set_dir: None };
        let h = match probe_scoped(env.spawner.clone(), &env.home, &env.cfg, &c, opts, Some(scope as Arc<dyn crate::probe::ProbeScope>)) {
            Ok(h) => h,
            Err(e) => {
                crate::klog::log("gate", &format!("{run_id}: probe: {e}"));
                remove_exact(&ws_src);
                rec.outcome = "infra".into();
                return rec;
            }
        };
        remove_exact(&ws_src);
        let ready = h.wait_ready(Duration::from_secs(env.gate.g2_ready_timeout_s));
        let t0 = Instant::now();
        let mut outcome = "done";
        if ready.is_err() {
            outcome = "no_ready";
        } else {
            rec.ready_ms = t0.elapsed().as_millis() as u64;
            let base = h.scope_summary()["requests"].as_u64().unwrap_or(0);
            h.send_input(&format!("[task {}] Workspace: {}/work/task\n\n{}", t.id, h.root().display(), t.task.prompt));
            let wall = Duration::from_secs(t.task.limits.wall_s.min(env.gate.task_wall_s.max(1)));
            let quiet = Duration::from_millis(env.gate.quiet_ms);
            let mut since: Option<Instant> = None;
            let mut capped_since: Option<Instant> = None;
            loop {
                if let Some(e) = h.exited(Duration::from_millis(200)) {
                    if !matches!(e.code, Some(0) | Some(75)) {
                        outcome = "crash";
                    }
                    break;
                }
                if t0.elapsed() >= wall {
                    outcome = "timeout";
                    break;
                }
                let s = h.scope_summary();
                let idle = s["inflight"].as_u64().unwrap_or(0) == 0 && s["requests"].as_u64().unwrap_or(0) > base && s["last_finish"]["reason"].as_str() == Some("stop");
                since = match (idle, since) {
                    (true, None) => Some(Instant::now()),
                    (true, x) => x,
                    _ => None,
                };
                if since.is_some_and(|x| x.elapsed() >= quiet) {
                    break;
                }
                if s["capped"].as_bool() == Some(true) {
                    let at = *capped_since.get_or_insert_with(Instant::now);
                    if at.elapsed() >= Duration::from_secs(30).min(quiet * 6) {
                        outcome = "cost_capped";
                        break;
                    }
                }
            }
        }
        rec.wall_ms = t0.elapsed().as_millis() as u64;
        let o = h.stop();
        rec.exit_code = o.exit.as_ref().and_then(|e| e.code);
        let s = &o.scope;
        let u = &s["usage"];
        let input_total = ["input_uncached", "input_cache_read", "input_cache_write", "input_cache_write_1h"].iter().map(|k| u[*k].as_u64().unwrap_or(0)).sum::<u64>();
        rec.requests = s["requests"].as_u64().unwrap_or(0);
        rec.turns = s["turns"].as_u64().unwrap_or(rec.requests);
        rec.caps_hit = s["caps_hit"].as_str().map(str::to_string);
        rec.model_served = s["model_served"].as_str().map(str::to_string);
        rec.cache = Some("fresh".into());
        rec.tokens_in = input_total;
        rec.tokens_out = u["output"].as_u64().unwrap_or(0);
        rec.cache_read = u["input_cache_read"].as_u64().unwrap_or(0);
        rec.input_total = input_total;
        rec.cost_ref_musd = s["cost_ref_pusd"].as_u64().unwrap_or(0) / 1_000_000;
        rec.spent_musd = s["spent_musd"].as_u64().unwrap_or(0);
        rec.gap_ms_p50 = s["gap_ms_p50"].as_u64().unwrap_or(0);
        rec.ttft_ms_p50 = s["ttft_ms_p50"].as_u64().unwrap_or(0);
        if let Some(m) = s["by_model"].as_object() {
            for (k, v) in m {
                rec.models.insert(k.clone(), json!({"cost_ref_musd": v["cost_ref_pusd"].as_u64().unwrap_or(0) / 1_000_000, "requests": v["requests"], "routed": v["routed"]}));
            }
        }
        rec.outcome = outcome.into();
        if outcome == "no_ready" || o.kept.is_empty() {
            rec.score_permille = 0;
        } else {
            let (score, ms) = self.check(t, run_id, &out);
            rec.score_permille = score.score_permille;
            rec.tamper = score.tamper;
            rec.check_ms = ms;
        }
        remove_exact(&out);
        // EV-TRACE: a failing pool run's session log goes to vault/gate/jobs/<job>/sessions/.
        if let Some(log) = kernel_gate::retain::session_log_in_dir(&sessions)
            && let Err(e) = kernel_gate::retain::retain(&env.home, &rec, &log)
        {
            crate::klog::log("gate", &format!("{run_id}: retain session log: {e}"));
        }
        remove_exact(&sessions);
        rec
    }
}

/// The run records of one job, appended to `vault/gate/jobs/<job>/runs.jsonl`.
pub fn write_runs(env: &Env, job: &str, runs: &[RunRecord]) {
    let dir = env.home.join("vault/gate/jobs").join(job);
    let _ = std::fs::create_dir_all(&dir);
    let mut b = Vec::new();
    for r in runs {
        if let Ok(v) = serde_json::to_vec(r) {
            b.extend(v);
            b.push(b'\n');
        }
    }
    let _ = std::fs::write(dir.join("runs.jsonl"), b);
}

/// The capability self-model's input (gate.md §3.10): LKG-arm runs, and on accept the candidate's,
/// appended to `vault/gate/results/<sha12>.jsonl` keyed by the arm's sha.
pub fn compact_results(env: &Env, runs: &[RunRecord], accepted: bool) {
    let dir = env.home.join("vault/gate/results");
    let _ = std::fs::create_dir_all(&dir);
    for r in runs.iter().filter(|r| r.arm == "p" || accepted) {
        let sha12: String = r.sha.chars().take(12).collect();
        if sha12.len() == 12 {
            if let Ok(v) = serde_json::to_vec(r) {
                let mut line = v;
                line.push(b'\n');
                let _ = crate::fsx::append_line(&dir.join(format!("{sha12}.jsonl")), &line);
            }
        }
    }
}

/// A value for tests and records: the agent-visible run list (holdout redacted).
pub fn agent_view(runs: &[RunRecord]) -> Value {
    Value::Array(runs.iter().map(|r| serde_json::to_value(r.redacted()).unwrap_or(Value::Null)).collect())
}
