//! ADR-013 (GBAL, goal-directed self-modification): the Root's fixed part. The loop itself (mode choice, instrument
//! tests, judging, acceptance, stop rules) is the model's, in the T0 skills `gbal` and `goal-loop`. The Root keeps:
//! 1. the **goal record**: only an owner line sets it (`/goal "<intent>" …`); the agent may only propose (`goal.propose`);
//! 2. the **per-goal budget meter** (zone spend while the goal is active, active wall time, rounds = predictions);
//! 3. the **prediction record** (`goal.predict`), timestamped by its lineage seq before the change it names goes live;
//!    a reading of a change with no earlier prediction is `registered:false` (unregistered);
//! 4. **observation cards** computed from what the Root already sees per owner turn (egress cost, wall, requests,
//!    cache reads, provider errors; owner corrections, `/undo` included), and their readings (Δ, σ, se, z);
//! 5. the **outcome record** and the calibration (Brier score per goal kind) folded from Root records only.
//!
//! Owner-declared guardrail cards and the agent's own `card:<id> gate` roll a change back through H12's path (always
//! honoured, never counted). Nothing here enters the L0–L4 ledger (ledger.rs folds none of these kinds), approves a
//! change (admission is unchanged), judges merit or runs a model: every number is integer arithmetic on Root records.
//! State: `ROOT_HOME/state/goals/state.json` (0600) and `<id>.md` texts; lineage kinds `goal`, `goal_proposal`,
//! `goal_predict`, `card_reading`, `goal_outcome`.

use crate::link::{Root, lock};
use crate::sentinel::Turn;
use crate::util::{self, now_ms, parse_rfc3339, rfc3339, sha256_hex};
use crate::zone::Err3;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Mutex;

pub const STOP: [&str; 6] = ["met", "budget", "plateau", "instrument_gap", "owner_stop", "harm"];
/// Owner turns kept for the cards (the newest), and the baseline window of one reading.
const OBS_MAX: usize = 400;
const BASE_MAX: usize = 40;
/// The noise floor needs this many baseline turns (ADR-013 §3: ≥ 5 baseline runs); a reading needs ≥ 3 after.
pub const BASE_MIN: usize = 5;
const AFTER_MIN: usize = 3;
/// L3 D1 (v0.5.3): "beyond noise" is a one-sided exact permutation p-value (goal/perm.rs) at or below Φ(−2) =
/// 0.02275013…, the normal tail at z = 2 that ADR-013 decision 4 states as the false-rollback rate per guardrail per
/// measured change; in ppm, rounded down so the stated rate is a bound. It used to be Welch's z ≥ 2, which only a
/// large sample makes a 2.3 % rule (STATS-053, on main: null guardrail rollbacks up to 9.3 % of readings and null
/// `better` up to 42 %, by card and size).
pub const NOISE_ALPHA_PPM: u64 = 22_750;
/// L3 D2 (v0.5.3): a goal's false-`met` rate is at most 5 % (the conventional α, Fisher's), spent evenly over the
/// goal's rounds budget R: the attributed reading needs p ≤ 5 % / R toward the threshold. `met_check` counts at most
/// one goal-card reading of each round's change, its first completed one (v0.5.3: `card.measure` can read a live change
/// again, and a later reading of it is ignored), so the union bound (Bonferroni) over the R rounds holds
/// under any dependence between readings (stacked changes share baselines). This is FWER control over R different
/// changes, not a group-sequential design (Pocock, O'Brien–Fleming), which spends α over repeated looks at one
/// accumulating sample.
pub const MET_ALPHA_PPM: u64 = 50_000;
const TEXT_MAX: usize = 8192;
/// L3 D3: the forecast the Root files for a live change the agent did not forecast, in a card goal: 500‰ that the
/// goal card moves toward the threshold, the forecast with no information. Its Brier score is 0.25 whatever happens,
/// and an honest forecast q scores q(1 − q) ≤ 0.25 in expectation, so leaving a change unforecast never lowers the
/// agent's expected Brier score, nor raises its skill (proper scoring, Gneiting & Raftery 2007, over every change).
const ABSTAIN_PERMILLE: u64 = 500;

/// A built-in observation card: one integer per owner turn, which direction is better, and how many owner turns an
/// owner guardrail on it reads by default (`guard_n`).
pub struct Card {
    pub id: &'static str,
    pub lower_better: bool,
    pub unit: &'static str,
    pub value: fn(&Obs) -> Option<i64>,
    pub guard_n: usize,
}

/// v0.5.3 (the owner's decision of 2026-10-29, from STATS-053's discrete-card table): an owner guardrail on a discrete
/// or count card (requests, cache reads, provider errors, corrections) reads 15 turns of the change. At 5 the exact
/// test is blind to a fivefold rise of a 2 % or 10 % rate (10 binary turns hold too few events); at 15 it holds the
/// 2.3 % level and catches at least what v0.5.2's 5-turn z caught, on every such card and rise. The continuous cards
/// (cost, wall) keep `BASE_MIN`.
pub const GUARD_DISCRETE: usize = 15;

pub const CARDS: [Card; 6] = [
    Card { id: "cost_turn", lower_better: true, unit: "musd", value: |o| Some(o.cost as i64), guard_n: BASE_MIN },
    Card { id: "wall_turn", lower_better: true, unit: "ms", value: |o| Some(o.wall as i64), guard_n: BASE_MIN },
    Card { id: "requests_turn", lower_better: true, unit: "requests", value: |o| Some(i64::from(o.req)), guard_n: GUARD_DISCRETE },
    Card { id: "cache_read", lower_better: false, unit: "permille", value: |o| (o.input > 0).then(|| (o.cache * 1000 / o.input) as i64), guard_n: GUARD_DISCRETE },
    Card { id: "provider_errors", lower_better: true, unit: "errors", value: |o| Some(i64::from(o.perr)), guard_n: GUARD_DISCRETE },
    Card { id: "corrections", lower_better: true, unit: "permille", value: |o| Some(if o.corr { 1000 } else { 0 }), guard_n: GUARD_DISCRETE },
];

pub fn card(id: &str) -> Option<&'static Card> {
    CARDS.iter().find(|c| c.id == id)
}

/// One closed owner turn, as the cards read it.
#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
pub struct Obs {
    pub seq: u64,
    pub end: i64,
    pub tuple: String,
    pub cost: u64,
    pub wall: u64,
    pub req: u32,
    pub perr: u32,
    pub corr: bool,
    pub input: u64,
    pub cache: u64,
}

impl Obs {
    pub fn of(t: &Turn) -> Obs {
        Obs { seq: t.seq, end: t.end, tuple: t.tuple.clone().unwrap_or_default(), cost: t.cost_musd, wall: t.wall_ms, req: t.requests,
            perr: t.provider_errors, corr: t.correction, input: t.input_tok, cache: t.cache_read_tok }
    }
}

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
pub struct Goal {
    pub id: String,
    /// `card:<id>`, `exemplar` or `owner`: the calibration class.
    pub kind: String,
    pub done: Value,
    pub usd_musd: u64,
    pub wall_s: u64,
    pub rounds: u64,
    /// Owner guardrails: (card, tolerance in permille of the baseline mean before a regression counts).
    pub guard: Vec<(String, i64)>,
    /// `active`, `paused` or `stopped`.
    pub state: String,
    pub spent_musd: u64,
    pub active_ms: i64,
    /// When the goal last became active (Root clock).
    pub since: i64,
    pub used_rounds: u64,
    pub since_checkin: u64,
    /// TUI-PICKER: when the agent asked the owner a question (`ask_owner`) that is not answered yet. The wall clock
    /// stops here, and runs again at the next owner line or zone egress request ([`resumed`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_since: Option<i64>,
    /// v0.5.3: the newest lineage seq whose `tuple` records have been charged to this goal's rounds (`spend_live`).
    /// Set when the goal is set or resumed; a goal from before this field starts charging at the next scan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_seq: Option<u64>,
}

