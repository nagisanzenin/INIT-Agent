//! The gate track (docs/tui.md §3.10a, TUI-OBSERVE): every self-change shown as what it is. The
//! kernel's notices about a promote carry structured `data` on the owner channel (`gate_start`, one
//! `gate` per stage, `promoted`, `lkg`, `rolled_back`); this module folds them into one live block
//! per change, at most [`MAX_ROWS`] rows with a ticking time on the running step, and collapses the
//! block into one committed line once the change is kept, refused or rolled back.
//!
//! Colour carries the state (palette roles, §3.9a): `Rewrite` only while a step is in flight, `Pass`
//! for passed steps and a kept change, `Harm` for a refusal or a rollback. The kernel performs these
//! rollbacks, so nothing here is in the Root's colour; the Root's own restore is a Root notice.
//!
//! Only kernel-sourced data is read (the caller skips frames the Root stamped `src:"root"`). Times
//! come from the notices' `ts`, so a replay after a reconnect shows the true durations.

use super::{Line, Role, Style, sanitize};
use serde_json::Value;

/// Rows a live block may use, its header included (the block never pushes the input box off).
pub const MAX_ROWS: usize = 7;
/// Changes tracked at once; older ones are dropped (their notices are still in the session log).
const MAX_RUNS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepSt {
    Pending,
    Running,
    Pass,
    Fail,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    /// `G0`…`G4`, `handoff` (a new body binary) or `live` (a data-only move).
    pub name: String,
    pub st: StepSt,
    /// When it began (ms since the epoch): the end of the step before it.
    pub since_ms: Option<i64>,
    /// When it ended (the notice's `ts`).
    pub end_ms: Option<i64>,
    pub wall_ms: Option<u64>,
    pub cache_permille: Option<u64>,
    pub score: Option<i64>,
    /// A failure's reason, or why a running step runs again (`insufficient`: an escalated re-run).
    pub reason: Option<String>,
    /// v0.5.2, G0 only (the kernel's G0 notice): the export, dependency and body build before G0's
    /// tests (`build_ms`), the dependency part of it (`deps_ms`) and how the dependencies came
    /// (`deps_mode`: `hit` from the cache, `stub` built now, `cold` built with the body).
    pub build_ms: Option<u64>,
    pub deps_ms: Option<u64>,
    pub deps_mode: Option<String>,
}

/// v0.5.2 (user session v0.5.1-new: 17 min 46 s of G0's cold dependency build read as "waited", with
/// no estimate while it ran): the estimate shown under a running dependency build. v0.5.3 (lane F): the
/// measured range, as the kernel's `gate_deps` notice says it. The dependency build alone took 907 s,
/// 17 min 16 s and 21 min 13 s on an M2 at load 10–23 or beside a release job (docs/guide.md "Time":
/// MERGE-T2CACHE, user sessions v0.5.1-new and v0.5.2-new); v0.5.2's "usually 10–20 min" had no receipt.
/// v0.5.4 (the release review): it sits under `deps+G0`, whose timer counts both, so it says it is the dependencies' part.
pub const DEPS_ESTIMATE: &str = "the dependencies alone: 15–21 min on a busy M2 (first build, not cached yet)";
/// v0.5.4 (user session v0.5.4 A: the range stayed on screen at 32 min): the estimate's upper bound, and what the
/// row says once the step has run longer.
pub const DEPS_UPPER_MS: i64 = 21 * 60_000;
/// v0.5.4 (round 8, user session v0.5.4 A): a pending G4 read as if a sealed test would check the change.
pub const G4_PENDING: &str = "scored only if INIT has sealed tasks (none on a fresh install)";
/// v0.5.4 (lane K, review 7): the merged step's timer runs from G0's start (the export, the dependencies, the body build and
/// G0's tests), while the range is the dependencies' alone; so the row names the time it passed, not "the estimate".
/// Round 2 (review SF2): "still building" first, and the row fits 80 columns (its 4-column indent included), so a cut
/// never takes the words that say nothing is stuck.
pub const DEPS_PAST: &str = "still building · past 21 min (dependencies alone: 15–21 min on a busy M2)";
/// v0.5.4 (lane K): the restarted kernel ended the change's job (`gate_stopped`, reason `kernel_restart`).
pub const BY_RESTART: &str = "when the agent restarted";

