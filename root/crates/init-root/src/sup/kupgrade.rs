//! ADR-011 §1.2 (lane X8): the kernel lane's swap, root/root.md §3.8 phases 1–6 on the Root side.
//!
//! A kernel change goes live only after the free admission (`change.open{scope:"k"}`: A0, A5, the Root conformance probe
//! of `kprobe.rs`, the system snapshot) and then its own `system.swap{group}`:
//! 1. request: L1 on the free path (L3 on gated), not safe mode, one swap at a time, and the swap must name the binaries
//!    of an open, admitted kernel group (`root.unwatched_change` otherwise: no unannounced kernel);
//! 2. the gate-of-gates only on request (`measure: ["gog"]`), recorded and deciding nothing on the free path; mandatory on
//!    the gated path (root.md §3.8);
//! 3. prepare: `prepare_swap{job, deadline_s}`; the old kernel answers `ok` (with its `kstate` blob) and exits 75 within
//!    10 s; a refusal, a late answer or a crash aborts the swap and the old kernel keeps running (or restarts);
//! 4. install: the candidate's `init-kernel` and `initctl`, then `tuple` (so `current` moves) and `installed`;
//! 5. ready within `kernel_ready_timeout_s` (the supervisor's own bound);
//! 6. probation (`kernel_probation_s`); `probation_pass` lets body groups open again (the sentinel reads the record).
//!
//! A crash of the new kernel in the ready wait or in probation (any exit the Root did not ask for, a missing `hello`,
//! `ready` or heartbeat, a lost link) is decided here, never asked of the new kernel: the whole-system rollback to the
//! previous tuple with `boot.kstate` (`boot_fail`, `probation_fail`), no demotion. The sentinel's H1 for the same crash
//! closes the kernel group and asks for the same restart, which the supervisor then drops (`absorbs`). After probation the
//! kernel group's window (40 turns, 120 h, at least 24 h) belongs to the sentinel, whose rollback of a kernel group is the
//! Root's whole-system rollback at once (sentinel/engine.rs).
//!
//! State: `Root.sup.swap` (one job). Records: `kernel_upgrade{job, stage, group, from, to, kstate, source, …}`. A Root
//! restart ends a job that had not installed (`aborted`) and restarts the probation of one that had (`resume`).

use crate::link::{Root, lock};
use crate::sup::{Plan, Slot, cfg};
use crate::tuple;
use crate::util::now_ms;
use crate::zone::Err3;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering::SeqCst;
use std::time::{Duration, Instant};

/// After answering `prepare_swap`, the kernel exits 75 within this (root.md §4.1.4).
pub const EXIT_75_MS: i64 = 10_000;
/// The sentinel's restart to the tuple this lane has just rolled back to is the same rollback within this.
const ABSORB: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq)]
pub enum Stage {
    /// The gate-of-gates is running (opt-in on the free path).
    Gog,
    /// `prepare_swap` sent as request `req` to kernel instance `inst`; answer and exit by `until` (Root clock).
    Preparing { req: String, inst: String, until: i64 },
    /// Answered `ok`: exit 75 by `until`.
    Prepared { inst: String, until: i64 },
    /// The old kernel exited 75; `start` installs the candidate.
    Installing,
    AwaitReady,
    Probation { until: i64 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    /// `s-NNNNNN`.
    pub id: String,
    /// The kernel change group (free path).
    pub group: Option<u64>,
    pub from: String,
    pub to: String,
    /// The old kernel's `state/kernel` snapshot (a `kstate` blob), restored first on a rollback in probation.
    pub kstate: Option<String>,
    pub stage: Stage,
    pub free: bool,
}

/// `Root.sup.swap`.
#[derive(Default)]
pub struct Swap {
    pub job: Option<Job>,
    loaded: bool,
    rolled: Option<(String, Instant)>,
}

fn free(root: &Root) -> bool {
    lock(&root.st).charter.as_ref().is_some_and(|l| l.ch.extend.free())
}

fn hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

fn stored(root: &Root, sha: &str) -> bool {
    hex64(sha) && tuple::blob(&root.p.rh, sha).is_some_and(|b| b.is_file())
}

/// A kernel-supplied error code, kept only if it looks like one (the lineage is mirrored: no free text from the zone).
fn code_of(v: &Value) -> String {
    let c = v["error"]["code"].as_str().unwrap_or("");
    if (1..=48).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_lowercase() || b"._".contains(&b)) { c.into() } else { "error".into() }
}

