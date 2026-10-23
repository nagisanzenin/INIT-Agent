//! R7: the autonomy ledger (root/root.md §3.13, as redefined by ADR-011 §7; lane X7). The level is a pure fold of
//! the lineage plus the Root clock: `ladder` records set it; the unlock criteria count **Root-written records only**
//! since the level's entry record (owner turns from `turns`, days, clean groups from `change_open`/`change_close`,
//! direct harm rollbacks, quarantines from `cap_state`, demotions, twin reports). No kernel-reported record enters
//! the record — `promotion`, gate records and zone-evidence usage stay out — so RT-GAP-1's class is closed by
//! construction. A demotion trigger opens an incident, and one incident costs one level. `tick` evaluates after every
//! record and every 60 s: a climb, and a demotion that rolls nothing back (`harm_rate`, quarantine debt), are
//! appended here; any other demotion goes to the supervisor, which owns the rollback (R1, sup.rs).
//! Triggers by path (charter `extend.mode`): on `"free"` §7.3's (H4 = a demoting refusal or an `escapee`, `/panic`, a
//! crash loop on `lkg`, quarantine debt after 72 h, `harm_rate`); on `"gated"` today's (sealed-audit regression,
//! agent mutation-suite failure, demoting refusal, crash loop, body safe mode, gate canary, `/panic`) plus `harm_rate`.

use crate::link::{Root, lock};
use crate::util::{now_ms, parse_rfc3339, rfc3339};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};

/// Phase 0 Root builds support only L0 (spec §19); test builds support every level (testing.md §3.4).
pub const SUPPORTED_MAX_LEVEL: i64 = if cfg!(feature = "test-fixtures") { 4 } else { 0 };
const DAY: i64 = 86_400_000;
/// An incident ends when the rollback target's kernel is ready (`zone_start`), or after 600 s (§3.13.3).
const INCIDENT_MS: i64 = 600_000;
/// §3.13.3 repeated demotion: this many `ladder down` records at L0 within `L0_REPEAT_MS` put the system in
/// safe mode, as a crash loop on LKG does (provisional owner decision 2026-10-20, build-deviations §3.9).
pub const L0_REPEAT: usize = 3;
pub const L0_REPEAT_MS: i64 = DAY;

/// ADR-011 §7.2: what a climb to L1…L4 needs, counted since the level's entry record. `direct_max` is over the last
/// 30 closed groups; `twins`: that many newest weekly twin reports with no regression; `quarantine_days`: no
/// quarantine in that many days (0 = no rule).
struct Need {
    days: i64,
    turns: u64,
    clean: u64,
    direct_max: u64,
    twins: usize,
    quarantine_days: i64,
}
const NEED: [Need; 4] = [
    Need { days: 7, turns: 40, clean: 10, direct_max: 6, twins: 0, quarantine_days: 0 },
    Need { days: 21, turns: 150, clean: 40, direct_max: 4, twins: 2, quarantine_days: 14 },
    Need { days: 30, turns: 400, clean: 100, direct_max: 3, twins: 4, quarantine_days: 0 },
    Need { days: 30, turns: 400, clean: 100, direct_max: 3, twins: 0, quarantine_days: 0 },
];
/// "The last 30 closed groups" (§7.2, §7.3).
const LAST_GROUPS: usize = 30;
/// The baseline ring (ADR-011 §3.2): 40 owner turns; a clean group needs 5 (§7.1).
const RING_M: u64 = 40;
const CLEAN_TURNS: u64 = 5;
/// X10 item 5 (orchestrator, 2026-10-22): the owner-corrections trend. The reference is the first 14 free-path owner
/// days (and at least 40 owner turns); the window is the last 14 owner days after it (at least 40 owner turns). It
/// fires when the window's correction rate is ≥ 1.5 × r0, with r0 = max(50‰, (c_ref + 1)/(n_ref + 2)), and the exact
/// one-sided two-sample test (Fisher: the window's corrections among all, P(X ≥ c) with X hypergeometric over both
/// spans) is ≤ 1/10,000. The two-sample test carries the reference's own sampling error: a one-sample binomial test
/// against r0 fired 11 times in 20 null years (a low reference by chance), the Fisher test 0. It alerts and blocks
/// level-ups; it rolls nothing back.
const TREND_DAYS: usize = 14;
const TREND_MIN_TURNS: u64 = 40;
const TREND_ALPHA: (u64, u64) = (1, 10_000);
const TREND_FLOOR: (u64, u64) = (50, 1000);

