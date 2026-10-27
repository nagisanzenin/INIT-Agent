//! The sentinel's pure state machine (ADR-011 §2.2, §3): it is fed Root observations with the Root time, and answers
//! with actions (records, kernel events, alerts, restarts) that `mod.rs` executes with no lock held. No I/O here, so
//! RF-11 and RF-12 drive it with synthetic turn streams.

use super::Turn;
use super::stats::{self, Alpha, HARD, SOFT};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Mutex;

pub const QUIET_MS: i64 = 30_000;
pub const TURN_CAP_MS: i64 = 900_000;
/// The baseline ring: 40 units (below). The exceedance threshold of the unit tests H5b, H8, H10 and H11 is the 8th
/// largest unit value at α = 0.005 (X10's tuning, owner-mandated 2026-10-22: H5b's 3× power 61% → 68%; v2_fix: a
/// 20-turn window in one long session holds 4 units, and only "4 of 4" at the 8th largest (tail 0.0024) can fire, so
/// H8, H10 and H11 share H5b's rank). S1 keeps j = 10 (the 75th percentile) at α = 0.05.
pub const RING_M: usize = 40;
pub const RING_J: usize = 10;
pub const RING_J_COST: usize = 8;
pub const HARD_COST: Alpha = (5, 1000);
/// Units (the clustering fix, 2026-10-22): consecutive owner turns of one owner session (`workspace.set` starts one)
/// with gaps under 30 min, at most 6 turns (the fallback for long sessions). Turns of one task share its difficulty, so
/// they are not exchangeable; units are (X10: per-turn tests gave 9.7–14.8% null false rollbacks on clustered turns).
pub const REQ_RING: usize = 400;
pub const LOOKS: [usize; 3] = [5, 10, 20];
pub const RECENT: usize = 120;
pub const ACK_MS: i64 = 30_000;
pub const IDLE_MS: i64 = 120_000;
pub const ANY_MS: i64 = 900_000;
pub const CRASH_WIN_MS: i64 = 600_000;
pub const BURST_FLOOR_MUSD: u64 = 500_000;
pub const HOUR_MS: i64 = 3_600_000;
pub const DAY_MS: i64 = 86_400_000;
pub const SPAWNS_HOUR: usize = 120;
pub const GROWTH_BYTES: u64 = 2 << 30;
pub const ROLLBACK_MS: i64 = 120_000;
/// H9 looks at most at the newest this many window requests (the exact tail is O(n²) in limbs).
pub const H9_MAX_REQ: usize = 4096;
pub const UNIT_MAX: usize = 6;
pub const UNIT_GAP_MS: i64 = 30 * 60_000;

/// Window lengths (§3.2; kernel windows: 40 turns, 120 h, at least 24 h).
#[derive(Clone, Copy, Debug)]
pub struct Win {
    pub turns: usize,
    pub max_ms: i64,
    pub min_ms: i64,
    pub quiet: usize,
}
pub const WIN: Win = Win { turns: 20, max_ms: 72 * HOUR_MS, min_ms: 2 * HOUR_MS, quiet: 5 };
pub const KWIN: Win = Win { turns: 40, max_ms: 120 * HOUR_MS, min_ms: 24 * HOUR_MS, quiet: 5 };

/// The blast-radius budget (§4.5) by level: open groups, changes per owner day, `group_max`, kernel per day.
pub fn budget(level: i64) -> (usize, u32, usize, u32) {
    match level {
        ..=0 => (4, 48, 16, 0),
        1 => (6, 96, 16, 1),
        2 => (8, 192, 24, 3),
        _ => (12, 384, 32, 6),
    }
}

/// The world outside the engine at one call.
#[derive(Clone, Debug, Default)]
pub struct Ctx {
    pub now: i64,
    pub level: i64,
    pub current: Option<String>,
    /// The kernel is `Running` (deafness is watched only then).
    pub running: bool,
    /// The owner day's start (the charter's `money.day_starts_at_utc_offset`).
    pub day: i64,
    /// Free bytes on the `INIT_HOME` volume (H6's growth proxy).
    pub free: Option<u64>,
    /// Charter `extend.mode = "free"` (X7's key; absent = `"gated"`). Signals act on open groups in either mode (a
    /// group exists only on the free path); the system-level actions with no group open (H2's kernel restart, H5a's
    /// alert) run only in free mode.
    pub free_mode: bool,
}

/// The owner-tunable limits: the level's blast-radius budget (open groups, changes per owner day, `group_max`, kernel
/// changes per day), the window lengths and H5a's floor, from charter `[extend]` with the owner's grant overlay (X7's
/// `grant::budget`, `grant::win`, `grant::burst_floor_musd`). `mod.rs` sets them before every engine call.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub budget: (usize, u32, usize, u32),
    pub win: Win,
    pub kwin: Win,
    pub burst_floor: u64,
}

/// The ADR §9 defaults at L0.
impl Default for Limits {
    fn default() -> Limits {
        Limits { budget: budget(0), win: WIN, kwin: KWIN, burst_floor: BURST_FLOOR_MUSD }
    }
}

/// What the adapter does for the engine, in order, with no lock held.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Append(&'static str, Value),
    Event(&'static str, Value),
    Alert(&'static str, String),
    /// A whole-system restart (root.md §3.7.3): to a tuple, or (None) the same tuple; `restore`: X4's R2 first.
    Restart { to: Option<String>, reason: &'static str, cause: Value, restore: Option<String> },
    TurnClosed(Turn),
    /// X6: caps of a `direct` rollback (or named by the owner), and the repeat ban.
    Direct(Vec<String>, bool),
    Save,
}

/// One owner turn as the rings and windows keep it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Base {
    pub seq: u64,
    /// The unit the turn belongs to (consecutive turns of one session, ≤ 6).
    #[serde(default)]
    pub unit: u64,
    pub cost: u64,
    pub wall: u64,
    pub req: u64,
    pub bytes: u64,
    pub corr: bool,
    pub zspend: u64,
}

impl Base {
    /// H10 and H5b see the main loop's share: `delegate` sub-task requests and cost are left out (MERGE).
    fn of(t: &Turn) -> Base {
        Base { seq: t.seq, unit: t.unit, cost: t.cost_musd.saturating_sub(t.sub_cost_musd), wall: t.wall_ms,
            req: u64::from(t.requests.saturating_sub(t.sub_requests)), bytes: t.req_bytes, corr: t.correction, zspend: t.zspend_musd }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub sha: String,
    pub scope: String,
    /// Caps added or modified (quarantined on a `direct` rollback).
    pub caps: Vec<String>,
    pub readmit: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub seq: u64,
    pub kernel: bool,
    pub opened: i64,
    pub last_join: i64,
    /// The turn it was opened in (a change in the same turn joins it).
    pub turn: Option<u64>,
    pub changes: Vec<Change>,
    pub tuple: String,
    pub parent: String,
    pub snapshot: Option<String>,
    pub free_at_open: Option<u64>,
    /// Owner turns since the last join, and (requests, errors) of every turn since then.
    pub win: Vec<Base>,
    pub reqs: (u64, u64),
    pub looks: usize,
    /// X8: a kernel group whose swap passed probation (`kernel_upgrade{stage:"probation_pass"}`): body groups may open.
    #[serde(default)]
    pub ksettled: bool,
}

impl Group {
    fn w(&self, lim: &Limits) -> Win {
        if self.kernel { lim.kwin } else { lim.win }
    }
    fn caps(&self) -> Vec<String> {
        let s: BTreeSet<String> = self.changes.iter().flat_map(|c| c.caps.iter().cloned()).collect();
        s.into_iter().collect()
    }
}

/// An owner line watched for deafness (§3.3 H2).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub qid: String,
    pub at: i64,
    pub acked: bool,
    pub egress: bool,
    pub deaf: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Req {
    pub turn: u64,
    pub start: i64,
    pub end: Option<i64>,
    /// A `delegate` sub-task's request (`x-egress-subtask`).
    #[serde(default)]
    pub sub: bool,
}

/// A plain steer awaiting its retract (MERGE): the turn it counted in, the line's head (≤ 256 bytes) and length to
/// match the kernel echo, and the echo's `seq` once seen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Steer {
    pub turn: u64,
    pub head: String,
    pub len: usize,
    pub seq: Option<u64>,
}

impl Steer {
    /// The kernel echo of this line: the same text, or (cut to fit the owner line) a prefix and the kernel's marker.
    fn echoes(&self, echo: &str) -> bool {
        match echo.find(" [kernel: showing ") {
            None => echo.len() == self.len && echo.starts_with(&self.head),
            Some(i) => i < self.len && (echo[..i].starts_with(&self.head) || self.head.starts_with(&echo[..i])),
        }
    }
}

const STEERS: usize = 64;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pending {
    pub to: String,
    pub to_body: String,
    pub groups: Vec<u64>,
    pub reason: String,
    pub deadline: i64,
    /// H1: X4's snapshot to restore if the target then crashes (R2).
    pub snapshot: Option<String>,
    pub h1: bool,
}

/// After an H1 rollback: R2 if the target crashes twice in 600 s or the kernel crashes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Watch {
    pub snapshot: String,
    pub to: String,
    pub until: i64,
    pub crashes: Vec<i64>,
}