fn exit_text(code: Option<i32>) -> String {
    code.map_or_else(|| "by a signal".into(), |c| format!("with code {c}"))
}

/// `kernel_upgrade{…}` and the kernel's `swap` progress event.
fn rec(root: &Root, j: &Job, stage: &str, extra: Value) {
    let mut d = json!({"from": j.from, "group": j.group, "job": j.id, "kstate": j.kstate, "source": "agent", "stage": stage, "to": j.to});
    if let Value::Object(m) = extra {
        for (k, v) in m {
            d[k.as_str()] = v;
        }
    }
    root.append("kernel_upgrade", d);
    if let Some(k) = root.sup.kernel() {
        k.event("swap", json!({"job": j.id, "msg": format!("kernel swap {}: {stage}", j.id), "stage": stage}));
    }
}

/// The swap ends without going live: recorded, alerted, and on the free path its group is closed through the sentinel's
/// veto (H12: unattributed, re-admissible once). The current tuple never moved, so nothing rolls back.
fn abort(root: &Root, j: Job, why: &str) {
    rec(root, &j, "aborted", json!({"why": why}));
    root.alert("kupgrade", &format!("[root] kernel swap {} aborted: {why}", j.id));
    if let Some(g) = j.group {
        let _ = crate::sentinel::veto(root, &json!({"group": g, "reason": format!("kernel swap {} aborted", j.id)}));
    }
}

/// The rollback out of a failed ready wait or probation: the previous tuple, its kernel restoring `kstate` first.
fn fail(root: &Root, j: Job, stage: &'static str, why: &str) -> Plan {
    rec(root, &j, stage, json!({"why": why}));
    let with = if j.kstate.is_some() { " and the old kernel's state" } else { "" };
    root.alert("kupgrade", &format!("[root] kernel swap {}: {stage} ({why}); whole-system rollback to {}{with}", j.id, j.from));
    Plan { target: Some("parent"), reason: "rollback_system", cause: json!({"job": j.id, "kstate": j.kstate, "to": j.from, "trigger": stage}),
        panic: Value::Null, mode: None }
}

/// The job, if it is still `id`; taken when `take`.
fn job_if(root: &Root, id: &str, take: bool) -> Option<Job> {
    let mut s = lock(&root.sup.swap);
    if s.job.as_ref().is_none_or(|j| j.id != id) {
        return None;
    }
    if take { s.job.take() } else { s.job.clone() }
}

fn group_open(root: &Root, g: u64) -> bool {
    crate::sentinel::open_groups(root).iter().any(|x| x["seq"] == g)
}

/// Once per Root process, before anything else here: a swap the previous Root left unfinished.
fn loaded(root: &Root) {
    if std::mem::replace(&mut lock(&root.sup.swap).loaded, true) {
        return;
    }
    let recs = crate::lineage::records(&root.p.rh).unwrap_or_default();
    let Some(d) = recs.iter().rev().find(|r| r["kind"] == "kernel_upgrade").map(|r| &r["data"]) else { return };
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    let j = Job { id: s(&d["job"]), group: d["group"].as_u64(), from: s(&d["from"]), to: s(&d["to"]), kstate: d["kstate"].as_str().map(String::from),
        stage: Stage::AwaitReady, free: free(root) };
    match d["stage"].as_str().unwrap_or("") {
        "requested" | "gog" | "gog_pass" | "prepared" => abort(root, j, "the Root restarted before the swap"),
        "installed" | "ready" if lock(&root.sup.ptrs).current.as_deref() == Some(j.to.as_str()) => lock(&root.sup.swap).job = Some(j),
        _ => {}
    }
}