/// The trend test's verdict (`ready`: both spans hold enough owner turns).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Trend {
    pub ready: bool,
    pub fire: bool,
    /// r0 in lowest terms.
    pub r0: (u64, u64),
    /// The exact one-sided Fisher tail in ppm (rounded down), computed only when the magnitude guard holds.
    pub p_ppm: Option<u64>,
    pub rate_permille: u64,
    pub r0_permille: u64,
    pub n: u64,
    pub c: u64,
}

/// The owner-corrections trend over `reference` = (owner turns, corrections) and `window` = (owner turns, corrections).
pub fn corrections_trend(reference: (u64, u64), window: (u64, u64)) -> Trend {
    use crate::sentinel::stats::{fisher_tail, max_rate, reduce};
    let ((nr, cr), (n, c)) = (reference, window);
    let r0 = max_rate(TREND_FLOOR, reduce(cr.min(nr) + 1, nr + 2));
    let mut t = Trend { r0, n, c, r0_permille: r0.0 * 1000 / r0.1, rate_permille: (c * 1000).checked_div(n).unwrap_or(0), ..Trend::default() };
    t.ready = nr >= TREND_MIN_TURNS && n >= TREND_MIN_TURNS;
    if !t.ready || c > n {
        return t;
    }
    if u128::from(c) * u128::from(r0.1) * 2 < u128::from(r0.0) * u128::from(n) * 3 {
        return t; // below 1.5 × r0: no tail is computed (the daily fold stays cheap)
    }
    let tail = fisher_tail(nr + n, cr.min(nr) + c, n, c);
    (t.p_ppm, t.fire) = (Some(tail.ppm()), tail.le(TREND_ALPHA));
    t
}

/// §7.3 triggers that roll nothing back: `harm_rate`, and quarantine debt (only §6.3's own rollback). R7 appends
/// their `ladder down` itself; the others go to the supervisor.
pub const NO_ROLLBACK: [&str; 2] = ["harm_rate", "quarantine_debt"];

pub fn cap(max_level: i64) -> i64 {
    max_level.min(SUPPORTED_MAX_LEVEL)
}

/// The rollback target of a demotion and of panic resume: `settled` on the free path (ADR-011 §4.1), audited-good on
/// `"gated"`. `tuple::rollback` falls back to LKG when the pointer is unset.
pub fn anchor(root: &Root) -> &'static str {
    if lock(&root.st).charter.as_ref().is_some_and(|l| l.ch.extend.free()) { "settled" } else { "audited" }
}

/// The level as the lineage says it, with the record since the level's entry record.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Level {
    pub level: i64,
    pub entered_seq: u64,
    pub entered_at: Option<String>,
    pub last_change: Option<Value>,
    pub seq: Option<u64>,
    /// A demotion trigger that no `ladder down` has answered yet: (kind, seq).
    pub pending: Option<(String, u64)>,
    entered_ms: i64,
    /// The newest `charter` record's `extend.mode` is `"free"`.
    free: bool,
    /// Owner turns from `turns` records whose day started at or after the entry record.
    turns: u64,
    /// Owner turns from `turns` days that started at or after the newest group close that was not `clean` (or
    /// genesis): a conservative, lineage-recomputable reading of "the baseline ring is full" (ADR-011 §3.2).
    ring: u64,
    ring_from: i64,
    /// Clean groups (§7.1) closed since the entry record.
    clean: u64,
    /// Per group: the parent tuple of its first change and the tuple of its last.
    groups: BTreeMap<u64, (String, String)>,
    /// The last 30 closes: was it a `direct` harm rollback.
    closes: VecDeque<bool>,
    /// When the newest `ladder down` records and quarantines were appended (bounded).
    downs: VecDeque<i64>,
    quarantines: VecDeque<i64>,
    /// The newest twin reports: no regression (bounded at 4).
    twins: VecDeque<bool>,
    /// The corrections trend: the reference span (days, owner turns, corrections), the window's days, the verdict, and
    /// whether its alert was recorded (`harm{signal:"corrections_trend"}`) since it last fired.
    trend_ref: (usize, u64, u64),
    trend_win: VecDeque<(u64, u64)>,
    pub trend: Trend,
    trend_alerted: bool,
    safe: bool,
    panicked: bool,
    /// The newest incident: (seq of its `ladder down`, at); `None` once its kernel is ready.
    incident: Option<(u64, i64)>,
    /// When the last `L0_REPEAT` demotions at L0 (`ladder down` from 0) were recorded.
    l0_downs: VecDeque<i64>,
    /// `state/root.json` `counters`, folded with the level so a Root restart keeps them.
    pub counters: Counters,
}