impl Goal {
    fn wall_ms(&self, now: i64) -> i64 {
        self.active_ms + if self.state == "active" { self.waiting_since.unwrap_or(now) - self.since } else { 0 }
    }
    /// Banks the active time before a pause or a stop (a wait in progress is not counted).
    fn bank(&mut self, now: i64) {
        self.active_ms = self.wall_ms(now);
        self.waiting_since = None;
    }
    /// TUI-PICKER: the agent asked the owner: an active goal's wall clock stops.
    fn wait(&mut self, now: i64) {
        if self.state == "active" && self.waiting_since.is_none() {
            self.waiting_since = Some(now);
        }
    }
    /// The owner wrote, or the zone made a request: the clock runs again; the wait is not the goal's time.
    fn unwait(&mut self, now: i64) {
        if let Some(w) = self.waiting_since.take() {
            self.since += now.max(w) - w;
        }
    }
    /// The first budget spent; `rounds` too when a new round is asked for (a turn never ends a goal on rounds: the
    /// last round's readings still complete).
    fn budget_hit(&self, now: i64, rounds: bool) -> Option<&'static str> {
        if self.spent_musd >= self.usd_musd {
            Some("usd")
        } else if self.wall_ms(now) >= self.wall_s as i64 * 1000 {
            Some("wall")
        } else if rounds && self.used_rounds >= self.rounds {
            Some("rounds")
        } else {
            None
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
pub struct Pred {
    pub id: String,
    pub goal: String,
    pub sha: String,
    pub card: Option<String>,
    pub expect: String,
    pub p: u64,
    pub seq: u64,
    pub kind: String,
    pub resolved: bool,
    /// Guardrail or gate rollbacks of this change (`guard:<card>`, `gate:<card>`), recorded on the outcome (G4).
    #[serde(default)]
    pub harm: Vec<String>,
    /// The goal round this prediction spends (a re-file that supersedes it keeps the same round).
    #[serde(default)]
    pub round: u64,
    /// G7: closed `superseded` by this later prediction before its change ever went live (no outcome, never registers).
    #[serde(default)]
    pub superseded_by: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
pub struct Measure {
    pub id: String,
    pub card: String,
    pub sha: String,
    pub goal: Option<String>,
    pub n: usize,
    /// The agent's self-imposed gate (`card:<id> gate`), or an owner guardrail: roll back unless the reading passes.
    pub gate: bool,
    pub guard: Option<i64>,
    /// L3 D3: the goal card's reading of a change that went live in a card goal with no forecast: scored at the
    /// Root's abstention forecast (`ABSTAIN_PERMILLE`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unforecast: bool,
}

#[derive(Serialize, Deserialize, Default, Debug)]
struct State {
    goals: Vec<Goal>,
    preds: Vec<Pred>,
    measures: Vec<Measure>,
    obs: Vec<Obs>,
    /// Counters for ids: goal, proposal, prediction, measure.
    next: [u64; 4],
    /// The goal whose check-in was already alerted.
    checkin_alerted: Option<String>,
}

/// What a state change asks the Root to do, run after the state lock is released.
enum Out {
    Append(&'static str, Value),
    Alert(&'static str, String),
    Notice(String),
    /// Roll the change `sha` back to the tuple `to` (H12's path for an open group, else `extend.rollback`).
    Rollback { sha: String, to: String, reason: String },
}

fn guard_lock() -> &'static Mutex<()> {
    static M: Mutex<()> = Mutex::new(());
    &M
}

fn no(c: &'static str, m: impl Into<String>) -> Err3 {
    (c, m.into(), Value::Null)
}

/// Load, change, save `state.json` under one lock; then run what the change asked for, in order.
fn with<T>(root: &Root, f: impl FnOnce(&mut State, &mut Vec<Out>, i64) -> Result<T, Err3>) -> Result<(T, Vec<Value>), Err3> {
    let (t, outs) = {
        let _g = lock(guard_lock());
        let path = root.p.r("state/goals/state.json");
        std::fs::create_dir_all(root.p.r("state/goals")).map_err(|e| no("root.internal", e.to_string()))?;
        let mut st: State = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let mut outs = Vec::new();
        let t = f(&mut st, &mut outs, now_ms())?;
        let body = serde_json::to_vec(&st).map_err(|e| no("root.internal", e.to_string()))?;
        util::write_atomic(&path, &body, 0o600).map_err(|e| no("root.internal", e.to_string()))?;
        (t, outs)
    };
    let mut recs = Vec::new();
    for o in outs {
        match o {
            Out::Append(kind, data) => recs.push(root.append(kind, data).ok_or_else(|| no("root.internal", format!("{kind}: the lineage append failed")))?),
            Out::Alert(code, m) => root.alert(code, &m),
            Out::Notice(m) => root.notice("info", &m),
            Out::Rollback { sha, to, reason } => rollback(root, &sha, &to, &reason),
        }
    }
    Ok((t, recs))
}

fn id(st: &mut State, k: usize, prefix: &str) -> String {
    st.next[k] += 1;
    format!("{prefix}-{:06}", st.next[k])
}

fn active(st: &mut State) -> Option<&mut Goal> {
    st.goals.iter_mut().find(|g| g.state == "active")
}

fn clean(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control() || *c == '\n').take(max).collect()
}

// ------------------------------------------------------------------------------------------------ statistics

/// (n, mean, sample sd), the mean and sd in milli-units of the card (integers only).
pub fn stats(xs: &[i64]) -> (usize, i64, i64) {
    let n = xs.len();
    if n == 0 {
        return (0, 0, 0);
    }
    let sum: i128 = xs.iter().map(|&x| i128::from(x)).sum();
    let mean = sum * 1000 / n as i128;
    if n < 2 {
        return (n, mean as i64, 0);
    }
    let ss: i128 = xs.iter().map(|&x| (i128::from(x) * 1000 - mean).pow(2)).sum();
    (n, mean as i64, isqrt((ss / (n as i128 - 1)) as u128) as i64)
}

fn isqrt(v: u128) -> u128 {
    if v < 2 {
        return v;
    }
    let mut x = (v as f64).sqrt() as u128; // a start value only; corrected exactly below
    while x * x > v {
        x -= 1;
    }
    while (x + 1) * (x + 1) <= v {
        x += 1;
    }
    x
}

/// One card reading: candidate turns `after` against the incumbent's `before`. `z` (Welch's se of the difference, in
/// permille of se, positive is better) describes the size; the verdict comes from the exact permutation test (L3 D1):
/// `better` or `worse` when that direction's one-sided p is at most `NOISE_ALPHA_PPM`, else `noise`;
/// `no_noise_floor` with fewer than 5 baseline turns. `p_better_ppm` and `p_worse_ppm` (rounded up) are recorded, so
/// the verdict and `met`'s attribution can be re-derived from the record; `test` says whether they were counted exactly.
pub fn reading(c: &Card, before: &[i64], after: &[i64]) -> Value {
    let ((nb, mb, sb), (na, ma, sa)) = (stats(before), stats(after));
    let delta = ma - mb;
    let se = if nb > 0 && na > 0 { isqrt((i128::from(sb).pow(2) / nb as i128 + i128::from(sa).pow(2) / na as i128) as u128) as i64 } else { 0 };
    let better = if c.lower_better { -delta } else { delta };
    let z = match se {
        0 => better.signum() * 99_999,
        _ => (i128::from(better) * 1000 / i128::from(se)).clamp(-99_999, 99_999) as i64,
    };
    let p = perm::test(before, after);
    // lower values are better on a lower-is-better card: its `better` tail is P(S* ≤ S)
    let (pb, pw) = if c.lower_better { (p.le_ppm, p.ge_ppm) } else { (p.ge_ppm, p.le_ppm) };
    let verdict = if nb < BASE_MIN {
        "no_noise_floor"
    } else if pb <= NOISE_ALPHA_PPM {
        "better"
    } else if pw <= NOISE_ALPHA_PPM {
        "worse"
    } else {
        "noise"
    };
    json!({"card": c.id, "delta_milli": delta, "mean_after_milli": ma, "mean_before_milli": mb, "n_after": na, "n_before": nb,
        "p_better_ppm": pb, "p_worse_ppm": pw, "sd_milli": sb, "se_milli": se, "test": if p.exact { "permutation" } else { "permutation_mc" },
        "unit": c.unit, "verdict": verdict, "z_permille": z})
}

/// Brier score in ppm of (p in permille, outcome 0|1) pairs: mean (p − o)².
pub fn brier_ppm(pairs: &[(u64, u64)]) -> u64 {
    if pairs.is_empty() {
        return 0;
    }
    pairs.iter().map(|&(p, o)| (p.min(1000) as i64 - o.min(1) as i64 * 1000).pow(2) as u64).sum::<u64>() / pairs.len() as u64
}

/// One goal kind's calibration (L3 D3): the outcomes, the agent's Brier score, and the Brier score of the base-rate
/// reference on the same outcomes (ppm).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cal {
    pub n: u64,
    pub brier_ppm: u64,
    pub ref_ppm: u64,
}

impl Cal {
    /// Brier skill against the reference, 1 − BS/BS_ref (Murphy 1973), positive exactly when BS < BS_ref.
    pub fn skilled(&self) -> bool {
        self.brier_ppm < self.ref_ppm
    }
}

/// Calibration per goal kind, folded from Root `goal_outcome` records only: kind → `Cal`. The reference (L3 D3) is
/// climatology per class (goal kind, `expect`): it forecasts each class's base rate ō_c, the share of that class's
/// outcomes that came true, and scores ō_c(1 − ō_c) per outcome, the uncertainty term of Murphy's (1973) Brier
/// decomposition. A forecaster beats it only when its forecasts resolve outcomes within a class better than they
/// mis-calibrate them; one that files `same` at p = 0.9 on changes that do nothing, which come out `same` 90 % of the
/// time, scores 0.09 and ties it exactly (the research note's gaming case). An outcome from before v0.5.3 has no
/// `expect`; it is taken from its `goal_predict` record, so old lineages fold to the same Brier scores as before. Each
/// class's term is rounded down to the ppm.
pub fn calibration(recs: &[Value]) -> BTreeMap<String, Cal> {
    let expect: BTreeMap<&str, &str> = recs.iter().filter(|r| r["kind"] == "goal_predict" && r["by"] == "root")
        .filter_map(|r| Some((r["data"]["id"].as_str()?, r["data"]["expect"].as_str()?))).collect();
    let mut by: BTreeMap<String, Vec<(u64, u64)>> = BTreeMap::new();
    let mut class: BTreeMap<(String, String), (u64, u64)> = BTreeMap::new();
    for r in recs.iter().filter(|r| r["kind"] == "goal_outcome" && r["by"] == "root") {
        let d = &r["data"];
        let (Some(k), Some(p), Some(o)) = (d["goal_kind"].as_str(), d["p_permille"].as_u64(), d["o"].as_u64()) else { continue };
        let e = d["expect"].as_str().or_else(|| d["prediction"].as_str().and_then(|i| expect.get(i).copied())).unwrap_or("");
        by.entry(k.into()).or_default().push((p, o));
        let c = class.entry((k.into(), e.into())).or_default();
        *c = (c.0 + o.min(1), c.1 + 1);
    }
    by.into_iter().map(|(k, v)| {
        let unc: u128 = class.iter().filter(|((ck, _), _)| *ck == k).map(|(_, &(h, n))| 1_000_000 * u128::from(h) * u128::from(n - h) / u128::from(n)).sum();
        let n = v.len() as u64;
        (k, Cal { n, brier_ppm: brier_ppm(&v), ref_ppm: (unc / u128::from(n)) as u64 })
    }).collect()
}

/// Calibration scales one freedom: how many rounds (predictions) the agent may run before it checks in with the
/// owner. It never raises the owner's budget (the cap) and never reaches the autonomy ledger. L3 D3: more than the
/// default also needs skill against the base-rate reference (`Cal::skilled`), on top of ADR-013's Brier brackets.
pub fn checkin_rounds(cal: Option<Cal>) -> u64 {
    match cal {
        Some(c) if c.n >= 10 && c.brier_ppm <= 80_000 && c.skilled() => 12,
        Some(c) if c.n >= 5 && c.brier_ppm <= 150_000 && c.skilled() => 6,
        Some(c) if c.n >= 5 && c.brier_ppm > 250_000 => 1,
        _ => 3,
    }
}

// ------------------------------------------------------------------------------------------------ the owner's goal

/// Parse `"<intent>" [done=…] [usd=…] [wall=…] [rounds=…] [guard=…]` (the owner's `/goal` line, after `/goal`).
pub fn parse(rest: &str) -> Result<(String, Goal), String> {
    let rest = rest.trim();
    let (intent, opts) = match rest.strip_prefix('"').and_then(|r| r.rsplit_once('"')) {
        Some((i, o)) => (i.trim().to_string(), o),
        None => return Err("usage: /goal \"<intent>\" [done=card:<id><=N|card:<id>>=N|exemplar:<sha256>|owner] [usd=1.00] [wall=24h] [rounds=10] [guard=<card>[:<tol‰>],…]".into()),
    };
    if intent.is_empty() || intent.len() > TEXT_MAX {
        return Err(format!("the intent is 1–{TEXT_MAX} bytes"));
    }
    let mut g = Goal { kind: "owner".into(), done: json!({"kind": "owner"}), usd_musd: 1_000_000, wall_s: 86_400, rounds: 10, state: "active".into(), ..Goal::default() };
    for w in opts.split_whitespace() {
        let (k, v) = w.split_once('=').ok_or_else(|| format!("{w:?}: options are key=value"))?;
        match k {
            "done" if v == "owner" => {}
            "done" if v.starts_with("exemplar:") => {
                let h = &v["exemplar:".len()..];
                if h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err("done=exemplar:<sha256 of the exemplar the owner calls good enough>".into());
                }
                (g.kind, g.done) = ("exemplar".into(), json!({"kind": "exemplar", "sha256": h.to_ascii_lowercase()}));
            }
            "done" if v.starts_with("card:") => {
                let s = &v["card:".len()..];
                let (id, op, n) = ["<=", ">="].iter().find_map(|op| s.split_once(op).map(|(a, b)| (a, *op, b))).ok_or("done=card:<id><=N or >=N")?;
                card(id).ok_or_else(|| format!("no card {id:?} (cards: {})", ids()))?;
                let n: i64 = n.parse().map_err(|_| "the threshold is an integer in the card's unit")?;
                (g.kind, g.done) = (format!("card:{id}"), json!({"card": id, "kind": "card", "op": op, "value": n}));
            }
            "usd" => g.usd_musd = crate::charter::musd(v).filter(|m| *m > 0).ok_or("usd=<decimal dollars>, like 0.50")?,
            "wall" => {
                let (n, u) = v.split_at(v.len().saturating_sub(1));
                let n: u64 = n.parse().map_err(|_| "wall=<n>h or <n>m")?;
                g.wall_s = match u { "h" => n * 3600, "m" => n * 60, _ => return Err("wall=<n>h or <n>m".into()) };
            }
            "rounds" => g.rounds = v.parse().ok().filter(|n| (1..=1000).contains(n)).ok_or("rounds=1..1000")?,
            "guard" => {
                for x in v.split(',') {
                    let (c, tol) = x.split_once(':').map_or((x, Ok(0)), |(c, t)| (c, t.parse::<i64>()));
                    card(c).ok_or_else(|| format!("no card {c:?} (cards: {})", ids()))?;
                    g.guard.push((c.into(), tol.ok().filter(|t| (0..=10_000).contains(t)).ok_or("guard=<card>[:<tolerance permille 0..10000>]")?));
                }
            }
            _ => return Err(format!("unknown option {k:?}")),
        }
    }
    Ok((intent, g))
}

/// A full commit id (SHA-1 40 or SHA-256 64 hex): readings match a change's tuple by its exact `body.sha`.
fn full_sha(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn ids() -> String {
    CARDS.iter().map(|c| c.id).collect::<Vec<_>>().join(", ")
}

fn goal_rec(g: &Goal, op: &str, by: &str) -> Value {
    json!({"budget": {"rounds": g.rounds, "usd_musd": g.usd_musd, "wall_s": g.wall_s}, "by": by, "done": g.done,
        "guard": g.guard.iter().map(|(c, t)| json!({"card": c, "tol_permille": t})).collect::<Vec<_>>(), "id": g.id, "kind": g.kind, "op": op,
        "spent": {"musd": g.spent_musd, "rounds": g.used_rounds, "wall_s": g.wall_ms(now_ms()) / 1000}})
}

/// The owner sets a goal (only an owner line reaches this). An active goal is paused first: one active goal at a time.
pub fn set(root: &Root, intent: &str, mut g: Goal) -> Result<Value, Err3> {
    let top = top_seq(&crate::lineage::records(&root.p.rh).unwrap_or_default());
    let (r, _) = with(root, |st, out, now| {
        if let Some(a) = active(st) {
            a.bank(now);
            a.state = "paused".into();
            let rec = goal_rec(a, "pause", "owner");
            out.push(Out::Append("goal", rec));
        }
        g.id = id(st, 0, "G");
        (g.since, g.from_seq) = (now, Some(top));
        util::write_atomic(&root.p.r(&format!("state/goals/{}.md", g.id)), intent.as_bytes(), 0o600).map_err(|e| no("root.internal", e.to_string()))?;
        let mut rec = goal_rec(&g, "set", "owner");
        rec["intent_bytes"] = json!(intent.len());
        rec["intent_sha256"] = json!(sha256_hex(intent.as_bytes()));
        out.push(Out::Append("goal", rec));
        out.push(Out::Notice(format!("[root] goal {} set ({}; ${}.{:02}, {} s, {} rounds). The agent sees it with initctl goal status.",
            g.id, g.kind, g.usd_musd / 1_000_000, g.usd_musd % 1_000_000 / 10_000, g.wall_s, g.rounds)));
        let v = json!({"id": g.id});
        st.goals.push(g);
        Ok(v)
    })?;
    Ok(r)
}

/// `pause`, `resume`, `stop` (owner_stop), `continue` (the check-in): owner ops on the newest live goal.
fn owner_op(root: &Root, op: &str) -> Result<Value, Err3> {
    let top = top_seq(&crate::lineage::records(&root.p.rh).unwrap_or_default());
    Ok(with(root, |st, out, now| {
        let g = st.goals.iter_mut().rev().find(|g| g.state != "stopped").ok_or_else(|| no("root.not_found", "no goal (set one: /goal \"<intent>\")"))?;
        match op {
            "pause" if g.state == "active" => {
                g.bank(now);
                g.state = "paused".into();
            }
            // changes made while the goal was paused are not its rounds
            "resume" | "continue" if g.state == "paused" => (g.since, g.state, g.from_seq) = (now, "active".into(), Some(top)),
            "continue" => {}
            "stop" => {
                if g.state == "active" {
                    g.bank(now);
                }
                g.state = "stopped".into();
            }
            _ => return Err(no("root.args", format!("goal {} is {}", g.id, g.state))),
        }
        g.since_checkin = 0;
        let mut rec = goal_rec(g, if op == "continue" { "checkin" } else { op }, "owner");
        if op == "stop" {
            rec["reason"] = json!("owner_stop");
        }
        out.push(Out::Append("goal", rec));
        let id = g.id.clone();
        if st.checkin_alerted.as_deref() == Some(id.as_str()) {
            st.checkin_alerted = None;
        }
        Ok(json!({"id": id, "op": op}))
    })?.0)
}

/// The owner's verdict on a prediction (`/goal accept|reject [P-id]`): the outcome of owner-judged goals.
fn owner_outcome(root: &Root, pid: Option<&str>, accept: bool) -> Result<Value, Err3> {
    let recs = crate::lineage::records(&root.p.rh).unwrap_or_default();
    Ok(with(root, |st, out, _| {
        let p = st.preds.iter_mut().rev().find(|p| !p.resolved && pid.is_none_or(|i| p.id == i)).ok_or_else(|| no("root.not_found", "no open prediction"))?;
        p.resolved = true;
        let o = u64::from(accept == (p.expect != "worse"));
        let mut rec = outcome(&recs, &p.forecast(), o, if accept { "owner_accept" } else { "owner_reject" }, Value::Null);
        if !p.harm.is_empty() {
            rec["harm"] = json!(p.harm);
        }
        out.push(Out::Append("goal_outcome", rec));
        Ok(json!({"prediction": p.id, "o": o}))
    })?.0)
}

/// A forecast to score: the agent's prediction, or the Root's abstention forecast for an unforecast change (L3 D3).
struct Forecast<'a> {
    id: Option<&'a str>,
    goal: &'a str,
    kind: &'a str,
    sha: &'a str,
    expect: &'a str,
    p: u64,
}

impl Pred {
    fn forecast(&self) -> Forecast<'_> {
        Forecast { id: Some(&self.id), goal: &self.goal, kind: &self.kind, sha: &self.sha, expect: &self.expect, p: self.p }
    }
}