/// Authoritative replay for a kernel that restarted after Root recorded a swap
/// verdict but before the kernel persisted its event receipt. Never infer a
/// successful probation from the currently installed tuple alone.
pub fn status(root: &Root, args: &Value) -> Result<Value, Err3> {
    let job = args["job"].as_str().filter(|s| {
        s.strip_prefix("s-").is_some_and(|n| !n.is_empty() && n.len() <= 20 && n.bytes().all(|b| b.is_ascii_digit()))
    }).ok_or_else(|| ("root.args", "system.swap_status needs a Root swap job id".into(), Value::Null))?;
    // This chain was verified at Root startup. Holding its writer lock prevents
    // observing a partial concurrent append. Zone callers cannot append this kind.
    let lineage = lock(&root.lin);
    if lineage.is_none() {
        return Err(("root.lineage", "verified Root lineage unavailable".into(), Value::Null));
    }
    let records = crate::lineage::records(&root.p.rh)
        .map_err(|_| ("root.lineage", "cannot read Root swap history".into(), Value::Null))?;
    swap_record(&records, job).ok_or_else(|| ("root.not_found", "no recorded Root swap with that id".into(), Value::Null))
}

fn swap_record(records: &[Value], job: &str) -> Option<Value> {
    records.iter().rev().find(|r| r["kind"] == "kernel_upgrade" && r["data"]["job"] == job).map(|r| {
        let d = &r["data"];
        json!({"job": d["job"], "stage": d["stage"], "from": d["from"], "to": d["to"],
            "group": d["group"], "record_seq": r["seq"], "record_hash": r["hash"]})
    })
}