/// `state/root.json` `counters` (root/root.md §4.5; WP21 D10), from the records: kernel crashes (`zone_exit`
/// class `crash`), whole-system rollbacks (`rollback`), and kernel swaps (`kernel_upgrade`: `installed` is a
/// swap, a later `boot_fail` or `probation_fail` turns it into a failed one, as does `gog_fail`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Counters {
    pub crashes_total: u64,
    pub rollbacks: u64,
    pub swaps_ok: u64,
    pub swaps_failed: u64,
}

impl Counters {
    fn apply(&mut self, kind: &str, d: &Value) {
        match (kind, d["stage"].as_str()) {
            ("zone_exit", _) if d["class"] == "crash" => self.crashes_total += 1,
            ("rollback", _) => self.rollbacks += 1,
            ("kernel_upgrade", Some("installed")) => self.swaps_ok += 1,
            ("kernel_upgrade", Some("boot_fail" | "probation_fail")) => (self.swaps_ok, self.swaps_failed) = (self.swaps_ok.saturating_sub(1), self.swaps_failed + 1),
            ("kernel_upgrade", Some("gog_fail")) => self.swaps_failed += 1,
            _ => {}
        }
    }
    pub fn json(&self) -> Value {
        json!({"crashes_total": self.crashes_total, "rollbacks": self.rollbacks, "swaps_failed": self.swaps_failed, "swaps_ok": self.swaps_ok})
    }
}

/// §3.13.1 `evaluate`: a climb with its criteria, a demotion for a pending trigger, or nothing.
#[derive(Debug, PartialEq)]
pub enum Step {
    Up(Value),
    Down(String, u64),
    Stay,
}

/// A twin report with no regression (ADR-011 Evaluation plan): its own `regression` flag when X9 writes one, else at
/// least 40 pairs, `ended` not significantly worse (p > 0.05 when the current arm ended fewer tasks) and a median
/// cost ratio ≤ 1.5.
fn twin_ok(d: &Value) -> bool {
    if let Some(r) = d["regression"].as_bool() {
        return !r;
    }
    let worse = d["ended"]["cur"].as_u64() < d["ended"]["twin"].as_u64() && d["ended"]["p_ppm"].as_u64().unwrap_or(0) <= 50_000;
    d["pairs"].as_u64().unwrap_or(0) >= 40 && !worse && d["cost"]["median_ratio_permille"].as_u64().is_some_and(|c| c <= 1500)
}

fn bounded<T>(q: &mut VecDeque<T>, x: T, n: usize) {
    q.push_back(x);
    while q.len() > n {
        q.pop_front();
    }
}

impl Level {
    pub fn fold(records: &[Value]) -> Level {
        let mut l = Level::default();
        records.iter().for_each(|r| l.apply(r));
        l
    }