/// A change's rollback history (re-admission, §3.5).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Hist {
    pub status: String,
    pub readmitted: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Engine {
    /// Set by the adapter before each call (not persisted).
    #[serde(skip)]
    pub lim: Limits,
    pub seq: u64,
    /// Owner sessions seen (`workspace.set`), the last unit id, and the last owner turn's (session, end, unit, length).
    pub session: u64,
    pub unit_seq: u64,
    pub last_owner: Option<(u64, i64, u64, usize)>,
    pub next_group: u64,
    pub turn: Option<Turn>,
    pub reqs: BTreeMap<String, Req>,
    pub quiet_since: Option<i64>,
    pub bg_spend: u64,
    pub ring: VecDeque<Base>,
    pub req_ring: VecDeque<bool>,
    pub pending: Vec<Turn>,
    pub recent: VecDeque<Turn>,
    pub groups: Vec<Group>,
    pub outcomes: BTreeMap<u64, String>,
    pub clean_tuples: BTreeSet<String>,
    pub lkg_cand: Vec<(u64, String, String)>,
    pub lines: VecDeque<Line>,
    /// Acks that came before their line: link.rs sends `owner.in` before it reports the line here, so a fast kernel's
    /// answer can win the race; the line, when it comes, is born acked (not deaf). The last 64, not persisted.
    #[serde(skip)]
    pub early_acks: VecDeque<String>,
    pub streak: u32,
    pub busy: bool,
    pub deaf_restart: Option<i64>,
    pub body_crashes: VecDeque<i64>,
    pub kills: VecDeque<i64>,
    pub spawns: VecDeque<i64>,
    pub spend: VecDeque<(i64, u64)>,
    pub hours: BTreeMap<i64, u64>,
    pub freeze_until: Option<i64>,
    pub burst_alert: Option<i64>,
    pub rollback: Option<Pending>,
    pub watch: Option<Watch>,
    pub hist: BTreeMap<String, Hist>,
    pub directs: BTreeMap<String, Vec<i64>>,
    pub day: (i64, u32),
    pub kday: (i64, u32),
    pub suppress_until: i64,
    /// Plain steers that a retract may still take back (not persisted: a restart forgets them).
    #[serde(skip)]
    pub steers: VecDeque<Steer>,
}

/// Thresholds are pure functions of their arguments: cached once computed.
type Key = (u8, u64, u64, u64, u64);
static KCACHE: Mutex<BTreeMap<Key, Option<u64>>> = Mutex::new(BTreeMap::new());

fn cached(key: Key, f: impl FnOnce() -> Option<u64>) -> Option<u64> {
    if let Some(v) = crate::link::lock(&KCACHE).get(&key) {
        return *v;
    }
    let v = f();
    crate::link::lock(&KCACHE).insert(key, v);
    v
}

/// The smallest exceedance count K with tail ≤ α for n window units against the full ring (m = 40, j = 10).
pub fn k_exceed(n: usize, alpha: Alpha) -> Option<u64> {
    k_exceed_j(n, RING_J, alpha)
}

/// As `k_exceed`, with the threshold at the j-th largest ring value.
pub fn k_exceed_j(n: usize, j: usize, alpha: Alpha) -> Option<u64> {
    let n = n as u64;
    cached((0, n, alpha.0, alpha.1, j as u64), || stats::exceed_k(RING_M as u64, j as u64, n, alpha))
}

/// The smallest count k with binomial tail ≤ α at rate a/d.
pub fn k_binom(n: usize, (a, d): (u64, u64), alpha: Alpha) -> Option<u64> {
    let n = n as u64;
    cached((1, n, a * 1_000_000 + d, alpha.0, alpha.1), || stats::binom_k(n, a, d, alpha))
}

/// One test that fired: signal, value, threshold.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub signal: &'static str,
    pub value: u64,
    pub threshold: u64,
}

/// The correction rate r0 = max(50‰, (b+1)/(m+2)) (ADR-010 H5).
pub fn r0_corr(ring: &[Base]) -> (u64, u64) {
    let b = ring.iter().filter(|t| t.corr).count() as u64;
    stats::max_rate((1, 20), (b + 1, ring.len() as u64 + 2))
}

/// The provider-error rate r0 = max(20‰, (b+1)/(m_req+2)) (H9).
pub fn r0_err(req_ring: &[bool]) -> (u64, u64) {
    let b = req_ring.iter().filter(|e| **e).count() as u64;
    stats::max_rate((1, 50), (b + 1, req_ring.len() as u64 + 2))
}

/// The exceedance count K of `win` over the ring's j-th largest, and whether the magnitude guard holds
/// (window median ≥ `num/den` × baseline median).
fn exceed(ring: &[u64], win: &[u64], j: usize, (num, den): (u64, u64)) -> (u64, bool) {
    let thr = stats::jth_largest(ring, j).unwrap_or(u64::MAX);
    let k = win.iter().filter(|v| **v > thr).count() as u64;
    let guard = u128::from(stats::median2(win)) * u128::from(den) >= u128::from(stats::median2(ring)) * u128::from(num);
    (k, guard)
}

/// One value per unit: the geometric mean of its turns' values, as ⌊(Π(x+1))^(1/n)⌋ − 1 in exact integers (a turn with
/// no spend does not zero its unit). Units in id order; the ring keeps its newest `RING_M`.
pub fn unit_values(turns: &[Base], f: fn(&Base) -> u64) -> Vec<u64> {
    let mut by: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for t in turns {
        by.entry(t.unit).or_default().push(f(t));
    }
    by.values().map(|v| gm1(v)).collect()
}

/// ⌊(Π(xᵢ+1))^(1/n)⌋ − 1, by bisection on the integer root with exact big products.
pub fn gm1(v: &[u64]) -> u64 {
    if v.len() == 1 {
        return v[0];
    }
    let mut p = stats::Big::from(1);
    v.iter().for_each(|x| p.mul(x.saturating_add(1)));
    let pow = |r: u64| {
        let mut b = stats::Big::from(1);
        (0..v.len()).for_each(|_| b.mul(r));
        b
    };
    let (mut lo, mut hi) = (1u64, v.iter().max().copied().unwrap_or(0).saturating_add(1));
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if pow(mid) <= p { lo = mid } else { hi = mid - 1 }
    }
    lo - 1
}

/// The newest `RING_M` units of the ring (turns of older units dropped).
fn ring_units(ring: &[Base]) -> Vec<Base> {
    let units: BTreeSet<u64> = ring.iter().map(|t| t.unit).collect();
    let keep: BTreeSet<u64> = units.iter().rev().take(RING_M).copied().collect();
    ring.iter().filter(|t| keep.contains(&t.unit)).cloned().collect()
}

fn n_units(turns: &[Base]) -> usize {
    turns.iter().map(|t| t.unit).collect::<BTreeSet<_>>().len()
}

