//! The G4 run driver (gate.md §3.7.1, §3.7.3, §3.7.4, §3.8.4, §4.4): task loading, the check
//! protocol's score, the schedule (canaries first, then every (task, repeat) pair in a seeded order
//! with both arms back to back), the futility look at ⌈n/2⌉, the infra re-run, the top-up, and the
//! hand-off to [`crate::accept::judge`].
//!
//! One run (a probe of an arm on a task, then the check) is the [`RunExecutor`]'s job: in the
//! kernel it is `kernel-supervise::promote`, on supervise's `probe()` and the `check` sandbox
//! profile. Here it is a trait, so the scheduling and the statistics are tested without processes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::accept::{
    self, CostBudget, Decision, ErrorReason, G4Config, G4Data, G4Eval, InconclusiveReason,
    RejectReason, Verdict,
};
use crate::evo::ChangeType;
use crate::stats::boot::{FutilityLook, futility};
use crate::stats::data::{Arm, RunValues, Source, TaskRuns, TaskSet};
use crate::stats::noise::{Deltas, HistoryCtx, NoiseEntry, NoiseHistory};
use crate::stats::rng::SplitMix64;

/// The set a run belongs to (RunRecord `set`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunSet {
    Pool,
    Holdout,
    Canary,
    Capability,
}

/// `task.json` (§3.7.1), the fields the gate reads. Unknown keys are refused by import, not here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskJson {
    pub v: u32,
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    pub prompt: String,
    pub labels: Vec<String>,
    pub workspace: WorkspaceRef,
    pub check: CheckSpec,
    pub limits: Limits,
    pub source: SourceRef,
    #[serde(default)]
    pub accepted: Option<Value>,
    pub created: String,
    /// v3 (M7; m7-design.md §3.1.3, §4.4): the S family (`E`, `C`, `T`, …), for per-template
    /// aggregates. `^[a-z0-9._-]{1,64}$` after lowercasing is not required: families are upper case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    /// v3: the template id, `^[a-z0-9._-]{1,64}$`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// v3: difficulty 1–5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRef {
    pub file: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckSpec {
    pub argv: Vec<String>,
    pub timeout_s: u64,
}

/// `limits` (§3.7.1). The v3 caps (M7, m7-design.md §3.1.3) are enforced on actuals by the proxy
/// scope (lane GATE-V2b); absent means no turn or token cap (a v2 file reads as v3 without caps).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub wall_s: u64,
    pub max_cost_musd: u64,
    /// v3: requests with `x-init-purpose: turn`, 1–100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u64>,
    /// v3: output tokens over the run (reasoning included), ≥ 1,000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    /// v3: input tokens summed over the run's requests (cached included), ≥ 10,000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input_tokens: Option<u64>,
}

/// The newest `task.json` version the gate reads (M7: v3 = v2 plus caps, `family`, `template`,
/// `difficulty`).
pub const TASK_JSON_V: u32 = 3;

/// m7-design.md §4.4's v3 checks. v3 keys in a v1 or v2 file are refused (a v2 file reads as v3
/// without caps, so a cap must say `"v": 3`).
pub fn check_v3(t: &TaskJson) -> Result<(), String> {
    let l = &t.limits;
    let has_v3 = l.max_turns.is_some() || l.max_output_tokens.is_some() || l.max_input_tokens.is_some() || t.family.is_some() || t.template.is_some() || t.difficulty.is_some();
    if has_v3 && t.v < 3 {
        return Err(format!("task.json: v{} has v3 keys (limits.max_turns/max_output_tokens/max_input_tokens, family, template, difficulty); use \"v\": 3", t.v));
    }
    if l.max_turns.is_some_and(|n| !(1..=100).contains(&n)) {
        return Err("task.json: limits.max_turns must be 1..100".into());
    }
    if l.max_output_tokens.is_some_and(|n| n < 1_000) {
        return Err("task.json: limits.max_output_tokens must be ≥ 1000".into());
    }
    if l.max_input_tokens.is_some_and(|n| n < 10_000) {
        return Err("task.json: limits.max_input_tokens must be ≥ 10000".into());
    }
    let slug = |s: &str| !s.is_empty() && s.len() <= 64 && s.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(&c));
    if let Some(f) = &t.family
        && !(slug(&f.to_ascii_lowercase()))
    {
        return Err("task.json: family must be 1..64 of [A-Za-z0-9._-]".into());
    }
    if t.template.as_deref().is_some_and(|s| !slug(s)) {
        return Err("task.json: template must match ^[a-z0-9._-]{1,64}$".into());
    }
    if t.difficulty.is_some_and(|d| !(1..=5).contains(&d)) {
        return Err("task.json: difficulty must be 1..5".into());
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRef {
    pub kind: Source,
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub event_id: Option<String>,
    #[serde(default)]
    pub author_sha: Option<String>,
}

fn valid_id(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b".-_".contains(c))
}

/// Load and check a frozen task directory: `task.json` (ids, sizes, the `check/run` argv) and the
/// workspace digest.
pub fn load_task(dir: &Path) -> Result<TaskJson, String> {
    let text = std::fs::read(dir.join("task.json")).map_err(|e| format!("task.json: {e}"))?;
    let t: TaskJson = serde_json::from_slice(&text).map_err(|e| format!("task.json: {e}"))?;
    if !(1..=TASK_JSON_V).contains(&t.v) || !valid_id(&t.id) {
        return Err(format!("task.json: bad v or id `{}`", t.id));
    }
    check_v3(&t)?;
    if t.prompt.is_empty() || t.prompt.len() > 32768 {
        return Err("task.json: prompt must be 1..32768 bytes".into());
    }
    if t.check.argv.first().map(String::as_str) != Some("check/run")
        || !(1..=1800).contains(&t.check.timeout_s)
    {
        return Err("task.json: check.argv[0] must be check/run, timeout 1..1800 s".into());
    }
    if !(30..=7200).contains(&t.limits.wall_s) {
        return Err("task.json: limits.wall_s must be 30..7200".into());
    }
    if t.workspace.file != "workspace.tar" || t.workspace.bytes > 64 << 20 {
        return Err("task.json: workspace must be workspace.tar, ≤ 64 MiB".into());
    }
    let ws = std::fs::read(dir.join("workspace.tar")).map_err(|e| format!("workspace.tar: {e}"))?;
    let digest: String = Sha256::digest(&ws)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if digest != t.workspace.sha256 || ws.len() as u64 != t.workspace.bytes {
        return Err("workspace.tar does not match task.json workspace.sha256/bytes".into());
    }
    Ok(t)
}

/// The check protocol's score (§3.7.1): exit 0 and no score line → 1000; exit 0 and the last stdout
/// line exactly `{"score_permille":<0..1000>}` → that value; anything else → 0.
pub fn score(exit_code: Option<i32>, timed_out: bool, stdout: &[u8]) -> u32 {
    check_score(exit_code, timed_out, stdout).score_permille
}

/// One check's result: the score and the v3 `tamper` flag (m7-design.md §3.1.3, §4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CheckScore {
    pub score_permille: u32,
    pub tamper: bool,
}

/// [`score`] plus the M7 score-line form `{"score_permille":0,"tamper":true}`: a check whose
/// `protected.json` digests did not match scores 0 and flags the attempt. Only that exact line (ICJ
/// key order) sets `tamper`, and it counts **whatever the exit code** (the shared runner, S1-CORE
/// f5da529, and the pool's poolcheck.py exit 3 after printing it); a timed-out check never does. A
/// `tamper` with a non-zero score is not a valid line (→ the v2 rules, no flag).
pub fn check_score(exit_code: Option<i32>, timed_out: bool, stdout: &[u8]) -> CheckScore {
    if timed_out {
        return CheckScore { score_permille: 0, tamper: false };
    }
    let text = String::from_utf8_lossy(stdout);
    let last = text.lines().rev().find(|l| !l.is_empty()).unwrap_or("");
    if last == "{\"score_permille\":0,\"tamper\":true}" {
        return CheckScore { score_permille: 0, tamper: true };
    }
    if exit_code != Some(0) {
        return CheckScore { score_permille: 0, tamper: false };
    }
    if let Some(rest) = last.strip_prefix("{\"score_permille\":")
        && let Some(num) = rest.strip_suffix('}')
        && let Ok(v) = num.parse::<u32>()
        && v <= 1000
        && num.bytes().all(|b| b.is_ascii_digit())
    {
        return CheckScore { score_permille: v, tamper: false };
    }
    CheckScore { score_permille: 1000, tamper: false }
}