/// The `goal_outcome` record of forecast `f` with outcome `o`, and the goal kind's calibration with it included (the
/// same fold as `calibration`; `brier_ref_ppm`: the reference's, L3 D3).
fn outcome(recs: &[Value], f: &Forecast, o: u64, source: &str, reading: Value) -> Value {
    let mut sub: Vec<Value> = recs.iter().filter(|r| (r["kind"] == "goal_outcome" || r["kind"] == "goal_predict") && r["by"] == "root").cloned().collect();
    let mut d = json!({"expect": f.expect, "goal": f.goal, "goal_kind": f.kind, "o": o, "p_permille": f.p, "prediction": f.id, "reading": reading,
        "sha": f.sha, "source": source});
    sub.push(json!({"by": "root", "data": d.clone(), "kind": "goal_outcome"}));
    let cal = calibration(&sub).get(f.kind).copied().unwrap_or_default();
    (d["brier_ppm"], d["brier_ref_ppm"], d["n"]) = (json!(cal.brier_ppm), json!(cal.ref_ppm), json!(cal.n));
    d
}

/// The owner's `/goal …` lines (root.md §4.2). False: not ours.
pub fn line(root: &Root, t: &str) -> bool {
    let Some(rest) = t.strip_prefix("/goal").filter(|r| r.is_empty() || r.starts_with(' ')) else { return false };
    let w: Vec<&str> = rest.split_whitespace().collect();
    let r = match w.as_slice() {
        [] => status(root).map(|s| json!({"text": format!("[root] {}", util::icj(&s).unwrap_or_default())})),
        [op @ ("pause" | "resume" | "stop" | "continue")] => owner_op(root, op),
        [op @ ("accept" | "reject"), pid @ ..] => owner_outcome(root, pid.first().copied(), *op == "accept"),
        ["adopt", gp, ..] => adopt(root, gp, rest.trim_start()["adopt".len()..].trim_start()[gp.len()..].trim()),
        _ => parse(rest).map_err(|m| no("root.args", m)).and_then(|(i, g)| set(root, &i, g)),
    };
    match r {
        Ok(v) if v["text"].is_string() => root.notice("info", v["text"].as_str().unwrap_or("")),
        Ok(v) if !w.is_empty() && !matches!(w[0], "accept" | "reject" | "adopt") && !w[0].starts_with('"') => {
            root.notice("info", &format!("[root] goal {}: {}", v["id"].as_str().unwrap_or(""), v["op"].as_str().unwrap_or("")))
        }
        Ok(v) if matches!(w.first(), Some(&("accept" | "reject"))) => root.notice("info", &format!("[root] outcome recorded: {}", util::icj(&v).unwrap_or_default())),
        Ok(_) => {}
        Err((_, m, _)) => root.notice("warn", &format!("[root] {m}")),
    }
    true
}