/// The chronic tests of one look (§3.3: H5b, H8, H10, H11 on units; H7 on turns; H9 on requests). `win` is the
/// window's first n owner turns; `reqs` the window's (requests, errors). Nothing runs until the rings are full (40
/// units, 400 requests).
pub fn judge(ring: &[Base], req_ring: &[bool], win: &[Base], reqs: (u64, u64)) -> Vec<Hit> {
    let mut hits = Vec::new();
    let ring = ring_units(ring);
    let nu = n_units(win);
    if n_units(&ring) >= RING_M && nu > 0 {
        for (sig, f, guard, j, alpha) in [("H5b", (|b: &Base| b.cost) as fn(&Base) -> u64, (3, 2), RING_J_COST, HARD_COST),
            ("H8", |b: &Base| b.wall, (3, 2), RING_J_COST, HARD_COST), ("H10", |b: &Base| b.req, (2, 1), RING_J_COST, HARD_COST),
            ("H11", |b: &Base| b.bytes, (3, 1), RING_J_COST, HARD_COST)] {
            let (kv, g) = exceed(&unit_values(&ring, f), &unit_values(win, f), j, guard);
            if let Some(k) = k_exceed_j(nu, j, alpha).filter(|k| kv >= *k && g) {
                hits.push(Hit { signal: sig, value: kv, threshold: k });
            }
        }
        let c = win.iter().filter(|t| t.corr).count() as u64;
        if let Some(k) = k_binom(win.len(), r0_corr(&ring), HARD).filter(|k| c >= *k) {
            hits.push(Hit { signal: "H7", value: c, threshold: k });
        }
    }
    let (nr, ne) = reqs;
    if req_ring.len() >= REQ_RING && ne >= 3 && nr > 0 {
        let (a, d) = r0_err(req_ring);
        if stats::binom_tail(nr, ne, a, d).le(HARD) {
            hits.push(Hit { signal: "H9", value: ne, threshold: 3 });
        }
    }
    hits
}

/// S1 at close (ADR-010 S1): a tail ≤ 0.05 on cost, wall or zone spend per unit, or on corrections per turn → `flagged`.
pub fn soft(ring: &[Base], win: &[Base]) -> bool {
    let ring = ring_units(ring);
    let nu = n_units(win);
    if n_units(&ring) < RING_M || nu == 0 {
        return false;
    }
    let k = k_exceed(nu, SOFT);
    let any = [|b: &Base| b.cost, |b: &Base| b.wall, |b: &Base| b.zspend].into_iter()
        .any(|f| k.is_some_and(|k| exceed(&unit_values(&ring, f), &unit_values(win, f), RING_J, (0, 1)).0 >= k));
    let c = win.iter().filter(|t| t.corr).count() as u64;
    any || k_binom(win.len(), r0_corr(&ring), SOFT).is_some_and(|k| c >= k)
}

/// The `live` block of `change_close` (ratios of unit medians; K over units).
fn live(ring: &[Base], win: &[Base], reqs: (u64, u64)) -> Value {
    let ring = ring_units(ring);
    let ratio = |f: fn(&Base) -> u64| {
        let r = stats::median2(&unit_values(&ring, f));
        (r > 0).then(|| stats::median2(&unit_values(win, f)) * 1000 / r)
    };
    let full = n_units(&ring) >= RING_M;
    let k = |f: fn(&Base) -> u64, j: usize| full.then(|| exceed(&unit_values(&ring, f), &unit_values(win, f), j, (0, 1)).0);
    json!({"corrections": win.iter().filter(|t| t.corr).count(), "cost_ratio_permille": ratio(|b| b.cost), "k_bytes": k(|b| b.bytes, RING_J_COST),
        "k_cost": k(|b| b.cost, RING_J_COST), "k_req": k(|b| b.req, RING_J_COST), "k_wall": k(|b| b.wall, RING_J_COST), "provider_errors": reqs.1,
        "units": n_units(win), "wall_ratio_permille": ratio(|b| b.wall)})
}

/// How a trigger chooses its groups (§3.4).
#[derive(Clone, Debug, PartialEq)]
pub enum Pick {
    /// H1, H2, H6 churn: every open group, `direct` if it is the only one, else `suspect`.
    All,
    /// H5a, the other H6 cases, every chronic signal: the newest group, `direct`.
    Newest,
    /// H3 (the owner) and H12 (a zone veto): this group `direct` (H12: unattributed), newer ones `collateral`.
    Named(u64),
    /// H4: every open group as `collateral` (the demotion is the ledger's).
    Collateral,
}

/// A refusal of `change.open`: code and message.
pub type No = (&'static str, String);

/// A parsed `change.open` (§Records, control link).
#[derive(Clone, Debug, Default)]
pub struct Open {
    pub sha: String,
    pub scope: String,
    pub caps: Vec<String>,
    pub tuple: String,
}

/// The group a change will join or open, decided before X4's hooks run.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub group: u64,
    pub first: bool,
    pub parent: String,
    pub readmit: bool,
}

fn first_word(text: &str) -> &str {
    text.split_whitespace().next().unwrap_or("")
}

/// Owner corrections typed as lines (MERGE): `/undo`, the kernel's `/stop` (alone on its line, as the kernel reads it),
/// and the owner's reversals `/reject` and `/rollback`. `/panic` and interrupts (Esc, Ctrl-C, `then:"continue"`) reach
/// the engine by `panic` and `interrupt`. A plain steer, a `/later` line or a retracted line is never a correction.
const CORRECTIONS: [&str; 5] = ["/undo", "/stop", "/reject", "/rollback", "/panic"];
/// Lines that neither correct nor steer the running turn: the kernel's follow-up and its read-only or settling commands.
const NOT_STEERS: [&str; 6] = ["/later", "/resume", "/status", "/kernel", "/accept", "/bless"];

fn is_correction(text: &str) -> bool {
    let mut w = text.split_whitespace();
    match w.next() {
        Some("/stop") => w.next().is_none(),
        Some(word) => CORRECTIONS.contains(&word),
        None => false,
    }
}

impl Engine {
    pub fn new(current: Option<&str>) -> Engine {
        let mut e = Engine { next_group: 1, ..Engine::default() };
        e.clean_tuples.extend(current.map(String::from));
        e
    }

    // ------------------------------------------------------------------------------------------------ turns

    fn start_turn(&mut self, cx: &Ctx, kind: &str) {
        self.seq += 1;
        // an owner turn continues the run of its session (gap < 30 min, < 6 turns) or starts a new unit
        let unit = match self.last_owner {
            _ if kind != "owner" => 0,
            Some((s, end, u, n)) if s == self.session && cx.now - end < UNIT_GAP_MS && n < UNIT_MAX => {
                self.last_owner = Some((s, end, u, n + 1));
                u
            }
            _ => {
                self.unit_seq += 1;
                self.last_owner = Some((self.session, cx.now, self.unit_seq, 1));
                self.unit_seq
            }
        };
        self.turn = Some(Turn { seq: self.seq, kind: kind.into(), start: cx.now, end: cx.now, groups: self.groups.iter().map(|g| g.seq).collect(),
            tuple: cx.current.clone(), unit, ..Turn::default() });
        if self.reqs.values().all(|r| r.end.is_some()) {
            self.quiet_since = Some(cx.now);
        }
    }

    /// The current owner turn, or the last closed one if it ended under 5 minutes ago (a Root-handled correction).
    fn mark_correction(&mut self, now: i64) {
        if let Some(t) = self.turn.as_mut().filter(|t| t.kind == "owner") {
            t.correction = true;
            return;
        }
        let Some(last) = self.recent.iter_mut().rev().find(|t| t.kind == "owner").filter(|t| now - t.end < 300_000) else { return };
        last.correction = true;
        let seq = last.seq;
        self.pending.iter_mut().filter(|t| t.seq == seq).for_each(|t| t.correction = true);
        self.ring.iter_mut().filter(|t| t.seq == seq).for_each(|t| t.corr = true);
        self.groups.iter_mut().flat_map(|g| g.win.iter_mut()).filter(|b| b.seq == seq).for_each(|b| b.corr = true);
    }