/// `system.swap{version, source_repo, source_sha, init_kernel, initctl, report, group?, measure?}` (root.md §3.8 phase 1;
/// ADR-011 §1.2). Refusals: `root.level` (demoting, D6 rule b: L1 free, L3 gated), `root.mode`, `root.busy`, `root.args`,
/// `root.store`, `root.unwatched_change` (free path: no open kernel group carries these binaries).
pub fn request(root: &Arc<Root>, a: &Value) -> Result<Value, Err3> {
    loaded(root);
    let no = |c: &'static str, m: &str| (c, m.to_string(), Value::Null);
    let free = free(root);
    let (level, safe) = { let st = lock(&root.st); (st.level.level, st.mode == "safe") };
    let need = if free { 1 } else { 3 };
    if level < need {
        root.append("ceiling_hit", json!({"attempted": need, "ceiling": "level", "code": "root.level", "demotes": true, "level": level,
            "limit": level, "rule": "b", "source": "broker"}));
        return Err(("root.level", format!("system.swap needs L{need}; the system is at L{level}"), json!({"level": level, "need": need})));
    }
    if safe {
        return Err(("root.mode", "no kernel swap in system safe mode".into(), json!({"mode": "safe"})));
    }
    if lock(&root.sup.swap).job.is_some() {
        return Err(("root.busy", "a kernel swap is in progress".into(), json!({"reason": "swap"})));
    }
    let ik = a["init_kernel"].as_str().filter(|s| hex64(s)).ok_or_else(|| no("root.args", "init_kernel: the stored init-kernel's sha256"))?;
    let ic = match a["initctl"].as_str() {
        None | Some("") => "",
        Some(s) if hex64(s) => s,
        Some(_) => return Err(no("root.args", "initctl: a sha256, or empty")),
    };
    if !stored(root, ik) || !(ic.is_empty() || stored(root, ic)) {
        return Err(no("root.store", "store.put the kernel binaries first"));
    }
    let from = lock(&root.sup.ptrs).current.clone().ok_or_else(|| no("root.internal", "no current tuple"))?;
    let cur = tuple::load(&root.p.rh, &from).ok_or_else(|| no("root.internal", "the current tuple is not in the store"))?;
    let (group, to) = if free {
        let groups = crate::sentinel::open_groups(root);
        let g = groups.iter().filter(|g| g["kernel"] == true).find(|g| a["group"].is_null() || a["group"] == g["seq"])
            .ok_or_else(|| no("root.unwatched_change", "system.swap names no open kernel change: change.open{scope:\"k\"} admits it first (ADR-011 §2.3)"))?;
        let to = g["tuple"].as_str().unwrap_or("").to_string();
        let t = tuple::load(&root.p.rh, &to).ok_or_else(|| no("root.internal", "the kernel group's tuple is not in the store"))?;
        if t["kernel"]["init_kernel"] != ik || t["kernel"]["initctl"].as_str().unwrap_or("") != ic {
            return Err(no("root.unwatched_change", "the swap's binaries are not the admitted kernel change's"));
        }
        if to == from {
            return Err(no("root.args", "the admitted kernel is already live"));
        }
        if t["body"] != cur["body"] || t["ext"] != cur["ext"] {
            return Err(no("root.args", "the zone's body moved since the kernel change was admitted; open a new kernel change"));
        }
        (g["seq"].as_u64(), to)
    } else {
        let version = a["version"].as_str().filter(|v| (1..=64).contains(&v.len()) && v.bytes().all(|c| c.is_ascii_graphic())).unwrap_or("agent");
        let built = a["source_sha"].as_str().filter(|v| v.len() <= 64 && v.bytes().all(|c| c.is_ascii_hexdigit())).unwrap_or("");
        let t = json!({"body": cur["body"], "ext": cur["ext"], "kernel": {"built_from": built, "init_kernel": ik, "initctl": ic, "source": "agent",
            "version": version}, "v": 1});
        (None, tuple::save(&root.p.rh, &t).map_err(|e| ("root.internal", format!("tuple: {e}"), Value::Null))?)
    };
    let gog = !free || a["measure"].as_array().is_some_and(|m| m.iter().any(|x| x == "gog"));
    let n = crate::lineage::records(&root.p.rh).unwrap_or_default().iter()
        .filter(|r| r["kind"] == "kernel_upgrade" && r["data"]["stage"] == "requested").count();
    let job = Job { id: format!("s-{:06}", n + 1), group, from, to, kstate: None, stage: if gog { Stage::Gog } else { Stage::Installing }, free };
    {
        let mut s = lock(&root.sup.swap);
        if s.job.is_some() {
            return Err(("root.busy", "a kernel swap is in progress".into(), json!({"reason": "swap"})));
        }
        s.job = Some(job.clone());
    }
    rec(root, &job, "requested", json!({"gog": gog}));
    if gog {
        let (r, gate, id) = (root.clone(), tuple::blob(&root.p.rh, ik).unwrap_or_default(), job.id.clone());
        std::thread::spawn(move || run_gog(&r, gate, &id));
    } else {
        prepare(root, &job.id);
    }
    Ok(json!({"job": job.id}))
}