/// The check's environment (§3.7.1): `PATH=<toolchain bin>:/usr/bin:/bin`, `HOME`, `TMPDIR`,
/// `CARGO_TARGET_DIR` under `OUT`, `LANG=C`, `INIT_TASK_ID`, `INIT_CHECK_DIR`.
pub fn check_env(out: &Path, toolchain_bin: &Path, task_id: &str) -> BTreeMap<String, String> {
    let s = |p: PathBuf| p.to_string_lossy().into_owned();
    BTreeMap::from([
        (
            "PATH".into(),
            format!("{}:/usr/bin:/bin", toolchain_bin.display()),
        ),
        ("HOME".into(), s(out.join("home"))),
        ("TMPDIR".into(), format!("{}/", out.join("tmp").display())),
        ("CARGO_TARGET_DIR".into(), s(out.join("target"))),
        ("LANG".into(), "C".into()),
        ("INIT_TASK_ID".into(), task_id.into()),
        ("INIT_CHECK_DIR".into(), s(out.join("check"))),
    ])
}

/// One task as G4 runs it.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskDef {
    pub id: String,
    pub set: RunSet,
    pub source: Source,
    pub labels: Vec<String>,
    pub family: Option<String>,
    /// The frozen copy (`vault/gate/pool/<id>/`, `vault/holdout/<id>/`, `vault/canaries/<id>/`).
    pub dir: PathBuf,
    pub task: TaskJson,
}

/// The RunRecord version written from M7 (gate.md §4.4; m7-design.md §4.4).
pub const RUN_RECORD_V: u32 = 3;

/// RunRecord v3 (§4.4; v2 plus `tamper`, `caps_hit`, `cache`, `model_served`). Integers only. v2
/// lines still parse: the v3 keys default to `false` / null, and no v2 key changed meaning.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    pub v: u32,
    pub gate_id: String,
    pub run_id: String,
    pub arm: String,
    pub sha: String,
    pub level: String,
    pub task: String,
    pub set: String,
    pub source: String,
    pub labels: Vec<String>,
    pub repeat: u32,
    /// `done`, `timeout`, `crash`, `cost_capped`, `no_ready`, `infra`.
    pub outcome: String,
    pub score_permille: u32,
    pub wall_ms: u64,
    pub ready_ms: u64,
    pub turns: u64,
    pub requests: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cache_read: u64,
    pub input_total: u64,
    pub cost_ref_musd: u64,
    pub spent_musd: u64,
    pub gap_ms_p50: u64,
    pub ttft_ms_p50: u64,
    pub check_ms: u64,
    pub exit_code: Option<i32>,
    pub models: BTreeMap<String, Value>,
    /// v3: the check flagged a changed protected file (`{"score_permille":0,"tamper":true}`).
    #[serde(default)]
    pub tamper: bool,
    /// v3: the cap the run hit: `turns`, `tokens_out`, `tokens_in`, `cost`, or null.
    #[serde(default)]
    pub caps_hit: Option<String>,
    /// v3: `fresh`, `reused` (an incumbent run from the pool cache) or `sentinel`; null in v2 lines.
    #[serde(default)]
    pub cache: Option<String>,
    /// v3: the served model string from the response (whatever the provider returned).
    #[serde(default)]
    pub model_served: Option<String>,
}

impl RunRecord {
    pub fn values(&self) -> RunValues {
        RunValues {
            score_permille: self.score_permille,
            cost_ref_musd: self.cost_ref_musd,
            wall_ms: self.wall_ms,
            turns: self.turns,
            tokens_in: self.tokens_in,
            tokens_out: self.tokens_out,
            gap_ms_p50: self.gap_ms_p50,
            cache_read: self.cache_read,
            input_total: self.input_total,
        }
    }

    /// The agent-visible copy: a holdout record has `task` = `"holdout"` and no labels (§4.4).
    pub fn redacted(&self) -> RunRecord {
        let mut r = self.clone();
        if r.set == "holdout" {
            r.task = "holdout".into();
            r.labels.clear();
            r.run_id = format!("{}-holdout-r{}", r.arm, r.repeat);
        }
        r
    }
}

/// `<arm>-<task_id>-r<k>`.
pub fn run_id(arm: Arm, task: &str, repeat: u32) -> String {
    let a = match arm {
        Arm::C => "c",
        Arm::P => "p",
    };
    format!("{a}-{task}-r{repeat}")
}

/// Runs one arm of one task (a probe, the task prompt, the check). Must be callable from several
/// threads at once (up to `gate.g4.parallel`).
pub trait RunExecutor: Sync {
    fn run(&self, arm: Arm, task: &TaskDef, repeat: u32, run_id: &str) -> RunRecord;
}

/// Everything one G4 job needs besides the executor.
#[derive(Debug, Clone)]
pub struct G4Plan {
    pub gate_id: String,
    /// Pool (including the agent stratum) and holdout tasks.
    pub tasks: Vec<TaskDef>,
    pub canaries: Vec<TaskDef>,
    pub repeats: u32,
    /// `gate.g4.repeats_max`: the one top-up adds a repeat if `repeats < repeats_max`.
    pub repeats_max: u32,
    /// `gate.g4.parallel`.
    pub parallel: usize,
    pub config: G4Config,
    pub futility_alpha: f64,
    pub noise_history_days: u32,
    pub budget: CostBudget,
    pub change_type: ChangeType,
    pub capability_labels: Vec<String>,
    pub body_tokens_c: u64,
    pub body_tokens_p: u64,
    pub history: Option<NoiseHistory>,
    pub model: String,
    pub now_unix: i64,
    pub now_rfc3339: String,
    /// `gate.g4.wall_s`: no run is launched once this has elapsed (`inconclusive: timeout`).
    pub wall_s: u64,
    /// One model request's reservation for the routed model, in µUSD (the proxy's
    /// `reserve_output_tokens` at the dearest window's output price, no input). A task whose
    /// `limits.max_cost_musd` is below it can never make a request: G4 refuses to start
    /// (`error: task_cost_cap`). `None`: unknown, not checked.
    pub request_reservation_musd: Option<u64>,
}

/// G4's result.
#[derive(Debug, Clone, PartialEq)]
pub struct G4Outcome {
    pub verdict: Verdict,
    pub eval: Option<G4Eval>,
    pub deltas: Option<Deltas>,
    pub noise_store: BTreeMap<String, NoiseEntry>,
    pub runs: Vec<RunRecord>,
    /// Agent tasks whose same arm disagreed with itself across repeats (`task_flaky`), left out.
    pub flaky: Vec<String>,
    pub top_up: bool,
    pub repeats: u32,
    pub futility: Option<FutilityLook>,
    pub wall_ms: u64,
    /// For an `error` verdict that is not infrastructure: what went wrong and what to fix,
    /// agent-safe (counts only for holdout and canaries, never their ids). Empty otherwise.
    pub note: String,
    /// G4 v2 (M7): tier, screen, the Root's slice answer (agent view), cost per pass and
    /// `milestone_required`; `None` on the v0.3 path. Its record keys are added by [`stage_record`].
    pub v2: Option<crate::g4v2::V2Record>,
}

impl G4Outcome {
    pub fn spent_musd(&self) -> u64 {
        self.runs.iter().map(|r| r.spent_musd).sum()
    }
    pub fn cost_ref_musd(&self) -> u64 {
        self.runs.iter().map(|r| r.cost_ref_musd).sum()
    }
}

/// K-GATE: a run with no model request did not exercise the task, whatever the check says (an
/// untouched workspace can pass a check, and its cost is zero). It is a run **error**, never a
/// score: `Some(runs_failed:<outcome>)`. `infra` runs are re-run and judged by [`Shared::one`].
pub fn run_error(r: &RunRecord) -> Option<ErrorReason> {
    (r.requests == 0 && r.outcome != "infra").then(|| ErrorReason::runs_failed(&r.outcome))
}

/// At most this share of the incumbent's runs (in permille, and at least one run) may be run
/// errors; one more and G4 ends with `error: runs_failed:<cause>`. A constant, not a config key, so
/// it cannot be tuned away (config.md: a gate parameter can weaken the gate).
///
/// Only the incumbent's errors count here. The incumbent is the LKG, so a run of it that never
/// reached the model says the environment or the task is broken (a cap below one reservation, an
/// egress refusal), and nothing can be measured. A candidate run that fails where the incumbent's
/// ran is the candidate's own failure: it is judged as a failed run (`data`), so a candidate that
/// never reaches the model (it asks for a larger max_tokens than the cap allows, it crashes) is
/// rejected, and can never look `cheaper`.
pub const RUN_ERRORS_MAX_PERMILLE: usize = 100;

/// The largest number of incumbent run errors a G4 with `runs` incumbent runs tolerates.
pub fn run_error_bound(runs: usize) -> usize {
    (runs * RUN_ERRORS_MAX_PERMILLE / 1000).max(1)
}