    pub fn apply(&mut self, r: &Value) {
        let (seq, d, kind) = (r["seq"].as_u64().unwrap_or(0), &r["data"], r["kind"].as_str().unwrap_or(""));
        let at = r["at"].as_str().and_then(parse_rfc3339).unwrap_or(0);
        let root = r["by"] == "root";
        self.seq = Some(seq);
        self.counters.apply(kind, d);
        match kind {
            "genesis" => {
                self.enter(r, seq, at);
                self.ring_from = at;
            }
            "ladder" => {
                self.level = d["to"].as_i64().unwrap_or(self.level);
                self.enter(r, seq, at);
                self.last_change = Some(json!({"at": r["at"], "cause": d["cause"], "direction": d["direction"],
                    "from": d["from"], "seq": seq, "to": d["to"], "trigger": d["trigger"]}));
                if d["direction"] == "down" {
                    (self.pending, self.incident) = (None, Some((seq, at)));
                    bounded(&mut self.downs, at, 16);
                    if d["from"] == 0 {
                        bounded(&mut self.l0_downs, at, L0_REPEAT);
                    }
                }
            }
            "charter" => self.free = d["extend"]["mode"] == "free",
            "safe" => self.safe = d["on"] == true,
            "panic" | "resume" => self.panicked = kind == "panic",
            "zone_start" => self.incident = None,
            "turns" if root => {
                let (start, owner) = (d["start"].as_str().and_then(parse_rfc3339).unwrap_or(0), d["owner"].as_u64().unwrap_or(0));
                if start >= self.entered_ms {
                    self.turns += owner;
                }
                if start >= self.ring_from {
                    self.ring += owner;
                }
                let corrected = d["owner_corrected"].as_u64().unwrap_or(0);
                let r = &mut self.trend_ref;
                if r.0 < TREND_DAYS || r.1 < TREND_MIN_TURNS {
                    *r = (r.0 + 1, r.1 + owner, r.2 + corrected);
                } else {
                    bounded(&mut self.trend_win, (owner, corrected), TREND_DAYS);
                    let w = self.trend_win.iter().fold((0, 0), |a, x| (a.0 + x.0, a.1 + x.1));
                    self.trend = corrections_trend((r.1, r.2), w);
                    self.trend_alerted &= self.trend.fire;
                }
            }
            "harm" if root && d["signal"] == "corrections_trend" => self.trend_alerted = true,
            "change_open" if root => {
                if let Some(g) = d["group"].as_u64() {
                    let tuple = d["tuple"].as_str().unwrap_or("").to_string();
                    let e = self.groups.entry(g).or_insert_with(|| (d["parent_tuple"].as_str().unwrap_or("").to_string(), String::new()));
                    e.1 = tuple;
                }
            }
            "change_close" if root => {
                let (parent, tuple) = d["group"].as_u64().and_then(|g| self.groups.remove(&g)).unwrap_or_default();
                let outcome = d["outcome"].as_str().unwrap_or("");
                if outcome == "clean" && d["turns"].as_u64().unwrap_or(0) >= CLEAN_TURNS && !tuple.is_empty() && tuple != parent {
                    self.clean += 1;
                }
                if outcome != "clean" {
                    (self.ring, self.ring_from) = (0, at + 1);
                }
                bounded(&mut self.closes, d["attribution"] == "direct", LAST_GROUPS);
            }
            "cap_state" if root && d["to"] == "quarantined" => bounded(&mut self.quarantines, at, 64),
            "twin_report" if root => bounded(&mut self.twins, twin_ok(d), 4),
            _ => {}
        }
        let trigger = self.trigger(kind, d, root);
        if let Some(t) = trigger.filter(|_| !self.open(at) && self.pending.is_none()) {
            self.pending = Some((t.into(), seq));
        }
    }