/// Phase 2: the gate-of-gates on the candidate's gate (`<init-kernel> gate judge`, root/sealed-eval.md §3.3).
fn run_gog(root: &Arc<Root>, gate: PathBuf, id: &str) {
    let r = crate::sealed::request(root, "sealed.gog", &json!({"args": [], "gate": gate, "source": "agent"}));
    let pass = r.as_ref().map_or(Value::Null, |v| v["mutation"]["pass"].clone()); // null: it could not run
    if let Err((_, m)) = &r {
        crate::util::log(&root.p.rh, "warn", &format!("kernel swap {id}: the gate-of-gates did not run: {m}"));
    }
    let Some(j) = job_if(root, id, false).filter(|j| j.stage == Stage::Gog) else { return };
    if j.free {
        rec(root, &j, "gog", json!({"pass": pass})); // an instrument on the free path (ADR-011 §8): it decides nothing
        return prepare(root, id);
    }
    if pass == true {
        rec(root, &j, "gog_pass", json!({}));
        return prepare(root, id);
    }
    let Some(j) = job_if(root, id, true) else { return };
    if pass == false {
        rec(root, &j, "gog_fail", json!({})); // the `mutation` record is R7's demotion trigger on the gated path
        root.alert("kupgrade", &format!("[root] kernel swap {}: the candidate's gate failed the gate-of-gates", j.id));
    } else {
        abort(root, j, "the gate-of-gates could not run (underpowered S, or an error)");
    }
}

/// Phase 3: `prepare_swap` to the running kernel (sent under the swap lock, so its answer finds the request id).
fn prepare(root: &Root, id: &str) {
    let k = root.sup.kernel().filter(|k| k.ready.load(SeqCst) && lock(&root.st).slot == Slot::Running);
    let secs = cfg(root).swap_prepare_timeout_s.max(1);
    let mut s = lock(&root.sup.swap);
    let Some(j) = s.job.as_mut().filter(|j| j.id == id) else { return };
    match k {
        Some(k) => {
            let req = k.request("prepare_swap", json!({"deadline_s": secs, "job": id}));
            j.stage = Stage::Preparing { req, inst: k.inst.clone(), until: now_ms() + (secs + 10) * 1000 };
        }
        None => {
            let j = s.job.take().expect("checked");
            drop(s);
            abort(root, j, "no kernel is running to prepare the swap");
        }
    }
}

/// `read_loop`: a kernel response. True when it was the answer to this lane's `prepare_swap`.
pub fn answer(root: &Root, id: &str, v: &Value) -> bool {
    let mut s = lock(&root.sup.swap);
    let Some(j) = s.job.as_mut() else { return false };
    let Stage::Preparing { req, inst, .. } = &j.stage else { return false };
    if req != id {
        return false;
    }
    if v["ok"] == true {
        let inst = inst.clone();
        j.kstate = v["result"]["kstate"].as_str().filter(|x| stored(root, x)).map(String::from);
        j.stage = Stage::Prepared { inst, until: now_ms() + EXIT_75_MS };
        let j = j.clone();
        drop(s);
        rec(root, &j, "prepared", json!({}));
    } else {
        let j = s.job.take().expect("checked");
        drop(s);
        abort(root, j, &format!("the running kernel refused prepare_swap ({})", code_of(v)));
    }
    true
}

/// `Ev::Exit` of the current kernel, after `kill_all` and before the crash rules: `(plan, zone_exit class)` when the swap
/// decides what starts next. Exit 75 after an `ok` → the install. Any exit of the new kernel in its ready wait or probation
/// → the rollback (the Root asked for none of them). An exit of the old kernel before the swap → aborted, crash rules.
pub fn exited(root: &Root, slot: &Slot, code: Option<i32>) -> Option<(Plan, &'static str)> {
    loaded(root);
    if !matches!(slot, Slot::Starting { .. } | Slot::Running) {
        return None; // a stop or restart the Root ordered: `boot` settles the swap at the next start
    }
    let mut s = lock(&root.sup.swap);
    let stage = s.job.as_ref()?.stage.clone();
    match stage {
        Stage::Prepared { .. } if code == Some(75) => {
            let j = s.job.as_mut().expect("checked");
            j.stage = Stage::Installing;
            let cause = json!({"job": j.id, "to": j.to});
            Some((Plan { target: Some("swap"), reason: "kernel_upgrade", cause, panic: Value::Null, mode: None }, "swap"))
        }
        Stage::Gog | Stage::Preparing { .. } | Stage::Prepared { .. } => {
            let j = s.job.take().expect("checked");
            drop(s);
            abort(root, j, &format!("the running kernel exited {} before the swap", exit_text(code)));
            None
        }
        Stage::AwaitReady | Stage::Probation { .. } => {
            let j = s.job.take().expect("checked");
            s.rolled = Some((j.from.clone(), Instant::now()));
            drop(s);
            let st = if stage == Stage::AwaitReady { "boot_fail" } else { "probation_fail" };
            Some((fail(root, j, st, &format!("the new kernel exited {}", exit_text(code))), "crash"))
        }
        Stage::Installing => None,
    }
}