/// `/goal adopt gp-NNNNNN [options]`: the owner makes an agent proposal the goal (the text is the agent's, the act the owner's).
fn adopt(root: &Root, gp: &str, opts: &str) -> Result<Value, Err3> {
    let ok = gp.len() == 9 && gp.starts_with("gp-") && gp[3..].bytes().all(|b| b.is_ascii_digit());
    let text = ok.then(|| std::fs::read_to_string(root.p.r(&format!("state/goals/{gp}.md"))).ok()).flatten().ok_or_else(|| no("root.not_found", format!("no goal proposal {gp:?}")))?;
    let (_, g) = parse(&format!("\"x\" {opts}")).map_err(|m| no("root.args", m))?;
    set(root, &text, g)
}

// ------------------------------------------------------------------------------------------------ the agent (control link)

/// `goal.*` and `card.*` on the control link (sup.rs `handle`).
pub fn kernel(root: &Root, cmd: &str, a: &Value) -> Result<Value, Err3> {
    match cmd {
        "goal.status" => status(root),
        "goal.propose" => propose(root, a),
        "goal.predict" => predict(root, a),
        "goal.stop" => stop(root, a),
        "card.read" => read(root, a),
        "card.measure" => {
            let (c, sha) = (a["card"].as_str().unwrap_or(""), a["sha"].as_str().unwrap_or(""));
            Ok(with(root, |st, _, _| measure(st, c, sha, a["n"].as_u64().map(|n| n as usize), a["gate"] == true, None))?.0)
        }
        c => Err(no("root.unknown_cmd", format!("command {c:?} is not supported by this Root"))),
    }
}

fn status(root: &Root) -> Result<Value, Err3> {
    let cal = calibration(&crate::lineage::records(&root.p.rh).unwrap_or_default());
    Ok(with(root, |st, _, now| {
        let g = st.goals.iter().rev().find(|g| g.state != "stopped").cloned();
        let goal = g.as_ref().map(|g| {
            let intent = std::fs::read_to_string(root.p.r(&format!("state/goals/{}.md", g.id))).unwrap_or_default();
            let every = checkin_rounds(cal.get(&g.kind).copied());
            json!({"budget": {"rounds": g.rounds, "usd_musd": g.usd_musd, "wall_s": g.wall_s}, "checkin_every": every,
                "rounds_before_checkin": every.saturating_sub(g.since_checkin), "done": g.done, "guard": g.guard, "id": g.id, "intent": intent,
                "kind": g.kind, "spent": {"musd": g.spent_musd, "rounds": g.used_rounds, "wall_s": g.wall_ms(now) / 1000}, "state": g.state,
                "waiting_owner": g.waiting_since.is_some()})
        });
        Ok(json!({"calibration": cal.iter().map(|(k, c)| (k.clone(), json!({"brier_ppm": c.brier_ppm, "n": c.n, "ref_brier_ppm": c.ref_ppm, "skill": c.skilled()})))
                .collect::<serde_json::Map<_, _>>(),
            "cards": CARDS.iter().map(|c| json!({"id": c.id, "lower_better": c.lower_better, "unit": c.unit})).collect::<Vec<_>>(),
            "goal": goal, "pending": st.measures,
            // open predictions of the live goal (G7: a re-file of the newest one, while its change is not live, supersedes it)
            "predictions": g.as_ref().map(|g| st.preds.iter().filter(|p| p.goal == g.id && !p.resolved)
                .map(|p| json!({"card": p.card, "expect": p.expect, "id": p.id, "round": p.round, "sha": p.sha})).collect::<Vec<_>>()),
            "stop_reasons": STOP}))
    })?.0)
}

/// `goal.propose{intent, why?}` → `{id}`: stored and alerted; only `/goal adopt` makes it a goal.
fn propose(root: &Root, a: &Value) -> Result<Value, Err3> {
    let text = clean(a["intent"].as_str().unwrap_or(""), TEXT_MAX);
    if text.trim().is_empty() {
        return Err(no("root.args", "intent is a non-empty string"));
    }
    Ok(with(root, |st, out, _| {
        let gp = id(st, 1, "gp");
        util::write_atomic(&root.p.r(&format!("state/goals/{gp}.md")), text.as_bytes(), 0o600).map_err(|e| no("root.internal", e.to_string()))?;
        out.push(Out::Append("goal_proposal", json!({"bytes": text.len(), "id": gp, "sha256": sha256_hex(text.as_bytes())})));
        let short: String = text.chars().map(|c| if c == '\n' { ' ' } else { c }).take(200).collect();
        out.push(Out::Alert("goal_proposal", format!("[root] the agent proposes goal {gp}: {short}. Type /goal adopt {gp} [done=… usd=… wall=… rounds=… guard=…] to set it.")));
        Ok(json!({"id": gp, "status": "pending"}))
    })?.0)
}