    /// §3.13.3 and ADR-011 §7.3: the demotion trigger a record is, on this path. One inside an open incident is linked
    /// to it (coalesced) by the caller.
    fn trigger(&self, kind: &str, d: &Value, root: bool) -> Option<&'static str> {
        let both = match kind {
            "ceiling_hit" if d["demotes"] == true => Some("ceiling_hit"),
            "crash_loop" if d["action"] == "demote" => Some("crash_loop"),
            "panic" => Some("panic"),
            "harm" if root && d["signal"] == "quarantine_debt" && d["action"] == "demote" => Some("quarantine_debt"),
            // direct harm rollbacks in the last 30 closed groups above twice the current level's threshold
            "change_close" if root && d["attribution"] == "direct" && self.level >= 1 => {
                let direct = self.closes.iter().filter(|x| **x).count() as u64;
                (direct > 2 * NEED[(self.level - 1) as usize].direct_max).then_some("harm_rate")
            }
            _ => None,
        };
        if both.is_some() {
            return both;
        }
        match kind {
            "escapee" if self.free => Some("escapee"),
            _ if self.free => None,
            "audit" if d["regression"] == true => Some("audit"),
            // only an agent-built candidate's failure demotes; a maintainer-built one alerts (sealed-eval.md §3.3)
            "mutation" if d["pass"] == false && d["source"] == "agent" => Some("mutation"),
            "body_safe" if d["on"] == true && !self.safe => Some("body_safe"),
            "gate" if d["reason"] == "canary" => Some("gate"),
            _ => None,
        }
    }

    /// The record restarts at the level's entry record (the newest `ladder`, or genesis).
    fn enter(&mut self, r: &Value, seq: u64, at: i64) {
        (self.entered_seq, self.entered_at, self.entered_ms) = (seq, r["at"].as_str().map(String::from), at);
        (self.turns, self.clean) = (0, 0);
    }

    /// §3.9: the newest demotion at L0 is the `L0_REPEAT`-th within `L0_REPEAT_MS` of `now`.
    pub fn repeated_l0(&self, now: i64) -> bool {
        self.l0_downs.len() >= L0_REPEAT && self.l0_downs.front().is_some_and(|&t| now - t < L0_REPEAT_MS)
    }

    fn open(&self, at: i64) -> bool {
        self.incident.is_some_and(|(_, t)| at - t < INCIDENT_MS)
    }

    /// ADR-011 §7.2: the criteria for `level + 1` as `[{id, have, need, met}]`.
    fn criteria(&self, now: i64) -> Vec<Value> {
        let c = |id: &str, have: i64, need: i64, met: bool| json!({"have": have, "id": id, "met": met, "need": need});
        let min = |id: &str, have: i64, need: i64| c(id, have, need, have >= need);
        let max = |id: &str, have: i64, need: i64| c(id, have, need, have <= need);
        let to = (self.level + 1).clamp(1, 4);
        let n = &NEED[(to - 1) as usize];
        let within = |q: &VecDeque<i64>, days: i64| q.iter().filter(|&&t| t <= now && now - t < days * DAY).count() as i64;
        let direct = self.closes.iter().filter(|x| **x).count() as i64;
        let mut v = vec![min("days_at_level", (now - self.entered_ms).div_euclid(DAY), n.days), min("owner_turns", self.turns as i64, n.turns as i64),
            min("clean_groups", self.clean as i64, n.clean as i64), max("direct_rollbacks_last_30_groups", direct, n.direct_max as i64)];
        if to == 1 {
            v.push(min("baseline_ring", self.ring.min(RING_M) as i64, RING_M as i64));
            v.push(max("demotions_last_7_days", within(&self.downs, 7), 0));
        }
        if n.quarantine_days > 0 {
            v.push(max("quarantines_last_14_days", within(&self.quarantines, n.quarantine_days), 0));
        }
        if n.twins > 0 {
            let streak = self.twins.iter().rev().take_while(|x| **x).count().min(n.twins);
            v.push(min("twin_reports_no_regression", streak as i64, n.twins as i64));
        }
        // the owner-corrections trend blocks every climb while it fires (X10 item 5)
        v.push(c("corrections_trend", self.trend.rate_permille as i64, self.trend.r0_permille as i64, !self.trend.fire));
        v
    }

    /// The trend fires and its alert is not recorded yet (select.rs records it).
    pub fn trend_alert_due(&self) -> Option<Trend> {
        (self.trend.fire && !self.trend_alerted).then(|| self.trend.clone())
    }

    /// §3.13.1: a pending trigger demotes; a climb needs no open incident, normal mode, no panic, room under
    /// the cap and every criterion of `level + 1`. At most one step per evaluation.
    pub fn step(&self, now: i64, cap: i64) -> Step {
        if let Some((k, s)) = &self.pending {
            return Step::Down(k.clone(), *s);
        }
        if self.level >= cap || self.open(now) || self.safe || self.panicked {
            return Step::Stay;
        }
        let c = self.criteria(now);
        if c.iter().all(|x| x["met"] == true) { Step::Up(json!(c)) } else { Step::Stay }
    }

    /// `state/ledger.json` and its view (§4.5); `next` is null at the cap.
    pub fn json(&self, max_level: i64) -> Value {
        let (now, cap) = (now_ms(), cap(max_level));
        let next = (self.level < cap).then(|| json!({"criteria": self.criteria(now), "level": self.level + 1}));
        let incident = self.incident.filter(|_| self.open(now)).map(|(s, t)| json!({"at": rfc3339(t), "seq": s}));
        json!({"cap": cap, "entered_at": self.entered_at, "entered_seq": self.entered_seq,
            "evolution_share_max_permille": null, "incident": incident, "last_change": self.last_change,
            "level": self.level, "max_level": max_level, "next": next, "path": if self.free { "free" } else { "gated" }, "seq": self.seq,
            "supported_max_level": SUPPORTED_MAX_LEVEL, "v": 1})
    }
}