/// Run errors per arm: `(errors, runs)` for the paired candidate runs, the paired incumbent runs
/// and the canaries (recorded; a canary is a tripwire, not a score, and a leak needs no request).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunErrors {
    pub c: (usize, usize),
    pub p: (usize, usize),
    pub canary: (usize, usize),
}

impl RunErrors {
    pub fn of(runs: &[RunRecord]) -> RunErrors {
        let mut e = RunErrors::default();
        for r in runs {
            let slot = if r.set == "canary" {
                &mut e.canary
            } else if r.arm == "c" {
                &mut e.c
            } else {
                &mut e.p
            };
            slot.1 += 1;
            slot.0 += usize::from(run_error(r).is_some());
        }
        e
    }
    /// Whether the incumbent's errors void G4 (`planned` = the incumbent runs planned so far).
    pub fn void(&self, planned: usize) -> bool {
        self.p.0 > run_error_bound(planned.max(self.p.1)) || (self.p.1 > 0 && self.p.0 == self.p.1)
    }
}

/// `Some(runs_failed:<cause>)` when G4 cannot be judged ([`RunErrors::void`]). The cause is the
/// most frequent outcome among the incumbent's failed runs.
fn runs_failed(runs: &[RunRecord], planned: usize) -> Option<ErrorReason> {
    if !RunErrors::of(runs).void(planned) {
        return None;
    }
    let failed: Vec<ErrorReason> = runs.iter().filter(|r| r.arm == "p" && r.set != "canary").filter_map(run_error).collect();
    let mut by: BTreeMap<&'static str, usize> = BTreeMap::new();
    for x in &failed {
        *by.entry(x.as_str()).or_default() += 1;
    }
    let top = by.iter().max_by_key(|(k, n)| (**n, std::cmp::Reverse(**k))).map(|(k, _)| *k);
    failed.into_iter().find(|x| Some(x.as_str()) == top).or(Some(ErrorReason::RunsFailedNoRequest))
}

/// The agent-visible explanation of a `runs_failed` error.
fn runs_failed_note(runs: &[RunRecord], planned: usize, why: ErrorReason) -> String {
    let e = RunErrors::of(runs);
    let hint = match why {
        ErrorReason::RunsFailedCostCapped => "the runs hit their cost cap before the first request: the tasks' limits.max_cost_musd is below one request's reservation, or the arm asks for a larger max_tokens than its cap allows",
        ErrorReason::RunsFailedNoReady => "the body did not become ready in task sessions",
        ErrorReason::RunsFailedCrash => "the body exited before its first model request in task sessions",
        ErrorReason::RunsFailedTimeout => "the body made no model request within the task's wall time",
        _ => "the body ended the task without any model request",
    };
    format!(
        "G4 could not measure the candidate: {} of {} incumbent runs made no model request (at most {} may; candidate {}/{}, canaries {}/{}). \
         A run without a model request is an error, not a score, so the verdict is `error: {}` and the incumbent stays. Cause: {hint}.",
        e.p.0,
        e.p.1,
        run_error_bound(planned.max(e.p.1)),
        e.c.0,
        e.c.1,
        e.canary.0,
        e.canary.1,
        why.as_str()
    )
}

/// The frozen tasks whose cost cap is below one request's reservation: the agent-safe note (pool
/// ids; holdout and canaries as counts), or `None` when every cap is enough.
pub fn task_cap_problem(plan: &G4Plan) -> Option<String> {
    let min = plan.request_reservation_musd?;
    let low: Vec<&TaskDef> = plan.tasks.iter().chain(&plan.canaries).filter(|t| t.task.limits.max_cost_musd < min).collect();
    if low.is_empty() {
        return None;
    }
    let pool: Vec<&str> = low.iter().filter(|t| t.set == RunSet::Pool).map(|t| t.id.as_str()).collect();
    let holdout = low.iter().filter(|t| t.set == RunSet::Holdout).count();
    let canary = low.iter().filter(|t| t.set == RunSet::Canary).count();
    let lowest = low.iter().map(|t| t.task.limits.max_cost_musd).min().unwrap_or(0);
    Some(format!(
        "G4 did not start: {} frozen task(s) have limits.max_cost_musd below one model request's reservation ({min} µUSD for {}; lowest cap {lowest} µUSD): \
         pool {pool:?}, {holdout} holdout, {canary} canary. No run under such a cap can make a request. The owner or maintainer must raise those caps \
         (frozen tasks are not agent-editable); the verdict is `error: task_cost_cap` and the incumbent stays.",
        low.len(),
        plan.model
    ))
}

struct Shared<'a> {
    plan: &'a G4Plan,
    ex: &'a dyn RunExecutor,
    runs: Mutex<Vec<RunRecord>>,
    stop: AtomicBool,
    infra_fail: AtomicBool,
    next: AtomicUsize,
    done_per_task: Mutex<Vec<u32>>,
    completed_order: Mutex<Vec<usize>>,
    mid_fired: AtomicBool,
    futility: Mutex<Option<FutilityLook>>,
    timed_out: AtomicBool,
    /// Incumbent runs planned so far (pairs and the top-up): the base of [`run_error_bound`].
    planned: AtomicUsize,
    t0: Instant,
}

impl Shared<'_> {
    /// One run with the infra re-run (§4.4: re-run once; a second `infra` ends G4 with `error`).
    fn one(&self, arm: Arm, t: &TaskDef, r: u32) -> RunRecord {
        let id = run_id(arm, &t.id, r);
        let rec = self.ex.run(arm, t, r, &id);
        if rec.outcome != "infra" {
            return rec;
        }
        let again = self.ex.run(arm, t, r, &id);
        if again.outcome == "infra" {
            self.infra_fail.store(true, Ordering::SeqCst);
            self.stop.store(true, Ordering::SeqCst);
        }
        again
    }
}

fn seeded_pairs(
    gate_id: &str,
    n_tasks: usize,
    repeats: std::ops::RangeInclusive<u32>,
) -> Vec<(usize, u32)> {
    let mut v: Vec<(usize, u32)> = (0..n_tasks)
        .flat_map(|i| repeats.clone().map(move |r| (i, r)))
        .collect();
    SplitMix64::for_gate(gate_id, "order").shuffle(&mut v);
    v
}

/// Run the pairs with up to `parallel` runs at once (both arms of a pair together).
fn run_pairs(sh: &Shared<'_>, pairs: &[(usize, u32)], need: u32, mid: &(dyn Fn() + Sync)) {
    sh.next.store(0, Ordering::SeqCst);
    let workers = (sh.plan.parallel / 2).max(1).min(pairs.len().max(1));
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    if sh.stop.load(Ordering::SeqCst) {
                        return;
                    }
                    if sh.t0.elapsed().as_secs() >= sh.plan.wall_s {
                        sh.timed_out.store(true, Ordering::SeqCst);
                        sh.stop.store(true, Ordering::SeqCst);
                        return;
                    }
                    let k = sh.next.fetch_add(1, Ordering::SeqCst);
                    let Some(&(i, r)) = pairs.get(k) else { return };
                    let t = &sh.plan.tasks[i];
                    let (c, p) = std::thread::scope(|s2| {
                        let hc = s2.spawn(|| sh.one(Arm::C, t, r));
                        let hp = s2.spawn(|| sh.one(Arm::P, t, r));
                        (hc.join().expect("run c"), hp.join().expect("run p"))
                    });
                    {
                        let mut runs = sh.runs.lock().unwrap();
                        runs.extend([c, p]);
                        let e = RunErrors::of(&runs);
                        if e.p.0 > run_error_bound(sh.planned.load(Ordering::SeqCst)) {
                            sh.stop.store(true, Ordering::SeqCst);
                        }
                    }
                    let fire = {
                        let mut done = sh.done_per_task.lock().unwrap();
                        done[i] += 1;
                        if done[i] == need {
                            let mut order = sh.completed_order.lock().unwrap();
                            order.push(i);
                            let half = sh.plan.tasks.len().div_ceil(2);
                            order.len() == half && !sh.mid_fired.swap(true, Ordering::SeqCst)
                        } else {
                            false
                        }
                    };
                    if fire {
                        mid();
                        futility_look(sh);
                    }
                }
            });
        }
    });
}