/// `goal.predict{sha, mode, card?, expect, p_permille, note?}` → `{id, seq}`, filed before the change goes live.
fn predict(root: &Root, a: &Value) -> Result<Value, Err3> {
    let sha = a["sha"].as_str().filter(|s| full_sha(s)).ok_or_else(|| no("root.args", "sha is the full id (40 hex) of the commit the change will submit: an abbreviated one never matches the change's tuple"))?;
    let mode = a["mode"].as_str().filter(|m| ["A", "B", "hybrid"].contains(m)).ok_or_else(|| no("root.args", "mode: A|B|hybrid"))?;
    let expect = a["expect"].as_str().filter(|m| ["better", "worse", "same"].contains(m)).ok_or_else(|| no("root.args", "expect: better|worse|same"))?;
    let p = a["p_permille"].as_u64().filter(|p| *p <= 1000).ok_or_else(|| no("root.args", "p_permille: 0..1000, the confidence that the outcome is as expected"))?;
    let c = a["card"].as_str().map(|c| card(c).map(|c| c.id).ok_or_else(|| no("root.args", format!("no card {c:?} (cards: {})", ids())))).transpose()?;
    if mode == "A" && c.is_none() {
        return Err(no("root.args", "mode A needs a Root card (card: …); without one the goal is mode B"));
    }
    let lin = crate::lineage::records(&root.p.rh).unwrap_or_default();
    // GBAL real-model proof (G3): a prediction filed after the change went live can never be registered; it used to be
    // accepted silently and spend one of the owner's rounds. Refused, with the reason, before anything is recorded.
    if let Some((seq, _, tid)) = anchor(&lin, sha) {
        return Err(no("goal.late", format!("change {} is already live (tuple {tid}, lineage seq {seq}): a prediction must precede it, so its readings stay unregistered. Predict the final commit (after any amend) before `initctl promote`", &sha[..12])));
    }
    let cal = calibration(&lin);
    // the rounds of changes that went live since the last owner turn, saved even when this prediction is refused
    with(root, |st, _, _| {
        spend_live(st, &lin);
        Ok(())
    })?;
    let (v, recs) = with(root, |st, out, now| {
        let gid = active(st).map(|g| g.id.clone()).ok_or_else(|| no("goal.none", "no active goal: the owner sets one (/goal), the agent may propose (goal.propose)"))?;
        // GBAL rerun (G7): an amend after the prediction makes it stale, and the agent re-files on the final sha. A
        // re-file that replaces this goal's newest prediction while that one's change never went live is the same
        // round: it spends no round and no check-in step, and the old prediction closes `superseded` (no outcome, its
        // pending readings dropped, never registered even if that commit goes live later).
        let old = st.preds.iter().rposition(|p| p.goal == gid).filter(|&i| !st.preds[i].resolved && anchor(&lin, &st.preds[i].sha).is_none());
        let g = active(st).expect("checked above");
        if let Some(hit) = g.budget_hit(now, old.is_none()) {
            return Err(no("goal.budget", format!("goal {}: its {hit} budget is spent", g.id)));
        }
        let every = checkin_rounds(cal.get(&g.kind).copied());
        if old.is_none() && g.since_checkin >= every {
            if st.checkin_alerted.as_deref() != Some(gid.as_str()) {
                st.checkin_alerted = Some(gid.clone());
                out.push(Out::Alert("goal_checkin", format!("[root] goal {gid}: {every} rounds since the last check-in. Read its ledger, then /goal continue (or /goal stop).")));
            }
            return Err(no("goal.checkin", format!("goal {gid}: {every} rounds since the owner's last check-in (calibration sets the interval); wait for /goal continue")));
        }
        if old.is_none() {
            (g.used_rounds, g.since_checkin) = (g.used_rounds + 1, g.since_checkin + 1);
        }
        let (kind, guard) = (g.kind.clone(), g.guard.clone());
        let pid = id(st, 2, "P");
        let (round, supersedes) = match old {
            Some(i) => {
                let o = &mut st.preds[i];
                (o.resolved, o.superseded_by) = (true, Some(pid.clone()));
                let (osha, oid, round) = (o.sha.clone(), o.id.clone(), o.round);
                st.measures.retain(|m| !(m.sha == osha && m.goal.as_deref() == Some(gid.as_str())));
                (round, Some(oid))
            }
            None => (active(st).map_or(0, |g| g.used_rounds), None),
        };
        let note = clean(a["note"].as_str().unwrap_or(""), TEXT_MAX);
        out.push(Out::Append("goal_predict", json!({"card": c, "expect": expect, "goal": gid, "goal_kind": kind, "id": pid, "mode": mode,
            "note_sha256": sha256_hex(note.as_bytes()), "p_permille": p, "round": round, "sha": sha, "supersedes": supersedes})));
        st.preds.push(Pred { id: pid.clone(), goal: gid.clone(), sha: sha.into(), card: c.map(String::from), expect: expect.into(), p, seq: 0, kind, resolved: false,
            harm: vec![], round, superseded_by: None });
        if let Some(c) = c {
            measure(st, c, sha, None, false, None)?;
        }
        for (gc, tol) in guard {
            measure(st, &gc, sha, None, true, Some(tol))?;
        }
        // v0.5.3: the forecast's own reading waits for the change's guardrails (15 turns on a discrete card), so a
        // guardrail's rollback is on the outcome as `harm` (G4); a change that stops getting turns sooner is read on
        // the turns it had (`complete`)
        if let Some(c) = c
            && let Some(g) = st.measures.iter().filter(|m| m.sha == sha && m.guard.is_some()).map(|m| m.n).max()
            && let Some(m) = st.measures.iter_mut().find(|m| m.sha == sha && m.card == c)
        {
            m.n = m.n.max(g);
        }
        st.measures.iter_mut().filter(|m| m.sha == sha && m.goal.is_none()).for_each(|m| m.goal = Some(gid.clone()));
        Ok(json!({"goal": gid, "id": pid, "round": round, "supersedes": supersedes}))
    })?;
    let seq = recs.first().and_then(|r| r["seq"].as_u64()).unwrap_or(0);
    with(root, |st, _, _| {
        if let Some(p) = st.preds.iter_mut().find(|p| v["id"] == p.id.as_str()) {
            p.seq = seq;
        }
        Ok(())
    })?;
    let mut v = v;
    (v["seq"], v["at"]) = (json!(seq), recs.first().map_or(Value::Null, |r| r["at"].clone()));
    Ok(v)
}

/// A pending reading of card `c` on change `sha`; `gate`: roll back unless it passes; `guard`: an owner guardrail,
/// which reads the card's `guard_n` turns by default (a guardrail that joins a pending reading of the same card raises
/// its turns to that).
fn measure(st: &mut State, c: &str, sha: &str, n: Option<usize>, gate: bool, guard: Option<i64>) -> Result<Value, Err3> {
    let c = card(c).ok_or_else(|| no("root.args", format!("no card {c:?} (cards: {})", ids())))?;
    if !full_sha(sha) {
        return Err(no("root.args", "sha is a full commit id (40 hex): an abbreviated one never matches the change's tuple"));
    }
    let default = if guard.is_some() { c.guard_n } else { BASE_MIN };
    if let Some(m) = st.measures.iter_mut().find(|m| m.card == c.id && m.sha == sha) {
        m.gate |= gate;
        if guard.is_some() && n.is_none() {
            m.n = m.n.max(default);
        }
        m.guard = m.guard.or(guard);
        return Ok(json!({"id": m.id, "status": "pending"}));
    }
    let n = n.unwrap_or(default).clamp(AFTER_MIN, BASE_MAX);
    let mid = id(st, 3, "M");
    let goal = st.goals.iter().find(|g| g.state == "active").map(|g| g.id.clone());
    st.measures.push(Measure { id: mid.clone(), card: c.id.into(), sha: sha.into(), goal, n, gate, guard, unforecast: false });
    Ok(json!({"id": mid, "n_after": n, "status": "pending"}))
}

/// `goal.stop{reason, note?}`: the agent ends the active goal with one of the six reasons. For a card goal, `met` is
/// checked by the Root (`verified`): the newest kept reading of the goal card reaches the threshold *and* a registered
/// change moved that card beyond noise in its prediction's direction (`attributed`). Reaching the threshold without such
/// a change is recorded as `met_unattributed` (owner alert). Other kinds stay for the owner (`verified: null`).
fn stop(root: &Root, a: &Value) -> Result<Value, Err3> {
    let reason = a["reason"].as_str().filter(|r| STOP.contains(r)).ok_or_else(|| no("root.args", format!("reason: {}", STOP.join("|"))))?;
    let recs = crate::lineage::records(&root.p.rh).unwrap_or_default();
    Ok(with(root, |st, out, now| {
        let preds = st.preds.clone();
        let g = st.goals.iter_mut().rev().find(|g| g.state != "stopped").ok_or_else(|| no("goal.none", "no goal to stop"))?;
        let (verified, attributed) = match (reason, g.done["kind"].as_str()) {
            ("met", Some("card")) => met_check(g, &recs, &preds),
            _ => (Value::Null, None),
        };
        // `met_unattributed`: the threshold is reached, but no registered change moved the card beyond noise as predicted
        let reason = if matches!(attributed, Some(Value::Null)) { "met_unattributed" } else { reason };
        if g.state == "active" {
            g.bank(now);
        }
        g.state = "stopped".into();
        let mut rec = goal_rec(g, "stop", "agent");
        (rec["reason"], rec["verified"], rec["note_sha256"]) = (json!(reason), verified.clone(), json!(sha256_hex(clean(a["note"].as_str().unwrap_or(""), TEXT_MAX).as_bytes())));
        if let Some(by) = &attributed {
            (rec["claimed"], rec["attributed"]) = (json!("met"), by.clone());
        }
        out.push(Out::Append("goal", rec));
        if reason == "met_unattributed" {
            out.push(Out::Alert("goal_met_unattributed", format!("[root] the agent stopped goal {} as met: the card reached its threshold, but no registered change moved it beyond noise in the predicted direction. Recorded as met_unattributed.", g.id)));
        } else {
            out.push(Out::Notice(format!("[root] the agent stopped goal {}: {reason}{}", g.id, if verified == false { " (not verified by the goal's card)" } else { "" })));
        }
        Ok(json!({"attributed": attributed, "id": g.id, "reason": reason, "verified": verified}))
    })?.0)
}