/// ADR-011 §2.3: on the free path a kernel `promotion{lkg:true}` must name the `sha` of a change whose group the Root
/// closed `clean`, `quiet` or `flagged`, so LKG stays Root-anchored (sup.rs refuses it otherwise).
pub fn promotion_closed(records: &[Value], sha: &str) -> bool {
    let mut groups = std::collections::BTreeSet::new();
    for r in records.iter().filter(|r| r["by"] == "root") {
        let (d, g) = (&r["data"], r["data"]["group"].as_u64());
        match r["kind"].as_str() {
            Some("change_open") if !sha.is_empty() && d["sha"] == sha => groups.extend(g),
            Some("change_close") if matches!(d["outcome"].as_str(), Some("clean" | "quiet" | "flagged")) && g.is_some_and(|g| groups.contains(&g)) => return true,
            _ => {}
        }
    }
    false
}

/// `ledger --verify` (ADR-011 Records): re-folds the chain and checks every `ladder` change against §7's terms at the
/// moment it was appended: a climb had every criterion of its level met; a `harm_rate` or other demotion answered a
/// trigger the fold holds. Returns the problems found (empty: verified).
pub fn verify(records: &[Value]) -> Vec<String> {
    let mut l = Level::default();
    let mut bad = Vec::new();
    for r in records {
        if r["kind"] == "ladder" && r["by"] == "root" {
            let (d, at, seq) = (&r["data"], r["at"].as_str().and_then(parse_rfc3339).unwrap_or(0), r["seq"].as_u64().unwrap_or(0));
            let fixture = d["trigger"]["kind"] == "test_fixture";
            match d["direction"].as_str() {
                Some("up") if !fixture => {
                    let unmet: Vec<String> = l.criteria(at).into_iter().filter(|c| c["met"] != true).map(|c| c["id"].as_str().unwrap_or("").to_string()).collect();
                    if !unmet.is_empty() || d["to"].as_i64() != Some(l.level + 1) {
                        bad.push(format!("seq {seq}: a climb to L{} with unmet criteria {unmet:?}", d["to"]));
                    }
                }
                // a panic always costs one level, even inside an open incident (panic.rs)
                Some("down") if l.pending.as_ref().is_none_or(|p| d["trigger"]["seq"].as_u64() != Some(p.1)) && d["trigger"]["kind"] != "panic" => {
                    bad.push(format!("seq {seq}: a demotion that answers no pending trigger"));
                }
                _ => {}
            }
        }
        l.apply(r);
    }
    bad
}

/// Evaluation (§3.13.1), after every appended record and every 60 s. First the grant overlay's expiry (grant.rs) and
/// selection's debts and day boundary (select.rs), each throttled and never re-entered. A climb, and a demotion that
/// rolls nothing back, are appended under the lineage lock, so two evaluations never step twice; any other pending
/// trigger goes to the supervisor (`Ev::Demote`).
pub fn tick(root: &Root) {
    crate::grant::tick(root);
    crate::select::tick(root);
    let mut lg = lock(&root.lin);
    if lg.is_none() {
        return;
    }
    let (step, from) = {
        let st = lock(&root.st);
        (st.level.step(now_ms(), cap(st.charter.as_ref().map_or(0, |l| l.ch.autonomy.max_level))), st.level.level)
    };
    let (rec, up) = match step {
        Step::Stay => return,
        Step::Down(k, seq) if NO_ROLLBACK.contains(&k.as_str()) => {
            let to = (from - 1).max(0);
            let cause = if k == "harm_rate" { "harm_rate" } else { "demotion" };
            let down = json!({"cause": cause, "direction": "down", "from": from, "to": to, "trigger": {"kind": k, "seq": seq}});
            (root.append_locked(&mut lg, "root", "ladder", down), false)
        }
        Step::Down(..) => {
            drop(lg);
            return root.sup.send(crate::sup::Ev::Demote);
        }
        Step::Up(criteria) => {
            let up = json!({"cause": "unlock", "criteria": criteria, "direction": "up", "from": from, "to": from + 1});
            (root.append_locked(&mut lg, "root", "ladder", up), true)
        }
    };
    drop(lg);
    if let Ok(r) = rec {
        root.after(&r);
        let (f, t) = (r["data"]["from"].as_i64().unwrap_or(0), r["data"]["to"].as_i64().unwrap_or(0));
        if up {
            root.alert("level_up", &format!("[root] level L{f}→L{t}: the unlock criteria held"));
        } else {
            let k = r["data"]["trigger"]["kind"].as_str().unwrap_or("");
            root.alert("demoted", &format!("[root] demoted L{f}→L{t} ({k}); nothing is rolled back; the hold time restarts"));
        }
        root.publish();
    }
}

#[cfg(test)]
mod tests;