impl Step {
    fn new(name: &str) -> Step {
        Step { name: name.into(), st: StepSt::Pending, since_ms: None, end_ms: None, wall_ms: None, cache_permille: None, score: None, reason: None,
            build_ms: None, deps_ms: None, deps_mode: None }
    }
    fn is_gate(&self) -> bool {
        self.name.starts_with('G')
    }
    /// A step that makes model requests (the boot and self-edit probes, G4's scoring): its spend is the
    /// checks' (v0.5.2: `incl. checks` on a turn only while one of these runs).
    fn spends(&self) -> bool {
        matches!(self.name.as_str(), "G2" | "G3" | "G4")
    }
    /// TUI-FIX (v0.5.1 user session: G0's live timer said 8 min 33 s, its row then 1 min 39 s, and
    /// about 8 minutes of the 17 min 47 s total were on no row): the time between the step before it
    /// (or the gate's start) and its end that is not in its own `wall_ms`: queued, waiting for a build
    /// slot, or kernel work outside the step. Shown from a second up, so the rows add up to the total.
    pub fn waited_ms(&self) -> Option<i64> {
        if !matches!(self.st, StepSt::Pass | StepSt::Fail) {
            return None;
        }
        let w = self.end_ms? - self.since_ms? - self.wall_ms? as i64;
        (w >= 1000).then_some(w)
    }
    /// TUI-FIX (v0.5.1): a G4 that "passed" without scoring anything (no tasks to score against, or
    /// not run on the free path) is not a score: why it did not run, or None for a real G4 result.
    pub fn unscored(&self) -> Option<&'static str> {
        if self.name != "G4" || self.st != StepSt::Pass {
            return None;
        }
        match self.reason.as_deref() {
            Some("no_instrument") => Some("no tasks to score against yet; kept on G0–G3"),
            Some("free") => Some("not run in free mode"),
            Some(_) => None,
            None if self.score.is_none() => Some("no score reported"),
            None => None,
        }
    }
    /// The step's own time: the kernel's `wall_ms`, else the span between the notices.
    fn took_ms(&self, now_ms: i64) -> Option<i64> {
        match self.st {
            StepSt::Running => self.since_ms.map(|s| (now_ms - s).max(0)),
            StepSt::Pending => None,
            _ => self.wall_ms.map(|w| w as i64).or_else(|| Some(self.end_ms? - self.since_ms?)),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// It became LKG.
    Kept,
    /// A stage refused it before it went live: nothing changed.
    Refused { stage: String, reason: String },
    /// It went live, then was rolled back (a failed G4, an expiry, `initctl rollback`).
    RolledBack { stage: Option<String>, reason: String, to: String, to_lkg: bool },
    /// v0.5.4 (user session v0.5.4 A: the panel counted on after /panic and /resume): the Root stopped the agent
    /// (`by`: "by /panic", "by a halt"). A panic kills every instance the Root holds, the kernel that ran the checks and
    /// their builds included (root panic.rs `zone.kill_all`; tests/wp21.rs), and the restarted kernel marks the job
    /// `failed`, `kernel_restart` (kernel-serve jobs.rs `JobStore::open`) without a notice: the change never finishes.
    Stopped { by: &'static str },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub sha: String,
    pub gate_id: Option<String>,
    pub scope: Option<String>,
    /// The running version it was gated against.
    pub parent: Option<String>,
    pub steps: Vec<Step>,
    pub start_ms: i64,
    pub end_ms: Option<i64>,
    /// It went live (`promoted`).
    pub live: bool,
    pub handoff_ms: Option<u64>,
    pub outcome: Option<Outcome>,
}

fn sha12(s: &str) -> String {
    s.chars().take(12).collect()
}

fn reason_of(v: &Value) -> Option<String> {
    let r = v.as_str()?;
    Some(r.strip_prefix("gate.").and_then(|x| x.split_once('.').map(|(_, b)| b)).unwrap_or(r).to_string())
}

impl Run {
    fn new(sha: &str, start_ms: i64) -> Run {
        Run { sha: sha.into(), gate_id: None, scope: None, parent: None, steps: Vec::new(), start_ms, end_ms: None, live: false, handoff_ms: None, outcome: None }
    }
    fn step_mut(&mut self, name: &str) -> &mut Step {
        if let Some(i) = self.steps.iter().position(|s| s.name == name) {
            return &mut self.steps[i];
        }
        // a step the plan did not name (a run seen from its middle): in G order, before hand-off
        let at = self.steps.iter().position(|s| !s.is_gate() || s.name.as_str() > name).unwrap_or(self.steps.len());
        self.steps.insert(at, Step::new(name));
        &mut self.steps[at]
    }
    /// The step after `name` starts at `ts`, if it is still pending.
    fn start_next(&mut self, name: &str, ts: i64) {
        if let Some(i) = self.steps.iter().position(|s| s.name == name)
            && let Some(n) = self.steps.get_mut(i + 1)
            && n.st == StepSt::Pending
        {
            n.st = StepSt::Running;
            n.since_ms = Some(ts);
        }
    }
    /// v0.5.2: the dependency build as a step of its own, before G0: it takes over G0's start (the
    /// build runs in G0's slot), and G0 waits for it.
    fn deps_step(&mut self, ts: i64) -> usize {
        if let Some(i) = self.steps.iter().position(|s| s.name == "deps") {
            return i;
        }
        let at = self.steps.iter().position(|s| s.name == "G0").unwrap_or(0);
        let mut d = Step::new("deps");
        d.st = StepSt::Running;
        d.since_ms = Some(ts);
        if let Some(g0) = self.steps.get_mut(at).filter(|s| s.name == "G0" && s.st == StepSt::Running) {
            d.since_ms = g0.since_ms.or(Some(ts));
            g0.st = StepSt::Pending;
            g0.since_ms = None;
        }
        self.steps.insert(at, d);
        at
    }

    /// v0.5.2: G0 ended (its notice at `ts`). The kernel's G0 time is its tests and clippy only; the
    /// export and builds before them (`build_ms`) are G0's too, and a dependency build within them
    /// (`deps_ms`, built now: `stub`) is the `deps` step. So the rows add up to the job's G0 time. An
    /// older kernel sends neither: G0 stays as it was (the rest shows as `waited`).
    fn split_g0(&mut self, ts: i64) {
        let Some(gi) = self.steps.iter().position(|s| s.name == "G0") else { return };
        let g = self.steps[gi].clone();
        let (Some(build), Some(tests)) = (g.build_ms, g.wall_ms) else {
            // v0.5.4 (the release review): a G0 that failed at the body build sends no build_ms, only deps_ms; a
            // dependency build it finished (`stub`) ends with its own time instead of being left "·"
            if let (Some(deps), Some("stub")) = (g.deps_ms, g.deps_mode.as_deref())
                && let Some(d) = self.steps.iter_mut().find(|s| s.name == "deps" && s.st == StepSt::Running)
            {
                let end = d.since_ms.map_or(ts, |s| (s + deps as i64).min(ts));
                (d.st, d.wall_ms, d.end_ms, d.deps_mode) = (StepSt::Pass, Some(deps), Some(end), g.deps_mode.clone());
                self.steps[gi].since_ms = Some(end);
            }
            return;
        };
        let deps = g.deps_ms.unwrap_or(0).min(build);
        let built_now = self.steps.iter().any(|s| s.name == "deps") || (matches!(g.deps_mode.as_deref(), Some("stub" | "cold")) && deps >= 1000);
        if !built_now {
            self.steps[gi].wall_ms = Some(build + tests);
            return;
        }
        let since = g.since_ms;
        let di = self.deps_step(ts);
        let gi = di + 1;
        let own = (build - deps + tests) as i64;
        let d = &mut self.steps[di];
        let start = d.since_ms.or(since).unwrap_or(ts);
        let end = (ts - own).max(start);
        d.st = StepSt::Pass;
        d.wall_ms = Some(deps);
        d.end_ms = Some(end);
        d.deps_mode = g.deps_mode.clone();
        let g0 = &mut self.steps[gi];
        g0.since_ms = Some(end);
        g0.wall_ms = Some(own as u64);
    }

    fn finish(&mut self, o: Outcome, ts: i64) {
        // a step still running when the change ends: kept means it passed (LKG follows the last
        // gate); otherwise it never finished
        let st = if o == Outcome::Kept { StepSt::Pass } else { StepSt::Pending };
        for s in self.steps.iter_mut().filter(|s| s.st == StepSt::Running) {
            s.st = st;
            s.end_ms.get_or_insert(ts);
        }
        self.outcome = Some(o);
        self.end_ms = Some(ts);
    }
    /// `passed/planned` gates (`4/4`); a G4 that scored nothing counts as neither.
    pub fn gate_count(&self) -> (usize, usize) {
        let g: Vec<&Step> = self.steps.iter().filter(|s| s.is_gate() && s.unscored().is_none()).collect();
        (g.iter().filter(|s| s.st == StepSt::Pass).count(), g.len())
    }
    fn failed(&self) -> Option<&Step> {
        self.steps.iter().find(|s| s.st == StepSt::Fail)
    }
}

/// The changes in flight, oldest first. The newest one without an outcome is drawn.
#[derive(Debug, Default)]
pub struct Track {
    pub runs: Vec<Run>,
    /// The shas of the changes that ended, newest last (at most [`MAX_RUNS`]): a `gate_stopped` for one of them is taken
    /// without a second line (a panic ended it here, then the restarted kernel says so too).
    ended: Vec<String>,
}

impl Track {
    fn run_mut(&mut self, sha: &str) -> Option<&mut Run> {
        self.runs.iter_mut().rev().find(|r| r.sha == sha)
    }

    fn take_done(&mut self) -> Vec<Run> {
        let (done, keep): (Vec<Run>, Vec<Run>) = std::mem::take(&mut self.runs).into_iter().partition(|r| r.outcome.is_some());
        self.runs = keep;
        self.ended.extend(done.iter().map(|r| r.sha.clone()));
        let over = self.ended.len().saturating_sub(MAX_RUNS);
        self.ended.drain(..over);
        done
    }

    /// Folds one notice's `data` (sent at `ts_ms`). `None`: not a gate-track notice, so the caller
    /// shows it as a plain notice. `Some(done)`: taken; `done` are the changes that just ended, to
    /// commit as one line each.
    pub fn on_notice(&mut self, d: &Value, ts_ms: i64) -> Option<Vec<Run>> {
        let kind = d["kind"].as_str()?;
        let sha = d["sha"].as_str().unwrap_or("");
        match kind {
            "gate_start" if !sha.is_empty() => {
                let mut r = Run::new(sha, ts_ms);
                r.gate_id = d["gate_id"].as_str().map(str::to_string);
                r.scope = d["scope"].as_str().map(str::to_string);
                r.parent = d["parent"].as_str().map(str::to_string);
                r.steps = d["steps"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(Step::new).collect()).unwrap_or_default();
                if let Some(s) = r.steps.first_mut() {
                    s.st = StepSt::Running;
                    s.since_ms = Some(ts_ms);
                }
                self.runs.retain(|x| x.sha != sha);
                self.runs.push(r);
                if self.runs.len() > MAX_RUNS {
                    self.runs.remove(0);
                }
            }
            "gate" if !sha.is_empty() => {
                // a `gate --quick` check is not a self-change
                if d["gate_kind"].as_str().is_some_and(|k| k != "promote" && k != "g4") {
                    return None;
                }
                let stage = d["stage"].as_str()?.to_string();
                if self.run_mut(sha).is_none() {
                    self.runs.push(Run::new(sha, ts_ms));
                }
                let r = self.run_mut(sha).expect("just made");
                let prev_end = r.steps.iter().filter_map(|s| s.end_ms).max().unwrap_or(r.start_ms);
                let live = r.live;
                let s = r.step_mut(&stage);
                s.since_ms.get_or_insert(prev_end);
                s.wall_ms = d["wall_ms"].as_u64();
                // the notice's `spent_musd` is the kernel's figure, at its own prices: not shown (one price per
                // request, the Root's: root/egress.md §3.4); /usage has the checks' cost from the ledger
                s.cache_permille = d["cache_permille"].as_u64();
                s.score = d["score"].as_i64();
                s.reason = reason_of(&d["reason"]);
                s.build_ms = d["build_ms"].as_u64();
                s.deps_ms = d["deps_ms"].as_u64();
                s.deps_mode = d["deps_mode"].as_str().map(str::to_string);
                if stage == "G0" && d["pass"].is_boolean() {
                    r.split_g0(ts_ms);
                }
                let s = r.step_mut(&stage);
                match d["pass"].as_bool() {
                    Some(true) => {
                        s.st = StepSt::Pass;
                        s.end_ms = Some(ts_ms);
                        r.start_next(&stage, ts_ms);
                    }
                    Some(false) => {
                        s.st = StepSt::Fail;
                        s.end_ms = Some(ts_ms);
                        if !live {
                            let reason = s.reason.clone().unwrap_or_else(|| "failed".into());
                            r.finish(Outcome::Refused { stage, reason }, ts_ms);
                        }
                        // after going live, the kernel rolls back at the next idle point: `rolled_back` ends it
                    }
                    // inconclusive: a re-run follows; the step keeps running, with the reason shown
                    None => s.st = StepSt::Running,
                }
            }
            // v0.5.2: G0 starts building the dependencies (a cache miss): its own step, with an estimate
            "gate_deps" if self.run_mut(sha).is_some() => {
                let r = self.run_mut(sha).expect("checked");
                r.deps_step(ts_ms);
            }
            "promoted" if self.run_mut(sha).is_some() => {
                let r = self.run_mut(sha).expect("checked");
                r.live = true;
                r.handoff_ms = d["handoff_ms"].as_u64();
                let name = if d["handoff"] == true { "handoff" } else { "live" };
                if r.steps.iter().any(|s| s.name == name) {
                    let s = r.step_mut(name);
                    s.st = StepSt::Pass;
                    s.end_ms = Some(ts_ms);
                    s.wall_ms = d["handoff_ms"].as_u64();
                    r.start_next(name, ts_ms);
                }
            }
            // v0.5.4 (lane K, user session v0.5.4 A): the kernel's cold start marked the change's job `failed`,
            // `kernel_restart` (kernel-serve `JobStore::open`; a kernel swap's job it reattached sends none). Live, or in
            // the replay of a later attach: the change ends as stopped. One that already ended here (a panic, a halt, a
            // stop) is not ended twice; one this TUI never saw start is a plain notice.
            "gate_stopped" => {
                if d["gate_kind"].as_str().is_some_and(|k| k != "promote" && k != "g4") {
                    return None;
                }
                let gid = d["gate_id"].as_str().filter(|g| !g.is_empty());
                let open = self.runs.iter_mut().rev()
                    .find(|r| r.outcome.is_none() && ((!sha.is_empty() && r.sha == sha) || (gid.is_some() && r.gate_id.as_deref() == gid)));
                match open {
                    // round 2 (review SF1): the restart finished the change's handoff, so it runs (`live`)
                    Some(r) => {
                        r.live |= d["live"] == true;
                        r.finish(Outcome::Stopped { by: BY_RESTART }, ts_ms);
                    }
                    None if !sha.is_empty() && self.ended.iter().any(|e| e == sha) => {}
                    None => return None,
                }
            }
            "provisional" if self.run_mut(sha).is_some() => {}
            "lkg" if self.run_mut(sha).is_some() => {
                let r = self.run_mut(sha).expect("checked");
                r.finish(Outcome::Kept, ts_ms);
            }
            "rolled_back" => {
                let from = d["from_sha"].as_str().unwrap_or("");
                if self.run_mut(from).is_none() {
                    self.runs.push(Run::new(from, ts_ms));
                }
                let r = self.run_mut(from).expect("just made");
                let stage = r.failed().map(|s| s.name.clone());
                let reason = reason_of(&d["reason"]).or_else(|| r.failed().and_then(|s| s.reason.clone())).unwrap_or_else(|| "rollback".into());
                r.live = true;
                let o = Outcome::RolledBack { stage, reason, to: d["to_sha"].as_str().unwrap_or("").into(), to_lkg: d["to_lkg"] == true };
                r.finish(o, ts_ms);
            }
            _ => return None,
        }
        Some(self.take_done())
    }

    /// v0.5.4: the Root stopped the agent (a panic or a halt): every change in flight ends `Stopped`, its timer and its
    /// estimate with it. Returns the changes that ended, to commit as one line each.
    pub fn stop_all(&mut self, by: &'static str, ts_ms: i64) -> Vec<Run> {
        for r in self.runs.iter_mut().filter(|r| r.outcome.is_none()) {
            r.finish(Outcome::Stopped { by }, ts_ms);
        }
        self.take_done()
    }

    /// The change drawn live: the newest without an outcome.
    pub fn current(&self) -> Option<&Run> {
        self.runs.iter().rev().find(|r| r.outcome.is_none())
    }

    /// v0.5.2: is a step that makes model requests (G2, G3, G4) in flight in some change?
    pub fn spending(&self) -> bool {
        self.runs.iter().filter(|r| r.outcome.is_none()).any(|r| r.steps.iter().any(|s| s.st == StepSt::Running && s.spends()))
    }

    /// Is a step ticking (the live region redraws for the timer)?
    pub fn in_flight(&self) -> bool {
        self.current().is_some()
    }

    /// The live block's rows (empty when nothing is in flight), at most `max_rows` (≤ [`MAX_ROWS`]).
    pub fn live_rows(&self, width: usize, now_ms: i64, max_rows: usize) -> Vec<Line> {
        self.current().map(|r| block(r, width, now_ms, max_rows.clamp(1, MAX_ROWS))).unwrap_or_default()
    }

    /// The rows the live region gives the block on a `height`-row terminal: 7, or a third of a small one.
    pub fn rows_for(height: usize) -> usize {
        (height / 3).clamp(1, MAX_ROWS)
    }
}

const LABEL_W: usize = 16;

fn label(name: &str) -> &'static str {
    match name {
        "deps" => "dependencies",
        "G0" => "build + tests",
        "G1" => "self-map",
        "G2" => "boot probe",
        "G3" => "self-edit probe",
        "G4" => "scored vs LKG",
        "handoff" => "",
        "live" => "",
        _ => "",
    }
}

fn step_name(name: &str) -> &str {
    match name {
        "handoff" => "hand-off",
        "live" => "goes live",
        n => n,
    }
}

fn secs(ms: i64) -> String {
    crate::time::elapsed(std::time::Duration::from_millis(ms.max(0) as u64))
}

/// A step's cell (`■` passed, `□` in flight, `✕` failed, `·` to come) and its role.
fn cell(st: StepSt) -> (&'static str, Style) {
    match st {
        StepSt::Pass => ("■", Style::fg(Role::Pass)),
        StepSt::Running => ("□", Style::fg(Role::Rewrite)),
        StepSt::Fail => ("✕", Style::fg(Role::Harm)),
        StepSt::Pending => ("·", Style::fg(Role::Mute)),
    }
}

/// v0.5.4 (user sessions v0.5.2-new and v0.5.3-new, finding 1: "deps □ 15 min 19 s" while G0 had run for
/// 2 min 27 s): the kernel says when the dependency build starts, not when it ends, and G0 follows it in the
/// same slot. So while it runs, the dependency build and G0 are one step in flight, `deps+G0`, and G0's own
/// cell and row are not drawn; G0's notice splits them into their true times ([`Run::split_g0`]).
fn deps_with_g0(s: &Step) -> bool {
    s.name == "deps" && s.st == StepSt::Running
}

/// `G0 ■ G1 ■ G2 □ G3 · hand-off · G4 ·`
fn strip(r: &Run) -> Line {
    let mut l = Line::new();
    let merged = r.steps.iter().any(deps_with_g0);
    for (i, s) in r.steps.iter().enumerate().filter(|(_, s)| !(merged && s.name == "G0")) {
        if i > 0 {
            l.push(" ", Style::PLAIN);
        }
        let n = if deps_with_g0(s) { "deps+G0" } else if s.is_gate() || s.name == "deps" { s.name.as_str() } else { "live" };
        l.push(format!("{n} "), Style::fg(Role::Mute));
        let (g, st) = step_cell(s);
        l.push(g, st);
    }
    l
}

/// The cell of a step: a G4 that scored nothing is `·` (not run), not `■`.
fn step_cell(s: &Step) -> (&'static str, Style) {
    if s.unscored().is_some() { cell(StepSt::Pending) } else { cell(s.st) }
}

fn split(left: Line, right: Line, width: usize) -> Line {
    let rw = right.width();
    if rw + 4 > width {
        return left.truncate(width);
    }
    let mut row = left.truncate(width.saturating_sub(rw + 2));
    let pad = width.saturating_sub(row.width() + rw);
    row.push(" ".repeat(pad.max(1)), Style::PLAIN);
    row.extend(right);
    row.truncate(width)
}

/// One step's row: `    G2  boot probe        □ 2.9 s` (the time ticks while it runs). `open`: the change is still in
/// flight, so a pending step may still run.
fn step_row(s: &Step, width: usize, now_ms: i64, detail: bool, open: bool) -> Line {
    let mut l = Line::from_span("    ", Style::PLAIN);
    // v0.5.4 (the release review): the merged step's label keeps to the label column (a longer one cut the timer under
    // about 57 columns and shifted the cell)
    let (name, lab) = if deps_with_g0(s) { ("deps+G0", "dependencies") } else { (step_name(&s.name), label(&s.name)) };
    l.push(format!("{name:<3} "), Style::fg(Role::Dim));
    l.push(format!("{:<w$}", lab, w = LABEL_W.saturating_sub(name.chars().count().saturating_sub(3))), Style::fg(Role::Mute));
    let (g, st) = step_cell(s);
    l.push(format!(" {g} "), st);
    let took = s.took_ms(now_ms).map(secs);
    let waited = s.waited_ms().map(|w| format!(" · waited {}", secs(w)));
    match s.st {
        StepSt::Pass if s.unscored().is_some() => {
            l.push("not scored", Style::PLAIN);
            l.push(format!(" · {}", s.unscored().unwrap_or_default()), Style::dim());
        }
        StepSt::Pass => {
            l.push(if s.name == "deps" { "built" } else { "pass" }, Style::fg(Role::Pass));
            if let Some(t) = took {
                l.push(format!(" · {t}"), Style::dim());
            }
            if let Some(w) = waited {
                l.push(w, Style::dim());
            }
            if s.name == "deps" {
                l.push(" · not cached before; later changes reuse it", Style::dim());
            }
        }
        StepSt::Fail => {
            l.extend(sanitize::inline(s.reason.as_deref().unwrap_or("failed"), Style::fg(Role::Harm)));
            if let Some(t) = took {
                l.push(format!(" · {t}"), Style::dim());
            }
            if let Some(w) = waited {
                l.push(w, Style::dim());
            }
        }
        StepSt::Running => {
            l.push(took.unwrap_or_default(), Style::fg(Role::Rewrite));
            if let Some(r) = &s.reason {
                l.push(" · ", Style::dim());
                l.extend(sanitize::inline(r, Style::dim()));
                l.push(": one more run", Style::dim());
            }
        }
        // v0.5.4 (round 8): the kernel plans G4 whether or not sealed tasks exist; it scores only when they do
        // v0.5.4 (lane K, review 7): not on a change that ended (refused, stopped, rolled back): its G4 will not run
        StepSt::Pending if s.name == "G4" && open => l.push(G4_PENDING, Style::dim()),
        StepSt::Pending => {}
    }
    if detail {
        if let Some(c) = s.cache_permille {
            l.push(format!(" · cache {}.{}%", c / 10, c % 10), Style::dim());
        }
        if let Some(sc) = s.score {
            l.push(format!(" · score {sc}‰"), Style::dim());
        }
    }
    l.truncate(width)
}

/// The live block: a header with the cell strip and the total time, then one row per step (a
/// window around the step in flight when `max_rows` is short).
fn block(r: &Run, width: usize, now_ms: i64, max_rows: usize) -> Vec<Line> {
    let mut left = Line::from_span("◇ ", Style::fg(Role::Rewrite));
    left.push("self-change ", Style::PLAIN);
    left.extend(sanitize::inline(&sha12(&r.sha), Style::dim()));
    if let Some(sc) = &r.scope {
        left.extend(sanitize::inline(&format!(" · {sc}"), Style::dim()));
    }
    left.push("  ", Style::PLAIN);
    left.extend(strip(r));
    let right = Line::from_span(secs(now_ms - r.start_ms), Style::fg(Role::Rewrite));
    let mut rows = vec![split(left, right, width)];
    // a step that failed after going live: the rollback comes at the next idle point
    let waiting = r.live && r.failed().is_some();
    // v0.5.2: a dependency build in flight says what it is and how long it usually takes
    let deps = r.steps.iter().any(|s| s.name == "deps" && s.st == StepSt::Running);
    let room = max_rows.saturating_sub(1 + usize::from(waiting) + usize::from(deps)).max(1);
    let n = r.steps.len();
    let at = r.steps.iter().position(|s| s.st != StepSt::Pass).unwrap_or(n.saturating_sub(1));
    // one step of look-ahead when there is room for it, never past the step in flight (v0.5.4, the release review: a
    // one-row window under 12 terminal rows showed the step after it)
    let skip = (at + 2).saturating_sub(room).min(at).min(n.saturating_sub(room));
    for s in r.steps.iter().filter(|s| !(deps && s.name == "G0")).skip(skip).take(room) {
        rows.push(step_row(s, width, now_ms, false, true));
        if s.name == "deps" && s.st == StepSt::Running && rows.len() < max_rows {
            let past = s.took_ms(now_ms).is_some_and(|t| t > DEPS_UPPER_MS);
            rows.push(Line::from_span(format!("    {}", if past { DEPS_PAST } else { DEPS_ESTIMATE }), Style::dim()).truncate(width));
        }
    }
    if waiting && rows.len() < max_rows {
        rows.push(Line::from_span("    ↺ rolling back at the next idle point", Style::fg(Role::Harm)).truncate(width));
    }
    rows
}

/// The committed line of a change that ended, and in the transcript view (`expanded`) its steps.
pub fn committed_rows(r: &Run, width: usize, expanded: bool) -> Vec<Line> {
    let total = r.end_ms.map(|e| secs(e - r.start_ms));
    let mut l = Line::new();
    let dim = |l: &mut Line, s: String| l.extend(sanitize::inline(&s, Style::dim()));
    match &r.outcome {
        Some(Outcome::Kept) | None => {
            l.push("✓ kept ", Style::fg(Role::Pass));
            dim(&mut l, sha12(&r.sha));
            let (p, n) = r.gate_count();
            if n > 0 {
                dim(&mut l, format!(" · {p}/{n} gates"));
            }
            if r.steps.iter().any(|s| s.unscored().is_some()) {
                dim(&mut l, " · G4 not scored".into());
            }
            if let Some(t) = total {
                dim(&mut l, format!(" · {t}"));
            }
            match r.handoff_ms {
                Some(h) => dim(&mut l, format!(" · hand-off {}", secs(h as i64))),
                None if r.steps.iter().any(|s| s.name == "live") => dim(&mut l, " · no restart".into()),
                None => {}
            }
        }
        Some(Outcome::Refused { stage, reason }) => {
            l.push(format!("✕ refused at {stage} "), Style::fg(Role::Harm));
            dim(&mut l, sha12(&r.sha));
            l.push(" · ", Style::dim());
            l.extend(sanitize::inline(reason, Style::PLAIN));
            if let Some(t) = total {
                dim(&mut l, format!(" · {t}"));
            }
            match &r.parent {
                Some(p) => dim(&mut l, format!(" · still on {}", sha12(p))),
                None => dim(&mut l, " · nothing changed".into()),
            }
        }
        Some(Outcome::Stopped { by }) => {
            l.push(format!("■ stopped {by} "), Style::fg(Role::Harm));
            dim(&mut l, sha12(&r.sha));
            let what = if r.live && *by == BY_RESTART {
                // v0.5.4 (lane K): the agent runs again already; no /resume is to come
                " · it had gone live; /root status shows which version runs now"
            } else if r.live {
                // round 9 (review 7): which version runs after /resume depends on how it stopped and on the mode
                " · it had gone live; /root status shows which version runs after /resume"
            } else {
                " · its checks were stopped with the agent, so this change did not go live and will not continue"
            };
            dim(&mut l, what.into());
            if let Some(t) = total {
                dim(&mut l, format!(" · {t}"));
            }
        }
        Some(Outcome::RolledBack { stage, reason, to, to_lkg }) => {
            let at = stage.as_ref().map(|s| format!(" at {s}")).unwrap_or_default();
            l.push(format!("✕ rolled back{at} "), Style::fg(Role::Harm));
            dim(&mut l, sha12(&r.sha));
            l.push(" · ", Style::dim());
            l.extend(sanitize::inline(reason, Style::PLAIN));
            let lkg = if *to_lkg { "LKG " } else { "" };
            dim(&mut l, format!(" · ↺ back on {lkg}{}", sha12(to)));
        }
    }
    let mut rows = vec![l.truncate(width)];
    if expanded {
        let end = r.end_ms.unwrap_or(r.start_ms);
        rows.extend(r.steps.iter().map(|s| step_row(s, width, end, true, r.outcome.is_none())));
        let mut ids = format!("    sha {}", r.sha);
        if let Some(p) = &r.parent {
            ids.push_str(&format!(" · from {p}"));
        }
        if let Some(g) = &r.gate_id {
            ids.push_str(&format!(" · gate {g}"));
        }
        ids.push_str(&format!(" · started {}", crate::time::utc_hms(r.start_ms)));
        rows.push(sanitize::inline(&ids, Style::dim()).truncate(width));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::grid;
    use serde_json::json;

    const T0: i64 = 1_792_530_245_000; // 2026-10-20T21:04:05Z

    fn golden(name: &str, got: &str) {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(format!("{name}.txt"));
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::write(&p, got).unwrap();
            return;
        }
        let want = std::fs::read_to_string(&p).unwrap_or_else(|_| panic!("missing golden {} (run with UPDATE_GOLDEN=1)", p.display()));
        for (i, (a, b)) in want.lines().zip(got.lines()).enumerate() {
            assert_eq!(a, b, "golden {name} differs at line {}", i + 1);
        }
        assert_eq!(want.lines().count(), got.lines().count(), "golden {name} differs in length");
    }

    fn start(t: &mut Track, sha: &str, steps: &[&str], at: i64) -> Option<Vec<Run>> {
        t.on_notice(&json!({"kind": "gate_start", "gate_id": "j-7", "gate_kind": "promote", "sha": sha, "parent": "3f9c0a1b2c4d5e6f7a8b", "scope": "t2", "steps": steps}), at)
    }
    fn gate(t: &mut Track, sha: &str, stage: &str, pass: Option<bool>, reason: Option<&str>, wall: u64, at: i64) -> Option<Vec<Run>> {
        // a G4 that ran has a score (an unscored one is tested on its own)
        let score = if stage == "G4" { json!(612) } else { Value::Null };
        t.on_notice(&json!({"kind": "gate", "gate_kind": "promote", "gate_id": "j-7", "sha": sha, "stage": stage, "pass": pass, "reason": reason,
            "score": score, "spent_musd": 0, "wall_ms": wall, "cache_permille": null}), at)
    }

    const SHA: &str = "06be87a1234567890abcdef";
    const T2: [&str; 6] = ["G0", "G1", "G2", "G3", "handoff", "G4"];

    fn screen(rows: &[Line], width: usize, title: &str) -> String {
        for r in rows {
            assert!(r.width() <= width, "row wider than {width}: {:?}", r.text());
        }
        format!("-- {title}\n{}", grid(rows, width))
    }

    /// A kept T2 change: live with G2 running (7 rows at most), then G4 running after the hand-off,
    /// then the one committed line; refused at G2; rolled back after G4; at 80 and 50 columns.
    #[test]
    fn gate_track_live_and_collapsed() {
        let mut out = String::new();
        let mut t = Track::default();
        assert_eq!(start(&mut t, SHA, &T2, T0), Some(vec![]));
        assert_eq!(gate(&mut t, SHA, "G0", Some(true), None, 9_100, T0 + 9_300), Some(vec![]));
        gate(&mut t, SHA, "G1", Some(true), None, 400, T0 + 9_800);
        let rows = t.live_rows(80, T0 + 12_700, MAX_ROWS);
        assert!(rows.len() <= MAX_ROWS);
        out.push_str(&screen(&rows, 80, "kept: G2 running (80)"));
        out.push_str(&screen(&t.live_rows(50, T0 + 12_700, MAX_ROWS), 50, "kept: G2 running (50)"));
        let small = t.live_rows(80, T0 + 12_700, Track::rows_for(12));
        assert_eq!(small.len(), 4);
        out.push_str(&screen(&small, 80, "kept: G2 running, a 12-row terminal (4 rows)"));
        gate(&mut t, SHA, "G2", Some(true), None, 3_000, T0 + 12_800);
        gate(&mut t, SHA, "G3", Some(true), None, 24_000, T0 + 36_800);
        assert_eq!(t.on_notice(&json!({"kind": "promoted", "sha": SHA, "handoff": true, "handoff_ms": 4_600}), T0 + 41_200), Some(vec![]));
        out.push_str(&screen(&t.live_rows(80, T0 + 43_000, MAX_ROWS), 80, "kept: G4 running after the hand-off"));
        gate(&mut t, SHA, "G4", Some(true), None, 1_000, T0 + 44_000);
        let done = t.on_notice(&json!({"kind": "lkg", "sha": SHA}), T0 + 44_100).unwrap();
        assert_eq!(done.len(), 1);
        assert!(t.live_rows(80, T0 + 50_000, MAX_ROWS).is_empty() && !t.in_flight());
        out.push_str(&screen(&committed_rows(&done[0], 80, false), 80, "kept: collapsed"));
        out.push_str(&screen(&committed_rows(&done[0], 80, true), 80, "kept: expanded (Ctrl-O)"));
        // refused at G2: never went live
        let mut t = Track::default();
        start(&mut t, SHA, &T2, T0);
        gate(&mut t, SHA, "G0", Some(true), None, 9_100, T0 + 9_300);
        gate(&mut t, SHA, "G1", Some(true), None, 400, T0 + 9_800);
        let done = gate(&mut t, SHA, "G2", Some(false), Some("probe_exit"), 3_100, T0 + 12_900).unwrap();
        assert!(matches!(&done[0].outcome, Some(Outcome::Refused { stage, reason }) if stage == "G2" && reason == "probe_exit"));
        out.push_str(&screen(&committed_rows(&done[0], 80, false), 80, "refused at G2: collapsed"));
        out.push_str(&screen(&committed_rows(&done[0], 50, false), 50, "refused at G2: collapsed (50)"));
        // went live, G4 failed: rolling back at idle, then rolled back
        let mut t = Track::default();
        start(&mut t, SHA, &T2, T0);
        for (g, at) in [("G0", 9_300), ("G1", 9_800), ("G2", 12_800), ("G3", 36_800)] {
            gate(&mut t, SHA, g, Some(true), None, 1_000, T0 + at);
        }
        t.on_notice(&json!({"kind": "promoted", "sha": SHA, "handoff": true, "handoff_ms": 4_600}), T0 + 41_200);
        assert_eq!(gate(&mut t, SHA, "G4", Some(false), Some("gate.g4.score_inferior"), 60_000, T0 + 101_200), Some(vec![]));
        out.push_str(&screen(&t.live_rows(80, T0 + 102_000, MAX_ROWS), 80, "rolled back: G4 failed, waiting for idle"));
        let done = t.on_notice(&json!({"kind": "rolled_back", "from_sha": SHA, "to_sha": "3f9c0a1b2c4d5e6f7a8b", "reason": "gate.g4.score_inferior", "to_lkg": true}), T0 + 103_000).unwrap();
        out.push_str(&screen(&committed_rows(&done[0], 80, false), 80, "rolled back: collapsed"));
        golden("gates_track", &out);
    }

    /// TUI-FIX (v0.5.1 user session): a T1 change whose G0 waited 6 min 54 s before its 1 min 39 s
    /// of work, and whose G4 had nothing to score. The rows add up to the total, and G4 does not
    /// read as a score that ran.
    #[test]
    fn rows_add_up_and_an_unscored_g4_is_not_a_pass() {
        let mut t = Track::default();
        start(&mut t, SHA, &["G0", "G1", "G2", "G3", "live", "G4"], T0);
        gate(&mut t, SHA, "G0", Some(true), None, 99_000, T0 + 513_000); // live timer 8 min 33 s; work 1 min 39 s
        gate(&mut t, SHA, "G1", Some(true), None, 400, T0 + 513_400);
        gate(&mut t, SHA, "G2", Some(true), None, 30_000, T0 + 600_000); // waited 56.6 s
        gate(&mut t, SHA, "G3", Some(true), None, 467_000, T0 + 1_067_000);
        t.on_notice(&json!({"kind": "promoted", "sha": SHA, "handoff": false}), T0 + 1_067_000);
        t.on_notice(&json!({"kind": "gate", "gate_kind": "g4", "sha": SHA, "stage": "G4", "pass": true, "reason": "gate.g4.no_instrument",
            "score": null, "spent_musd": 0, "wall_ms": 0, "cache_permille": null}), T0 + 1_067_000);
        let done = t.on_notice(&json!({"kind": "lkg", "sha": SHA}), T0 + 1_067_000).unwrap();
        let rows: Vec<String> = committed_rows(&done[0], 100, true).iter().map(|l| l.text()).collect();
        assert_eq!(rows[0], "✓ kept 06be87a12345 · 4/4 gates · G4 not scored · 17 min 47 s · no restart", "{rows:#?}");
        assert!(rows[1].contains("G0  build + tests    ■ pass · 1 min 39 s · waited 6 min 54 s"), "{rows:#?}");
        assert!(rows[3].contains("■ pass · 30 s · waited 56 s"), "{rows:#?}");
        assert!(rows[6].contains("G4  scored vs LKG    · not scored · no tasks to score against yet; kept on G0–G3"), "{rows:#?}");
        assert!(!rows[6].contains("pass"), "{rows:#?}");
        // every step's own time plus its wait is the change's total
        let r = &done[0];
        let sum: i64 = r.steps.iter().map(|s| s.took_ms(0).unwrap_or(0) + s.waited_ms().unwrap_or(0)).sum();
        assert!((r.end_ms.unwrap() - r.start_ms - sum).abs() < 1000, "the rows add up: {sum} ms of {} ms", r.end_ms.unwrap() - r.start_ms);
    }

    /// v0.5.2 (user session v0.5.1-new: "G0 build + tests □ 12 min 3 s" with no ETA, then "G0 ■ pass ·
    /// 2 min 32 s · waited 17 min 46 s", while the job record said G0 took 1,219,172 ms, most of it a
    /// cold dependency build): the build is its own step with an estimate while it runs, and the rows
    /// add up to the job's G0 time with no "waited". A warm cache shows no such step.
    #[test]
    fn a_cold_dependency_build_is_its_own_step() {
        let mut t = Track::default();
        start(&mut t, SHA, &["G0", "G1", "G2", "G3", "live", "G4"], T0);
        assert_eq!(t.on_notice(&json!({"kind": "gate_deps", "gate_id": "j-7", "sha": SHA}), T0 + 5_000), Some(vec![]));
        let live: Vec<String> = t.live_rows(80, T0 + 723_000, MAX_ROWS).iter().map(|l| l.text()).collect();
        // v0.5.4 (user session v0.5.3-new, finding 1): the kernel does not say when the dependency build ends, and G0
        // follows it: one step in flight, never "deps □" beside a G0 that is said to be waiting
        assert!(live[0].contains("deps+G0 □ G1 · G2 ·") && !live[0].contains("G0 ·"), "{live:#?}");
        assert!(live[1].contains("deps+G0 dependencies □ 12 min 3 s"), "{live:#?}");
        assert_eq!(live[2].trim(), DEPS_ESTIMATE, "{live:#?}");
        assert_eq!(DEPS_ESTIMATE, "the dependencies alone: 15–21 min on a busy M2 (first build, not cached yet)", "the measured range, as a part");
        // v0.5.4 (the release review): the row keeps its columns, so the timer is not cut on a narrow terminal
        let narrow: Vec<String> = t.live_rows(40, T0 + 723_000, MAX_ROWS).iter().map(|l| l.text()).collect();
        assert!(narrow[1].trim_end().ends_with("□ 12 min 3 s"), "{narrow:#?}");
        assert_eq!(narrow[1].find('□'), live[3].find('·'), "the cell lines up with the next row's: {narrow:#?} {live:#?}");
        assert!(live[3].contains("G1  self-map") && !live.iter().any(|l| l.contains("G0  build") || l.contains("waited")), "{live:#?}");
        assert!(live.len() <= MAX_ROWS);
        assert!(!t.spending(), "a build spends nothing");
        // G0 ends: its notice's wall is the tests only; build_ms and deps_ms are the rest of the job's G0
        let g0 = |t: &mut Track, build: u64, deps: u64, mode: &str, at: i64| {
            t.on_notice(&json!({"kind": "gate", "gate_kind": "promote", "gate_id": "j-7", "sha": SHA, "stage": "G0", "pass": true, "reason": null,
                "score": null, "spent_musd": 0, "wall_ms": 152_648, "cache_permille": null, "build_ms": build, "deps_ms": deps, "deps_mode": mode}), at)
        };
        g0(&mut t, 1_066_524, 1_036_000, "stub", T0 + 1_219_172);
        // G0's notice splits the step in flight into the dependency build and G0, each with its own cell
        let live: Vec<String> = t.live_rows(80, T0 + 1_220_000, MAX_ROWS).iter().map(|l| l.text()).collect();
        assert!(live[0].contains("deps ■ G0 ■ G1 □") && !live.iter().any(|l| l.contains("deps+G0")), "{live:#?}");
        gate(&mut t, SHA, "G1", Some(true), None, 2_565, T0 + 1_221_737);
        assert!(t.spending(), "G2's probes spend");
        gate(&mut t, SHA, "G2", Some(true), None, 24_179, T0 + 1_245_916);
        gate(&mut t, SHA, "G3", Some(true), None, 721_485, T0 + 1_967_401);
        t.on_notice(&json!({"kind": "promoted", "sha": SHA, "handoff": false}), T0 + 1_967_401);
        t.on_notice(&json!({"kind": "gate", "gate_kind": "g4", "sha": SHA, "stage": "G4", "pass": true, "reason": "gate.g4.no_instrument",
            "score": null, "spent_musd": 0, "wall_ms": 0, "cache_permille": null}), T0 + 1_969_000);
        let done = t.on_notice(&json!({"kind": "lkg", "sha": SHA}), T0 + 1_969_000).unwrap();
        let rows: Vec<String> = committed_rows(&done[0], 100, true).iter().map(|l| l.text()).collect();
        assert!(rows[1].contains("deps dependencies    ■ built · 17 min 16 s · not cached before"), "{rows:#?}");
        assert!(rows[2].contains("G0  build + tests    ■ pass · 3 min 3 s"), "{rows:#?}");
        assert!(!rows.iter().take(3).any(|l| l.contains("waited")), "{rows:#?}");
        let r = &done[0];
        let g0_job: i64 = r.steps.iter().take(2).map(|s| s.took_ms(0).unwrap_or(0) + s.waited_ms().unwrap_or(0)).sum();
        assert!((g0_job - 1_219_172).abs() < 1000, "deps + G0 is the job record's G0: {g0_job} ms");
        let sum: i64 = r.steps.iter().map(|s| s.took_ms(0).unwrap_or(0) + s.waited_ms().unwrap_or(0)).sum();
        assert!((r.end_ms.unwrap() - r.start_ms - sum).abs() < 1000, "the rows add up: {sum} ms");
        // a warm cache: no dependency step; G0's row is its build and tests
        let mut t = Track::default();
        start(&mut t, SHA, &T2, T0);
        g0(&mut t, 32_000, 2_000, "hit", T0 + 190_000);
        let r = t.current().unwrap();
        assert_eq!((r.steps[0].name.as_str(), r.steps[0].took_ms(0), r.steps[0].waited_ms()), ("G0", Some(184_648), Some(5_352)));
        assert!(!r.steps.iter().any(|s| s.name == "deps"));
    }

    /// v0.5.4 (round 8): the kernel plans G4 whether or not sealed tasks exist, so a pending G4 says it scores only when
    /// they do, instead of reading as if a sealed test will check the change.
    #[test]
    fn a_pending_g4_says_when_it_scores() {
        let mut t = Track::default();
        start(&mut t, SHA, &T2, T0);
        let rows: Vec<String> = t.live_rows(120, T0 + 1_000, MAX_ROWS).iter().map(|l| l.text()).collect();
        let g4 = rows.iter().find(|l| l.contains("G4  scored vs LKG")).expect("the G4 row");
        assert!(g4.contains(G4_PENDING), "{g4}");
        assert!(rows.iter().filter(|l| l.contains(G4_PENDING)).count() == 1, "only G4: {rows:#?}");
        // v0.5.4 (lane K, review 7): a change that ended before G4 (refused, stopped) never runs it: no note on its G4 row
        let refused = gate(&mut t, SHA, "G0", Some(false), Some("gate.g0.test"), 1_000, T0 + 60_000).unwrap();
        let mut t = Track::default();
        start(&mut t, SHA, &T2, T0);
        let stopped = t.stop_all("by /panic", T0 + 60_000);
        for r in refused.iter().chain(&stopped) {
            let rows: Vec<String> = committed_rows(r, 120, true).iter().map(|l| l.text()).collect();
            let g4 = rows.iter().find(|l| l.contains("G4  scored vs LKG")).expect("the G4 row");
            assert!(!g4.contains(G4_PENDING), "{rows:#?}");
        }
    }

    /// v0.5.4 (user session v0.5.4 A): the dependency build's estimate stayed on screen at 32 min, and after /panic and
    /// /resume the panel kept counting. Past the range the row says so; a panic ends the change in flight, its timer and
    /// its estimate with it.
    #[test]
    fn a_blown_estimate_says_so_and_a_panic_stops_the_change() {
        let mut t = Track::default();
        start(&mut t, SHA, &["G0", "G1", "G2", "G3", "live", "G4"], T0);
        t.on_notice(&json!({"kind": "gate_deps", "gate_id": "j-7", "sha": SHA}), T0 + 5_000);
        let rows = |t: &Track, at: i64| -> Vec<String> { t.live_rows(100, at, MAX_ROWS).iter().map(|l| l.text()).collect() };
        assert!(rows(&t, T0 + 20 * 60_000).iter().any(|l| l.trim() == DEPS_ESTIMATE), "within the range: the estimate");
        let late = rows(&t, T0 + 32 * 60_000 + 44_000);
        assert!(late.iter().any(|l| l.trim() == DEPS_PAST) && !late.iter().any(|l| l.contains(DEPS_ESTIMATE)), "{late:#?}");
        assert!(DEPS_PAST.starts_with("still building · past 21 min") && DEPS_PAST.contains("dependencies alone: 15–21 min"));
        // round 2 (review SF2): whole at 80 columns
        let at80: Vec<String> = t.live_rows(80, T0 + 32 * 60_000 + 44_000, MAX_ROWS).iter().map(|l| l.text()).collect();
        assert!(at80.iter().any(|l| l == &format!("    {DEPS_PAST}")), "{at80:#?}");
        // the Root's PANIC notice: the change ends, nothing is drawn live any more
        let done = t.stop_all("by /panic", T0 + 32 * 60_000 + 44_000);
        assert_eq!((done.len(), t.in_flight(), rows(&t, T0 + 34 * 60_000 + 47_000).len()), (1, false, 0));
        let line = committed_rows(&done[0], 200, false)[0].text();
        assert!(line.starts_with("■ stopped by /panic ") && line.contains("did not go live and will not continue") && line.contains("32 min 44 s"),
            "{line}");
        assert!(!line.contains("owner_socket") && !line.contains("held-only"), "{line}");
        assert!(t.stop_all("by /panic", T0 + 40 * 60_000).is_empty(), "nothing left to stop");
        // a change that had gone live says what /resume runs
        let mut t = Track::default();
        start(&mut t, SHA, &T2, T0);
        t.on_notice(&json!({"kind": "promoted", "sha": SHA, "handoff": true, "handoff_ms": 900}), T0 + 60_000);
        let done = t.stop_all("by /panic", T0 + 90_000);
        assert!(committed_rows(&done[0], 200, false)[0].text().contains("it had gone live; /root status shows which version runs after /resume"));
    }

    /// v0.5.4 (the release review): a G0 that failed at the body build sends `deps_ms` but no `build_ms`; the
    /// dependency build it finished then ends with its own time, and is not left "·" next to G0's ✕.
    #[test]
    fn a_failed_body_build_still_ends_the_dependency_build() {
        let mut t = Track::default();
        start(&mut t, SHA, &["G0", "G1", "G2", "G3", "live", "G4"], T0);
        t.on_notice(&json!({"kind": "gate_deps", "gate_id": "j-7", "sha": SHA}), T0 + 5_000);
        let done = t.on_notice(&json!({"kind": "gate", "gate_kind": "promote", "gate_id": "j-7", "sha": SHA, "stage": "G0", "pass": false,
            "reason": "gate.build.failed", "score": null, "spent_musd": 0, "wall_ms": 40_000, "cache_permille": null, "deps_ms": 780_000,
            "deps_mode": "stub"}), T0 + 830_000).unwrap();
        let rows: Vec<String> = committed_rows(&done[0], 100, true).iter().map(|l| l.text()).collect();
        assert!(rows[1].contains("deps dependencies    ■ built · 13 min"), "{rows:#?}");
        assert!(rows[2].contains("G0  build + tests    ✕ failed · 40") && !rows.iter().any(|l| l.contains("deps dependencies    ·")), "{rows:#?}");
    }

    /// v0.5.4 (the release review): on a terminal under 12 rows the block has one step row, and it is the step in
    /// flight, not the one after it.
    #[test]
    fn a_one_row_window_shows_the_step_in_flight() {
        let mut t = Track::default();
        start(&mut t, SHA, &T2, T0);
        gate(&mut t, SHA, "G0", Some(true), None, 1_000, T0 + 1_000);
        gate(&mut t, SHA, "G1", Some(true), None, 1_000, T0 + 2_000);
        for h in [6, 8, 11] {
            let rows: Vec<String> = t.live_rows(80, T0 + 5_000, Track::rows_for(h)).iter().map(|l| l.text()).collect();
            assert!(rows.iter().skip(1).any(|l| l.contains("G2  boot probe") && l.contains("□")), "{h} rows: {rows:#?}");
        }
    }

    #[test]
    fn rules() {
        let mut t = Track::default();
        // a quick gate check and an unknown kind are not the track's
        assert_eq!(t.on_notice(&json!({"kind": "gate", "gate_kind": "quick", "sha": SHA, "stage": "G0", "pass": true}), T0), None);
        assert_eq!(t.on_notice(&json!({"kind": "level", "level": 1}), T0), None);
        assert_eq!(t.on_notice(&json!({"kind": "promoted", "sha": "unknown"}), T0), None, "a promote the track never saw stays a plain notice");
        // inconclusive: the step keeps running with its reason
        start(&mut t, SHA, &["G0", "G1", "live", "G4"], T0);
        gate(&mut t, SHA, "G0", Some(true), None, 1, T0 + 1);
        gate(&mut t, SHA, "G1", Some(true), None, 1, T0 + 2);
        t.on_notice(&json!({"kind": "promoted", "sha": SHA, "handoff": false}), T0 + 3);
        gate(&mut t, SHA, "G4", None, Some("gate.g4.insufficient"), 1, T0 + 4);
        let r = t.current().unwrap();
        assert_eq!((r.steps[3].st, r.steps[3].reason.as_deref()), (StepSt::Running, Some("insufficient")));
        let done = t.on_notice(&json!({"kind": "lkg", "sha": SHA}), T0 + 9).unwrap();
        assert_eq!(committed_rows(&done[0], 80, false)[0].text(), format!("✓ kept 06be87a12345 · 3/3 gates · {} · no restart", secs(9)));
        // a rollback of a change the track never saw (`initctl rollback`) is still one line
        let done = t.on_notice(&json!({"kind": "rolled_back", "from_sha": "aaaaaaaaaaaaaaaa", "to_sha": "bbbbbbbbbbbbbbbb", "reason": "initctl", "to_lkg": true}), T0).unwrap();
        assert_eq!(committed_rows(&done[0], 80, false)[0].text(), "✕ rolled back aaaaaaaaaaaa · initctl · ↺ back on LKG bbbbbbbbbbbb");
        // a G4 notice for a change that started before this TUI did: a run of its own
        let mut t = Track::default();
        gate(&mut t, SHA, "G4", Some(true), None, 1_000, T0);
        assert_eq!(t.current().map(|r| r.steps.len()), Some(1));
        // zone text in a reason is inert
        let mut t = Track::default();
        start(&mut t, SHA, &T2, T0);
        let done = gate(&mut t, SHA, "G0", Some(false), Some("x\u{1b}[2Jy"), 1, T0 + 1).unwrap();
        assert!(!committed_rows(&done[0], 80, true).iter().any(|l| l.text().contains('\u{1b}')));
    }

    /// v0.5.3 (the release review): a self-change's expanded steps showed each step's `spent_musd`, the kernel's
    /// figure at its own prices (DeepSeek's off-peak window included), beside the Root's ledger figure in /usage:
    /// the two prices for one request that root/egress.md §3.4 removes. No step shows a dollar figure now.
    #[test]
    fn steps_show_no_kernel_price() {
        let mut t = Track::default();
        start(&mut t, SHA, &["G0", "G1", "G2", "live"], T0);
        for (i, stage) in ["G0", "G1", "G2"].iter().enumerate() {
            t.on_notice(&json!({"kind": "gate", "gate_kind": "promote", "gate_id": "j-7", "sha": SHA, "stage": stage, "pass": true, "reason": null,
                "score": null, "spent_musd": 18_049, "wall_ms": 1_000, "cache_permille": 990}), T0 + 1_000 * (i as i64 + 1));
        }
        let live: Vec<String> = t.live_rows(120, T0 + 3_500, 7).iter().map(|l| l.text()).collect();
        t.on_notice(&json!({"kind": "promoted", "sha": SHA, "handoff": false}), T0 + 4_000);
        let done = t.on_notice(&json!({"kind": "lkg", "sha": SHA}), T0 + 4_000).unwrap();
        let rows: Vec<String> = committed_rows(&done[0], 120, true).iter().map(|l| l.text()).collect();
        assert!(rows[3].contains("■ pass · 1.0 s · cache 99.0%"), "{rows:#?}");
        for r in live.iter().chain(&rows) {
            assert!(!r.contains('$'), "a kernel price: {r}");
        }
    }
}