/// `Ev::Restart`: the sentinel's restart to the tuple this lane has just rolled back to (its H1 on the same crash) is
/// dropped, so the system restarts once. An R2 escalation (`cause.restore`) is never dropped.
pub fn absorbs(root: &Root, p: &Plan) -> bool {
    let mut s = lock(&root.sup.swap);
    let hit = p.target == Some("parent") && p.cause["restore"].is_null()
        && s.rolled.as_ref().is_some_and(|(to, at)| p.cause["to"] == to.as_str() && at.elapsed() < ABSORB);
    if hit {
        s.rolled = None;
    }
    hit
}

/// `start` with target `"swap"` (phase 4): the candidate's kernel into `INIT_HOME/kernel/bin`, then `tuple` (`current`
/// moves) and `installed`. When the group closed meanwhile or the install fails, the swap is aborted and the previous
/// tuple starts again, its kernel reinstalled, with reason `cold`.
pub fn install(root: &Root, plan: &mut Plan) -> Result<(String, Option<Value>), String> {
    let j = lock(&root.sup.swap).job.clone().filter(|j| j.stage == Stage::Installing && plan.cause["job"] == j.id.as_str());
    let Some(j) = j else {
        plan.reason = "cold";
        return tuple::rollback(root, "current", "swap").map(|(id, _)| (id, None));
    };
    let open = j.group.is_none_or(|g| group_open(root, g));
    let t = tuple::load(&root.p.rh, &j.to).filter(|t| tuple::verify(&root.p.rh, t));
    let r = match (open, t) {
        (false, _) => Err("its change group closed before the install".to_string()),
        (true, None) => Err("the candidate tuple does not verify".to_string()),
        (true, Some(t)) => tuple::install_kernel(&root.p, &t).map(|()| t).map_err(|e| format!("install: {e}")),
    };
    match r {
        Ok(t) => {
            root.append("tuple", json!({"id": j.to, "tuple": t}));
            if let Some(x) = lock(&root.sup.swap).job.as_mut() {
                x.stage = Stage::AwaitReady;
            }
            rec(root, &j, "installed", json!({}));
            Ok((j.to.clone(), None))
        }
        Err(why) => {
            lock(&root.sup.swap).job = None;
            plan.reason = "cold";
            let back = tuple::load(&root.p.rh, &j.from).filter(|t| tuple::verify(&root.p.rh, t));
            abort(root, j.clone(), &why);
            match back.map(|t| tuple::install_kernel(&root.p, &t)) {
                Some(Ok(())) => Ok((j.from, None)),
                _ => tuple::rollback(root, "lkg", "swap_abort").map(|(id, d)| (id, Some(d))),
            }
        }
    }
}

/// `start`, once its tuple is picked: a start on another tuple than the candidate's ends an open swap. In the ready wait
/// or probation that is a rollback ordered elsewhere (the sentinel, the owner): the previous tuple gets `boot.kstate`,
/// and the swap ends `probation_fail`. Before the install it is an abort.
pub fn boot(root: &Root, plan: &mut Plan, id: &str) {
    loaded(root);
    let mut s = lock(&root.sup.swap);
    let Some(j) = s.job.as_ref() else { return };
    if j.to == id || j.stage == Stage::Installing {
        return;
    }
    let j = s.job.take().expect("checked");
    drop(s);
    match j.stage {
        Stage::AwaitReady | Stage::Probation { .. } => {
            if id == j.from && plan.cause.get("kstate").is_none_or(Value::is_null) {
                plan.cause["kstate"] = json!(j.kstate);
            }
            let st = if j.stage == Stage::AwaitReady { "boot_fail" } else { "probation_fail" };
            rec(root, &j, st, json!({"why": format!("the zone restarted ({}) on {id}", plan.reason)}));
        }
        _ => abort(root, j, &format!("the zone restarted ({}) before the swap", plan.reason)),
    }
}