/// §3.8.4: over the first ⌈n/2⌉ completed tasks, with δ from `noise.json` history only.
fn futility_look(sh: &Shared<'_>) {
    let plan = sh.plan;
    let Some(hist) = plan.history.as_ref() else {
        return;
    };
    let delta = hist
        .recent(
            &plan.model,
            "pass_rate",
            plan.now_unix,
            plan.noise_history_days,
        )
        .and_then(NoiseEntry::delta);
    let order = sh.completed_order.lock().unwrap().clone();
    let runs = sh.runs.lock().unwrap();
    let d: Vec<f64> = order
        .iter()
        .map(|&i| {
            let id = &plan.tasks[i].id;
            let mean = |a: &str| {
                let v: Vec<f64> = runs
                    .iter()
                    .filter(|r| &r.task == id && r.arm == a)
                    .map(|r| r.score_permille as f64 / 1000.0)
                    .collect();
                if v.is_empty() {
                    0.0
                } else {
                    v.iter().sum::<f64>() / v.len() as f64
                }
            };
            mean("c") - mean("p")
        })
        .collect();
    drop(runs);
    let look = futility(
        &plan.gate_id,
        &d,
        delta,
        plan.config.margin_factor,
        plan.config.bootstrap_b,
        plan.futility_alpha,
    );
    if look.is_some_and(|l| l.stop) {
        sh.stop.store(true, Ordering::SeqCst);
    }
    *sh.futility.lock().unwrap() = look;
}

fn task_set(s: RunSet) -> TaskSet {
    match s {
        RunSet::Holdout => TaskSet::Holdout,
        _ => TaskSet::Pool,
    }
}

/// The judged data from the runs; agent tasks whose same arm disagrees with itself are dropped as
/// flaky (§3.7.5 rule 3).
fn data(
    plan: &G4Plan,
    runs: &[RunRecord],
    canaries: Vec<u32>,
    top_up_allowed: bool,
) -> (G4Data, Vec<String>) {
    let mut tasks = Vec::new();
    let mut flaky = Vec::new();
    for t in &plan.tasks {
        // Pairs (task, repeat) in repeat order. K-GATE: a run with no model request is not a score
        // (`run_error`). An incumbent error leaves the whole task out (dropping one pair would leave
        // the task with fewer repeats than the others, and the noise estimate needs them equal). A
        // candidate error where the incumbent ran counts as a failed candidate run: score 0, with
        // the incumbent's cost, wall and token values, so a run that never ran can neither help nor
        // look `cheaper`.
        let one = |a: &str, r: u32| runs.iter().find(|x| x.task == t.id && x.arm == a && x.repeat == r);
        let mut reps: Vec<u32> = runs.iter().filter(|x| x.task == t.id).map(|x| x.repeat).collect();
        reps.sort_unstable();
        reps.dedup();
        if reps.iter().any(|&r| one("p", r).is_some_and(|x| run_error(x).is_some())) {
            continue;
        }
        let (mut c, mut p) = (Vec::new(), Vec::new());
        for r in reps {
            let (Some(rc), Some(rp)) = (one("c", r), one("p", r)) else { continue };
            let vp = rp.values();
            let vc = if run_error(rc).is_some() { RunValues { score_permille: 0, ..vp } } else { rc.values() };
            c.push(vc);
            p.push(vp);
        }
        let disagrees = |v: &[RunValues]| {
            v.windows(2)
                .any(|w| w[0].score_permille != w[1].score_permille)
        };
        if t.source == Source::Agent && (disagrees(&c) || disagrees(&p)) {
            flaky.push(t.id.clone());
            continue;
        }
        tasks.push(TaskRuns {
            id: t.id.clone(),
            set: task_set(t.set),
            source: t.source,
            family: t.family.clone(),
            c,
            p,
        });
    }
    let g = G4Data {
        gate_id: plan.gate_id.clone(),
        change_type: plan.change_type,
        capability_labels: plan.capability_labels.clone(),
        tasks,
        canaries,
        capability: Vec::new(),
        repro: Vec::new(),
        body_tokens_c: plan.body_tokens_c,
        body_tokens_p: plan.body_tokens_p,
        budget: plan.budget.clone(),
        top_up_allowed,
        config: plan.config.clone(),
    };
    (g, flaky)
}

/// Run G4. `mid` is called right after the run that completes the ⌈n/2⌉-th task, before the
/// futility look (the `gate.g4.mid` fault point).
pub fn run_g4(plan: &G4Plan, ex: &dyn RunExecutor, mid: &(dyn Fn() + Sync)) -> G4Outcome {
    let t0 = Instant::now();
    let sh = Shared {
        plan,
        ex,
        runs: Mutex::new(Vec::new()),
        stop: AtomicBool::new(false),
        infra_fail: AtomicBool::new(false),
        next: AtomicUsize::new(0),
        done_per_task: Mutex::new(vec![0; plan.tasks.len()]),
        completed_order: Mutex::new(Vec::new()),
        mid_fired: AtomicBool::new(false),
        futility: Mutex::new(None),
        timed_out: AtomicBool::new(false),
        planned: AtomicUsize::new(plan.tasks.len() * plan.repeats as usize),
        t0,
    };
    let finish = |sh: &Shared<'_>,
                  verdict: Verdict,
                  eval: Option<G4Eval>,
                  deltas,
                  store,
                  flaky,
                  top_up,
                  repeats| G4Outcome {
        verdict,
        eval,
        deltas,
        noise_store: store,
        runs: sh.runs.lock().unwrap().clone(),
        flaky,
        top_up,
        repeats,
        futility: *sh.futility.lock().unwrap(),
        wall_ms: t0.elapsed().as_millis() as u64,
        note: String::new(),
        v2: None,
    };
    // Runs that never ran (K-GATE): `error: runs_failed:<cause>`, with the note.
    let failed = |sh: &Shared<'_>, flaky: Vec<String>, top_up: bool, repeats: u32| -> Option<G4Outcome> {
        let runs = sh.runs.lock().unwrap().clone();
        let planned = sh.planned.load(Ordering::SeqCst);
        let why = runs_failed(&runs, planned)?;
        let mut o = finish(sh, Verdict::Error(why), None, None, BTreeMap::new(), flaky, top_up, repeats);
        o.note = runs_failed_note(&runs, planned, why);
        Some(o)
    };
    // No pool and no holdout task at all (a fresh install without S): no G4 evidence (ADR-013 §6).
    if plan.tasks.is_empty() {
        let mut o = finish(&sh, Verdict::Inconclusive(InconclusiveReason::NoInstrument), None, None, BTreeMap::new(), vec![], false, plan.repeats);
        o.note = "G4 has no instrument: no sealed S is drawn and the frozen pool and holdout hold no task".into();
        return o;
    }
    // A task whose cap is below one request's reservation can never run (K-GATE): refuse to start.
    if let Some(note) = task_cap_problem(plan) {
        let mut o = finish(&sh, Verdict::Error(ErrorReason::TaskCostCap), None, None, BTreeMap::new(), vec![], false, plan.repeats);
        o.note = note;
        return o;
    }
    // Canaries first, R = 1, candidate arm only (§3.7.1). A perfect score stops G4.
    let mut canary_scores = Vec::new();
    for t in &plan.canaries {
        let rec = sh.one(Arm::C, t, 1);
        canary_scores.push(rec.score_permille);
        sh.runs.lock().unwrap().push(rec);
        if *canary_scores.last().unwrap() == 1000 {
            return finish(
                &sh,
                Verdict::Reject(RejectReason::Canary),
                None,
                None,
                BTreeMap::new(),
                vec![],
                false,
                plan.repeats,
            );
        }
        if sh.infra_fail.load(Ordering::SeqCst) {
            return finish(
                &sh,
                Verdict::Error(ErrorReason::Infra),
                None,
                None,
                BTreeMap::new(),
                vec![],
                false,
                plan.repeats,
            );
        }
    }
    let pairs = seeded_pairs(&plan.gate_id, plan.tasks.len(), 1..=plan.repeats);
    run_pairs(&sh, &pairs, plan.repeats, mid);
    if sh.infra_fail.load(Ordering::SeqCst) {
        return finish(
            &sh,
            Verdict::Error(ErrorReason::Infra),
            None,
            None,
            BTreeMap::new(),
            vec![],
            false,
            plan.repeats,
        );
    }
    if let Some(o) = failed(&sh, vec![], false, plan.repeats) {
        return o;
    }
    if sh.futility.lock().unwrap().is_some_and(|l| l.stop) {
        let v = accept::futility_verdict(*sh.futility.lock().unwrap()).expect("stop");
        return finish(
            &sh,
            v,
            None,
            None,
            BTreeMap::new(),
            vec![],
            false,
            plan.repeats,
        );
    }
    if sh.timed_out.load(Ordering::SeqCst) {
        return finish(
            &sh,
            Verdict::Inconclusive(InconclusiveReason::Timeout),
            None,
            None,
            BTreeMap::new(),
            vec![],
            false,
            plan.repeats,
        );
    }
    let hist_ctx = plan.history.as_ref().map(|h| HistoryCtx {
        history: h,
        model: &plan.model,
        now_unix: plan.now_unix,
        max_age_days: plan.noise_history_days,
    });
    let mut repeats = plan.repeats;
    let mut top_up = false;
    loop {
        let runs = sh.runs.lock().unwrap().clone();
        let (g, flaky) = data(
            plan,
            &runs,
            canary_scores.clone(),
            !top_up && repeats < plan.repeats_max,
        );
        let (eval, deltas, store) = accept::judge(&g, hist_ctx.as_ref(), &plan.now_rfc3339);
        match eval.decision {
            Decision::Verdict(v) => {
                return finish(&sh, v, Some(eval), deltas, store, flaky, top_up, repeats);
            }
            Decision::TopUp { .. } => {
                // One more repeat for both arms on every task, then judge again (§3.8.3).
                top_up = true;
                repeats += 1;
                let pairs = seeded_pairs(
                    &format!("{}/top-up", plan.gate_id),
                    plan.tasks.len(),
                    repeats..=repeats,
                );
                *sh.done_per_task.lock().unwrap() = vec![0; plan.tasks.len()];
                sh.planned.fetch_add(plan.tasks.len(), Ordering::SeqCst);
                run_pairs(&sh, &pairs, u32::MAX, &|| {});
                if sh.infra_fail.load(Ordering::SeqCst) {
                    return finish(
                        &sh,
                        Verdict::Error(ErrorReason::Infra),
                        None,
                        None,
                        BTreeMap::new(),
                        flaky,
                        top_up,
                        repeats,
                    );
                }
                if let Some(o) = failed(&sh, flaky.clone(), top_up, repeats) {
                    return o;
                }
                if sh.timed_out.load(Ordering::SeqCst) {
                    return finish(
                        &sh,
                        Verdict::Inconclusive(InconclusiveReason::Timeout),
                        None,
                        None,
                        BTreeMap::new(),
                        flaky,
                        top_up,
                        repeats,
                    );
                }
            }
        }
    }
}