/// `met` on a card goal (`stop`): (`verified`, `attributed`). Of each change only its first completed reading of the
/// goal card counts (v0.5.3: a re-measure of the same change is not a new round). The newest kept reading of the goal
/// card (in this goal, not cancelled, its change not rolled back) reaches the threshold, *and* a registered change moved that card beyond
/// noise in the direction its prediction named, toward the threshold, at the goal's spent level (L3 D2): its p in that
/// direction is at most `MET_ALPHA_PPM` / rounds, so a goal whose changes all do nothing ends `met` at most 5 % of the
/// time, however the agent picks its moment to stop. A reading recorded before v0.5.3 has no p and attributes nothing.
/// `attributed` is None when the threshold is not reached, `Some(Null)` when it is reached with no such change
/// (`met_unattributed`). Pure: the null simulations (goal/sim.rs) call it as `stop` does.
fn met_check(g: &Goal, recs: &[Value], preds: &[Pred]) -> (Value, Option<Value>) {
    // the goal card's readings in this goal on changes that were not rolled back (by a guardrail or gate, or by the
    // sentinel: v0.5.3), oldest first; V9 (v0.5.3): only the first completed one of each change, since a later
    // `card.measure` of the same live change is a second look at it, which the α spent per round does not cover
    let gone = undone(recs);
    let mut seen = std::collections::BTreeSet::new();
    let first: Vec<&Value> = recs.iter().filter(|r| r["kind"] == "card_reading" && r["data"]["card"] == g.done["card"] && r["data"]["goal"] == g.id.as_str()
        && !matches!(r["data"]["action"].as_str(), Some("cancelled" | "closed"))).filter(|r| seen.insert(r["data"]["sha"].to_string())).collect();
    let kept: Vec<&Value> = first.into_iter().filter(|r| r["data"]["sha"].as_str().is_none_or(|s| !gone.contains(s))
        && !recs.iter().any(|x| x["kind"] == "card_reading" && x["data"]["sha"] == r["data"]["sha"] && x["data"]["action"] == "rollback"))
        .map(|r| &r["data"]).collect();
    let (v, le) = (g.done["value"].as_i64().unwrap_or(0) * 1000, g.done["op"] == "<=");
    // the threshold, on the newest kept reading
    let reached = kept.last().and_then(|r| r["mean_after_milli"].as_i64()).is_some_and(|m| if le { m <= v } else { m >= v });
    // attribution: a registered change moved the card beyond noise, in the direction its prediction named, toward the
    // threshold (reaching it on noise, or on an unpredicted change, is not the agent's doing)
    let alpha_ppm = MET_ALPHA_PPM / g.rounds.max(1);
    let p_of = |r: &Value| r[if r["verdict"] == "better" { "p_better_ppm" } else { "p_worse_ppm" }].as_u64();
    let by = kept.iter().rev().find(|r| {
        let d = r["delta_milli"].as_i64().unwrap_or(0);
        r["registered"] == true && (r["verdict"] == "better" || r["verdict"] == "worse") && (if le { d < 0 } else { d > 0 })
            && preds.iter().any(|p| r["prediction"] == p.id.as_str() && r["verdict"] == p.expect.as_str())
            && p_of(r).is_some_and(|p| u128::from(p) * u128::from(g.rounds.max(1)) <= u128::from(MET_ALPHA_PPM))
    }).map(|r| json!({"alpha_ppm": alpha_ppm, "p_ppm": p_of(r), "prediction": r["prediction"], "sha": r["sha"], "verdict": r["verdict"], "z_permille": r["z_permille"]}));
    (json!(reached && by.is_some()), if reached { by.or(Some(Value::Null)) } else { None })
}

/// Whether a completed reading fails its measure: the agent's own gate (`card:<id> gate`) unless the reading is `better`;
/// an owner guardrail on a `worse` reading beyond the owner's tolerance (‰ of the baseline mean). Pure (`complete`, and
/// the null simulations in goal/sim.rs).
fn fails(c: &Card, m: &Measure, r: &Value) -> bool {
    let tol_ok = |tol: i64| {
        let (d, mb) = (r["delta_milli"].as_i64().unwrap_or(0), r["mean_before_milli"].as_i64().unwrap_or(0).abs());
        (if c.lower_better { d } else { -d }) * 1000 <= mb * tol
    };
    (m.gate && m.guard.is_none() && r["verdict"] != "better") || m.guard.is_some_and(|tol| r["verdict"] == "worse" && !tol_ok(tol))
}

/// `card.read{card}`: the noise floor of a card on the current tuple's newest owner turns (n, mean, sd).
fn read(root: &Root, a: &Value) -> Result<Value, Err3> {
    let c = card(a["card"].as_str().unwrap_or("")).ok_or_else(|| no("root.args", format!("card: one of {}", ids())))?;
    let cur = lock(&root.sup.ptrs).current.clone().unwrap_or_default();
    Ok(with(root, |st, _, _| {
        let xs: Vec<i64> = st.obs.iter().rev().filter(|o| o.tuple == cur).filter_map(c.value).take(BASE_MAX).collect();
        let (n, mean, sd) = stats(&xs);
        Ok(json!({"card": c.id, "mean_milli": mean, "n": n, "noise_floor": n >= BASE_MIN, "sd_milli": sd, "tuple": cur, "unit": c.unit}))
    })?.0)
}

// ------------------------------------------------------------------------------------------------ hooks

/// Registers the closed-turn hook with the sentinel (main.rs).
pub fn register() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| lock(crate::sentinel::hooks()).turn_closed.push(turn_closed));
}

/// `change.open` accepted (sentinel.rs): the commit's `Measure: card:<id>[ gate]` requests become pending readings.
pub fn opened(root: &Root, a: &Value) {
    let (sha, ms) = (a["sha"].as_str().unwrap_or(""), a["measure"].as_array().cloned().unwrap_or_default());
    if ms.is_empty() {
        return;
    }
    let r = with(root, |st, _, _| {
        for m in ms.iter().filter_map(Value::as_str) {
            let (c, gate) = m.strip_suffix(" gate").map_or((m, false), |c| (c, true));
            if let Some(c) = c.strip_prefix("card:") {
                measure(st, c, sha, None, gate, None)?;
            }
        }
        Ok(())
    });
    if let Err((_, m, _)) = r {
        util::log(&root.p.rh, "warn", &format!("goal: change {sha}: {m}"));
    }
}

/// The active goal's budget check (money or wall time): the Root stops the goal itself.
fn budget_stop(st: &mut State, out: &mut Vec<Out>, now: i64) {
    if let Some(g) = active(st)
        && let Some(hit) = g.budget_hit(now, false)
    {
        g.bank(now);
        g.state = "stopped".into();
        let mut rec = goal_rec(g, "stop", "root");
        (rec["reason"], rec["budget"]["hit"]) = (json!("budget"), json!(hit));
        out.push(Out::Append("goal", rec));
        out.push(Out::Alert("goal_budget", format!("[root] goal {} stopped: its {hit} budget is spent", g.id)));
    }
}

/// Reads `state.json` (no write) and asks `f` of its goals: the cheap check before a change.
fn any_goal(root: &Root, f: impl Fn(&Goal) -> bool) -> bool {
    let _g = lock(guard_lock());
    std::fs::read_to_string(root.p.r("state/goals/state.json")).ok().and_then(|s| serde_json::from_str::<State>(&s).ok())
        .is_some_and(|st| st.goals.iter().any(f))
}

/// TUI-PICKER: the Root relayed an agent question (a kernel `display` frame with `ask`, sent by the body's `ask_owner`).
/// The active goal's wall clock stops until [`resumed`]: the goal waits on the owner and must not spend its time budget
/// meanwhile. Money, tokens and rounds are untouched.
pub fn asked(root: &Root) {
    if !any_goal(root, |g| g.state == "active" && g.waiting_since.is_none()) {
        return;
    }
    if let Err((_, m, _)) = with(root, |st, _, now| {
        if let Some(g) = active(st) {
            g.wait(now);
        }
        Ok(())
    }) {
        util::log(&root.p.rh, "error", &format!("goal: asked: {m}"));
    }
}

/// TUI-PICKER: an owner line arrived, or the egress admitted a zone request: a waiting goal's clock runs again. The
/// request case makes the pause unforgeable: a question that is not really waiting (the agent keeps calling models)
/// buys no wall time.
pub fn resumed(root: &Root) {
    if !any_goal(root, |g| g.waiting_since.is_some()) {
        return;
    }
    if let Err((_, m, _)) = with(root, |st, _, now| {
        st.goals.iter_mut().for_each(|g| g.unwait(now));
        Ok(())
    }) {
        util::log(&root.p.rh, "error", &format!("goal: resumed: {m}"));
    }
}

/// The per-goal money meter: every zone egress request the Root settles while a goal is active is charged to that goal,
/// at settlement (egress.rs `finish_with`, the same `cost_musd` the egress ledger's `end` record carries). It used to
/// add each closed turn's cost, which missed requests that settled after their turn closed and the turn in which the
/// agent stopped the goal (GBAL rerun: meter 51,290 vs ledger 59,718 µUSD). With no active goal nothing is written.
pub fn egress_spent(root: &Root, cost_musd: u64) {
    if cost_musd == 0 {
        return;
    }
    let busy = {
        let _g = lock(guard_lock());
        std::fs::read_to_string(root.p.r("state/goals/state.json")).ok().and_then(|s| serde_json::from_str::<State>(&s).ok())
            .is_some_and(|st| st.goals.iter().any(|g| g.state == "active"))
    };
    if !busy {
        return;
    }
    let r = with(root, |st, out, now| {
        if let Some(g) = active(st) {
            g.unwait(now); // a model request ran: the agent is not waiting (also resumed at admission)
            g.spent_musd += cost_musd;
        }
        budget_stop(st, out, now);
        Ok(())
    });
    if let Err((_, m, _)) = r {
        util::log(&root.p.rh, "error", &format!("goal: meter: {m}"));
    }
}

/// Every closed turn: the active goal's wall budget; owner turns feed the cards, register guardrail readings for a
/// tuple seen for the first time, and complete the readings that now have enough turns. (Money is metered per request
/// at settlement: `egress_spent`.)
pub fn turn_closed(root: &Root, t: &Turn) {
    let recs = if t.kind == "owner" { crate::lineage::records(&root.p.rh).unwrap_or_default() } else { Vec::new() };
    let r = with(root, |st, out, now| {
        budget_stop(st, out, now);
        if t.kind != "owner" {
            return Ok(());
        }
        spend_live(st, &recs);
        let o = Obs::of(t);
        let new_tuple = !o.tuple.is_empty() && !st.obs.iter().any(|x| x.tuple == o.tuple);
        if new_tuple && st.obs.last().is_some() && let Some(sha) = crate::tuple::load(&root.p.rh, &o.tuple).and_then(|v| v["body"]["sha"].as_str().map(String::from)) {
            let guard = st.goals.iter().find(|g| g.state == "active").map(|g| g.guard.clone()).unwrap_or_default();
            for (c, tol) in guard {
                measure(st, &c, &sha, None, true, Some(tol))?;
            }
        }
        // L3 D3: the change's commit from its `tuple` record, as `anchor` reads it
        if new_tuple && st.obs.last().is_some()
            && let Some(sha) = recs.iter().rev().find(|r| r["kind"] == "tuple" && r["data"]["id"] == o.tuple.as_str()).and_then(|r| r["data"]["tuple"]["body"]["sha"].as_str())
        {
            unforecast(st, &recs, sha, &o.tuple)?;
        }
        st.obs.push(o);
        let over = st.obs.len().saturating_sub(OBS_MAX);
        st.obs.drain(..over);
        complete(st, &recs, out);
        Ok(())
    });
    if let Err((_, m, _)) = r {
        util::log(&root.p.rh, "error", &format!("goal: turn {}: {m}", t.seq));
    }
}