/// `Ev::Ready`: the candidate is `ready` (phase 5 done); probation starts (again, after a restart on the same tuple).
pub fn ready(root: &Root) {
    loaded(root);
    let cur = lock(&root.sup.ptrs).current.clone();
    let ms = cfg(root).kernel_probation_s.max(1) * 1000;
    let mut s = lock(&root.sup.swap);
    let Some(j) = s.job.as_mut().filter(|j| cur.as_deref() == Some(j.to.as_str()) && matches!(j.stage, Stage::AwaitReady | Stage::Probation { .. }))
    else { return };
    let first = j.stage == Stage::AwaitReady;
    j.stage = Stage::Probation { until: now_ms() + ms };
    let j = j.clone();
    drop(s);
    if first {
        rec(root, &j, "ready", json!({"probation_s": ms / 1000}));
    }
}

/// The supervisor's tick: the prepare deadlines, a group closed before the install, the end of probation.
pub fn tick(root: &Root) {
    loaded(root);
    let Some(j) = lock(&root.sup.swap).job.clone() else { return };
    let now = now_ms();
    match &j.stage {
        Stage::Preparing { until, .. } if now >= *until => {
            if let Some(j) = job_if(root, &j.id, true) {
                abort(root, j, "no answer to prepare_swap in time; the running kernel keeps running");
            }
        }
        Stage::Prepared { inst, until } if now >= *until => {
            if let Some(j) = job_if(root, &j.id, true) {
                abort(root, j, "the kernel answered prepare_swap but did not exit 75 within 10 s; it is stopped");
                root.zone.kill(inst, libc::SIGKILL); // the crash rules restart the same (old) tuple
            }
        }
        Stage::Gog | Stage::Preparing { .. } | Stage::Prepared { .. } if j.group.is_some_and(|g| !group_open(root, g)) => {
            if let Some(j) = job_if(root, &j.id, true) {
                abort(root, j, "its change group closed before the swap");
            }
        }
        Stage::Probation { until } if now >= *until => {
            let live = lock(&root.st).slot == Slot::Running && lock(&root.sup.ptrs).current.as_deref() == Some(j.to.as_str());
            if let Some(j) = live.then(|| job_if(root, &j.id, true)).flatten() {
                rec(root, &j, "probation_pass", json!({}));
                root.notice("info", &format!("[root] kernel swap {} passed probation; the kernel change's window goes on", j.id));
            }
        }
        _ => {}
    }
}