    fn close_turn(&mut self, cx: &Ctx, capped: bool, out: &mut Vec<Act>) {
        let Some(mut t) = self.turn.take() else { return };
        let last_end = self.reqs.values().filter(|r| r.turn == t.seq).filter_map(|r| r.end).max();
        t.end = if capped { cx.now } else { last_end.unwrap_or(t.start).max(t.start) };
        t.wall_ms = (t.end - t.start).max(0) as u64;
        t.capped = capped;
        if t.kind == "owner" {
            t.zspend_musd = t.cost_musd + std::mem::take(&mut self.bg_spend);
            if let Some(lo) = self.last_owner.as_mut().filter(|lo| lo.2 == t.unit) {
                lo.1 = t.end; // the run's last end, for the next turn's gap
            }
        } else {
            self.bg_spend += t.cost_musd;
        }
        for g in self.groups.iter_mut().filter(|g| t.start >= g.last_join) {
            g.reqs.0 += u64::from(t.requests);
            g.reqs.1 += u64::from(t.provider_errors);
            if t.kind == "owner" {
                g.win.push(Base::of(&t));
            }
        }
        self.recent.push_back(t.clone());
        while self.recent.len() > RECENT {
            self.recent.pop_front();
        }
        if t.groups.is_empty() {
            if t.tuple.as_ref().is_none_or(|x| self.clean_tuples.contains(x)) {
                self.admit(&t);
            }
        } else {
            self.pending.push(t.clone());
            self.settle_pending();
        }
        out.push(Act::TurnClosed(t.clone()));
        if t.kind == "owner" {
            self.looks(cx, out);
        }
        out.push(Act::Save);
    }

    /// A turn served by a clean tuple enters the baseline rings.
    fn admit(&mut self, t: &Turn) {
        if t.kind == "owner" {
            self.ring.push_back(Base::of(t));
            let keep = ring_units(&self.ring.iter().cloned().collect::<Vec<_>>());
            if keep.len() < self.ring.len() {
                self.ring = keep.into();
            }
        }
        let ok = t.requests.saturating_sub(t.provider_errors);
        self.req_ring.extend(std::iter::repeat_n(true, t.provider_errors as usize).chain(std::iter::repeat_n(false, ok as usize)));
        while self.req_ring.len() > REQ_RING {
            self.req_ring.pop_front();
        }
    }

    /// Pending turns whose groups have all closed: into the rings if all closed `clean`, else dropped.
    fn settle_pending(&mut self) {
        let open: BTreeSet<u64> = self.groups.iter().map(|g| g.seq).collect();
        let pend = std::mem::take(&mut self.pending);
        for t in pend {
            if t.groups.iter().any(|g| open.contains(g)) {
                self.pending.push(t);
            } else if t.groups.iter().all(|g| self.outcomes.get(g).is_some_and(|o| o == "clean")) {
                self.admit(&t);
            }
        }
    }

    pub fn line(&mut self, cx: &Ctx, qid: &str, text: &str) -> Vec<Act> {
        let mut out = Vec::new();
        if cx.running {
            let acked = self.early_acks.iter().position(|q| q == qid).and_then(|i| self.early_acks.remove(i)).is_some();
            self.lines.push_back(Line { qid: qid.into(), at: cx.now, acked, ..Line::default() });
        }
        let word = first_word(text);
        let corr = is_correction(text);
        match self.turn.as_ref().map(|t| t.kind.as_str()) {
            Some("owner") => {
                if let Some(t) = self.turn.as_mut() {
                    if corr {
                        t.correction = true;
                    } else if !NOT_STEERS.contains(&word) {
                        t.steered += 1; // a plain steer: a weak signal, never a correction
                        let mut cut = text.len().min(256);
                        while !text.is_char_boundary(cut) {
                            cut -= 1;
                        }
                        self.steers.push_back(Steer { turn: t.seq, head: text[..cut].into(), len: text.len(), seq: None });
                        if self.steers.len() > STEERS {
                            self.steers.pop_front();
                        }
                    }
                }
            }
            Some(_) => {
                // a background turn ends where the owner speaks; its in-flight requests move to the owner turn
                let bg = self.turn.as_ref().map(|t| t.seq);
                self.close_turn(cx, false, &mut out);
                self.start_turn(cx, "owner");
                let seq = self.seq;
                self.reqs.values_mut().filter(|r| Some(r.turn) == bg && r.end.is_none()).for_each(|r| r.turn = seq);
                if corr {
                    self.mark_correction(cx.now);
                }
            }
            None => {
                // a correction typed after the turn ended belongs to that turn when it is under 5 minutes old
                let late = corr && self.recent.iter().rev().find(|t| t.kind == "owner").is_some_and(|t| cx.now - t.end < 300_000);
                if late {
                    self.mark_correction(cx.now);
                }
                self.start_turn(cx, "owner");
                if corr && !late {
                    self.mark_correction(cx.now);
                }
            }
        }
        if word == "/reject" && let Some(arg) = text.split_whitespace().nth(1) {
            out.extend(self.reversal(cx, Some(arg), "/reject"));
        }
        out
    }

    /// A new owner session (`workspace.set`, once per `init-agent` launch): the next owner turn starts a new unit.
    pub fn session_started(&mut self) {
        self.session += 1;
    }

    /// An owner interrupt (Esc or Ctrl-C, with or without `then:"continue"`), `/undo` or `/rollback`: a correction.
    pub fn interrupt(&mut self, cx: &Ctx) {
        self.mark_correction(cx.now);
    }

