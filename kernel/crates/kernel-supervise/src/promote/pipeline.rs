//! The pipeline itself: [`promote`] (synchronous part, then the job thread), [`gate_quick`],
//! [`rollback`], the G4 job ([`run_g4_job`], with F12 retries and the one escalated re-run of
//! §3.12) and the provisional watch (F16).
//!
//! Failure branches (evolution.md §3.5.2) and where they end:
//! F1 `promote` error before the job · F2 `E_BUSY`/`E_PROVISIONAL_DEPTH` · F3–F6
//! `E_GATE_FAILED{stage}` · F7 sync G4 reject/inconclusive → `E_GATE_FAILED{stage:"G4"}` ·
//! F8/F9 `E_PAUSE_TIMEOUT`/`E_HANDOFF_FAILED` (supervise's handoff) · F11 async reject →
//! `g4_reject` (rollback at idle) · F12 infra → retried ≤ 2 · F13 inconclusive → one escalated
//! re-run → `inconclusive_final` · F14 canary → reject, `refs/init/quarantine/<sha>`, alert ·
//! F16 provisional too old → rollback.

use super::admit::{self, ExtendLink};
use super::g4run::{self, ArmSpec, ProbeRuns};
use super::record::Record;
use super::stages::{self, job_cache, Built, G1Out, StageFail};
use super::{Env, Job};
use crate::probe::Candidate;
use init_proto::envelope::{ErrorBody, ErrorDetail};
use init_proto::errors::ErrorCode;
use init_proto::initctl::{GateResult, JobState, PromoteArgs, RollbackArgs, Stage, StageResult};
use kernel_gate::accept::{self, CostBudget, Resolution, Verdict};
use kernel_gate::g4::{G4Outcome, G4Plan, RunSet};
use kernel_gate::scope::G4Need;
use kernel_gate::stats::NoiseHistory;
use kernel_evo::proposal::ChangeEv as ProposalEv;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

/// Releases the pipeline slot when the job thread ends.
struct Slot<'a>(&'a Env);
impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.0.pipeline_busy.store(false, Ordering::SeqCst);
    }
}

fn err(code: ErrorCode, msg: impl AsRef<str>) -> ErrorBody {
    ErrorBody::new(code, msg)
}

fn coded(e: crate::git::CodedError) -> ErrorBody {
    err(ErrorCode::parse(e.code), e.message)
}

/// `E_GATE_FAILED {stage, reason, report}`.
fn gate_failed(f: &StageFail, report: &std::path::Path, env: &Env) -> ErrorBody {
    let rel = report.strip_prefix(&env.home).unwrap_or(report).to_string_lossy().into_owned();
    let mut e = err(ErrorCode::GateFailed, format!("{} failed: {}", f.stage, f.code));
    e.data = Some(json!({"stage": f.stage, "reason": f.code, "report": rel}));
    e.details = vec![ErrorDetail { code: f.code.clone(), detail: kernel_gate::record::cap(&f.detail, 1000) }];
    e
}

fn stage_result(stage: Stage, pass: bool, ms: u64, code: Option<&str>) -> StageResult {
    StageResult { stage, pass, wall_ms: ms, code: code.map(str::to_string), reason: code.map(|c| c.rsplit('.').next().unwrap_or(c).to_string()) }
}

/// `promote` (initctl.md §4.1; supervise.md §3.6): the synchronous part (fetch, kernel checks,
/// validator, scope) runs in the caller and returns `{sha}` or the error; the rest runs on a job
/// thread that ends in exactly one terminal call on `job`.
pub fn promote(env: Arc<Env>, args: &PromoteArgs, job: Arc<dyn Job>) -> Result<Value, ErrorBody> {
    let Some(r) = args.git_ref.as_deref() else {
        return Err(err(if args.sha.is_some() && env.level < 2 { ErrorCode::Level } else { ErrorCode::Args }, "promote takes {\"ref\":\"evo/<id>\"} (`sha` is L2, live lane)"));
    };
    let Some(id) = r.strip_prefix("evo/") else { return Err(err(ErrorCode::Args, "ref must be evo/<id>")) };
    let git = env.git();
    if !git.validate_evo_id(id) {
        return Err(err(ErrorCode::Args, format!("evo id `{id}` must match ^[a-z0-9][a-z0-9._-]{{0,62}}$")));
    }
    if let Some(s) = env.sup.snapshot() {
        if s.busy {
            return Err(err(ErrorCode::Busy, "a handoff or rollback is in progress"));
        }
    }
    if env.pipeline_busy.swap(true, Ordering::SeqCst) {
        return Err(err(ErrorCode::Busy, "a G0-G3 pipeline is running (one at a time)"));
    }
    let release = || env.pipeline_busy.store(false, Ordering::SeqCst);
    let jid = job.id();
    let cand_ref = format!("refs/init/candidates/{jid}");
    // ADR-011: charter `extend.mode`, read once per promote (validator, scope and job agree).
    let free = env.free_link();
    let pre = (|| -> Result<stages::Fetched, ErrorBody> {
        job.progress(JobState::Running, Some(Stage::Fetch), 0, "fetch");
        let sha = stages::fetch(&env, &env.home.join("body"), r, &cand_ref).map_err(coded)?;
        let _ = git.archive_ref(&sha, &format!("init: candidate {jid} {r}"));
        let running = git.rev_parse_commit("refs/heads/main").ok_or_else(|| err(ErrorCode::Internal, "lineage has no refs/heads/main"))?;
        git.check_candidate("refs/heads/main", &sha, "refs/upstream/").map_err(coded)?;
        job.progress(JobState::Running, Some(Stage::Validate), 0, "validate");
        let validated = match &free {
            Some(_) => admit::validate(&env, &running, &sha, kernel_gate::evo::ExtendMode::Free),
            None => stages::validate(&env, &running, &sha),
        }
        .map_err(validation_error)?;
        let (mut scope, dep_change) = stages::scope(&env, &running, &sha).map_err(|m| err(ErrorCode::Internal, format!("scope: {m}")))?;
        if free.is_some() {
            scope = scope.scope.decision_free(); // no G4 on any scope; G3 after going live (A3)
        }
        Ok(stages::Fetched { sha, running, branch: r.to_string(), validated, scope, dep_change })
    })();
    let f = match pre {
        Ok(f) => f,
        Err(e) => {
            let _ = git.run(&["update-ref", "-d", &cand_ref], None, std::time::Duration::from_secs(30));
            release();
            return Err(e);
        }
    };
    job.progress(JobState::Gating, Some(Stage::Build), 0, "queued");
    let sha = f.sha.clone();
    let env2 = env.clone();
    std::thread::Builder::new()
        .name(format!("promote-{jid}"))
        .spawn(move || {
            let slot = Slot(&env2);
            match free {
                Some(link) => run_free(&env2, f, job.as_ref(), slot, link),
                None => run_promote(&env2, f, job.as_ref(), slot),
            }
            let _ = env2.git().run(&["update-ref", "-d", &format!("refs/init/candidates/{jid}")], None, std::time::Duration::from_secs(30));
        })
        .map_err(|e| {
            release();
            err(ErrorCode::Internal, format!("job thread: {e}"))
        })?;
    Ok(json!({"sha": sha}))
}