/// `hello`: the `kstate` the starting kernel restores first (a rollback out of a swap), else null.
pub fn kstate(p: &Plan) -> Value {
    p.cause.get("kstate").cloned().unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_status_replays_only_the_requested_root_record() {
        let records = vec![
            json!({"kind":"kernel_upgrade","seq":1,"data":{"job":"s-000001","stage":"installed","to":"tuple-a"}}),
            json!({"kind":"kernel_upgrade","seq":2,"data":{"job":"s-000001","stage":"probation_pass","to":"tuple-a"}}),
            json!({"kind":"kernel_upgrade","seq":3,"data":{"job":"s-000002","stage":"aborted","to":"tuple-b"}}),
            json!({"kind":"agent_note","seq":4,"data":{"job":"s-000001","stage":"aborted"}}),
        ];
        let replay = swap_record(&records,"s-000001").unwrap();
        assert_eq!(replay["stage"], "probation_pass");
        assert_eq!(replay["record_seq"], 2);
        assert_eq!(replay["to"], "tuple-a");
        assert!(swap_record(&records,"s-000003").is_none());
    }

    #[test]
    fn codes_from_the_zone_are_filtered() {
        assert_eq!(code_of(&json!({"error": {"code": "root.busy"}})), "root.busy");
        assert_eq!(code_of(&json!({"error": {"code": "/Users/x"}})), "error");
        assert_eq!(code_of(&json!({"error": {"code": "a".repeat(49)}})), "error");
        assert_eq!(exit_text(Some(70)), "with code 70");
        assert_eq!(exit_text(None), "by a signal");
    }

    #[test]
    fn kstate_rides_in_the_plan_cause() {
        let p = Plan { target: Some("parent"), reason: "rollback_system", cause: json!({"kstate": "ab", "to": "t1"}), panic: Value::Null, mode: None };
        assert_eq!(kstate(&p), json!("ab"));
        assert_eq!(kstate(&Plan::plain("cold")), Value::Null);
        let mut c = Value::Null;
        c["kstate"] = json!(null);
        assert!(c.get("kstate").is_some_and(Value::is_null), "an absent cause becomes an object");
    }

    /// X8's sentinel rules (ADR-011 §1.2): body groups wait only while the kernel change has not passed probation, and
    /// a rollback taking a kernel group is the Root's restart at once (never `extend.rollback`, never a pending
    /// confirmation the new kernel could fake), unless the change never went live.
    #[test]
    fn the_sentinel_never_asks_a_kernel_to_roll_itself_back() {
        use crate::sentinel::engine::{Act, Ctx, Engine, Open, Pick};
        let cx = Ctx { now: 1_900_000_000_000, level: 1, current: Some("tP".into()), running: true, day: 1_899_993_600_000,
            free: Some(100 << 30), free_mode: true };
        let mut e = Engine::new(Some("tP"));
        e.lim.budget = crate::sentinel::engine::budget(1); // the L1 budget the adapter sets from the charter (X3 1496814)
        let k = Open { sha: "9".repeat(40), scope: "k".into(), tuple: "tK".into(), ..Open::default() };
        let p = e.plan(&cx, &k).unwrap();
        e.commit(&cx, &k, &p, Some("snap".into()));
        let b = Open { sha: "8".repeat(40), scope: "t0".into(), tuple: "tB".into(), ..Open::default() };
        let live = Ctx { current: Some("tK".into()), ..cx.clone() }; // the swap installed the candidate
        assert_eq!(e.plan(&live, &b).unwrap_err().0, "extend.budget", "no body group during probation");
        e.ksettle(p.group);
        let pb = e.plan(&live, &b).unwrap();
        e.commit(&live, &b, &pb, None);
        let acts = e.trigger(&Ctx { current: Some("tB".into()), ..live.clone() }, "H2", 2, 2, Pick::All, "rollback_all");
        assert!(acts.iter().any(|a| matches!(a, Act::Restart { to: Some(t), .. } if t == "tP")), "{acts:?}");
        assert!(!acts.iter().any(|a| matches!(a, Act::Event("extend.rollback", _))), "{acts:?}");
        assert!(e.rollback.is_none() && e.groups.is_empty());
        assert!(e.watch.as_ref().is_some_and(|w| w.to == "tP" && w.snapshot == "snap"), "R2 if the old kernel cannot run on the state");

        let mut e = Engine::new(Some("tP"));
        e.lim.budget = crate::sentinel::engine::budget(1); // the L1 budget the adapter sets from the charter (X3 1496814)
        let p = e.plan(&cx, &k).unwrap();
        e.commit(&cx, &k, &p, None);
        let acts = e.veto(&cx, p.group).unwrap(); // an aborted swap: the change never went live
        assert!(!acts.iter().any(|a| matches!(a, Act::Restart { .. } | Act::Event("extend.rollback", _))), "{acts:?}");
    }
}