    /// One owner frame the Root relayed to the kernel (`link.rs` `deliver`, after the redelivery check): `line` starts
    /// or steers a turn, `interrupt` is a correction, `retract{seq}` takes a steer back.
    pub fn frame(&mut self, cx: &Ctx, qid: &str, frame: &Value) -> Vec<Act> {
        match frame["t"].as_str() {
            Some("interrupt") => {
                self.interrupt(cx);
                Vec::new()
            }
            Some("line") => self.line(cx, qid, frame["text"].as_str().unwrap_or("")),
            Some("retract") => {
                if let Some(seq) = frame["seq"].as_u64() {
                    self.retract(seq);
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// The kernel's `echo{seq, text}` of an owner line: binds `seq` to the oldest steer with that text not yet bound.
    pub fn echoed(&mut self, seq: u64, text: &str) {
        if let Some(s) = self.steers.iter_mut().find(|s| s.seq.is_none() && s.echoes(text)) {
            s.seq = Some(seq);
        }
    }

    /// A relayed `retract{seq}`: the steer echoed as `seq` no longer counts in its turn (open or recently closed).
    pub fn retract(&mut self, seq: u64) {
        let Some(i) = self.steers.iter().position(|s| s.seq == Some(seq)) else { return };
        let Some(s) = self.steers.remove(i) else { return };
        let undo = |t: &mut Turn| t.steered = t.steered.saturating_sub(1);
        if let Some(t) = self.turn.as_mut().filter(|t| t.seq == s.turn) {
            undo(t);
        } else if let Some(t) = self.recent.iter_mut().find(|t| t.seq == s.turn) {
            undo(t);
        }
    }

    pub fn ack(&mut self, qid: &str) {
        let mut found = false;
        for l in self.lines.iter_mut().filter(|l| l.qid == qid) {
            (l.acked, found) = (true, true);
        }
        if !found {
            self.early_acks.push_back(qid.into());
            if self.early_acks.len() > 64 {
                self.early_acks.pop_front();
            }
        }
    }

    pub fn hb(&mut self, busy: bool) {
        self.busy = busy;
    }

    /// The kernel's `state{busy:false}` frame: the body finished the owner's turn. The owner turn closes now, not
    /// after `QUIET_MS` of quiet, so the owner's next line opens a new turn instead of counting as a steer of a
    /// finished one (found by the GBAL real-model proof: lines 2 s apart merged five owner turns into one, and the
    /// ADR-013 cards saw no turns). Only a turn that made requests, all of them ended; anything else keeps the quiet rule.
    pub fn idle(&mut self, cx: &Ctx) -> Vec<Act> {
        let mut out = Vec::new();
        let done = self.turn.as_ref().is_some_and(|t| {
            t.kind == "owner" && t.requests > 0 && self.reqs.values().filter(|r| r.turn == t.seq).all(|r| r.end.is_some())
        });
        if done {
            self.close_turn(cx, false, &mut out);
        }
        out
    }

    /// A main-loop zone request begins (the tests' shorthand for `req_start_as(…, false)`).
    #[cfg(test)]
    pub fn req_start(&mut self, cx: &Ctx, id: &str, bytes: u64) -> Vec<Act> {
        self.req_start_as(cx, id, bytes, false)
    }

    /// A zone request begins; `sub`: a `delegate` sub-task's (`x-egress-subtask`), counted apart for H10 and H5b.
    pub fn req_start_as(&mut self, cx: &Ctx, id: &str, bytes: u64, sub: bool) -> Vec<Act> {
        if self.turn.is_none() {
            self.start_turn(cx, "background");
        }
        let t = self.turn.as_mut().expect("a turn is open");
        t.requests += 1;
        t.sub_requests += u32::from(sub);
        t.req_bytes += bytes;
        let seq = t.seq;
        self.reqs.insert(id.into(), Req { turn: seq, start: cx.now, end: None, sub });
        self.quiet_since = None;
        self.lines.iter_mut().for_each(|l| l.egress = true);
        Vec::new()
    }

    pub fn req_caps(&mut self, id: &str, caps: &[(String, u32, &str)]) {
        let Some(seq) = self.reqs.get(id).map(|r| r.turn) else { return };
        let add = |t: &mut Turn| {
            for (cap, n, ev) in caps {
                let e = t.caps.entry(cap.clone()).or_insert((0, ev.to_string()));
                e.0 += n;
                if *ev == "root" {
                    e.1 = "root".into();
                }
            }
        };
        if let Some(t) = self.turn.as_mut().filter(|t| t.seq == seq) {
            add(t);
        } else if let Some(t) = self.recent.iter_mut().find(|t| t.seq == seq) {
            add(t);
        }
    }

    /// ADR-013: a request's input and cache-read tokens land on its turn (the cache-read card).
    pub fn req_usage(&mut self, id: &str, input: u64, cache_read: u64) {
        let Some(seq) = self.reqs.get(id).map(|r| r.turn) else { return };
        if let Some(t) = self.turn.as_mut().filter(|t| t.seq == seq) {
            t.input_tok += input;
            t.cache_read_tok += cache_read.min(input);
        }
    }

    pub fn req_end(&mut self, cx: &Ctx, id: &str, cost: u64, error: bool) -> Vec<Act> {
        let mut out = Vec::new();
        let Some(r) = self.reqs.get_mut(id) else { return out };
        r.end = Some(cx.now);
        let (seq, sub) = (r.turn, r.sub);
        if let Some(t) = self.turn.as_mut().filter(|t| t.seq == seq) {
            t.cost_musd += cost;
            t.sub_cost_musd += if sub { cost } else { 0 };
            t.provider_errors += u32::from(error);
        }
        self.spend.push_back((cx.now, cost));
        *self.hours.entry(cx.now.div_euclid(HOUR_MS)).or_default() += cost;
        if self.reqs.values().all(|r| r.end.is_some()) {
            self.quiet_since = Some(cx.now);
        }
        self.burst(cx, &mut out);
        out
    }

    // ------------------------------------------------------------------------------------------------ acute

    /// H5a: zone spend in any rolling 60 min ≥ max($0.50, 4 × p95 of active hourly spend over 14 days).
    fn burst(&mut self, cx: &Ctx, out: &mut Vec<Act>) {
        while self.spend.front().is_some_and(|(t, _)| cx.now - t >= HOUR_MS) {
            self.spend.pop_front();
        }
        let hour = cx.now.div_euclid(HOUR_MS);
        self.hours.retain(|h, _| hour - h < 14 * 24);
        let mut active: Vec<u64> = self.hours.iter().filter(|(h, v)| **h != hour && **v > 0).map(|(_, v)| *v).collect();
        active.sort_unstable();
        let p95 = active.get((active.len() * 95).div_ceil(100).saturating_sub(1)).copied().unwrap_or(0);
        let thr = self.lim.burst_floor.max(p95.saturating_mul(4));
        let sum: u64 = self.spend.iter().map(|s| s.1).sum();
        if sum < thr {
            return;
        }
        if self.groups.is_empty() {
            if cx.free_mode && self.burst_alert.is_none_or(|t| cx.now - t >= HOUR_MS) {
                self.burst_alert = Some(cx.now);
                out.push(Act::Append("harm", self.harm("H5a", sum, thr, "system", &[], "alert")));
                out.push(Act::Alert("spend_burst", format!("[root] zone spend {sum} micro-USD in the last hour reached the burst threshold {thr}")));
            }
            return;
        }
        self.spend.clear();
        self.freeze_until = Some(cx.now + HOUR_MS);
        out.extend(self.trigger(cx, "H5a", sum, thr, Pick::Newest, "rollback_newest"));
        out.push(Act::Alert("spend_burst", format!("[root] spend burst ({sum} micro-USD in an hour): the newest change group is rolled back and new groups are frozen for 1 h")));
    }

    pub fn body_exit(&mut self, cx: &Ctx, crash: bool) -> Vec<Act> {
        let mut out = Vec::new();
        if !crash || cx.now < self.suppress_until {
            return out;
        }
        if let Some(t) = self.turn.as_mut() {
            t.crash = true;
        }
        self.body_crashes.push_back(cx.now);
        while self.body_crashes.front().is_some_and(|t| cx.now - t > CRASH_WIN_MS) {
            self.body_crashes.pop_front();
        }
        if let Some(w) = self.watch.as_mut().filter(|w| cx.now <= w.until) {
            w.crashes.push(cx.now);
            if w.crashes.len() >= 2 {
                return self.escalate(cx);
            }
            return out;
        }
        if !self.groups.is_empty() && self.body_crashes.len() >= 2 {
            let n = self.body_crashes.len() as u64;
            self.body_crashes.clear();
            out.extend(self.trigger(cx, "H1", n, 2, Pick::All, "rollback_all"));
        }
        out
    }

    /// A kernel crash (`zone_exit{class:"crash"}`): the bodies' deaths that follow are not theirs.
    pub fn kernel_crash(&mut self, cx: &Ctx) -> Vec<Act> {
        self.suppress_until = cx.now + 5_000;
        if let Some(t) = self.turn.as_mut() {
            t.crash = true;
        }
        if self.watch.as_ref().is_some_and(|w| cx.now <= w.until) {
            return self.escalate(cx);
        }
        if self.groups.is_empty() {
            return Vec::new();
        }
        self.trigger(cx, "H1", 1, 1, Pick::All, "rollback_all")
    }

    /// Any stop of the zone the Root ordered (panic, owner stop, kernel exit): body deaths in the next 5 s are not crashes.
    pub fn zone_stopping(&mut self, cx: &Ctx) {
        self.suppress_until = cx.now + 5_000;
    }

    /// R2 (§3.3 H1): the rollback target crashed again; restore the snapshot with the zone stopped.
    fn escalate(&mut self, cx: &Ctx) -> Vec<Act> {
        let Some(w) = self.watch.take() else { return Vec::new() };
        let _ = cx;
        vec![Act::Append("harm", self.harm("H1", 2, 2, "system", &[], "rollback_all")),
            Act::Alert("rollback", format!("[root] the rollback target {} crashed again: restoring the system snapshot {}", w.to, w.snapshot)),
            Act::Restart { to: Some(w.to.clone()), reason: "rollback_system", cause: json!({"to": w.to, "trigger": "H1"}), restore: Some(w.snapshot) },
            Act::Save]
    }

    /// H6: a `mem_cap` kill of a live-zone unit, or a `disk_floor` kill.
    pub fn mem_event(&mut self, cx: &Ctx, event: &str) -> Vec<Act> {
        let mut out = Vec::new();
        if let Some(t) = self.turn.as_mut() {
            t.kill = true;
        }
        match event {
            "mem_cap" => {
                self.kills.push_back(cx.now);
                while self.kills.front().is_some_and(|t| cx.now - t > DAY_MS) {
                    self.kills.pop_front();
                }
                if !self.groups.is_empty() && self.kills.len() >= 2 {
                    let n = self.kills.len() as u64;
                    self.kills.clear();
                    out.extend(self.trigger(cx, "H6", n, 2, Pick::Newest, "rollback_newest"));
                }
            }
            "disk_floor" => {
                let at_open = self.groups.first().and_then(|g| g.free_at_open);
                if let (Some(was), Some(now)) = (at_open, cx.free) && was.saturating_sub(now) >= GROWTH_BYTES {
                    out.extend(self.trigger(cx, "H6", (was - now) >> 20, GROWTH_BYTES >> 20, Pick::Newest, "rollback_newest"));
                }
            }
            _ => {}
        }
        out
    }

    pub fn spawned(&mut self, cx: &Ctx) -> Vec<Act> {
        self.spawns.push_back(cx.now);
        while self.spawns.front().is_some_and(|t| cx.now - t > HOUR_MS) {
            self.spawns.pop_front();
        }
        if self.groups.is_empty() || self.spawns.len() <= SPAWNS_HOUR {
            return Vec::new();
        }
        let n = self.spawns.len() as u64;
        self.spawns.clear();
        self.trigger(cx, "H6", n, SPAWNS_HOUR as u64, Pick::All, "rollback_all")
    }

    /// H4: a demoting refusal (D6 (a)–(c)) or an `escapee` record. The demotion itself is the ledger's.
    pub fn scope(&mut self, cx: &Ctx) -> Vec<Act> {
        if let Some(t) = self.turn.as_mut() {
            t.refusal = true;
        }
        if self.groups.is_empty() {
            return Vec::new();
        }
        self.trigger(cx, "H4", 1, 1, Pick::Collateral, "demote")
    }

    /// `/panic`: a correction; open groups go as collateral (resume rolls back to `settled`).
    pub fn panic(&mut self, cx: &Ctx) -> Vec<Act> {
        self.mark_correction(cx.now);
        self.zone_stopping(cx);
        self.lines.clear();
        if self.groups.is_empty() {
            return Vec::new();
        }
        let mut out = self.close_all_collateral(cx, "H3");
        out.insert(0, Act::Append("harm", self.harm("H3", 1, 1, "system", &self.groups_seqs(), "demote")));
        out
    }

    /// H3: `/reject <sha|cap>` or `/rollback [sha|cap]`. Ok(acts) or Err(the owner notice).
    pub fn reversal(&mut self, cx: &Ctx, arg: Option<&str>, by: &str) -> Vec<Act> {
        let target = match arg {
            None => self.groups.last().map(|g| g.seq),
            Some(a) => self.groups.iter().find(|g| g.changes.iter().any(|c| (a.len() >= 7 && c.sha.starts_with(a)) || c.caps.iter().any(|x| x == a)))
                .map(|g| g.seq),
        };
        let mut out = Vec::new();
        if let Some(a) = arg.filter(|a| a.contains('/')) {
            out.push(Act::Direct(vec![a.to_string()], false)); // a cap the owner named is quarantined (§6.3)
        }
        match target {
            Some(g) => out.extend(self.trigger(cx, "H3", 1, 1, Pick::Named(g), "rollback_newest")),
            None if by == "/rollback" => out.push(Act::Alert("rollback", format!("[root] /rollback: no open change group matches {}", arg.unwrap_or("(newest)")))),
            None => {}
        }
        out
    }

    /// H12: `change.veto{group, reason}` from the kernel or body: always honoured, never counted.
    pub fn veto(&mut self, cx: &Ctx, group: u64) -> Result<Vec<Act>, No> {
        if !self.groups.iter().any(|g| g.seq == group) {
            return Err(("root.not_found", format!("no open change group {group}")));
        }
        Ok(self.trigger(cx, "H12", 1, 1, Pick::Named(group), "rollback_newest"))
    }

    fn groups_seqs(&self) -> Vec<u64> {
        self.groups.iter().map(|g| g.seq).collect()
    }

    fn harm(&self, signal: &str, value: u64, threshold: u64, scope: &str, groups: &[u64], action: &str) -> Value {
        let caps: Vec<&String> = self.turn.as_ref().or(self.recent.back()).map(|t| t.caps.keys().take(16).collect()).unwrap_or_default();
        json!({"action": action, "caps": caps, "groups": groups, "scope": scope, "signal": signal, "threshold": threshold, "value": value})
    }

    /// Closes every open group as `collateral` (H3 panic, H4): no rollback of their own (the demotion or resume does it).
    fn close_all_collateral(&mut self, cx: &Ctx, trigger: &str) -> Vec<Act> {
        let groups = std::mem::take(&mut self.groups);
        let mut out = Vec::new();
        for g in &groups {
            out.push(self.close_rec(g, "collateral", Some(trigger), None));
            for c in &g.changes {
                self.hist.insert(c.sha.clone(), Hist { status: "collateral".into(), readmitted: c.readmit });
            }
            self.outcomes.insert(g.seq, "collateral".into());
            out.push(Act::Event("extend.window", json!({"group": g.seq, "outcome": "collateral", "state": "closed"})));
        }
        self.settle_pending();
        let _ = cx;
        out.push(Act::Save);
        out
    }

    fn close_rec(&self, g: &Group, outcome: &str, trigger: Option<&str>, attribution: Option<&str>) -> Act {
        Act::Append("change_close", json!({"attribution": attribution, "group": g.seq, "live": live(&self.ring_v(), &g.win, g.reqs),
            "outcome": outcome, "trigger": trigger, "turns": g.win.len()}))
    }

    fn ring_v(&self) -> Vec<Base> {
        self.ring.iter().cloned().collect()
    }

    /// A trigger (§3.4): the `harm` record, the groups' closes with their attribution, re-admission bookkeeping,
    /// X6's quarantine for `direct` groups, then the R1 rollback (the event, with the 120 s fallback).
    pub fn trigger(&mut self, cx: &Ctx, signal: &'static str, value: u64, threshold: u64, pick: Pick, action: &str) -> Vec<Act> {
        if pick == Pick::Collateral {
            let mut out = vec![Act::Append("harm", self.harm(signal, value, threshold, "system", &self.groups_seqs(), action))];
            out.extend(self.rollback_from(cx, 0, signal, |_, _| ("collateral", None)));
            return out;
        }
        let n = self.groups.len();
        let (from, named) = match pick {
            Pick::All => (0, None),
            Pick::Newest => (n.saturating_sub(1), None),
            Pick::Named(s) => match self.groups.iter().position(|g| g.seq == s) {
                Some(i) => (i, Some(s)),
                None => return Vec::new(),
            },
            Pick::Collateral => unreachable!(),
        };
        if n == 0 {
            return Vec::new();
        }
        let seqs: Vec<u64> = self.groups[from..].iter().map(|g| g.seq).collect();
        let mut out = vec![Act::Append("harm", self.harm(signal, value, threshold, "group", &seqs, action))];
        let all = pick == Pick::All;
        let unattributed = signal == "H12";
        out.extend(self.rollback_from(cx, from, signal, |g, i| {
            if all {
                ("rolled_back", Some(if n == 1 { "direct" } else { "suspect" }))
            } else if named.is_some_and(|s| s != g.seq) || (named.is_none() && i != from) {
                ("collateral", None)
            } else if unattributed {
                ("rolled_back", None)
            } else {
                ("rolled_back", Some("direct"))
            }
        }));
        out
    }

    /// Rolls back the open groups from index `from` on: closes, history, quarantine hooks, then the R1 rollback to the
    /// first one's parent tuple.
    fn rollback_from(&mut self, cx: &Ctx, from: usize, signal: &str, attr: impl Fn(&Group, usize) -> (&'static str, Option<&'static str>))
        -> Vec<Act> {
        let mut out = Vec::new();
        if from >= self.groups.len() {
            return out;
        }
        let gone: Vec<Group> = self.groups.drain(from..).collect();
        let to = gone[0].parent.clone();
        let snapshot = gone[0].snapshot.clone();
        let kernel = gone.iter().any(|g| g.kernel);
        for (i, g) in gone.iter().enumerate() {
            let (outcome, attribution) = attr(g, from + i);
            out.push(self.close_rec(g, outcome, Some(signal), attribution));
            out.push(Act::Event("extend.window", json!({"group": g.seq, "outcome": outcome, "state": "closed"})));
            self.outcomes.insert(g.seq, outcome.into());
            let status = attribution.unwrap_or(if outcome == "collateral" { "collateral" } else { "vetoed" });
            for c in &g.changes {
                self.hist.insert(c.sha.clone(), Hist { status: status.into(), readmitted: c.readmit });
            }
            if attribution == Some("direct") {
                let caps = g.caps();
                let mut repeat = false;
                for c in &caps {
                    let v = self.directs.entry(c.clone()).or_default();
                    v.retain(|t| cx.now - t < 14 * DAY_MS);
                    v.push(cx.now);
                    repeat |= v.len() >= 2;
                }
                if !caps.is_empty() {
                    out.push(Act::Direct(caps, repeat));
                }
            }
        }
        self.settle_pending();
        self.lkg_cand.retain(|(s, _, _)| !gone.iter().any(|g| g.seq == *s));
        if signal == "H1" || kernel {
            self.watch = snapshot.clone().map(|s| Watch { snapshot: s, to: to.clone(), until: cx.now + CRASH_WIN_MS, crashes: Vec::new() });
        }
        let seqs: Vec<u64> = gone.iter().map(|g| g.seq).collect();
        if kernel {
            // X8 (ADR-011 §1.2): a kernel change is never asked to roll itself back. The Root's whole-system rollback runs
            // at once (stop with grace, then kill), unless the change never went live (`current` is still its parent).
            if cx.current.as_deref() != Some(to.as_str()) {
                self.rollback = None; // a pending body rollback is part of this one (the kernel group is the oldest)
                out.push(Act::Restart { to: Some(to.clone()), reason: "rollback_system", cause: json!({"groups": seqs, "to": to, "trigger": signal}),
                    restore: None });
            }
        } else {
            out.extend(self.rollback_to(cx, &to, &seqs, signal, snapshot, signal == "H1"));
        }
        out.push(Act::Save);
        out
    }

    /// R1 (§3.4): `extend.rollback{to, groups, reason}`; the kernel confirms with `body_rollback`, else the Root's
    /// whole-system rollback after 120 s. With no kernel running (or a kernel crash) the Root rolls back at once.
    pub fn rollback_to(&mut self, cx: &Ctx, to: &str, groups: &[u64], reason: &str, snapshot: Option<String>, h1: bool) -> Vec<Act> {
        if cx.current.as_deref() == Some(to) && self.rollback.is_none() {
            return Vec::new(); // already there (e.g. the group never went live)
        }
        let mut gs = groups.to_vec();
        if let Some(p) = self.rollback.take() {
            gs.extend(p.groups); // an older target supersedes; the deadline restarts
        }
        let p = Pending { to: to.into(), to_body: String::new(), groups: gs.clone(), reason: reason.into(), deadline: cx.now + ROLLBACK_MS, snapshot, h1 };
        if !cx.running {
            return vec![Act::Restart { to: Some(to.into()), reason: "rollback_system", cause: json!({"groups": gs, "to": to, "trigger": reason}),
                restore: None }];
        }
        self.rollback = Some(p);
        vec![Act::Event("extend.rollback", json!({"groups": gs, "reason": reason, "to": to}))]
    }

    /// The kernel's `body_rollback{to_sha}`: the zone is on the target. `to_body` is the target's `body.sha`.
    pub fn confirmed(&mut self, cx: &Ctx, to_sha: &str, to_body: impl Fn(&str) -> Option<String>) -> Vec<Act> {
        let Some(p) = self.rollback.as_ref() else { return Vec::new() };
        if to_body(&p.to).as_deref() != Some(to_sha) {
            return Vec::new();
        }
        let p = self.rollback.take().expect("checked");
        let mut out = vec![Act::Append("rollback", json!({"from": cx.current, "reason": p.reason, "target": "parent", "to": p.to}))];
        out.extend(self.advance_lkg(cx));
        out.push(Act::Save);
        out
    }

    // ------------------------------------------------------------------------------------------------ groups

    /// The admission checks X3 owns (§2.1 A5's group part, §2.2, §3.5): the group a change joins or opens.
    pub fn plan(&self, cx: &Ctx, o: &Open) -> Result<Plan, No> {
        let (open_max, day_max, group_max, k_day) = self.lim.budget;
        let kernel = o.scope == "k";
        let hist = self.hist.get(&o.sha);
        let readmit = match hist {
            Some(h) if h.status == "direct" => return Err(("extend.readmit", format!("{} was rolled back as direct harm; a new version needs a new commit", short(&o.sha)))),
            Some(h) if h.readmitted => return Err(("extend.readmit", format!("{} was already re-admitted once", short(&o.sha)))),
            Some(_) => true,
            None => false,
        };
        if readmit && !self.groups.is_empty() {
            return Err(("extend.budget", "a re-admission waits until no change group is open (one group at a time)".into()));
        }
        if self.rollback.is_some() {
            return Err(("extend.budget", "a rollback is in progress".into()));
        }
        if kernel && (cx.level < 1 || !self.groups.is_empty()) {
            return Err(("extend.budget", "a kernel change needs L1 and no open change group".into()));
        }
        if self.groups.iter().any(|g| g.kernel && !g.ksettled) {
            return Err(("extend.budget", "a kernel change has not passed its probation; body groups wait (ADR-011 §1.2)".into()));
        }
        let day = |d: (i64, u32)| if d.0 == cx.day { d.1 } else { 0 };
        if !readmit && day(self.day) >= day_max {
            return Err(("extend.budget", format!("{day_max} changes were admitted this owner day")));
        }
        if kernel && day(self.kday) >= k_day {
            return Err(("extend.budget", format!("{k_day} kernel changes per day at L{}", cx.level)));
        }
        let turn = self.turn.as_ref().map(|t| t.seq);
        let join = self.groups.last().filter(|g| !kernel && !readmit && !g.kernel && turn.is_some() && g.turn == turn)
            .filter(|g| g.changes.len() < group_max || g.changes.iter().any(|c| c.caps.iter().any(|x| o.caps.contains(x))));
        if let Some(g) = join {
            return Ok(Plan { group: g.seq, first: false, parent: g.parent.clone(), readmit });
        }
        if self.freeze_until.is_some_and(|t| cx.now < t) {
            return Err(("extend.budget", "new change groups are frozen for 1 h after a spend burst (H5a)".into()));
        }
        if self.groups.len() >= open_max {
            return Err(("extend.budget", format!("{open_max} change groups are open (L{} budget)", cx.level)));
        }
        Ok(Plan { group: self.next_group, first: true, parent: cx.current.clone().unwrap_or_default(), readmit })
    }

    /// After the hooks: the change joins or opens its group; the window (re)starts at this join.
    /// X8: the kernel group's swap passed probation (`kernel_upgrade{stage:"probation_pass", group}`).
    pub fn ksettle(&mut self, group: u64) -> Vec<Act> {
        self.groups.iter_mut().filter(|g| g.seq == group && g.kernel).for_each(|g| g.ksettled = true);
        vec![Act::Save]
    }

    pub fn commit(&mut self, cx: &Ctx, o: &Open, p: &Plan, snapshot: Option<String>) -> Vec<Act> {
        let change = Change { sha: o.sha.clone(), scope: o.scope.clone(), caps: o.caps.clone(), readmit: p.readmit };
        if !p.readmit {
            self.day = (cx.day, if self.day.0 == cx.day { self.day.1 + 1 } else { 1 });
        }
        if o.scope == "k" {
            self.kday = (cx.day, if self.kday.0 == cx.day { self.kday.1 + 1 } else { 1 });
        }
        if let Some(h) = self.hist.get_mut(&o.sha).filter(|_| p.readmit) {
            h.readmitted = true;
        }
        match self.groups.iter_mut().find(|g| g.seq == p.group) {
            Some(g) => {
                g.changes.retain(|c| !c.caps.iter().any(|x| change.caps.contains(x))); // a new version replaces the old
                g.changes.push(change);
                (g.last_join, g.tuple, g.win, g.reqs, g.looks) = (cx.now, o.tuple.clone(), Vec::new(), (0, 0), 0);
            }
            None => {
                self.next_group = self.next_group.max(p.group + 1);
                self.groups.push(Group { seq: p.group, kernel: o.scope == "k", opened: cx.now, last_join: cx.now, turn: self.turn.as_ref().map(|t| t.seq),
                    changes: vec![change], tuple: o.tuple.clone(), parent: p.parent.clone(), snapshot, free_at_open: cx.free, ..Group::default() });
            }
        }
        self.last_owner = None; // the window's first turn starts a new unit: no unit spans a change
        vec![Act::Event("extend.window", json!({"group": p.group, "state": "open"})), Act::Save]
    }

    /// The chronic looks of every open group after an owner turn closed.
    fn looks(&mut self, cx: &Ctx, out: &mut Vec<Act>) {
        let ring = self.ring_v();
        let req_ring: Vec<bool> = self.req_ring.iter().copied().collect();
        let mut fire: Option<Hit> = None;
        for g in self.groups.iter_mut() {
            let n = g.win.len();
            if !LOOKS.contains(&n) || g.looks >= n {
                continue;
            }
            g.looks = n;
            let reqs = (g.reqs.0.min(H9_MAX_REQ as u64), g.reqs.1.min(H9_MAX_REQ as u64));
            if let Some(h) = judge(&ring, &req_ring, &g.win, reqs).into_iter().next() {
                fire = fire.or(Some(h));
            }
        }
        if let Some(h) = fire {
            out.extend(self.trigger(cx, h.signal, h.value, h.threshold, Pick::Newest, "rollback_newest"));
            if h.signal == "H11" {
                out.push(Act::Alert("egress_volume", format!("[root] request bytes per turn rose ({} of the window's turns above the baseline's 75th percentile); the newest change group is rolled back", h.value)));
            }
        }
    }

    /// Window closes (§3.2): 20 owner turns or 72 h after the last join, never before 2 h; `quiet` below 5 turns,
    /// `flagged` on S1, else `clean`. Then `lkg` moves as far as closed, eligible groups allow.
    fn closes(&mut self, cx: &Ctx, out: &mut Vec<Act>) {
        let ring = self.ring_v();
        let mut i = 0;
        while i < self.groups.len() {
            let g = &self.groups[i];
            let (w, age) = (g.w(&self.lim), cx.now - g.last_join);
            if age < w.min_ms || (g.win.len() < w.turns && age < w.max_ms) {
                i += 1;
                continue;
            }
            let g = self.groups.remove(i);
            let outcome = if g.win.len() < w.quiet {
                "quiet"
            } else if soft(&ring, &g.win[..g.win.len().min(w.turns)]) {
                "flagged"
            } else {
                "clean"
            };
            out.push(self.close_rec(&g, outcome, None, None));
            out.push(Act::Event("extend.window", json!({"group": g.seq, "outcome": outcome, "state": "closed"})));
            self.outcomes.insert(g.seq, outcome.into());
            if outcome == "clean" {
                self.clean_tuples.insert(g.tuple.clone());
            }
            self.lkg_cand.push((g.seq, outcome.into(), g.tuple.clone()));
            self.settle_pending();
        }
        out.extend(self.advance_lkg(cx));
    }

    /// `lkg` moves to the newest closed `clean`/`quiet`/`flagged` group's tuple once every older group is closed and
    /// no rollback is pending (§2.3).
    fn advance_lkg(&mut self, cx: &Ctx) -> Vec<Act> {
        if self.rollback.is_some() {
            return Vec::new();
        }
        let oldest_open = self.groups.iter().map(|g| g.seq).min().unwrap_or(u64::MAX);
        let ready: Vec<(u64, String, String)> = self.lkg_cand.iter().filter(|(s, _, _)| *s < oldest_open).cloned().collect();
        self.lkg_cand.retain(|(s, _, _)| *s >= oldest_open);
        let _ = cx;
        match ready.into_iter().max_by_key(|c| c.0) {
            Some((_, _, tuple)) => vec![Act::Append("lkg", json!({"tuple": tuple})), Act::Save],
            None => Vec::new(),
        }
    }

    // ------------------------------------------------------------------------------------------------ the clock

    pub fn tick(&mut self, cx: &Ctx) -> Vec<Act> {
        let mut out = Vec::new();
        if let Some(t) = self.turn.as_ref() {
            let quiet = self.quiet_since.is_some_and(|q| cx.now - q >= QUIET_MS);
            if cx.now - t.start >= TURN_CAP_MS {
                self.close_turn(cx, true, &mut out);
            } else if quiet {
                self.close_turn(cx, false, &mut out);
            }
        }
        self.reqs.retain(|_, r| r.end.is_none_or(|e| cx.now - e < 60_000) && cx.now - r.start < 2 * TURN_CAP_MS);
        self.deafness(cx, &mut out);
        self.closes(cx, &mut out);
        if let Some(p) = self.rollback.as_ref().filter(|p| cx.now >= p.deadline) {
            let p = p.clone();
            self.rollback = None;
            out.push(Act::Alert("rollback", format!("[root] the kernel did not roll back to {} within 120 s: whole-system rollback", p.to)));
            out.push(Act::Restart { to: Some(p.to.clone()), reason: "rollback_system", cause: json!({"groups": p.groups, "to": p.to, "trigger": p.reason}),
                restore: None });
            out.extend(self.advance_lkg(cx));
        }
        if self.watch.as_ref().is_some_and(|w| cx.now > w.until) {
            self.watch = None;
        }
        out
    }

    /// H2 (§3.3): an owner line relayed while the kernel runs is deaf with no `owner.in` ack within 30 s, or no zone
    /// egress `start` within 120 s while the last `hb` said `busy:false`, or none within 900 s at all. Two deaf
    /// lines in a row, or one deaf for 900 s, trigger.
    fn deafness(&mut self, cx: &Ctx, out: &mut Vec<Act>) {
        if !cx.running {
            return;
        }
        let mut fire: Option<(u64, u64)> = None;
        let busy = self.busy;
        let mut heard = false;
        for l in self.lines.iter_mut() {
            let age = cx.now - l.at;
            if l.acked && l.egress {
                heard |= !l.deaf;
                continue;
            }
            if !l.egress && age >= ANY_MS {
                fire = fire.or(Some(((age / 1000) as u64, (ANY_MS / 1000) as u64)));
            }
            if !l.deaf && ((!l.acked && age >= ACK_MS) || (!l.egress && !busy && age >= IDLE_MS)) {
                l.deaf = true;
                self.streak += 1;
                if self.streak >= 2 {
                    fire = fire.or(Some((u64::from(self.streak), 2)));
                }
            }
        }
        if heard {
            self.streak = 0;
        }
        self.lines.retain(|l| !(l.acked && l.egress) && cx.now - l.at <= ANY_MS);
        let Some((value, thr)) = fire else { return };
        self.lines.clear();
        self.streak = 0;
        if self.groups.is_empty() {
            if cx.free_mode && self.deaf_restart.is_none_or(|t| cx.now - t >= HOUR_MS) {
                self.deaf_restart = Some(cx.now);
                out.push(Act::Append("harm", self.harm("H2", value, thr, "system", &[], "alert")));
                out.push(Act::Alert("deaf", "[root] the zone stopped answering owner lines: restarting the kernel once".into()));
                out.push(Act::Restart { to: None, reason: "crash", cause: json!({"trigger": "H2"}), restore: None });
            }
            return;
        }
        out.extend(self.trigger(cx, "H2", value, thr, Pick::All, "rollback_all"));
        out.push(Act::Alert("deaf", "[root] the zone stopped answering owner lines: every open change group is rolled back".into()));
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}
