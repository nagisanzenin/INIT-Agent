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
    pub spent_musd: Option<u64>,
    pub cache_permille: Option<u64>,
    pub score: Option<i64>,
    /// A failure's reason, or why a running step runs again (`insufficient`: an escalated re-run).
    pub reason: Option<String>,
}

impl Step {
    fn new(name: &str) -> Step {
        Step { name: name.into(), st: StepSt::Pending, since_ms: None, end_ms: None, wall_ms: None, spent_musd: None, cache_permille: None, score: None, reason: None }
    }
    fn is_gate(&self) -> bool {
        self.name.starts_with('G')
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
    /// `passed/planned` gates (`4/4`).
    pub fn gate_count(&self) -> (usize, usize) {
        let g: Vec<&Step> = self.steps.iter().filter(|s| s.is_gate()).collect();
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
}

impl Track {
    fn run_mut(&mut self, sha: &str) -> Option<&mut Run> {
        self.runs.iter_mut().rev().find(|r| r.sha == sha)
    }

    fn take_done(&mut self) -> Vec<Run> {
        let (done, keep): (Vec<Run>, Vec<Run>) = std::mem::take(&mut self.runs).into_iter().partition(|r| r.outcome.is_some());
        self.runs = keep;
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
                s.spent_musd = d["spent_musd"].as_u64();
                s.cache_permille = d["cache_permille"].as_u64();
                s.score = d["score"].as_i64();
                s.reason = reason_of(&d["reason"]);
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

    /// The change drawn live: the newest without an outcome.
    pub fn current(&self) -> Option<&Run> {
        self.runs.iter().rev().find(|r| r.outcome.is_none())
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

/// `G0 ■ G1 ■ G2 □ G3 · hand-off · G4 ·`
fn strip(r: &Run) -> Line {
    let mut l = Line::new();
    for (i, s) in r.steps.iter().enumerate() {
        if i > 0 {
            l.push(" ", Style::PLAIN);
        }
        l.push(format!("{} ", if s.is_gate() { s.name.as_str() } else { "live" }), Style::fg(Role::Mute));
        let (g, st) = cell(s.st);
        l.push(g, st);
    }
    l
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

/// One step's row: `    G2  boot probe        □ 2.9 s` (the time ticks while it runs).
fn step_row(s: &Step, width: usize, now_ms: i64, detail: bool) -> Line {
    let mut l = Line::from_span("    ", Style::PLAIN);
    let name = step_name(&s.name);
    l.push(format!("{name:<3} "), Style::fg(Role::Dim));
    l.push(format!("{:<w$}", label(&s.name), w = LABEL_W.saturating_sub(name.chars().count().saturating_sub(3))), Style::fg(Role::Mute));
    let (g, st) = cell(s.st);
    l.push(format!(" {g} "), st);
    let took = s.took_ms(now_ms).map(secs);
    match s.st {
        StepSt::Pass => {
            l.push("pass", Style::fg(Role::Pass));
            if let Some(t) = took {
                l.push(format!(" · {t}"), Style::dim());
            }
        }
        StepSt::Fail => {
            l.extend(sanitize::inline(s.reason.as_deref().unwrap_or("failed"), Style::fg(Role::Harm)));
            if let Some(t) = took {
                l.push(format!(" · {t}"), Style::dim());
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
        StepSt::Pending => {}
    }
    if detail {
        if let Some(m) = s.spent_musd.filter(|m| *m > 0) {
            l.push(format!(" · {}", super::usd_fine(m as i64)), Style::dim());
        }
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
    let room = max_rows.saturating_sub(1 + usize::from(waiting));
    let n = r.steps.len();
    let at = r.steps.iter().position(|s| s.st != StepSt::Pass).unwrap_or(n.saturating_sub(1));
    let skip = (at + 2).saturating_sub(room).min(n.saturating_sub(room));
    rows.extend(r.steps.iter().skip(skip).take(room).map(|s| step_row(s, width, now_ms, false)));
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
        rows.extend(r.steps.iter().map(|s| step_row(s, width, end, true)));
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
        t.on_notice(&json!({"kind": "gate", "gate_kind": "promote", "gate_id": "j-7", "sha": sha, "stage": stage, "pass": pass, "reason": reason,
            "score": null, "spent_musd": 0, "wall_ms": wall, "cache_permille": null}), at)
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
}