/// L3 D3: every live change in a card goal is forecast. A change that goes live (its first tuple, `tid`) in the active
/// card goal with no prediction of that goal gets the goal card's reading, scored at the Root's abstention forecast
/// (`complete`). Without it the agent chose which changes its Brier score covered: proper scores stay proper only
/// over every question (the research note on GBAL's scoring). Owner and exemplar goals have no Root-observable outcome
/// for an unforecast change; their outcomes stay the owner's.
fn unforecast(st: &mut State, recs: &[Value], sha: &str, tid: &str) -> Result<(), Err3> {
    let Some((gid, gc)) = st.goals.iter().find(|g| g.state == "active" && g.done["kind"] == "card").map(|g| (g.id.clone(), g.done["card"].as_str().unwrap_or("").to_string())) else {
        return Ok(());
    };
    let first = anchor(recs, sha).is_some_and(|(_, _, t)| t == tid);
    if !first || st.preds.iter().any(|p| p.goal == gid && p.sha == sha && p.superseded_by.is_none()) {
        return Ok(());
    }
    measure(st, &gc, sha, None, false, None)?;
    if let Some(m) = st.measures.iter_mut().find(|m| m.card == gc && m.sha == sha) {
        (m.unforecast, m.goal) = (true, Some(gid));
    }
    Ok(())
}

/// The newest seq in the records (0 for none).
fn top_seq(recs: &[Value]) -> u64 {
    recs.iter().filter_map(|r| r["seq"].as_u64()).max().unwrap_or(0)
}

/// v0.5.3 (the budget gap): every change that goes live while a goal is active spends one of its rounds and one
/// check-in step, predicted or not. A prediction spends its round when it is filed (`predict`); a change with none
/// used to spend nothing, so the owner's rounds and check-ins bounded only the changes the agent chose to forecast.
/// Here each `tuple` record after the goal's `from_seq` that is the first tuple of its body commit (not a rollback, a
/// re-admission or an owner upgrade, which reuse a commit), and that no prediction of this goal names, spends a round;
/// D3 scores it at the abstention forecast. A kernel change keeps the body commit and cannot be predicted, so it spends
/// nothing. Called before a prediction is checked against the budget, and at every owner turn.
fn spend_live(st: &mut State, recs: &[Value]) {
    let (goals, preds) = (&mut st.goals, &st.preds);
    let Some(g) = goals.iter_mut().find(|g| g.state == "active") else { return };
    let top = top_seq(recs);
    let Some(from) = g.from_seq else {
        g.from_seq = Some(top); // a goal from before v0.5.3: charge from now on, never its past
        return;
    };
    for r in recs.iter().filter(|r| r["kind"] == "tuple" && r["seq"].as_u64().is_some_and(|q| q > from)) {
        let Some(sha) = r["data"]["tuple"]["body"]["sha"].as_str() else { continue };
        let first = anchor(recs, sha).is_some_and(|(q, _, _)| r["seq"].as_u64() == Some(q));
        if first && !preds.iter().any(|p| p.goal == g.id && p.sha == sha && p.superseded_by.is_none()) {
            (g.used_rounds, g.since_checkin) = (g.used_rounds + 1, g.since_checkin + 1);
        }
    }
    g.from_seq = Some(from.max(top));
}

/// v0.5.3: the commits whose change the sentinel undid and did not re-admit: a change whose newest `change_open` is in
/// a group the sentinel closed `rolled_back` or `collateral`, or whose tuple a whole-system `rollback` left after that
/// `change_open` (gated: there is no group). Their pending readings end (`complete`), and none of their readings
/// counts for `met` (`met_check`), as a guardrail's own rollback never did.
fn undone(recs: &[Value]) -> std::collections::BTreeSet<String> {
    let root = |r: &&Value| r["by"] == "root";
    let groups: std::collections::BTreeSet<u64> = recs.iter().filter(root).filter(|r| r["kind"] == "change_close"
        && matches!(r["data"]["outcome"].as_str(), Some("rolled_back" | "collateral"))).filter_map(|r| r["data"]["group"].as_u64()).collect();
    // sha → (seq, group) of its newest change_open; tuple id → sha
    let mut open: BTreeMap<&str, (u64, Option<u64>)> = BTreeMap::new();
    let mut sha_of: BTreeMap<&str, &str> = BTreeMap::new();
    for r in recs.iter().filter(root) {
        let (d, seq) = (&r["data"], r["seq"].as_u64().unwrap_or(0));
        match r["kind"].as_str() {
            Some("change_open") => {
                if let Some(sha) = d["sha"].as_str() {
                    open.insert(sha, (seq, d["group"].as_u64()));
                }
            }
            Some("tuple") => {
                if let (Some(t), Some(sha)) = (d["id"].as_str(), d["tuple"]["body"]["sha"].as_str()) {
                    sha_of.entry(t).or_insert(sha);
                }
            }
            _ => {}
        }
    }
    let mut out: std::collections::BTreeSet<String> = open.iter().filter(|(_, (_, g))| g.is_some_and(|g| groups.contains(&g))).map(|(s, _)| s.to_string()).collect();
    for r in recs.iter().filter(root).filter(|r| r["kind"] == "rollback") {
        let seq = r["seq"].as_u64().unwrap_or(0);
        if let Some(sha) = r["data"]["from"].as_str().and_then(|t| sha_of.get(t)) && open.get(sha).is_none_or(|(q, _)| *q < seq) {
            out.insert(sha.to_string());
        }
    }
    out
}

/// The verdict that moves card `c` toward goal `g`'s threshold (`<=`: lower; `>=`: higher).
fn toward(g: &Goal, c: &Card) -> &'static str {
    if (g.done["op"] == "<=") == c.lower_better { "better" } else { "worse" }
}

/// The newest owner upgrade's commit (`lkg{upgrade}`, upgrade.rs; root.md §3.7.2): (seq, time, upgrade id). No reading
/// spans it: its turns before and after ran on two kernels, and its rollback would reinstall the old one.
fn upgraded(recs: &[Value]) -> Option<(u64, i64, Value)> {
    recs.iter().rev().find(|r| r["kind"] == "lkg" && r["data"]["upgrade"].is_string())
        .map(|r| (r["seq"].as_u64().unwrap_or(0), r["at"].as_str().and_then(parse_rfc3339).unwrap_or(0), r["data"]["upgrade"].clone()))
}

/// Readings of changes that went live before the newest owner upgrade end, recorded `card_reading{action:"cancelled",
/// verdict:"cancelled", upgrade}`: whatever registered them (a commit's `Measure:`, `card.measure`, a guardrail on the
/// upgraded tuple, which keeps the old body), they would count turns of the old kernel only, or roll back to it.
fn cancel_upgraded(st: &mut State, recs: &[Value], out: &mut Vec<Out>) {
    let Some((useq, _, uid)) = upgraded(recs) else { return };
    st.measures.retain(|m| match anchor(recs, &m.sha) {
        Some((seq, _, tid)) if seq < useq => {
            out.push(Out::Append("card_reading", json!({"action": "cancelled", "card": m.card, "gate": m.gate && m.guard.is_none(), "goal": m.goal,
                "guard": m.guard, "measure": m.id, "sha": m.sha, "tuple": tid, "upgrade": uid, "verdict": "cancelled"})));
            false
        }
        _ => true,
    });
}

/// `upgrade --commit` (upgrade.rs), after its `lkg{upgrade}`: the readings in flight on changes from before it end now
/// (`cancel_upgraded`). Returns this upgrade's cancelled readings, from the records, so a re-run reports them too.
pub fn upgrade_committed(root: &Root, upgrade: &Value) -> Result<Vec<Value>, String> {
    let recs = crate::lineage::records(&root.p.rh).map_err(|e| e.to_string())?;
    with(root, |st, out, _| {
        cancel_upgraded(st, &recs, out);
        Ok(())
    }).map_err(|(_, m, _)| m)?;
    Ok(crate::lineage::records(&root.p.rh).map_err(|e| e.to_string())?.into_iter()
        .filter(|r| r["kind"] == "card_reading" && r["data"]["action"] == "cancelled" && r["data"]["upgrade"] == *upgrade).map(|r| r["data"].clone()).collect())
}

/// The change's anchor: the first `tuple` record whose body is `sha` (seq, time, tuple id).
fn anchor(recs: &[Value], sha: &str) -> Option<(u64, i64, String)> {
    recs.iter().find(|r| r["kind"] == "tuple" && r["data"]["tuple"]["body"]["sha"] == sha)
        .map(|r| (r["seq"].as_u64().unwrap_or(0), r["at"].as_str().and_then(parse_rfc3339).unwrap_or(0), r["data"]["id"].as_str().unwrap_or("").to_string()))
}