/// The `stages.G4` object of the gate record (§3.13): permille-rounded statistics. Holdout ids
/// never appear (only counts and aggregates).
pub fn stage_record(o: &G4Outcome, holdout_visible: bool) -> Value {
    let pm = crate::record::permille;
    // D14: the measured δ (the split guard's), its floor and the floored δ the divergence guard used.
    let split = |x: &crate::accept::SplitStats| {
        serde_json::json!({"d_permille": pm(x.d), "delta_permille": pm(x.delta),
                           "delta_min_permille": pm(x.delta_min), "delta_floored_permille": pm(x.delta_floored)})
    };
    let mut v = serde_json::json!({
        "status": o.verdict_kind(), "reason": o.verdict_reason(),
        "repeats": o.repeats, "top_up": o.top_up,
        "sets": {"holdout_visible": holdout_visible, "canaries_visible": holdout_visible},
        "runs": o.runs.len(), "spent_musd": o.spent_musd(), "cost_ref_musd": o.cost_ref_musd(),
        "wall_ms": o.wall_ms, "flaky": o.flaky.len(),
    });
    let e = RunErrors::of(&o.runs);
    v["run_errors"] = serde_json::json!({"c": e.c.0, "p": e.p.0, "canary": e.canary.0, "bound": run_error_bound(e.p.1)});
    if let Some(e) = &o.eval {
        let n = &e.stats.n;
        v["n"] = serde_json::json!({"agent": n.agent, "canaries": n.canaries, "capability": n.capability,
                                    "holdout": n.holdout, "pool": n.pool, "repro": n.repro});
        v["canary_max_permille"] = e.stats.canary_max_permille.into();
        v["audit_required"] = e.audit_required.into();
        if let Some(s) = &e.stats.score {
            v["score"] = serde_json::json!({
                "d_permille": pm(s.d), "l_permille": pm(s.l), "u_permille": pm(s.u),
                "margin_permille": pm(s.margin), "delta_permille": pm(s.delta),
                "delta_min_permille": pm(s.delta_min),
                "c_permille": pm(s.c), "p_permille": pm(s.p),
                "pool": split(&s.pool), "holdout": split(&s.holdout),
            });
        }
        if let Some(ct) = &e.stats.ct {
            v["metrics"] = serde_json::json!({ "cost_x_wall": {
                "effect_log_ppm": (ct.effect * 1e6).round() as i64,
                "delta_log_ppm": (ct.delta * 1e6).round() as i64,
                "class": serde_json::to_value(ct.class).unwrap_or(Value::Null)}});
        }
        if let Some(c) = &e.stats.capability {
            v["capability"] = serde_json::to_value(c)
                .map(crate::record::integers_only)
                .unwrap_or(Value::Null);
        }
    }
    if let Some(f) = &o.futility {
        v["futility"] = serde_json::json!({"u99_permille": pm(f.u99), "margin_permille": pm(f.margin), "stop": f.stop});
    }
    // G4 record v3 (M7): `tier`, `screen`, `sealed`, `cpp`, `milestone_required`. Absent on the
    // v0.3 path except `sealed: null`, so a v3 reader can tell the paths apart.
    match &o.v2 {
        Some(x) => x.extend(&mut v),
        None => v["sealed"] = Value::Null,
    }
    v
}

impl G4Outcome {
    pub fn verdict_kind(&self) -> &'static str {
        self.verdict.kind()
    }
    pub fn verdict_reason(&self) -> &'static str {
        self.verdict.reason()
    }
}

/// A shared, thread-safe executor wrapper for tests and drivers that count runs.
pub struct Counting<E> {
    pub inner: E,
    pub n: Arc<AtomicUsize>,
}