/// `promote --check` (GBAL rerun G7): promote's commit checks on `evo/<id>` and nothing else. The same fetch, the
/// candidate checks against lineage `main`, the same validator under the same `extend.mode` (the free path's matrix or
/// the gated one) and the scope decision, so a commit that passes here passes promote's synchronous part as it stands
/// now. Nothing is promoted: no job, no pipeline slot, no gate, no record; the fetched candidate ref is deleted again.
/// The agent runs it before it files its `goal predict`, so an amend never makes a filed prediction stale.
pub fn check(env: Arc<Env>, args: &PromoteArgs) -> Result<Value, ErrorBody> {
    let Some(r) = args.git_ref.as_deref() else { return Err(err(ErrorCode::Args, "promote --check takes {\"ref\":\"evo/<id>\",\"check\":true}")) };
    let Some(id) = r.strip_prefix("evo/") else { return Err(err(ErrorCode::Args, "ref must be evo/<id>")) };
    let git = env.git();
    if !git.validate_evo_id(id) {
        return Err(err(ErrorCode::Args, format!("evo id `{id}` must match ^[a-z0-9][a-z0-9._-]{{0,62}}$")));
    }
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = format!("refs/init/checks/{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst));
    let free = env.free_link().is_some();
    let res = (|| -> Result<Value, ErrorBody> {
        let sha = stages::fetch(&env, &env.home.join("body"), r, &tmp).map_err(coded)?;
        let running = git.rev_parse_commit("refs/heads/main").ok_or_else(|| err(ErrorCode::Internal, "lineage has no refs/heads/main"))?;
        git.check_candidate("refs/heads/main", &sha, "refs/upstream/").map_err(coded)?;
        let validated = match free {
            true => admit::validate(&env, &running, &sha, kernel_gate::evo::ExtendMode::Free),
            false => stages::validate(&env, &running, &sha),
        }
        .map_err(validation_error)?;
        let (mut scope, _) = stages::scope(&env, &running, &sha).map_err(|m| err(ErrorCode::Internal, format!("scope: {m}")))?;
        if free {
            scope = scope.scope.decision_free();
        }
        Ok(json!({"check": true, "mode": if free { "free" } else { "gated" }, "ok": true, "parent": running, "ref": r,
            "scope": scope.scope.as_str(), "sha": sha, "stages": scope.stages, "change_type": validated.change_type, "summary": validated.summary}))
    })();
    let _ = git.run(&["update-ref", "-d", &tmp], None, std::time::Duration::from_secs(30));
    res
}

/// The validator's problems as promote reports them: the first code, and every problem in `details`.
fn validation_error(errs: Vec<kernel_gate::evo::ValidationError>) -> ErrorBody {
    let first = errs.first().map(|e| e.code.clone()).unwrap_or_else(|| "evo.io".into());
    let mut e = err(ErrorCode::Other(first.clone()), format!("{} problem(s) in the commit; first: {first}", errs.len()));
    e.details = errs.iter().map(|x| ErrorDetail { code: x.code.clone(), detail: x.detail.clone() }).collect();
    e
}

/// G0–G3 on `sha` against `running` (shared by promote and `gate --quick`).
struct Gated {
    built: Built,
    g1: G1Out,
}

#[allow(clippy::too_many_arguments)]
fn g0_to_g3(env: &Env, job: &dyn Job, rec: &mut Record, sha: &str, running: &str, v: &kernel_gate::evo::Validated, with_g2: bool, with_g3: bool) -> Result<Gated, StageFail> {
    let jid = job.id();
    let _slot = env.slot_lock.lock().unwrap_or_else(|e| e.into_inner());
    let t = Instant::now();
    job.progress(JobState::Gating, Some(Stage::Build), 5, "export and build");
    // ADR-011 A1: on the free path clippy and the body's tests are advisory (A3 may veto later).
    let built = stages::build_and_g0(env, &jid, sha, running, &job_cache(env, &jid), env.free_link().is_some());
    let built = match built {
        Ok(b) => b,
        Err(f) => {
            rec.stage("G0", f.record.clone());
            job.stage_done(stage_result(f.stage.clone(), false, t.elapsed().as_millis() as u64, Some(&f.code)));
            return Err(f);
        }
    };
    rec.stage("G0", built.record.clone());
    rec.notice(env, "G0", Some(true), None, None);
    job.stage_done(stage_result(Stage::G0, true, t.elapsed().as_millis() as u64, None));
    job.progress(JobState::Gating, Some(Stage::G1), 40, "self-map");
    let t = Instant::now();
    let src = crate::build::BuildSlot::new(&env.home).src();
    let g1 = match stages::g1(env, &jid, &src, &built, v, running) {
        Ok(g) => g,
        Err(mut f) => {
            f.record = json!({"pass": false, "reason": f.reason()});
            rec.stage("G1", f.record.clone());
            job.stage_done(stage_result(Stage::G1, false, t.elapsed().as_millis() as u64, Some(&f.code)));
            return Err(f);
        }
    };
    rec.stage("G1", g1.record.clone());
    rec.notice(env, "G1", Some(true), None, None);
    job.stage_done(stage_result(Stage::G1, true, t.elapsed().as_millis() as u64, None));
    if with_g2 {
        job.progress(JobState::Gating, Some(Stage::G2), 45, "boot probes");
        let t = Instant::now();
        let cand = Candidate { sha: sha.into(), binary: built.binary.clone(), ext_set_dir: None };
        let parent = Candidate { sha: running.into(), binary: env.home.join("bin/init-body"), ext_set_dir: None };
        let o = stages::g2(env, &jid, &jid, &cand, &parent, &g1, v.trailers.prefix_change);
        let mut r = o.record.clone();
        r["wall_ms"] = json!(t.elapsed().as_millis() as u64);
        rec.stage("G2", r.clone());
        if !o.pass {
            let code = o.code().unwrap_or("gate.g2.probe_exit").to_string();
            rec.stage("G2", {
                let mut r = r;
                r["reason"] = json!(code.rsplit('.').next());
                r
            });
            job.stage_done(stage_result(Stage::G2, false, t.elapsed().as_millis() as u64, Some(&code)));
            let mut f = StageFail::new(Stage::G2, code, o.detail());
            f.record = rec.v["stages"]["G2"].clone();
            return Err(f);
        }
        rec.notice(env, "G2", Some(true), None, None);
        job.stage_done(stage_result(Stage::G2, true, t.elapsed().as_millis() as u64, None));
    }
    if with_g3 {
        job.progress(JobState::Gating, Some(Stage::G3), 60, "scripted self-edit");
        let t = Instant::now();
        let cand = Candidate { sha: sha.into(), binary: built.binary.clone(), ext_set_dir: None };
        match stages::g3(env, &jid, &cand, &built, v.trailers.prefix_change) {
            Ok(o) => {
                rec.stage("G3", o.record);
                let _ = env.git().run(&["update-ref", "-d", &format!("refs/init/g3/{jid}")], None, std::time::Duration::from_secs(30));
                rec.notice(env, "G3", Some(true), None, None);
                job.stage_done(stage_result(Stage::G3, true, t.elapsed().as_millis() as u64, None));
            }
            Err(f) => {
                rec.stage("G3", f.record.clone());
                let _ = env.git().run(&["update-ref", "-d", &format!("refs/init/g3/{jid}")], None, std::time::Duration::from_secs(30));
                job.stage_done(stage_result(Stage::G3, false, t.elapsed().as_millis() as u64, Some(&f.code)));
                return Err(f);
            }
        }
    }
    Ok(Gated { built, g1 })
}

/// Record a failed stage: verdict, report, lineage, note, notice, terminal failure.
fn fail_job(env: &Env, job: &dyn Job, rec: &mut Record, f: &StageFail) {
    rec.failure_text = Some(f.detail.clone());
    let verdict = if f.code == "gate.g2.parent_probe" { "error" } else { "reject" };
    rec.verdict(verdict, f.code.rsplit('.').next().unwrap_or(&f.code));
    let report = rec.write_report(env);
    let sha = rec.sha();
    if let Err(e) = rec.publish(env, Some(&sha)) {
        crate::klog::log("gate", &format!("gate record for {}: {e}", job.id()));
    }
    rec.notice(env, &format!("{}", f.stage), Some(false), Some(f.reason()), None);
    job.fail(gate_failed(f, &report, env));
}

fn run_promote(env: &Arc<Env>, f: stages::Fetched, job: &dyn Job, slot: Slot<'_>) {
    let jid = job.id();
    env.gates.fetch_add(1, Ordering::SeqCst);
    let lkg = env.sup.snapshot().map(|s| s.lkg_sha).unwrap_or_else(|| env.git().rev_parse_commit("refs/heads/lkg").unwrap_or_default());
    let mut rec = Record::new(env, &jid, "promote", Some(f.scope.scope.as_str()), &f.sha, Some(&f.branch), Some(&f.validated), &f.running, &lkg);
    // TUI-OBSERVE: the owner's gate track starts here (owner channel only; the body gets nothing).
    env.sup.owner_event("gate_start", json!({"gate_id": jid, "gate_kind": "promote", "sha": f.sha, "parent": f.running, "scope": f.scope.scope.as_str(), "steps": crate::supervisor::gate_steps(&f.scope)}));
    if env.proxy.open_gate(&jid, "promote", env.gate.gate_musd).is_err() {
        crate::klog::log("gate", &format!("{jid}: open_gate refused"));
    }
    let stages_on = f.scope.stages;
    let with_g2 = stages_on.contains(&"G2");
    let with_g3 = stages_on.contains(&"G3");
    let gated = match g0_to_g3(env, job, &mut rec, &f.sha, &f.running, &f.validated, with_g2, with_g3) {
        Ok(g) => g,
        Err(sf) => {
            env.proxy.close_gate(&jid);
            fail_job(env, job, &mut rec, &sf);
            cleanup(env, &jid);
            return;
        }
    };
    // Synchronous G4 (dep, t0_escalated): before the handoff (F7).
    if f.scope.needs_g4 == G4Need::Sync {
        job.progress(JobState::Gating, Some(Stage::G4), 80, "synchronous G4");
        let c = ArmSpec { sha: f.sha.clone(), binary: gated.built.binary.clone() };
        let p = ArmSpec { sha: lkg.clone(), binary: env.home.join("bin/init-body.lkg") };
        let (o, escalated) = g4_resolved(env, &jid, &f.validated, c, p, gated.g1.body_tokens, &lkg);
        rec.stage("G4", kernel_gate::g4::stage_record(&o, env.level >= 3));
        rec.v["escalated"] = json!(escalated);
        if !matches!(o.verdict, Verdict::Accept(_)) {
            let code = o.verdict.code().unwrap_or_else(|| "gate.g4.infra".into());
            let note = if o.note.is_empty() { String::new() } else { format!("\n{}", o.note) };
            let mut sf = StageFail::new(Stage::G4, code, format!("synchronous G4: {} {}{note}", o.verdict.kind(), o.verdict.reason()));
            sf.record = rec.v["stages"]["G4"].clone();
            env.proxy.close_gate(&jid);
            if o.verdict == Verdict::Reject(accept::RejectReason::Canary) {
                quarantine(env, &f.sha, &jid);
            }
            fail_job(env, job, &mut rec, &sf);
            cleanup(env, &jid);
            return;
        }
    }
    if !f.scope.needs_handoff {
        run_data_only(env, &f, &gated, job, &mut rec, slot);
        return;
    }
    // Stage the candidate binary (§3.6.2 "Staging the binary"), then wait for the boundary.
    let staged = match stage_binary(env, &gated.built) {
        Ok(s) => s,
        Err(e) => {
            env.proxy.close_gate(&jid);
            job.fail(err(ErrorCode::Internal, format!("staging bin/init-body.staged: {e}")));
            cleanup(env, &jid);
            return;
        }
    };
    job.progress(JobState::AwaitingBoundary, Some(Stage::Pause), 90, "awaiting the step boundary");
    if !job.awaiting_boundary() {
        env.proxy.close_gate(&jid);
        let _ = std::fs::remove_file(env.home.join("bin/init-body.staged"));
        cleanup(env, &jid);
        return;
    }
    job.progress(JobState::HandingOff, Some(Stage::Handoff), 95, "overlapped handoff");
    let t = Instant::now();
    let h = env.sup.handoff(&jid, &f.sha, &staged, Some(f.branch.clone()));
    env.point("gate.after_update_ref_main");
    match h {
        Ok(()) => {
            job.stage_done(stage_result(Stage::Handoff, true, t.elapsed().as_millis() as u64, None));
            let sync_done = f.scope.needs_g4 == G4Need::Sync;
            if sync_done {
                // dep: G4 already passed; the record goes to the Root before LKG.
                rec.verdict("accept", rec.v["stages"]["G4"]["reason"].as_str().unwrap_or("accept").to_string().as_str());
                rec.v["transition"] = json!({"from": "gating", "to": "lkg"});
                match rec.publish(env, Some(&f.sha)) {
                    Ok(_) => {
                        let _ = env.sup.g4_pass(&f.sha, &jid);
                    }
                    Err(e) => crate::klog::log("gate", &format!("{jid}: lineage before LKG: {e}")),
                }
            } else {
                rec.verdict("accept", "g0_g3");
                rec.v["transition"] = json!({"from": "gating", "to": "provisional"});
                let _ = rec.publish(env, Some(&f.sha));
            }
            env.proxy.close_gate(&jid);
            let stages: Vec<StageResult> = Vec::new();
            job.succeed(json!({"job": jid, "sha": f.sha, "state": "succeeded", "scope": f.scope.scope.as_str(), "stages": stages}));
            cleanup(env, &jid);
            drop(slot); // a G4 may overlap the next G0-G3 pipeline (gate.md §3.1)
            if !sync_done && f.scope.needs_g4 == G4Need::Async {
                let g4job = env.jobs.create("g4", json!({"sha": f.sha, "promote_job": jid}));
                let (env, sha, v, bt) = (env.clone(), f.sha.clone(), f.validated.clone(), gated.g1.body_tokens);
                std::thread::spawn(move || run_g4_job(&env, g4job.as_ref(), &sha, &v, bt, false));
            }
        }
        Err(e) => {
            env.proxy.close_gate(&jid);
            let _ = std::fs::remove_file(env.home.join("bin/init-body.staged"));
            job.stage_done(stage_result(Stage::Handoff, false, t.elapsed().as_millis() as u64, Some("handoff_failed")));
            rec.verdict("reject", "handoff_failed");
            rec.failure_text = Some(e.clone());
            let _ = rec.publish(env, Some(&f.sha));
            let code = if e.contains("pause") { ErrorCode::PauseTimeout } else { ErrorCode::HandoffFailed };
            job.fail(err(code, e));
            cleanup(env, &jid);
        }
    }
}

/// supervise.md §3.6.8 (T0/T1 data; evolution.md §3.2): no pause and no handoff. The kept set the
/// gate built is published first (t1), then supervise moves `main` and either commits LKG at once
/// (t0 after G0–G1; t0_escalated after its synchronous G4 judged the whole tree against LKG) or
/// makes the version provisional, after which the asynchronous G4 runs (t1). A t0 on top of a
/// provisional chain must not skip that chain's G4: it stays provisional and G4 judges the head.
fn run_data_only(env: &Arc<Env>, f: &stages::Fetched, gated: &Gated, job: &dyn Job, rec: &mut Record, slot: Slot<'_>) {
    let jid = job.id();
    let provisional = env.sup.snapshot().is_some_and(|s| s.provisional);
    let sync_passed = f.scope.needs_g4 == G4Need::Sync;
    let mut lkg_now = sync_passed || (f.scope.needs_g4 == G4Need::None && !provisional);
    job.progress(JobState::HandingOff, Some(Stage::Handoff), 95, "data-only promotion (no handoff)");
    let bin = env.home.join("bin");
    let ext = match crate::extset::publish(&bin, &gated.built.components) {
        Ok(Some(set)) if crate::extset::target(&bin, "ext").as_deref() != Some(set.id.as_str()) => Some(set),
        Ok(_) => None,
        Err(e) => {
            env.proxy.close_gate(&jid);
            rec.verdict("error", "ext_publish");
            let _ = rec.publish(env, Some(&f.sha));
            job.fail(err(ErrorCode::Internal, format!("publishing the kept set to bin/ext.d: {e}")));
            cleanup(env, &jid);
            return;
        }
    };
    let reason = if sync_passed { rec.v["stages"]["G4"]["reason"].as_str().unwrap_or("accept").to_string() } else if f.scope.stages.contains(&"G3") { "g0_g3".to_string() } else { "g0_g1".to_string() };
    rec.verdict("accept", &reason);
    if lkg_now {
        // Lineage before LKG (§3.12): the gate record is acknowledged before supervise commits.
        rec.v["transition"] = json!({"from": "gating", "to": "lkg"});
        if let Err(e) = rec.publish(env, Some(&f.sha)) {
            // G-46: keep the version provisional; provisional_watch resubmits, then commits LKG.
            crate::klog::log("gate", &format!("{jid}: lineage before LKG: {e}"));
            pending(env, &jid, &f.sha, rec);
            alert(env, "lineage_unavailable", &format!("gate record {jid} not acknowledged: {e}"), &jid);
            lkg_now = false;
        }
    }
    let t = Instant::now();
    let p = crate::supervisor::DataOnly { job: jid.clone(), sha: f.sha.clone(), from: f.running.clone(), scope: f.scope.scope.as_str().to_string(), lkg: lkg_now, ext };
    match env.sup.data_only(p) {
        Ok(()) => {
            crate::klog::log("gate", &format!("{jid}: data-only promotion in {} ms", t.elapsed().as_millis()));
            if !lkg_now && rec.v["transition"].is_null() {
                rec.v["transition"] = json!({"from": "gating", "to": "provisional"});
                let _ = rec.publish(env, Some(&f.sha));
            }
            env.proxy.close_gate(&jid);
            let stages: Vec<StageResult> = Vec::new();
            job.succeed(json!({"job": jid, "sha": f.sha, "state": "succeeded", "scope": f.scope.scope.as_str(), "stages": stages, "handoff": false, "lkg": lkg_now}));
            cleanup(env, &jid);
            drop(slot);
            let pending_record = env.home.join("vault/gate/pending").join(format!("{jid}.json")).exists();
            if !lkg_now && !pending_record {
                let g4job = env.jobs.create("g4", json!({"sha": f.sha, "promote_job": jid}));
                let (env, sha, v, bt) = (env.clone(), f.sha.clone(), f.validated.clone(), gated.g1.body_tokens);
                std::thread::spawn(move || run_g4_job(&env, g4job.as_ref(), &sha, &v, bt, false));
            }
        }
        Err(e) => {
            env.proxy.close_gate(&jid);
            let _ = std::fs::remove_file(env.home.join("vault/gate/pending").join(format!("{jid}.json")));
            rec.verdict("error", "data_only_failed");
            rec.failure_text = Some(e.clone());
            let _ = rec.publish(env, Some(&f.sha));
            let code = if e.contains("in progress") { ErrorCode::Busy } else { ErrorCode::Internal };
            job.fail(err(code, format!("data-only promotion: {e}")));
            cleanup(env, &jid);
        }
    }
}

/// The free path (ADR-011 §2.3; [`admit`]): A1 and A2 here, A0, A4 and A5 at the Root through
/// `change.open`, then the change goes live (handoff or data-only reload) as an open version; the
/// gate record is v5 (`accept:"free"`), and A3 runs asynchronously. `lkg` follows the Root's
/// `change_close` (supervisor `extend.window`), never a G4.
fn run_free(env: &Arc<Env>, f: stages::Fetched, job: &dyn Job, slot: Slot<'_>, link: Arc<dyn ExtendLink>) {
    let jid = job.id();
    env.gates.fetch_add(1, Ordering::SeqCst);
    let scope = f.scope.scope;
    let (needs_handoff, cs) = (f.scope.needs_handoff, scope.change_scope());
    let lkg = env.sup.snapshot().map(|s| s.lkg_sha).unwrap_or_else(|| env.git().rev_parse_commit("refs/heads/lkg").unwrap_or_default());
    let mut rec = Record::new(env, &jid, "promote", Some(cs), &f.sha, Some(&f.branch), Some(&f.validated), &f.running, &lkg);
    rec.free();
    let note = |extra: Value| {
        let mut n = json!({"job": jid});
        if let (Some(o), Some(x)) = (n.as_object_mut(), extra.as_object()) {
            o.extend(x.clone());
        }
        n
    };
    admit::change_state(&env.home, env.lineage.as_ref(), &f.sha, "opening", note(json!({})));
    let back = |why: &str| crate::recovery::proposal_event(&env.home, env.lineage.as_ref(), &f.sha, &ProposalEv::NoVerdict { reason: why.into() });
    // A1 (G0, G1) and A2 (G2), kernel-run: quality filters, not the guarantee (§2.1).
    let with_g0 = f.scope.stages.contains(&"G0");
    let gated = if with_g0 {
        let _ = env.proxy.open_gate(&jid, "promote", env.gate.gate_musd);
        let g = g0_to_g3(env, job, &mut rec, &f.sha, &f.running, &f.validated, f.scope.stages.contains(&"G2"), false);
        env.proxy.close_gate(&jid);
        match g {
            Ok(g) => Some(g),
            Err(sf) => {
                rec.free_stages(&[]);
                fail_job(env, job, &mut rec, &sf);
                cleanup(env, &jid);
                back("a1_a2_failed");
                return;
            }
        }
    } else {
        None
    };
    // What will run: the candidate binary (t2, dep) or the running one; the published kept set (t1)
    // or the running set.
    let bin = env.home.join("bin");
    let mut published = None;
    if let (kernel_gate::scope::Scope::T1, Some(g)) = (scope, gated.as_ref()) {
        match crate::extset::publish(&bin, &g.built.components) {
            Ok(set) => published = set,
            Err(e) => {
                rec.verdict("error", "ext_publish");
                let _ = rec.publish(env, Some(&f.sha));
                job.fail(err(ErrorCode::Internal, format!("publishing the kept set to bin/ext.d: {e}")));
                cleanup(env, &jid);
                back("ext_publish");
                return;
            }
        }
    }
    let running_set = crate::extset::target(&bin, "ext").and_then(|id| crate::extset::components_of(&bin.join("ext.d").join(&id)).ok().map(|c| crate::extset::ExtSet { id, components: c }));
    let live_set = published.clone().or(running_set);
    let binary = match (&gated, needs_handoff) {
        (Some(g), true) => g.built.binary.clone(),
        _ => bin.join("init-body"),
    };
    job.progress(JobState::AwaitingRoot, Some(Stage::Admission), 85, "change.open: A0, A4 and A5 at the Root");
    let t = Instant::now();
    let cand = admit::Candidate {
        job: &jid,
        sha: &f.sha,
        from_sha: &f.running,
        scope,
        binary: &binary,
        ext: live_set.as_ref().map(|s| (s.id.as_str(), s.components.as_slice())),
        validated: &f.validated,
        cache: &job_cache(env, &jid),
    };
    let a = match admit::open(env, link.as_ref(), &cand) {
        Ok(a) => a,
        Err(e) => {
            let root_code = e.data.as_ref().and_then(|d| d["root_code"].as_str()).unwrap_or(e.code.as_str()).to_string();
            rec.free_stages(&[]);
            rec.stage("admission", json!({"pass": false, "code": root_code, "wall_ms": t.elapsed().as_millis() as u64}));
            rec.failure_text = Some(e.message.clone());
            rec.verdict("reject", root_code.rsplit('.').next().unwrap_or(&root_code));
            rec.write_report(env);
            let _ = rec.publish(env, Some(&f.sha));
            job.stage_done(stage_result(Stage::Admission, false, t.elapsed().as_millis() as u64, Some(&root_code)));
            job.fail(e);
            cleanup(env, &jid);
            back("admission_refused");
            return;
        }
    };
    job.stage_done(stage_result(Stage::Admission, true, t.elapsed().as_millis() as u64, None));
    rec.v["group"] = json!(a.group);
    rec.v["change_seq"] = json!(a.seq);
    rec.v["snapshot"] = json!(a.snapshot);
    rec.v["t0"] = json!(a.t0);
    rec.free_stages(&["A0", "A5"]);
    admit::change_state(&env.home, env.lineage.as_ref(), &f.sha, "open", note(json!({"group": a.group, "seq": a.seq})));
    // Go live: the overlapped handoff (t2, dep) or the data-only reload (t0, t1).
    let live = if needs_handoff {
        let g = gated.as_ref().expect("t2 and dep run A1");
        match stage_binary(env, &g.built) {
            Err(e) => Err((ErrorCode::Internal, format!("staging bin/init-body.staged: {e}"))),
            Ok(staged) => {
                job.progress(JobState::AwaitingBoundary, Some(Stage::Pause), 90, "awaiting the step boundary");
                if !job.awaiting_boundary() {
                    let _ = std::fs::remove_file(env.home.join("bin/init-body.staged"));
                    Err((ErrorCode::Cancelled, "the job was cancelled before the handoff".to_string()))
                } else {
                    job.progress(JobState::HandingOff, Some(Stage::Handoff), 95, "overlapped handoff");
                    let t = Instant::now();
                    let h = env.sup.handoff(&jid, &f.sha, &staged, Some(f.branch.clone()));
                    env.point("gate.after_update_ref_main");
                    job.stage_done(stage_result(Stage::Handoff, h.is_ok(), t.elapsed().as_millis() as u64, h.as_ref().err().map(|_| "handoff_failed")));
                    h.map_err(|e| {
                        let _ = std::fs::remove_file(env.home.join("bin/init-body.staged"));
                        (if e.contains("pause") { ErrorCode::PauseTimeout } else { ErrorCode::HandoffFailed }, e)
                    })
                }
            }
        }
    } else {
        job.progress(JobState::HandingOff, Some(Stage::Handoff), 95, "data-only promotion (no handoff)");
        let ext = published.filter(|s| crate::extset::target(&bin, "ext").as_deref() != Some(s.id.as_str()));
        let p = crate::supervisor::DataOnly { job: jid.clone(), sha: f.sha.clone(), from: f.running.clone(), scope: cs.to_string(), lkg: false, ext };
        env.sup.data_only(p).map_err(|e| (if e.contains("in progress") { ErrorCode::Busy } else { ErrorCode::Internal }, format!("data-only promotion: {e}")))
    };
    if let Err((code, e)) = live {
        // Admitted but never live: the zone asks the Root to close the group (H12).
        if let Err(v) = admit::veto(env, a.group, &format!("not live: {}", kernel_gate::record::cap(&e, 150))) {
            crate::klog::log("gate", &format!("{jid}: change.veto group {}: {v}", a.group));
        }
        rec.verdict("reject", "not_live");
        rec.failure_text = Some(e.clone());
        let _ = rec.publish(env, Some(&f.sha));
        job.fail(err(code, e));
        cleanup(env, &jid);
        back("not_live");
        return;
    }
    rec.verdict("accept", "free");
    rec.v["transition"] = json!({"from": "gating", "to": "open"});
    if let Err(e) = rec.publish(env, Some(&f.sha)) {
        crate::klog::log("gate", &format!("{jid}: gate record: {e}"));
    }
    let stages: Vec<StageResult> = Vec::new();
    job.succeed(json!({"job": jid, "sha": f.sha, "state": "succeeded", "scope": cs, "stages": stages, "handoff": needs_handoff, "lkg": false,
                       "accept": "free", "group": a.group, "seq": a.seq, "snapshot": a.snapshot, "window": a.window}));
    drop(slot); // the next promote may start while A3 runs
    match gated {
        Some(g) => {
            let g0 = rec.v["stages"]["G0"].clone();
            admit::a3_after_live(env, admit::A3Job { job: jid.clone(), group: a.group, sha: f.sha.clone(), scope, built: Some(g.built), validated: f.validated.clone(), g0 });
        }
        None => cleanup(env, &jid),
    }
}

fn stage_binary(env: &Env, b: &Built) -> std::io::Result<String> {
    let bin = env.home.join("bin");
    let tmp = bin.join(".init-body.staged.tmp");
    let _ = std::fs::remove_file(&tmp);
    std::fs::copy(&b.binary, &tmp)?;
    std::fs::File::open(&tmp)?.sync_all()?;
    let sha = crate::fsx::sha256_file(&tmp)?;
    std::fs::rename(&tmp, bin.join("init-body.staged"))?;
    crate::fsx::sync_dir(&bin)?;
    Ok(sha)
}

/// [`cleanup`] for the free path's A3 thread, which outlives the promote job.
pub(crate) fn cleanup_job(env: &Env, jid: &str) {
    cleanup(env, jid)
}

/// `cache/gate/<job>/` is deleted by exact path when the job no longer needs its binary.
fn cleanup(env: &Env, jid: &str) {
    let d = job_cache(env, jid);
    if d.starts_with(env.home.join("cache/gate")) && d != env.home.join("cache/gate") {
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// F14: `refs/init/quarantine/<sha>`, an alert line.
fn quarantine(env: &Env, sha: &str, jid: &str) {
    let git = env.git();
    if let Err(e) = git.update_ref(&format!("refs/init/quarantine/{sha}"), sha, Some(""), Some(&format!("init: canary trip in {jid}"))) {
        crate::klog::log("gate", &format!("quarantine ref for {sha}: {e}"));
    }
    alert(env, "canary", &format!("a canary scored 1000 for {sha}"), jid);
}

/// Build the G4 plan: the frozen sets, selection, budget and history.
fn plan(env: &Env, gate_id: &str, v: &kernel_gate::evo::Validated, body_tokens_c: u64, body_tokens_p: u64, repeats: u32, all_tasks: bool) -> G4Plan {
    let pool = g4run::load_set(env, "vault/gate/pool", RunSet::Pool);
    let holdout = g4run::load_set(env, "vault/holdout", RunSet::Holdout);
    let canaries = g4run::load_set(env, "vault/canaries", RunSet::Canary);
    let pool_tasks: Vec<kernel_gate::tasks::PoolTask> = pool.iter().map(|t| kernel_gate::tasks::PoolTask { id: t.id.clone(), labels: t.labels.clone(), source: t.source, family: t.family.clone() }).collect();
    let params = kernel_gate::tasks::SelectParams { pool_k: if all_tasks { pool.len().max(1) } else { env.gate.pool_k }, min_pool: env.gate.g4.min_pool, agent_stratum_max: env.gate.agent_stratum_max, max_repro_tasks: 5 };
    let pick = kernel_gate::tasks::select_pool(gate_id, &pool_tasks, &Default::default(), &[], params);
    let h_ids: Vec<String> = holdout.iter().map(|t| t.id.clone()).collect();
    let h_pick = kernel_gate::tasks::select_holdout(&h_ids, if all_tasks { h_ids.len().max(1) } else { env.gate.holdout_h }, env.gates.load(Ordering::SeqCst));
    let k_ids: Vec<String> = canaries.iter().map(|t| t.id.clone()).collect();
    let k_pick = kernel_gate::tasks::select_canaries(gate_id, &k_ids, env.gate.canaries);
    let mut tasks: Vec<_> = pool.into_iter().filter(|t| pick.contains(&t.id)).collect();
    tasks.extend(holdout.into_iter().filter(|t| h_pick.contains(&t.id)));
    let canaries: Vec<_> = canaries.into_iter().filter(|t| k_pick.contains(&t.id)).collect();
    let history = std::fs::read_to_string(env.home.join("vault/gate/noise.json")).ok().and_then(|t| NoiseHistory::parse(&t).ok());
    let now_ms = crate::clock::now_ms();
    G4Plan {
        gate_id: gate_id.into(),
        tasks,
        canaries,
        repeats,
        repeats_max: env.gate.repeats_max.max(repeats),
        parallel: env.gate.parallel,
        config: env.gate.g4.clone(),
        futility_alpha: env.gate.futility_alpha,
        noise_history_days: env.gate.noise_history_days,
        budget: CostBudget { soft_day_musd: env.gate.soft_day_musd, evolution_share_permille: env.gate.evolution_share_permille, tasks_per_day: 10.0 },
        change_type: v.change_type,
        capability_labels: v.capability_labels.clone(),
        body_tokens_c,
        body_tokens_p,
        history,
        model: env.model.clone(),
        now_unix: now_ms / 1000,
        now_rfc3339: crate::clock::now_rfc3339(),
        wall_s: env.gate.wall_s,
        request_reservation_musd: env.proxy.request_reservation_musd(),
    }
}

/// `body_tokens` of the LKG from its committed `self.json` (deterministic, G1's measure).
fn lkg_body_tokens(env: &Env, lkg: &str) -> u64 {
    let git = env.git();
    git.path_id(lkg, "self.json").ok().flatten().and_then(|o| git.read_blob(&o).ok()).and_then(|b| serde_json::from_slice::<Value>(&b).ok()).map_or(0, |r| self_json_body_tokens(&r))
}

/// `body_tokens` is a top-level field of the committed `self.json` (selfmap.md §4.1). Reading a nested
/// `budget.body_tokens` made every LKG look 0 tokens, so the `smaller` accept could never fire (SELFJSON).
fn self_json_body_tokens(r: &Value) -> u64 {
    r["body_tokens"].as_u64().unwrap_or(0)
}


/// One G4 run, plus the noise store and run records. M7: G4 v2 on the Root's sealed slice when the
/// candidate is eligible (promote/g4v2.rs); otherwise, or with no S (`root.sealed_none`), the v0.3
/// kernel path below, whose record says `sealed: null` and why.
#[allow(clippy::too_many_arguments)]
fn g4_once(env: &Env, gate_id: &str, v: &kernel_gate::evo::Validated, c: ArmSpec, p: ArmSpec, body_tokens_c: u64, lkg: &str, repeats: u32, escalated: bool) -> G4Outcome {
    let body_tokens_p = lkg_body_tokens(env, lkg);
    let job = super::g4v2::V2Job { gate_id, v, c: c.clone(), p: p.clone(), body_tokens_c, body_tokens_p, lkg, escalated };
    let fallback = match super::g4v2::g4_v2(env, &job) {
        super::g4v2::V2Path::Ran(o) => {
            g4run::write_runs(env, gate_id, &o.runs);
            super::g4v2::write_last(env, gate_id, &o);
            return *o;
        }
        super::g4v2::V2Path::Fallback(why) => why,
    };
    let mut o = g4_v03(env, gate_id, v, c, p, body_tokens_c, body_tokens_p, repeats, escalated);
    o.v2 = Some(kernel_gate::g4v2::V2Record { sealed_refusal: Some(kernel_gate::record::cap(&fallback, 300)), accept_class: kernel_gate::g4v2::accept_class(&o.verdict), ..Default::default() });
    super::g4v2::write_last(env, gate_id, &o);
    o
}

/// The v0.3 kernel path (gate.md §3.7–§3.9): the gate's own pool, holdout and canaries.
#[allow(clippy::too_many_arguments)]
fn g4_v03(env: &Env, gate_id: &str, v: &kernel_gate::evo::Validated, c: ArmSpec, p: ArmSpec, body_tokens_c: u64, body_tokens_p: u64, repeats: u32, escalated: bool) -> G4Outcome {
    let plan = plan(env, gate_id, v, body_tokens_c, body_tokens_p, repeats, escalated);
    let ex = ProbeRuns { env, job: gate_id.into(), c, p };
    let mid = || env.point("gate.g4.mid");
    let o = kernel_gate::g4::run_g4(&plan, &ex, &mid);
    g4run::write_runs(env, gate_id, &o.runs);
    if !o.noise_store.is_empty() {
        let mut h = plan.history.clone().unwrap_or_default();
        let m = h.models.entry(env.model.clone()).or_default();
        for (k, e) in &o.noise_store {
            m.insert(k.clone(), e.clone());
        }
        let _ = crate::fsx::write_atomic(&env.home.join("vault/gate/noise.json"), h.to_json().as_bytes());
    }
    o
}

/// G4 with the mechanical resolution of §3.12 (synchronous scopes): an inconclusive verdict gets
/// exactly one escalated re-run; still inconclusive → `reject: inconclusive_final`.
fn g4_resolved(env: &Env, gate_id: &str, v: &kernel_gate::evo::Validated, c: ArmSpec, p: ArmSpec, body_tokens_c: u64, lkg: &str) -> (G4Outcome, bool) {
    let o = g4_once(env, gate_id, v, c.clone(), p.clone(), body_tokens_c, lkg, env.gate.repeats, false);
    match accept::resolve(o.verdict, false, env.gate.escalate_repeats) {
        Resolution::Escalate { repeats } => {
            let mut o2 = g4_once(env, &format!("{gate_id}-x"), v, c, p, body_tokens_c, lkg, repeats, true);
            if let Resolution::Final(vf) = accept::resolve(o2.verdict, true, repeats) {
                o2.verdict = vf;
            }
            (o2, true)
        }
        Resolution::Final(vf) => {
            let mut o = o;
            o.verdict = vf;
            (o, false)
        }
    }
}

/// The asynchronous G4 job of a provisional `sha` (gate.md §3.12): candidate (`bin/init-body`) vs
/// LKG (`bin/init-body.lkg`). Accept → the gate record to Root lineage, then (only after the ack)
/// the LKG commit and the note. Reject → rollback at idle. Inconclusive → one escalated re-run
/// (a new job), then `inconclusive_final`. Error → retried ≤ `g4_infra_retries` (F12).
pub fn run_g4_job(env: &Env, job: &dyn Job, sha: &str, v: &kernel_gate::evo::Validated, body_tokens_c: u64, escalated: bool) {
    let jid = job.id();
    *env.g4_running.lock().unwrap() = Some(jid.clone());
    let lkg = env.sup.snapshot().map(|s| s.lkg_sha).unwrap_or_default();
    let running = env.sup.snapshot().map(|s| s.current_sha).unwrap_or_default();
    let mut rec = Record::new(env, &jid, "g4", None, sha, None, Some(v), &running, &lkg);
    rec.v["escalated"] = json!(escalated);
    let _ = env.proxy.open_gate(&jid, "g4", env.gate.gate_musd);
    job.progress(JobState::Running, Some(Stage::G4), 0, "G4");
    let c = ArmSpec { sha: sha.into(), binary: env.home.join("bin/init-body") };
    let p = ArmSpec { sha: lkg.clone(), binary: env.home.join("bin/init-body.lkg") };
    let repeats = if escalated { env.gate.escalate_repeats } else { env.gate.repeats };
    let o = g4_once(env, &jid, v, c, p, body_tokens_c, &lkg, repeats, escalated);
    env.proxy.close_gate(&jid);
    *env.g4_running.lock().unwrap() = None;
    rec.stage("G4", kernel_gate::g4::stage_record(&o, env.level >= 3));
    let score = o.eval.as_ref().and_then(|e| e.stats.score.map(|s| kernel_gate::record::permille(s.c)));
    let resolution = accept::resolve(o.verdict, escalated, env.gate.escalate_repeats);
    match resolution {
        Resolution::Final(Verdict::Accept(r)) => {
            let reason = Verdict::Accept(r).reason();
            if r == accept::AcceptReason::NoInstrument {
                // ADR-013 §6: kept on G0–G3 alone; the owner learns once per version that G4 measured nothing.
                rec.failure_text = Some(o.note.clone());
                alert(env, "g4_no_instrument", &format!("{sha}: G4 has no instrument (no sealed S, empty pool and holdout); \
                    the version becomes LKG on G0–G3 alone. Draw S (`init-root sealed draw`) to measure T1/T2 changes."), &jid);
            }
            rec.verdict("accept", reason);
            rec.v["transition"] = json!({"from": "provisional", "to": "lkg"});
            g4run::compact_results(env, &o.runs, true);
            rec.notice(env, "G4", Some(true), None, score);
            // Lineage before LKG (§3.12): no ack, no transition; resubmitted by provisional_watch.
            match rec.publish(env, Some(sha)) {
                Ok(_) => match env.sup.g4_pass(sha, &jid) {
                    // M7: the accept class (better | lineage_gain | tie_break:<which>) and the ratchet flag.
                    Ok(()) => {
                        crate::recovery::proposal_event(&env.home, env.lineage.as_ref(), sha, &ProposalEv::Verdict { verdict: "accept".into(), reason: reason.into() }); // X0b
                        job.succeed(json!({"job": jid, "sha": sha, "verdict": "accept", "reason": reason,
                                           "accept_class": kernel_gate::g4v2::accept_class(&o.verdict),
                                           "milestone_required": o.v2.as_ref().is_some_and(|x| x.milestone_required)}))
                    }
                    Err(e) => job.fail(err(ErrorCode::Root, format!("LKG commit: {e}"))),
                },
                Err(e) => {
                    pending(env, &jid, sha, &rec);
                    alert(env, "lineage_unavailable", &format!("gate record {jid} not acknowledged: {e}"), &jid);
                    job.fail(err(ErrorCode::Other("gate.lineage.unavailable".into()), e));
                }
            }
        }
        Resolution::Final(vf @ Verdict::Reject(r)) => {
            rec.verdict("reject", vf.reason());
            g4run::compact_results(env, &o.runs, false);
            if r == accept::RejectReason::Canary {
                quarantine(env, sha, &jid);
            }
            if r == accept::RejectReason::InconclusiveFinal {
                alert(env, "inconclusive_final", &format!("{sha} stayed inconclusive after the escalated re-run"), &jid);
            }
            let _ = rec.publish(env, Some(sha));
            rec.notice(env, "G4", Some(false), Some(vf.reason()), score);
            env.sup.g4_reject(sha, &jid, &format!("gate.g4.{}", vf.reason()));
            crate::recovery::proposal_event(&env.home, env.lineage.as_ref(), sha, &ProposalEv::Verdict { verdict: "reject".into(), reason: vf.reason().into() }); // X0b
            job.succeed(json!({"job": jid, "sha": sha, "verdict": "reject", "reason": vf.reason()}));
        }
        Resolution::Escalate { .. } => {
            rec.verdict("inconclusive", o.verdict.reason());
            let _ = rec.publish(env, Some(sha));
            rec.notice(env, "G4", None, Some(o.verdict.reason()), score);
            alert(env, "provisional_inconclusive", &format!("{sha}: {}; one escalated re-run", o.verdict.reason()), &jid);
            job.succeed(json!({"job": jid, "sha": sha, "verdict": "inconclusive", "reason": o.verdict.reason(), "escalated": true}));
            let j2 = env.jobs.create("g4", json!({"sha": sha, "escalated": true}));
            run_g4_job(env, j2.as_ref(), sha, v, body_tokens_c, true);
        }
        Resolution::Final(vf) => {
            // error: `infra` is retried ≤ g4_infra_retries (F12). Runs that never ran
            // (`runs_failed:<cause>`) and a task cap below one reservation (`task_cost_cap`) are
            // not: a re-run fails the same way. The incumbent stays LKG; the provisional expiry
            // (F16) decides; the note says what to fix (K-GATE).
            rec.verdict(vf.kind(), vf.reason());
            if !o.note.is_empty() {
                rec.failure_text = Some(o.note.clone());
            }
            let _ = rec.publish(env, Some(sha));
            let code = vf.code().unwrap_or_else(|| "gate.g4.infra".into());
            rec.notice(env, "G4", None, Some(&code), None);
            let retryable = matches!(vf, Verdict::Error(e) if e.retryable());
            if !retryable {
                alert(env, "g4_error", &format!("{sha}: {code}: {}", o.note), &jid);
            }
            let n = {
                let mut a = env.g4_attempts.lock().unwrap();
                let n = a.entry(sha.to_string()).or_insert(0);
                *n += 1;
                *n
            };
            job.succeed(json!({"job": jid, "sha": sha, "verdict": vf.kind(), "reason": vf.reason(), "note": o.note}));
            if retryable && n <= env.gate.g4_infra_retries {
                let j2 = env.jobs.create("g4", json!({"sha": sha, "retry": n}));
                run_g4_job(env, j2.as_ref(), sha, v, body_tokens_c, escalated);
            }
        }
    }
}

/// A gate record the Root has not acknowledged (`vault/gate/pending/<job>.json`).
fn pending(env: &Env, jid: &str, sha: &str, rec: &Record) {
    let d = env.home.join("vault/gate/pending");
    let _ = std::fs::create_dir_all(&d);
    let _ = crate::fsx::write_json(&d.join(format!("{jid}.json")), &json!({"job": jid, "sha": sha, "record": rec.v}));
}

fn alert(env: &Env, code: &str, message: &str, jid: &str) {
    // supervise.md §3.7.1 record; the Root copy is supervise's `alert` (rate-limited there).
    let line = json!({"v": 1, "ts": crate::clock::now_rfc3339(), "source": "gate", "code": code, "message": message, "job": jid});
    let _ = std::fs::create_dir_all(env.home.join("state/kernel"));
    let _ = crate::fsx::append_line(&env.home.join("state/kernel/alerts.jsonl"), &init_proto::icj::to_line(&line).unwrap_or_default());
}

/// The provisional watch (run every 60 s by the host): resubmit unacknowledged accept records and
/// apply the LKG transition once acknowledged (G-46); roll back a provisional version older than
/// `evolution.provisional_max_age_s` (F16). `age_s` is the provisional version's age.
pub fn provisional_watch(env: &Env, age_s: u64) -> Value {
    let mut done = Vec::new();
    let d = env.home.join("vault/gate/pending");
    if let Ok(rd) = std::fs::read_dir(&d) {
        for e in rd.flatten() {
            let Ok(b) = std::fs::read(e.path()) else { continue };
            let Ok(p) = serde_json::from_slice::<Value>(&b) else { continue };
            let (jid, sha) = (p["job"].as_str().unwrap_or(""), p["sha"].as_str().unwrap_or(""));
            let mut rec = Record { v: p["record"].clone(), failure_text: None };
            if rec.publish(env, Some(sha)).is_ok() {
                let _ = std::fs::remove_file(e.path());
                let r = env.sup.g4_pass(sha, jid);
                done.push(json!({"job": jid, "lkg": r.is_ok()}));
            }
        }
    }
    let mut rolled = false;
    if let Some(s) = env.sup.snapshot() {
        // ADR-011: on the free path an open version waits for the Root's window, never for a G4 (no F16).
        if s.provisional && env.free_link().is_none() && accept::provisional_expired(age_s, env.gate.provisional_max_age_s) && !d.exists_nonempty() {
            env.sup.g4_reject(&s.current_sha, "provisional_watch", "provisional_stale");
            crate::recovery::proposal_event(&env.home, env.lineage.as_ref(), &s.current_sha, &ProposalEv::NoVerdict { reason: "provisional_stale".into() }); // X0b
            alert(env, "provisional_stale", &format!("{} provisional for {age_s} s without a verdict", s.current_sha), "");
            rolled = true;
        }
    }
    json!({"resubmitted": done, "rolled_back": rolled})
}

trait NonEmpty {
    fn exists_nonempty(&self) -> bool;
}
impl NonEmpty for std::path::Path {
    fn exists_nonempty(&self) -> bool {
        std::fs::read_dir(self).map(|mut r| r.next().is_some()).unwrap_or(false)
    }
}

/// `gate --quick` (gate.md §3.1 kind `quick`): G0–G3, advisory, on a branch of `repo` (on a probe
/// lane G0–G2 only). The terminal result is a [`GateResult`] (`pass:false` is a success).
pub fn gate_quick(env: Arc<Env>, repo: &std::path::Path, rev: &str, on_probe_lane: bool, job: Arc<dyn Job>) -> Result<Value, ErrorBody> {
    let jid = job.id();
    let branch = rev.strip_prefix("refs/heads/").unwrap_or(rev).to_string();
    let dst = format!("refs/init/quick/{jid}");
    let sha = stages::fetch_rev(&env, repo, rev, &dst).map_err(coded)?;
    let running = env.git().rev_parse_commit("refs/heads/main").unwrap_or_default();
    let validated = stages::validate(&env, &running, &sha).ok();
    let env2 = env.clone();
    let sha2 = sha.clone();
    std::thread::spawn(move || {
        let env = env2;
        let v = validated.unwrap_or(kernel_gate::evo::Validated {
            sha: sha2.clone(),
            parent: running.clone(),
            change_type: kernel_gate::evo::ChangeType::Refactor,
            scope: None,
            summary: String::new(),
            trailers: Default::default(),
            changed_paths: vec![],
            capability_labels: vec![],
        });
        let mut rec = Record::new(&env, &jid, "quick", None, &sha2, Some(&branch), Some(&v), &running, "");
        let _ = env.proxy.open_gate(&jid, "quick", env.gate.quick_gate_musd);
        let r = g0_to_g3(&env, job.as_ref(), &mut rec, &sha2, &running, &v, true, !on_probe_lane);
        env.proxy.close_gate(&jid);
        let (pass, warnings) = match &r {
            Ok(_) => (true, vec![]),
            Err(f) => {
                rec.failure_text = Some(f.detail.clone());
                (false, vec![f.code.clone()])
            }
        };
        rec.verdict(if pass { "accept" } else { "reject" }, if pass { "quick_pass" } else { r.as_ref().err().map(|f| f.reason()).unwrap_or("") });
        let report = rec.write_report(&env);
        let _ = rec.publish(&env, Some(&sha2));
        let _ = env.git().run(&["update-ref", "-d", &dst], None, std::time::Duration::from_secs(30));
        cleanup(&env, &jid);
        let stages: Vec<StageResult> = Vec::new();
        let res = GateResult { job: jid.clone(), pass, report: report.strip_prefix(&env.home).unwrap_or(&report).to_string_lossy().into_owned(), sha: sha2, stages, warnings };
        job.succeed(serde_json::to_value(res).unwrap_or(Value::Null));
    });
    Ok(json!({"sha": sha}))
}

/// `rollback` (initctl.md §4.1): `from`/`to`, or `E_ALREADY_LKG`; then the boundary, then
/// supervise's rollback (§3.6.7).
///
/// ADR-013 G2 (the free path): when the running version is a change in a change group, `rollback`
/// undoes that group only, back to its parent, instead of going all the way to LKG (which drops
/// every older open group too). An open group is vetoed (H12; the Root answers `extend.rollback`
/// to the parent, which the kernel performs); a group the Root already closed is rolled back here.
pub fn rollback(env: Arc<Env>, args: &RollbackArgs, job: Arc<dyn Job>) -> Result<Value, ErrorBody> {
    let s = env.sup.snapshot().ok_or_else(|| err(ErrorCode::Internal, "the supervisor is gone"))?;
    if s.busy {
        return Err(err(ErrorCode::Busy, "a handoff or rollback is in progress"));
    }
    if env.free_link().is_some() {
        let t = super::admit::Groups::load(&env.home);
        if let Some(g) = t.holding(&s.current_sha).cloned() {
            match g.parent().map(str::to_string) {
                Some(parent) if parent == s.lkg_sha || t.data_only_back(&s.current_sha, &parent) => return group_rollback(env, args, job, s, g, parent),
                _ => {}
            }
        }
    }
    if s.current_sha == s.lkg_sha {
        return Err(err(ErrorCode::AlreadyLkg, "the running version is the LKG"));
    }
    let (from, to) = (s.current_sha.clone(), s.lkg_sha.clone());
    let reason = args.reason.clone();
    let env2 = env.clone();
    std::thread::spawn(move || {
        if !job.awaiting_boundary() {
            return;
        }
        job.progress(JobState::HandingOff, Some(Stage::Handoff), 50, "rollback");
        match env2.sup.rollback(&reason, true) {
            Ok(()) => {
                crate::recovery::proposal_event(&env2.home, env2.lineage.as_ref(), &from, &ProposalEv::NoVerdict { reason: "rollback".into() }); // X0b
                job.succeed(json!({"job": job.id(), "from": from, "to": to, "state": "succeeded"}))
            }
            Err(e) => job.fail(err(ErrorCode::HandoffFailed, e)),
        }
    });
    Ok(json!({"from": s.current_sha, "to": s.lkg_sha}))
}

/// How long `initctl rollback` of an open group waits for the Root's `extend.rollback` to land.
const GROUP_ROLLBACK_WAIT: std::time::Duration = std::time::Duration::from_secs(300);

/// ADR-013 G2: `initctl rollback` of the change group `g` that holds the running version, to its parent.
fn group_rollback(env: Arc<Env>, args: &RollbackArgs, job: Arc<dyn Job>, s: super::SupState, g: super::admit::GroupRec, parent: String) -> Result<Value, ErrorBody> {
    let (from, to, reason) = (s.current_sha.clone(), parent, args.reason.clone());
    let env2 = env.clone();
    let (from2, to2) = (from.clone(), to.clone());
    std::thread::spawn(move || {
        if !job.awaiting_boundary() {
            return;
        }
        job.progress(JobState::HandingOff, Some(Stage::Handoff), 50, "rollback of the running change group");
        let why = format!("initctl rollback: {}", kernel_gate::record::cap(&reason, 150));
        // an open group: the Root's veto (H12, honoured) closes it and sends `extend.rollback` to its parent
        let vetoed = g.state == "open" && match super::admit::veto(&env2, g.group, &why) {
            Ok(()) => true,
            Err(e) => {
                crate::klog::log("gate", &format!("rollback: change.veto group {}: {e}; rolling back here", g.group));
                false
            }
        };
        let r = if vetoed {
            let deadline = std::time::Instant::now() + GROUP_ROLLBACK_WAIT;
            loop {
                match env2.sup.snapshot() {
                    Some(st) if st.current_sha == to2 && !st.busy => break Ok(()),
                    None => break Err("the supervisor is gone".to_string()),
                    _ if std::time::Instant::now() >= deadline => break Err(format!("group {} was vetoed but the body did not reach {} within {} s", g.group, &to2[..to2.len().min(12)], GROUP_ROLLBACK_WAIT.as_secs())),
                    _ => std::thread::sleep(std::time::Duration::from_millis(200)),
                }
            }
        } else {
            env2.sup.rollback_to(&why, &to2, true)
        };
        match r {
            Ok(()) => {
                crate::recovery::proposal_event(&env2.home, env2.lineage.as_ref(), &from2, &ProposalEv::NoVerdict { reason: "rollback".into() }); // X0b
                job.succeed(json!({"job": job.id(), "from": from2, "to": to2, "group": g.group, "state": "succeeded"}))
            }
            Err(e) => job.fail(err(ErrorCode::HandoffFailed, e)),
        }
    });
    Ok(json!({"from": from, "to": to, "group": g.group}))
}

#[cfg(test)]
mod self_json_tests {
    use super::*;

    #[test]
    fn lkg_size_comes_from_the_top_level_field() {
        let shipped: Value = serde_json::from_str(include_str!("../../../../../seed/self.json")).unwrap();
        assert!(self_json_body_tokens(&shipped) > 0, "the shipped seed self.json has a body size");
        assert_eq!(self_json_body_tokens(&serde_json::json!({"body_tokens": 21873})), 21873);
        assert_eq!(self_json_body_tokens(&serde_json::json!({"budget": {"body_tokens": 5}})), 0);
    }
}