/// Readings whose change has `n` owner turns: `card_reading`, the prediction's `goal_outcome`, and the rollback of a
/// gate or guardrail that the reading fails.
///
/// GBAL real-model proof (G4): the outcome scores what the agent forecast, the prediction's own card: o = 1 when that
/// card moved in the `expect` direction beyond noise (`better`/`worse` at |z| ≥ 2, `same` within noise), whatever a
/// guardrail did. A guardrail or gate rollback of the change is recorded on the outcome (`harm`) but is not by itself a
/// miss: a regression the agent predicted, caught by the owner's guardrail, is a hit. A reading with no noise floor
/// scores nothing (no outcome). A change rolled back before its card reading has `n` turns is read on the turns it had
/// (at least 3), since no more turns will come on it.
fn complete(st: &mut State, recs: &[Value], out: &mut Vec<Out>) {
    cancel_upgraded(st, recs, out);
    // the baseline (and so the rollback target) never reaches back before an owner upgrade
    let since = upgraded(recs).map_or(i64::MIN, |u| u.1);
    let mut done = Vec::new();
    // guardrails and gates first: their harm is on the prediction before the goal card's own reading resolves it
    let mut ms = st.measures.clone();
    ms.sort_by_key(|m| m.guard.is_none() && !m.gate);
    let mut rolled: Vec<String> = recs.iter().filter(|r| r["kind"] == "card_reading" && r["data"]["action"] == "rollback")
        .filter_map(|r| r["data"]["sha"].as_str().map(String::from)).collect();
    let gone = undone(recs);
    // tuple id → the seq of its first `tuple` record (which change came later)
    let mut newer: BTreeMap<&str, u64> = BTreeMap::new();
    for r in recs.iter().filter(|r| r["kind"] == "tuple") {
        if let (Some(t), Some(q)) = (r["data"]["id"].as_str(), r["seq"].as_u64()) {
            newer.entry(t).or_insert(q);
        }
    }
    for m in &ms {
        let Some((seq, at, tid)) = anchor(recs, &m.sha) else { continue };
        let c = card(&m.card).expect("checked at registration");
        let after: Vec<i64> = st.obs.iter().filter(|o| o.tuple == tid && o.end >= at).filter_map(c.value).take(m.n).collect();
        // v0.5.3: a newer change went live and served an owner turn: this change's tuple gets no more turns either
        let replaced = st.obs.iter().any(|o| o.end > at && newer.get(o.tuple.as_str()).is_some_and(|&q| q > seq));
        let ended = rolled.contains(&m.sha) || gone.contains(&m.sha) || replaced;
        if after.len() < m.n && ended && after.len() < AFTER_MIN {
            // v0.5.3: the change was rolled back (or replaced by a newer change) before it had the turns a reading needs;
            // no more will come. The reading ends with no verdict (`closed`, `rolled_back` or `replaced`), and its
            // prediction resolves with no outcome
            let verdict = if rolled.contains(&m.sha) || gone.contains(&m.sha) { "rolled_back" } else { "replaced" };
            out.push(Out::Append("card_reading", json!({"action": "closed", "card": m.card, "gate": m.gate && m.guard.is_none(), "goal": m.goal,
                "guard": m.guard, "measure": m.id, "n_after": after.len(), "sha": m.sha, "tuple": tid, "verdict": verdict})));
            if m.guard.is_some() {
                out.push(Out::Alert("goal_guard_unchecked", unchecked(m, &format!("its reading closed with no verdict ({verdict} after {} owner \
                    turns; a reading needs {AFTER_MIN})", after.len()))));
            }
            if let Some(p) = st.preds.iter_mut().find(|p| p.sha == m.sha && p.superseded_by.is_none() && p.card.as_deref() == Some(m.card.as_str())
                && m.goal.as_deref().is_none_or(|g| g == p.goal))
            {
                p.resolved = true;
            }
            done.push(m.id.clone());
            continue;
        }
        if after.len() < m.n && !(ended && after.len() >= AFTER_MIN) {
            continue;
        }
        let prior: Vec<&Obs> = st.obs.iter().filter(|o| o.end <= at && o.end >= since && o.tuple != tid).collect();
        let parent = prior.last().map(|o| o.tuple.clone()).unwrap_or_default();
        let before: Vec<i64> = prior.iter().rev().filter(|o| o.tuple == parent).filter_map(|o| (c.value)(o)).take(BASE_MAX).collect::<Vec<_>>().into_iter().rev().collect();
        let mut r = reading(c, &before, &after);
        let pred = st.preds.iter_mut().find(|p| p.sha == m.sha && p.seq > 0 && p.seq < seq && p.superseded_by.is_none() && m.goal.as_deref().is_none_or(|g| g == p.goal));
        let fail = fails(c, m, &r);
        (r["sha"], r["tuple"], r["parent_tuple"], r["goal"], r["measure"]) = (json!(m.sha), json!(tid), json!(parent), json!(m.goal), json!(m.id));
        (r["registered"], r["prediction"], r["gate"], r["guard"]) = (json!(pred.is_some()), json!(pred.as_ref().map(|p| p.id.clone())), json!(m.gate && m.guard.is_none()), json!(m.guard));
        r["action"] = json!(if fail { "rollback" } else { "none" });
        out.push(Out::Append("card_reading", r.clone()));
        if m.guard.is_some() && r["verdict"] == "no_noise_floor" {
            out.push(Out::Alert("goal_guard_unchecked", unchecked(m, &format!("its parent tuple {} had {} owner turns, under the {BASE_MIN} \
                a noise floor needs", if parent.is_empty() { "(none)" } else { parent.as_str() }, before.len()))));
        }
        // L3 D3: an unforecast change in a card goal is scored at the Root's abstention forecast
        if pred.is_none() && m.unforecast && r["verdict"] != "no_noise_floor"
            && let Some(g) = st.goals.iter().find(|g| m.goal.as_deref() == Some(g.id.as_str()))
        {
            let expect = toward(g, c);
            let f = Forecast { id: None, goal: &g.id, kind: &g.kind, sha: &m.sha, expect, p: ABSTAIN_PERMILLE };
            out.push(Out::Append("goal_outcome", outcome(recs, &f, u64::from(r["verdict"] == expect), "unforecast",
                json!({"card": m.card, "verdict": r["verdict"], "z_permille": r["z_permille"]}))));
        }
        if let Some(p) = pred.filter(|p| !p.resolved) {
            if fail {
                p.harm.push(format!("{}:{}", if m.guard.is_some() { "guard" } else { "gate" }, m.card));
            }
            if p.card.as_deref() == Some(m.card.as_str()) {
                p.resolved = true;
                let hit = match p.expect.as_str() { "better" => r["verdict"] == "better", "worse" => r["verdict"] == "worse", _ => r["verdict"] == "noise" };
                if r["verdict"] != "no_noise_floor" {
                    let mut rec = outcome(recs, &p.forecast(), u64::from(hit), "card", json!({"card": m.card, "verdict": r["verdict"], "z_permille": r["z_permille"]}));
                    if !p.harm.is_empty() {
                        rec["harm"] = json!(p.harm);
                    }
                    out.push(Out::Append("goal_outcome", rec));
                }
            }
        }
        // one rollback per change: a second failing guardrail in the same pass only adds its harm and alert
        let first = fail && !rolled.contains(&m.sha);
        if first {
            rolled.push(m.sha.clone());
        }
        if first && !parent.is_empty() {
            let why = format!("{}:{}", if m.guard.is_some() { "guardrail" } else { "gate" }, m.card);
            out.push(Out::Alert("goal_guard", format!("[root] change {} rolled back: {why} reading {} (z {}‰)", &m.sha[..m.sha.len().min(12)], r["verdict"], r["z_permille"])));
            out.push(Out::Rollback { sha: m.sha.clone(), to: parent.clone(), reason: why });
        }
        done.push(m.id.clone());
    }
    st.measures.retain(|m| !done.contains(&m.id));
}

/// V11 (v0.5.3): the owner alert for an owner guardrail that ended without checking its change: the reading closed with no
/// verdict (the change was replaced, or rolled back, before 3 owner turns) or read `no_noise_floor` (its parent tuple had
/// under 5 owner turns), which never rolls back. Changes stacked fast end every guardrail this way (ADR-013, open risks).
fn unchecked(m: &Measure, why: &str) -> String {
    format!("[root] the owner guardrail card:{} did not check change {}: {why}, so it could not roll that change back",
        m.card, &m.sha[..m.sha.len().min(12)])
}

/// A failed gate or guardrail: the change's open group is vetoed (H12's path: honoured, never counted); a change with
/// no open group (gated mode, or a closed window) is rolled back to the parent tuple through the kernel.
fn rollback(root: &Root, sha: &str, to: &str, reason: &str) {
    let recs = crate::lineage::records(&root.p.rh).unwrap_or_default();
    let group = recs.iter().rev().find(|r| r["kind"] == "change_open" && r["data"]["sha"] == sha).and_then(|r| r["data"]["group"].as_u64());
    let open: Vec<u64> = crate::sentinel::open_groups(root).iter().filter_map(|g| g["seq"].as_u64()).collect();
    match group.filter(|g| open.contains(g)) {
        Some(g) => {
            if let Err((_, m, _)) = crate::sentinel::veto(root, &json!({"group": g, "reason": reason})) {
                util::log(&root.p.rh, "warn", &format!("goal: veto of group {g}: {m}"));
            }
        }
        None => crate::sentinel::rollback(root, to, reason),
    }
    util::log(&root.p.rh, "info", &format!("goal: {reason}: change {sha} → {to} at {}", rfc3339(now_ms())));
}

#[cfg(test)]
mod l3_tests;
mod perm;
#[cfg(test)]
pub(crate) mod sim;
#[cfg(test)]
mod tests;