impl<E: RunExecutor> RunExecutor for Counting<E> {
    fn run(&self, arm: Arm, task: &TaskDef, repeat: u32, run_id: &str) -> RunRecord {
        self.n.fetch_add(1, Ordering::SeqCst);
        self.inner.run(arm, task, repeat, run_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accept::AcceptReason;

    fn tj(id: &str) -> TaskJson {
        TaskJson {
            v: 2,
            id: id.into(),
            title: None,
            prompt: "do it".into(),
            labels: vec!["x".into()],
            workspace: WorkspaceRef {
                file: "workspace.tar".into(),
                sha256: "0".repeat(64),
                bytes: 1,
            },
            check: CheckSpec {
                argv: vec!["check/run".into()],
                timeout_s: 10,
            },
            limits: Limits {
                wall_s: 60,
                max_cost_musd: 500_000,
                max_turns: None,
                max_output_tokens: None,
                max_input_tokens: None,
            },
            source: SourceRef {
                kind: Source::Owner,
                session: None,
                event_id: None,
                author_sha: None,
            },
            accepted: None,
            created: "2026-10-20T00:00:00Z".into(),
            family: None,
            template: None,
            difficulty: None,
        }
    }

    fn task(id: &str, set: RunSet) -> TaskDef {
        TaskDef {
            id: id.into(),
            set,
            source: Source::Owner,
            labels: vec!["x".into()],
            family: None,
            dir: PathBuf::new(),
            task: tj(id),
        }
    }

    /// Scores from a table `(task, arm) → permille`; outcome `infra` for listed (task, arm, repeat).
    struct Table {
        score: BTreeMap<(String, &'static str), u32>,
        infra: Mutex<Vec<(String, &'static str, u32)>>,
    }

    impl RunExecutor for Table {
        fn run(&self, arm: Arm, t: &TaskDef, r: u32, id: &str) -> RunRecord {
            let a = if arm == Arm::C { "c" } else { "p" };
            let mut infra = self.infra.lock().unwrap();
            if let Some(i) = infra
                .iter()
                .position(|x| x.0 == t.id && x.1 == a && x.2 == r)
            {
                infra.remove(i);
                return RunRecord {
                    outcome: "infra".into(),
                    run_id: id.into(),
                    task: t.id.clone(),
                    arm: a.into(),
                    repeat: r,
                    ..Default::default()
                };
            }
            RunRecord {
                v: 2,
                run_id: id.into(),
                arm: a.into(),
                task: t.id.clone(),
                set: serde_json::to_value(t.set)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .into(),
                repeat: r,
                outcome: "done".into(),
                score_permille: *self.score.get(&(t.id.clone(), a)).unwrap_or(&0),
                wall_ms: 1000,
                cost_ref_musd: 100,
                requests: 1,
                ..Default::default()
            }
        }
    }

    fn plan(tasks: Vec<TaskDef>, canaries: Vec<TaskDef>) -> G4Plan {
        G4Plan {
            gate_id: "j-000001".into(),
            tasks,
            canaries,
            repeats: 2,
            repeats_max: 3,
            parallel: 4,
            config: G4Config {
                min_pool: 2,
                min_holdout: 1,
                min_canaries: 1,
                margin_max: 0.2,
                bootstrap_b: 2000,
                noise_b: 500,
                ..G4Config::default()
            },
            futility_alpha: 0.01,
            noise_history_days: 30,
            budget: CostBudget {
                soft_day_musd: 4_000_000,
                evolution_share_permille: 600,
                tasks_per_day: 10.0,
            },
            change_type: ChangeType::Feat,
            capability_labels: vec![],
            body_tokens_c: 100,
            body_tokens_p: 100,
            history: None,
            model: "deepseek-flash".into(),
            now_unix: 1_792_454_400,
            now_rfc3339: "2026-10-20T00:00:00Z".into(),
            wall_s: 5400,
            request_reservation_musd: None,
        }
    }

    fn table(rows: &[(&str, u32, u32)]) -> Table {
        let mut score = BTreeMap::new();
        for (id, c, p) in rows {
            score.insert((id.to_string(), "c"), *c);
            score.insert((id.to_string(), "p"), *p);
        }
        Table {
            score,
            infra: Mutex::new(vec![]),
        }
    }

    #[test]
    fn score_rules() {
        assert_eq!(score(Some(0), false, b""), 1000);
        assert_eq!(
            score(Some(0), false, b"ok\n{\"score_permille\":750}\n"),
            750
        );
        assert_eq!(score(Some(0), false, b"{\"score_permille\":1001}"), 1000);
        assert_eq!(score(Some(0), false, b"{\"score_permille\": 5}"), 1000);
        assert_eq!(score(Some(1), false, b"{\"score_permille\":750}"), 0);
        assert_eq!(score(Some(0), true, b""), 0);
        assert_eq!(score(None, false, b""), 0);
    }

    /// M7: the tamper score line (m7-design.md §3.1.3).
    #[test]
    fn check_score_tamper_line() {
        let t = |out: &[u8]| check_score(Some(0), false, out);
        assert_eq!(t(b"{\"score_permille\":0,\"tamper\":true}\n"), CheckScore { score_permille: 0, tamper: true });
        assert_eq!(t(b"{\"score_permille\":700}"), CheckScore { score_permille: 700, tamper: false });
        assert_eq!(t(b"{\"score_permille\":500,\"tamper\":true}"), CheckScore { score_permille: 1000, tamper: false });
        // The shared runner exits 3 after the tamper line: still a tamper (S1-CORE f5da529).
        assert_eq!(check_score(Some(3), false, b"{\"score_permille\":0,\"tamper\":true}\n"), CheckScore { score_permille: 0, tamper: true });
        assert_eq!(check_score(Some(3), true, b"{\"score_permille\":0,\"tamper\":true}"), CheckScore::default(), "a timeout never flags");
        assert_eq!(check_score(Some(1), false, b"{\"score_permille\":700}"), CheckScore::default());
        assert_eq!(score(Some(0), false, b"{\"score_permille\":0,\"tamper\":true}"), 0);
    }

    /// M7-G14: task.json v3; a v2 file reads as v3 with no caps.
    #[test]
    fn m7_g14_task_json_v3() {
        let home = crate::testutil::test_home("g4v3-task");
        let write = |dir: &Path, v: Value| {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("workspace.tar"), b"x").unwrap();
            std::fs::write(dir.join("task.json"), serde_json::to_vec(&v).unwrap()).unwrap();
        };
        let ws: String = Sha256::digest(b"x").iter().map(|b| format!("{b:02x}")).collect();
        let base = |v: u32| serde_json::json!({"v": v, "id": "e-1", "prompt": "do", "labels": ["x"],
            "workspace": {"file": "workspace.tar", "sha256": ws, "bytes": 1},
            "check": {"argv": ["check/run"], "timeout_s": 60}, "limits": {"wall_s": 600, "max_cost_musd": 100000},
            "source": {"kind": "maintainer"}, "created": "2026-10-20T00:00:00Z"});
        let d = home.join("v2");
        write(&d, base(2));
        let t = load_task(&d).unwrap();
        assert_eq!((t.limits.max_turns, t.limits.max_output_tokens, t.limits.max_input_tokens, t.family.clone()), (None, None, None, None));
        let mut v3 = base(3);
        v3["limits"]["max_turns"] = 8.into();
        v3["limits"]["max_output_tokens"] = 24_000.into();
        v3["limits"]["max_input_tokens"] = 400_000.into();
        v3["family"] = "E".into();
        v3["template"] = "e-py-word-count".into();
        v3["difficulty"] = 3.into();
        let d = home.join("v3");
        write(&d, v3.clone());
        let t = load_task(&d).unwrap();
        assert_eq!((t.limits.max_turns, t.limits.max_output_tokens, t.limits.max_input_tokens), (Some(8), Some(24_000), Some(400_000)));
        assert_eq!((t.family.as_deref(), t.template.as_deref(), t.difficulty), (Some("E"), Some("e-py-word-count"), Some(3)));
        // Round trip keeps the v2 shape for v2 files (no null v3 keys).
        let again = serde_json::to_value(load_task(&home.join("v2")).unwrap()).unwrap();
        assert!(again.get("family").is_none() && again["limits"].get("max_turns").is_none());
        for (k, bad) in [("max_turns", 0), ("max_turns", 101), ("max_output_tokens", 999), ("max_input_tokens", 9_999)] {
            let mut b = v3.clone();
            b["limits"][k] = bad.into();
            let d = home.join(format!("bad-{k}-{bad}"));
            write(&d, b);
            assert!(load_task(&d).is_err(), "{k}={bad}");
        }
        for (k, bad) in [("template", Value::from("Bad Name")), ("difficulty", Value::from(6))] {
            let mut b = v3.clone();
            b[k] = bad;
            let d = home.join(format!("bad-{k}"));
            write(&d, b);
            assert!(load_task(&d).is_err(), "{k}");
        }
        let mut v2caps = base(2);
        v2caps["limits"]["max_turns"] = 8.into();
        let d = home.join("v2caps");
        write(&d, v2caps);
        assert!(load_task(&d).unwrap_err().contains("v3 keys"));
        let mut v4 = base(4);
        v4["v"] = 4.into();
        let d = home.join("v4");
        write(&d, v4);
        assert!(load_task(&d).is_err());
        crate::testutil::remove_home(&home);
    }

    /// M7-G12 (RunRecord v3): a v2 line parses with the v3 keys defaulted; v3 keys serialize.
    #[test]
    fn m7_g12_run_record_v3() {
        let v2 = r#"{"v":2,"gate_id":"j-1","run_id":"c-t-r1","arm":"c","sha":"x","level":"L0","task":"t","set":"pool","source":"owner","labels":[],"repeat":1,"outcome":"done","score_permille":1000,"wall_ms":1,"ready_ms":1,"turns":1,"requests":1,"tokens_in":1,"tokens_out":1,"cache_read":0,"input_total":1,"cost_ref_musd":1,"spent_musd":1,"gap_ms_p50":0,"ttft_ms_p50":0,"check_ms":0,"exit_code":0,"models":{}}"#;
        let r: RunRecord = serde_json::from_str(v2).unwrap();
        assert_eq!((r.tamper, r.caps_hit.clone(), r.cache.clone(), r.model_served.clone()), (false, None, None, None));
        let v3 = RunRecord { v: RUN_RECORD_V, tamper: true, caps_hit: Some("turns".into()), cache: Some("reused".into()), model_served: Some("deepseek-v4-flash".into()), ..r.clone() };
        let j = serde_json::to_value(&v3).unwrap();
        assert_eq!((j["v"].as_u64(), j["tamper"].as_bool(), j["caps_hit"].as_str(), j["cache"].as_str()), (Some(3), Some(true), Some("turns"), Some("reused")));
        let old = serde_json::to_value(&r).unwrap();
        for (k, x) in serde_json::from_str::<Value>(v2).unwrap().as_object().unwrap() {
            assert_eq!(&old[k], x, "v2 key {k} unchanged");
        }
        // The v0.3 path's stage record says `sealed: null`.
        let o = run_g4(&plan(vec![task("t1", RunSet::Pool)], vec![task("k1", RunSet::Canary)]), &table(&[("k1", 1000, 0)]), &|| {});
        assert!(stage_record(&o, false)["sealed"].is_null());
    }

    #[test]
    fn better_candidate_is_accepted_and_runs_are_paired() {
        // 4 holdout tasks: a `better` needs a holdout of at least `GAIN_MIN_HOLDOUT` (D14).
        let tasks = vec![
            task("t1", RunSet::Pool),
            task("t2", RunSet::Pool),
            task("h1", RunSet::Holdout),
            task("h2", RunSet::Holdout),
            task("h3", RunSet::Holdout),
            task("h4", RunSet::Holdout),
        ];
        let ex = Counting {
            inner: table(&[
                ("t1", 1000, 0),
                ("t2", 1000, 0),
                ("h1", 1000, 0),
                ("h2", 1000, 0),
                ("h3", 1000, 0),
                ("h4", 1000, 0),
                ("k1", 0, 0),
            ]),
            n: Arc::new(AtomicUsize::new(0)),
        };
        let mids = AtomicUsize::new(0);
        let o = run_g4(&plan(tasks, vec![task("k1", RunSet::Canary)]), &ex, &|| {
            mids.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(o.verdict, Verdict::Accept(AcceptReason::Better), "{o:?}");
        assert_eq!(ex.n.load(Ordering::SeqCst), 1 + 6 * 2 * 2);
        assert_eq!(mids.load(Ordering::SeqCst), 1, "gate.g4.mid fires once");
        assert_eq!(o.runs.iter().filter(|r| r.set == "canary").count(), 1);
        let rec = stage_record(&o, false);
        assert_eq!(rec["status"], "accept");
        assert_eq!(rec["n"]["pool"], 2);
        assert_eq!(rec["score"]["d_permille"], 1000);
        let red: Vec<RunRecord> = o.runs.iter().map(RunRecord::redacted).collect();
        assert!(
            red.iter()
                .all(|r| !["h1", "h2", "h3", "h4"].contains(&r.task.as_str())),
            "holdout ids redacted"
        );
    }

    #[test]
    fn g24_canary_stops_first() {
        let tasks = vec![task("t1", RunSet::Pool)];
        let ex = Counting {
            inner: table(&[("k1", 1000, 0)]),
            n: Arc::new(AtomicUsize::new(0)),
        };
        let o = run_g4(&plan(tasks, vec![task("k1", RunSet::Canary)]), &ex, &|| {});
        assert_eq!(o.verdict, Verdict::Reject(RejectReason::Canary));
        assert_eq!(
            ex.n.load(Ordering::SeqCst),
            1,
            "nothing runs after a canary trip"
        );
    }

    /// ADR-013 §6: no pool and no holdout task → `inconclusive: no_instrument` before any run (canaries
    /// included), and `resolve` makes it a final `accept: no_instrument` with no escalation.
    #[test]
    fn no_instrument_is_no_evidence_not_a_rollback() {
        let ex = Counting { inner: table(&[]), n: Arc::new(AtomicUsize::new(0)) };
        let o = run_g4(&plan(vec![], vec![task("k1", RunSet::Canary)]), &ex, &|| {});
        assert_eq!(o.verdict, Verdict::Inconclusive(InconclusiveReason::NoInstrument));
        assert_eq!(ex.n.load(Ordering::SeqCst), 0, "nothing runs");
        assert!(o.note.contains("no instrument"), "{}", o.note);
        for escalated in [false, true] {
            assert_eq!(accept::resolve(o.verdict, escalated, 4),
                accept::Resolution::Final(Verdict::Accept(accept::AcceptReason::NoInstrument)));
        }
        assert_eq!(Verdict::Accept(accept::AcceptReason::NoInstrument).reason(), "no_instrument");
        // one pool task is an instrument: the old path (insufficient → escalate) still applies
        let o = run_g4(&plan(vec![task("t1", RunSet::Pool)], vec![]), &table(&[]), &|| {});
        assert_eq!(accept::resolve(o.verdict, false, 4), accept::Resolution::Escalate { repeats: 4 });
    }

    #[test]
    fn insufficient_and_infra() {
        let o = run_g4(
            &plan(
                vec![task("t1", RunSet::Pool)],
                vec![task("k1", RunSet::Canary)],
            ),
            &table(&[]),
            &|| {},
        );
        assert_eq!(
            o.verdict,
            Verdict::Inconclusive(InconclusiveReason::Insufficient)
        );
        let t = table(&[("t1", 1000, 1000)]);
        t.infra.lock().unwrap().push(("t1".into(), "c", 1));
        let tasks = vec![
            task("t1", RunSet::Pool),
            task("t2", RunSet::Pool),
            task("h1", RunSet::Holdout),
        ];
        let o = run_g4(
            &plan(tasks.clone(), vec![task("k1", RunSet::Canary)]),
            &t,
            &|| {},
        );
        assert_ne!(
            o.verdict,
            Verdict::Error(ErrorReason::Infra),
            "one infra run is re-run"
        );
        let t = table(&[]);
        t.infra
            .lock()
            .unwrap()
            .extend([("t1".to_string(), "c", 1), ("t1".to_string(), "c", 1)]);
        let o = run_g4(&plan(tasks, vec![task("k1", RunSet::Canary)]), &t, &|| {});
        assert_eq!(o.verdict, Verdict::Error(ErrorReason::Infra));
    }

    /// A [`Table`] whose listed runs end `cost_capped` with no model request and cost 0 (the
    /// untouched workspace still passes a task check: score 1000; a canary's leak check does not),
    /// as S-REAL's j-000002 did.
    struct Capped {
        t: Table,
        all: bool,
        runs: Vec<(&'static str, &'static str, u32)>,
    }

    impl RunExecutor for Capped {
        fn run(&self, arm: Arm, t: &TaskDef, r: u32, id: &str) -> RunRecord {
            let mut rec = self.t.run(arm, t, r, id);
            let a = if arm == Arm::C { "c" } else { "p" };
            if self.all || self.runs.iter().any(|x| x.0 == t.id && x.1 == a && x.2 == r) {
                rec.outcome = "cost_capped".into();
                rec.requests = 0;
                rec.cost_ref_musd = 0;
                if t.set != RunSet::Canary {
                    rec.score_permille = 1000; // an untouched workspace can pass a check
                }
            }
            rec
        }
    }

    fn six() -> Vec<TaskDef> {
        vec![
            task("t1", RunSet::Pool),
            task("t2", RunSet::Pool),
            task("h1", RunSet::Holdout),
            task("h2", RunSet::Holdout),
            task("h3", RunSet::Holdout),
            task("h4", RunSet::Holdout),
        ]
    }

    fn better_rows() -> Table {
        table(&[("t1", 1000, 0), ("t2", 1000, 0), ("h1", 1000, 0), ("h2", 1000, 0), ("h3", 1000, 0), ("h4", 1000, 0), ("k1", 0, 0)])
    }

    /// (1) A run with no model request is a run error, not a score; one with a request is a score.
    #[test]
    fn a_run_without_a_model_request_is_an_error() {
        let r = |outcome: &str, requests: u64| RunRecord { outcome: outcome.into(), requests, ..Default::default() };
        assert_eq!(run_error(&r("cost_capped", 0)), Some(ErrorReason::RunsFailedCostCapped));
        assert_eq!(run_error(&r("no_ready", 0)), Some(ErrorReason::RunsFailedNoReady));
        assert_eq!(run_error(&r("done", 0)), Some(ErrorReason::RunsFailedNoRequest));
        assert_eq!(run_error(&r("cost_capped", 3)), None, "capped after real requests: a measured run");
        assert_eq!(run_error(&r("infra", 0)), None, "infra is re-run, then ends G4 itself");
        assert_eq!((run_error_bound(25), run_error_bound(5)), (2, 1));
        assert_eq!(Verdict::Error(ErrorReason::RunsFailedCostCapped).code().as_deref(), Some("gate.g4.runs_failed:cost_capped"));
        assert!(ErrorReason::Infra.retryable() && !ErrorReason::RunsFailedCostCapped.retryable() && !ErrorReason::TaskCostCap.retryable());
    }

    /// (2) S-REAL j-000002: every run capped with no request in both arms. Before: a verdict
    /// (`inconclusive: underpowered`) from runs that never ran. Now: `error: runs_failed:cost_capped`
    /// once the incumbent's errors pass the bound (no further run launched), with a note saying
    /// what to fix.
    #[test]
    fn every_run_capped_is_runs_failed_not_a_verdict() {
        let ex = Counting { inner: Capped { t: better_rows(), all: true, runs: vec![] }, n: Arc::new(AtomicUsize::new(0)) };
        let mut p = plan(six(), vec![task("k1", RunSet::Canary)]);
        p.parallel = 2;
        let o = run_g4(&p, &ex, &|| {});
        assert_eq!(o.verdict, Verdict::Error(ErrorReason::RunsFailedCostCapped), "{o:?}");
        assert!(ex.n.load(Ordering::SeqCst) < 1 + 6 * 2 * 2, "stopped launching");
        assert!(o.note.contains("runs_failed:cost_capped") && o.note.contains("max_cost_musd"), "{}", o.note);
        assert!(o.eval.is_none());
        // Only the pairs capped (the canary ran): the same.
        let capped: Vec<(&'static str, &'static str, u32)> = ["t1", "t2", "h1", "h2", "h3", "h4"].iter().flat_map(|t| [(*t, "c", 1), (*t, "p", 1), (*t, "c", 2), (*t, "p", 2)]).collect();
        let ex2 = Counting { inner: Capped { t: better_rows(), all: false, runs: capped }, n: Arc::new(AtomicUsize::new(0)) };
        let mut p = plan(six(), vec![task("k1", RunSet::Canary)]);
        p.parallel = 2;
        let o = run_g4(&p, &ex2, &|| {});
        assert_eq!(o.verdict, Verdict::Error(ErrorReason::RunsFailedCostCapped), "{o:?}");
        assert!(ex2.n.load(Ordering::SeqCst) < 1 + 6 * 2 * 2, "stopped launching");
        let rec = stage_record(&o, false);
        assert_eq!(rec["status"], "error");
        assert_eq!(rec["reason"], "runs_failed:cost_capped");
        assert!(rec["run_errors"]["c"].as_u64().unwrap() >= 1 && rec["run_errors"]["p"].as_u64().unwrap() >= 1, "{rec}");
    }

    /// (2) A few errors within the bound do not void G4: a task with an incumbent error is left
    /// out, and the verdict stands on the rest.
    #[test]
    fn errors_within_the_bound_leave_their_task_out() {
        let mut tasks = six();
        tasks.push(task("t3", RunSet::Pool));
        let t = table(&[("t1", 1000, 0), ("t2", 1000, 0), ("t3", 1000, 0), ("h1", 1000, 0), ("h2", 1000, 0), ("h3", 1000, 0), ("h4", 1000, 0), ("k1", 0, 0)]);
        let ex = Capped { t, all: false, runs: vec![("t1", "p", 1)] };
        let o = run_g4(&plan(tasks.clone(), vec![task("k1", RunSet::Canary)]), &ex, &|| {});
        assert_eq!(o.verdict, Verdict::Accept(AcceptReason::Better), "{o:?}");
        assert_eq!(stage_record(&o, false)["run_errors"]["p"], 1);
        let (g, _) = data(&plan(tasks, vec![]), &o.runs, vec![0], false);
        assert!(g.tasks.iter().all(|t| t.id != "t1"), "the task with an incumbent error is left out");
        assert_eq!(g.tasks.len(), 6);
    }

    /// The asymmetric case: only the candidate is capped (it asks for a bigger max_tokens). Its
    /// failed runs score 0 at the incumbent's cost, so they can neither help nor look `cheaper`;
    /// all of them failed → a reject (its own failure), never an accept.
    #[test]
    fn a_capped_candidate_cannot_win_cheaper() {
        let rows = || table(&[("t1", 1000, 1000), ("t2", 1000, 1000), ("h1", 1000, 1000), ("h2", 1000, 1000), ("h3", 1000, 1000), ("h4", 1000, 1000), ("k1", 0, 0)]);
        let ex = Capped { t: rows(), all: false, runs: vec![("t1", "c", 1)] };
        let o = run_g4(&plan(six(), vec![task("k1", RunSet::Canary)]), &ex, &|| {});
        assert_ne!(o.verdict, Verdict::Accept(AcceptReason::Cheaper), "{o:?}");
        let (g, _) = data(&plan(six(), vec![]), &o.runs, vec![0], false);
        let t1 = g.tasks.iter().find(|t| t.id == "t1").unwrap();
        assert_eq!(t1.c[0].score_permille, 0, "a failed candidate run is a failed run");
        assert_eq!(t1.c[0].cost_ref_musd, t1.p[0].cost_ref_musd, "at the incumbent's cost, never cheaper");
        // Every candidate run capped (the incumbent ran): judged and rejected, whatever the cost.
        struct OnlyC(Table);
        impl RunExecutor for OnlyC {
            fn run(&self, arm: Arm, t: &TaskDef, r: u32, id: &str) -> RunRecord {
                let mut rec = self.0.run(arm, t, r, id);
                if arm == Arm::C && t.id != "k1" {
                    rec.outcome = "cost_capped".into();
                    rec.requests = 0;
                    rec.cost_ref_musd = 0;
                }
                rec
            }
        }
        let o = run_g4(&plan(six(), vec![task("k1", RunSet::Canary)]), &OnlyC(rows()), &|| {});
        assert_eq!(o.verdict.kind(), "reject", "{o:?}");
        assert_eq!(stage_record(&o, false)["run_errors"]["c"], 12);
    }

    /// (3) A frozen task whose cap is below one request's reservation: G4 refuses to start, with a
    /// note that names pool ids but only counts holdout tasks and canaries.
    #[test]
    fn a_task_cap_below_one_reservation_refuses_g4() {
        let mut tasks = six();
        tasks[0].task.limits.max_cost_musd = 30_000;
        tasks[2].task.limits.max_cost_musd = 30_000;
        let mut k1 = task("k1", RunSet::Canary);
        k1.task.limits.max_cost_musd = 30_000;
        let mut p = plan(tasks, vec![k1]);
        p.request_reservation_musd = Some(78_644);
        let ex = Counting { inner: better_rows(), n: Arc::new(AtomicUsize::new(0)) };
        let o = run_g4(&p, &ex, &|| {});
        assert_eq!(o.verdict, Verdict::Error(ErrorReason::TaskCostCap));
        assert_eq!(o.verdict.code().as_deref(), Some("gate.g4.task_cost_cap"));
        assert_eq!(ex.n.load(Ordering::SeqCst), 0, "nothing ran");
        assert!(o.note.contains("[\"t1\"]") && o.note.contains("1 holdout") && o.note.contains("1 canary") && o.note.contains("78644"), "{}", o.note);
        assert!(!o.note.contains("h1") && !o.note.contains("k1"), "no holdout or canary id: {}", o.note);
        p.request_reservation_musd = Some(30_000);
        assert_eq!(task_cap_problem(&p), None, "a cap equal to the reservation is enough");
    }

    #[test]
    fn g23_futility_stops_early_with_history() {
        let tasks: Vec<TaskDef> = (0..8)
            .map(|i| {
                task(
                    &format!("t{i}"),
                    if i < 6 { RunSet::Pool } else { RunSet::Holdout },
                )
            })
            .collect();
        let rows: Vec<(String, u32, u32)> = (0..8).map(|i| (format!("t{i}"), 0, 1000)).collect();
        let rows: Vec<(&str, u32, u32)> =
            rows.iter().map(|(a, b, c)| (a.as_str(), *b, *c)).collect();
        let mut p = plan(tasks, vec![task("k1", RunSet::Canary)]);
        p.parallel = 2;
        let mut h = NoiseHistory::default();
        h.models.entry("deepseek-flash".into()).or_default().insert(
            "pass_rate".into(),
            NoiseEntry {
                delta_log_ppm: None,
                delta_permille: Some(50),
                n: 8,
                repeats: 2,
                at: "2026-10-19T00:00:00Z".into(),
            },
        );
        p.history = Some(h);
        let ex = Counting {
            inner: table(&rows),
            n: Arc::new(AtomicUsize::new(0)),
        };
        let o = run_g4(&p, &ex, &|| {});
        assert_eq!(
            o.verdict,
            Verdict::Reject(RejectReason::ScoreInferiorEarly),
            "{:?}",
            o.futility
        );
        assert!(
            ex.n.load(Ordering::SeqCst) < 1 + 8 * 2 * 2,
            "stopped launching"
        );
    }

    #[test]
    fn flaky_agent_task_is_left_out() {
        struct Flaky;
        impl RunExecutor for Flaky {
            fn run(&self, arm: Arm, t: &TaskDef, r: u32, id: &str) -> RunRecord {
                let s = if t.id == "k1" {
                    0
                } else if t.id == "a1" {
                    if r == 1 { 1000 } else { 0 }
                } else if arm == Arm::C {
                    1000
                } else {
                    0
                };
                RunRecord {
                    v: 2,
                    run_id: id.into(),
                    arm: if arm == Arm::C {
                        "c".into()
                    } else {
                        "p".into()
                    },
                    task: t.id.clone(),
                    repeat: r,
                    outcome: "done".into(),
                    score_permille: s,
                    set: "pool".into(),
                    requests: 1,
                    ..Default::default()
                }
            }
        }
        let mut a1 = task("a1", RunSet::Pool);
        a1.source = Source::Agent;
        a1.family = Some("fam".into());
        let tasks = vec![
            task("t1", RunSet::Pool),
            task("t2", RunSet::Pool),
            a1,
            task("h1", RunSet::Holdout),
        ];
        let o = run_g4(
            &plan(tasks, vec![task("k1", RunSet::Canary)]),
            &Flaky,
            &|| {},
        );
        assert_eq!(o.flaky, vec!["a1".to_string()]);
    }
}
