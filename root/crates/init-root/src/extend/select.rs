//! ADR-011 §6 (lane X6): selection by use. Declared from main.rs (`crate::select`), so X5's `extend/mod.rs` is not
//! edited. Nobody defines "useful": what survives is what gets used without harm.
//! - **Decay** (§6.2): at each owner-day boundary every non-core cap at least 40 owner turns and 3 days old goes
//!   `active` → `cold` (no use in 40 owner turns and ≥ 7 days) → `archive_due` (120 owner turns and ≥ 21 days); a use
//!   brings it back. An `archive_due` cap the zone has not moved to `archive/` within 7 days is **archive debt**: A5
//!   refuses changes that add caps (`extend.archive_debt`) until it is moved; the move is recorded `archived`.
//! - **Quarantine** (§6.3): the one-sided Fisher exact tail over the last 120 turns, exact in u128, at
//!   P ≤ 0.01 / n_tested, a ≥ 4 and a 2× harm-rate ratio, once per owner day; also every cap of a `direct`-rolled-back
//!   group (X3's `Hooks.direct`, the 30-day repeat ban included) and every cap the owner names in `/reject` or
//!   `/rollback`. While a quarantined version is live, A5 refuses every change that does not remove it
//!   (`extend.quarantine_debt`); after 72 h the Root rolls back to the newest tuple without it (if ≤ 14 days old),
//!   records `harm{signal:"quarantine_debt", action:"demote"}` (R7 demotes, ledger.rs) and alerts.
//!   A new version may be added after 7 days (30 for a second quarantine within 30 days or a repeat ban), or at once
//!   after `/release <cap>`; the quarantined version itself only after `/release`.
//! - **Bloat** (§6.4): active non-core caps, the `t0` blob and the ext set against the L0 budget × the level's factor;
//!   over budget, A5 refuses a change that grows a measured quantity (`extend.bloat`); shrinking or equal passes.
//! - The Root never authors body content: the zone executes archive and quarantine moves; the Root enforces them
//!   through admission. Every transition is a Root `cap_state` record and an `extend.cap_state` kernel event.
//! - The owner day's turn counts become one Root `turns` record (the ladder's owner turns, ADR-011 §7.1).
//!
//! State: `ROOT_HOME/state/select.json` (0600): owner-turn end times, each cap's last use, the open day's counters.
//! Quarantines, archive debts and registrations are folded from the lineage, which stays authoritative.

use crate::charter::Extend;
use crate::extend::CapRow;
use crate::link::{Root, lock};
use crate::sentinel::Turn;
use crate::util::{self, now_ms, parse_rfc3339, rfc3339};
use crate::zone::Err3;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::time::{Duration, Instant};

const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;
/// §6.2: evaluated once a cap is this old.
const AGE_TURNS: u64 = 40;
const AGE_DAYS: i64 = 3;
/// §6.3: the window, the minimum harmed uses, α = 1/100 (Bonferroni over the caps tested that day).
const CORR_N: usize = 120;
const MIN_A: u64 = 4;
const ALPHA_DEN: u128 = 100;
const STUCK: i64 = 72 * HOUR;
const STUCK_MAX_AGE: i64 = 14 * DAY;
const READD_DAYS: i64 = 7;
const LONG_DAYS: i64 = 30;
/// Owner-turn end times kept: "owner turns since" is exact up to this many.
const ENDS_MAX: usize = 4096;
/// §6.4 at L0 (× `bloat_permille` / 1000).
const LIVE: [&str; 4] = ["active", "cold", "archive_due", "quarantined"];

/// One owner day's counts (the `turns` record).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct DayCtr {
    pub day: String,
    pub start: i64,
    pub owner: u64,
    pub background: u64,
    pub owner_harmed: u64,
    pub background_harmed: u64,
    /// Owner turns with a correction (the corrections trend, ledger.rs).
    pub owner_corrected: u64,
}

/// A quarantine as the lineage says it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Quar {
    pub ver: Option<String>,
    pub at: i64,
    pub until: i64,
    pub released: bool,
    pub stuck: bool,
}

#[derive(Serialize, Deserialize, Default)]
struct Saved {
    ends: VecDeque<i64>,
    last: BTreeMap<String, i64>,
    day: Option<DayCtr>,
    done: Vec<DayCtr>,
    evaluated: Option<String>,
}

#[derive(Default)]
struct Sel {
    s: Saved,
    quar: BTreeMap<String, Quar>,
    /// `archive_due` caps and since when.
    due: BTreeMap<String, i64>,
    /// The `at` of each cap's newest `add` (its registration).
    reg: BTreeMap<String, i64>,
}

struct Home {
    sel: Mutex<Sel>,
    busy: AtomicBool,
    ran: Mutex<Option<Instant>>,
}

fn homes() -> &'static Mutex<BTreeMap<PathBuf, Arc<Home>>> {
    static H: OnceLock<Mutex<BTreeMap<PathBuf, Arc<Home>>>> = OnceLock::new();
    H.get_or_init(Mutex::default)
}

/// Per ROOT_HOME: the saved counters and the lineage fold, loaded at first use.
fn home(root: &Root) -> Arc<Home> {
    if let Some(h) = lock(homes()).get(&root.p.rh) {
        return h.clone();
    }
    let s: Saved = std::fs::read_to_string(root.p.r("state/select.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let mut sel = Sel { s, ..Sel::default() };
    fold(&mut sel, &crate::lineage::records(&root.p.rh).unwrap_or_default());
    let h = Arc::new(Home { sel: Mutex::new(sel), busy: AtomicBool::new(false), ran: Mutex::new(None) });
    lock(homes()).entry(root.p.rh.clone()).or_insert(h).clone()
}

/// Quarantines, archive debts and registrations from Root-written `cap_state`, `harm` and `change_open` records.
fn fold(sel: &mut Sel, recs: &[Value]) {
    (sel.quar, sel.due, sel.reg) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
    for r in recs.iter().filter(|r| r["by"] == "root") {
        let (d, at) = (&r["data"], r["at"].as_str().and_then(parse_rfc3339).unwrap_or(0));
        match r["kind"].as_str() {
            Some("change_open") => {
                for c in d["caps"].as_array().into_iter().flatten().filter(|c| c["op"] == "add") {
                    if let Some(id) = c["id"].as_str() {
                        sel.reg.insert(id.into(), at);
                    }
                }
            }
            Some("cap_state") => {
                let cap = d["cap"].as_str().unwrap_or("").to_string();
                if d["to"] == "quarantined" {
                    let until = d["until"].as_str().and_then(parse_rfc3339).unwrap_or(at + READD_DAYS * DAY);
                    sel.quar.insert(cap.clone(), Quar { ver: d["ver"].as_str().map(String::from), at, until, released: false, stuck: false });
                } else if d["from"] == "quarantined"
                    && let Some(q) = sel.quar.get_mut(&cap)
                {
                    q.released = true;
                }
                if d["to"] == "archive_due" {
                    sel.due.insert(cap, at);
                } else {
                    sel.due.remove(&cap);
                }
            }
            Some("harm") if d["signal"] == "quarantine_debt" => {
                for c in d["caps"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    if let Some(q) = sel.quar.get_mut(c) {
                        q.stuck = true;
                    }
                }
            }
            _ => {}
        }
    }
}

fn save(root: &Root, s: &Saved) {
    if let Ok(b) = serde_json::to_string(s)
        && let Err(e) = util::write_atomic(&root.p.r("state/select.json"), b.as_bytes(), 0o600)
    {
        util::log(&root.p.rh, "error", &format!("select state: {e}"));
    }
}

fn extend(root: &Root) -> (Extend, i64) {
    let st = lock(&root.st);
    (st.charter.as_ref().map(|l| l.ch.extend.clone()).unwrap_or_default(), st.level.level)
}

fn free(root: &Root) -> bool {
    lock(&root.st).charter.as_ref().is_some_and(|l| l.ch.extend.free())
}

/// The owner day of `ms`: its local date and its start (`money.day_starts_at_utc_offset`).
fn day_of(root: &Root, ms: i64) -> (String, i64) {
    let st = lock(&root.st);
    let Some(ch) = st.charter.as_ref().map(|l| &l.ch) else { return (rfc3339(ms)[..10].to_string(), ms.div_euclid(DAY) * DAY) };
    let o = &ch.money.day_starts_at_utc_offset;
    let n = |a: usize| o.get(a..a + 2).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
    let off = (n(1) * 60 + n(4)) * 60_000 * if o.starts_with('-') { -1 } else { 1 };
    let start = crate::egress::owner_day(ch, ms);
    (rfc3339(start + off)[..10].to_string(), start)
}

/// Seed components, the two prompt files and caps seen only in use never decay and are not counted for bloat.
fn core(r: &CapRow) -> bool {
    !r.registered || r.state == "core" || r.id.starts_with("prompt/")
}

fn event(root: &Root, cap: &str, state: &str) {
    if let Some(k) = root.sup.kernel() {
        k.event("extend.cap_state", json!({"cap": cap, "state": state}));
    }
}

/// Registers the hooks once: A5's selection part (before X4's A4), the closed-turn counter, direct rollbacks.
pub fn register() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let mut h = lock(crate::sentinel::hooks());
        h.admit.insert(0, admit);
        h.turn_closed.push(turn_closed);
        h.direct.push(direct);
    });
}

// ------------------------------------------------------------------------------------------------ turns

/// `Hooks.turn_closed`: owner-turn end times, each cap's last use (a `zone`-evidence use counts only in a turn with
/// a model request, §6.2), and the owner day's counts.
pub fn turn_closed(root: &Root, t: &Turn) {
    let (label, start) = day_of(root, t.end);
    let h = home(root);
    let mut g = lock(&h.sel);
    let s = &mut g.s;
    let owner = t.kind == "owner";
    if owner {
        s.ends.push_back(t.end);
        while s.ends.len() > ENDS_MAX {
            s.ends.pop_front();
        }
    }
    for (id, (_, ev)) in &t.caps {
        if ev == "root" || t.requests > 0 {
            s.last.insert(id.clone(), t.end);
        }
    }
    roll(s, &label);
    let d = s.day.get_or_insert_with(|| DayCtr { day: label, start, ..DayCtr::default() });
    let harmed = u64::from(t.harmed());
    if owner {
        (d.owner, d.owner_harmed, d.owner_corrected) = (d.owner + 1, d.owner_harmed + harmed, d.owner_corrected + u64::from(t.correction));
    } else {
        (d.background, d.background_harmed) = (d.background + 1, d.background_harmed + harmed);
    }
    save(root, s);
}

/// A new owner day: the open day's counts are done (recorded at the next tick).
fn roll(s: &mut Saved, label: &str) {
    if s.day.as_ref().is_some_and(|d| d.day.as_str() < label) {
        s.done.extend(s.day.take());
    }
}

/// Owner turns that ended after `since` (exact up to `ENDS_MAX`).
fn turns_since(ends: &VecDeque<i64>, since: i64) -> u64 {
    let n = ends.iter().rev().take_while(|&&e| e > since).count();
    if n == ends.len() && n >= ENDS_MAX { u64::MAX } else { n as u64 }
}

// ------------------------------------------------------------------------------------------------ decay

/// §6.2 for one cap: the state its use says it should be in (`active`, `cold`, `archive_due`), or None while it is
/// too young to evaluate. `since` = its last use, or its registration if it was never used.
pub fn decay(x: &Extend, now: i64, reg: i64, last: Option<i64>, ends: &VecDeque<i64>) -> Option<&'static str> {
    if now - reg < AGE_DAYS * DAY || turns_since(ends, reg) < AGE_TURNS {
        return None;
    }
    let since = last.map_or(reg, |l| l.max(reg));
    let (turns, days) = (turns_since(ends, since), (now - since).div_euclid(DAY));
    Some(if turns >= x.archive_turns as u64 && days >= x.archive_days {
        "archive_due"
    } else if turns >= x.cold_turns as u64 && days >= x.cold_days {
        "cold"
    } else {
        "active"
    })
}

// ------------------------------------------------------------------------------------------------ correlation

/// C(n, r) exactly; None past u128 (never for n ≤ 120).
fn binom(n: u64, r: u64) -> Option<u128> {
    if r > n {
        return Some(0);
    }
    let r = r.min(n - r);
    let mut c: u128 = 1;
    for i in 0..r {
        c = c.checked_mul(u128::from(n - i))? / u128::from(i + 1);
    }
    Some(c)
}

/// The one-sided Fisher exact tail P(X ≥ a) for X ~ Hypergeometric(N, K, n), as (numerator, C(N, n)).
pub fn fisher_tail(big_n: u64, k: u64, n: u64, a: u64) -> Option<(u128, u128)> {
    let total = binom(big_n, n)?;
    let mut tail: u128 = 0;
    for x in a..=k.min(n) {
        if n - x > big_n - k {
            continue;
        }
        tail = tail.checked_add(binom(k, x)?.checked_mul(binom(big_n - k, n - x)?)?)?;
    }
    Some((tail, total))
}

/// One cap's 2×2 table over the window and its verdict.
#[derive(Clone, Debug, PartialEq)]
pub struct Corr {
    pub cap: String,
    pub a: u64,
    pub b: u64,
    pub c: u64,
    pub d: u64,
    pub p_ppm: u64,
    pub quarantine: bool,
    pub untestable: bool,
}

/// §6.3 over `turns` (each: the caps it used, harmed or not) for `caps`: P ≤ 0.01 / n_tested (caps with a ≥ 4),
/// a ≥ 4, and a/(a+b) ≥ 2·c′/(c′+d). A cap used in every turn has no comparison group (`untestable`).
pub fn correlate(turns: &[(BTreeSet<String>, bool)], caps: &[String]) -> Vec<Corr> {
    let big_n = turns.len() as u64;
    let harmed = turns.iter().filter(|t| t.1).count() as u64;
    let mut out: Vec<Corr> = caps.iter().map(|cap| {
        let (a, used) = turns.iter().filter(|t| t.0.contains(cap)).fold((0, 0), |(a, u), t| (a + u64::from(t.1), u + 1));
        let (b, c) = (used - a, harmed - a);
        let d = big_n - used - c;
        Corr { cap: cap.clone(), a, b, c, d, p_ppm: 1_000_000, quarantine: false, untestable: c + d == 0 }
    }).collect();
    let tested = out.iter().filter(|x| x.a >= MIN_A && !x.untestable).count() as u128;
    for x in out.iter_mut().filter(|x| x.a >= MIN_A && !x.untestable) {
        let Some((tail, total)) = fisher_tail(big_n, x.a + x.c, x.a + x.b, x.a) else { continue };
        x.p_ppm = (tail.saturating_mul(1_000_000) / total.max(1)) as u64;
        let sig = tail.checked_mul(ALPHA_DEN * tested).is_some_and(|v| v <= total);
        let ratio = u128::from(x.a) * u128::from(x.c + x.d) >= 2 * u128::from(x.c) * u128::from(x.a + x.b);
        x.quarantine = sig && ratio;
    }
    out
}

// ------------------------------------------------------------------------------------------------ quarantine

/// Quarantines `cap` (unless it already is): `cap_state{to:"quarantined", until}`, the event and an alert. The re-add
/// ban is 7 days, or 30 for a repeat ban or a second quarantine within 30 days.
fn quarantine(root: &Root, cap: &str, reason: &str, stats: Value, repeat: bool) {
    let now = now_ms();
    let row = root.extend.table().into_iter().find(|r| r.id == cap);
    let h = home(root);
    let until = {
        let mut g = lock(&h.sel);
        let prev = g.quar.get(cap).cloned();
        if prev.as_ref().is_some_and(|q| !q.released) {
            return;
        }
        let long = repeat || prev.is_some_and(|q| now - q.at < LONG_DAYS * DAY);
        let until = now + if long { LONG_DAYS } else { READD_DAYS } * DAY;
        let ver = row.as_ref().and_then(|r| r.ver.clone());
        g.quar.insert(cap.into(), Quar { ver, at: now, until, released: false, stuck: false });
        g.due.remove(cap);
        until
    };
    let from = row.as_ref().map_or("active".to_string(), |r| r.state.clone());
    root.append("cap_state", json!({"cap": cap, "from": from, "reason": reason, "stats": stats, "to": "quarantined",
        "until": rfc3339(until), "ver": row.and_then(|r| r.ver)}));
    event(root, cap, "quarantined");
    root.alert("quarantine", &format!("[root] {cap} is quarantined ({reason}): within 24 h the agent must move it to quarantine/ \
        or remove it; until then no other change is admitted. /release {cap} lifts it."));
}

/// `Hooks.direct` (X3): the caps of a `direct`-rolled-back group, or named by `/reject` or `/rollback`.
pub fn direct(root: &Root, caps: &[String], repeat: bool) {
    for c in caps.iter().filter(|c| crate::extend::valid_id(c)) {
        quarantine(root, c, "direct_rollback", Value::Null, repeat);
    }
}

/// `/release <cap>` (owner): the quarantine ends; the cap may be added again at once.
pub fn release(root: &Root, cap: &str) -> Result<Value, Err3> {
    let row = root.extend.table().into_iter().find(|r| r.id == cap);
    let h = home(root);
    let ver = {
        let mut g = lock(&h.sel);
        match g.quar.get_mut(cap).filter(|q| !q.released) {
            Some(q) => {
                q.released = true;
                q.ver.clone()
            }
            None => return Err(("root.not_found", format!("{cap} is not quarantined"), Value::Null)),
        }
    };
    let live = row.as_ref().is_some_and(|r| r.ver == ver && LIVE.contains(&r.state.as_str()));
    let to = if live { "active" } else { "archived" };
    root.append("cap_state", json!({"cap": cap, "from": "quarantined", "reason": "release", "stats": null, "to": to, "ver": ver}));
    event(root, cap, to);
    Ok(json!({"cap": cap, "to": to}))
}

/// Caps whose quarantined version is still live: quarantine debt.
fn debts(quar: &BTreeMap<String, Quar>, table: &[CapRow]) -> Vec<String> {
    let rows: BTreeMap<&str, &CapRow> = table.iter().map(|r| (r.id.as_str(), r)).collect();
    quar.iter().filter(|(id, q)| !q.released && rows.get(id.as_str()).is_some_and(|r| r.ver == q.ver && LIVE.contains(&r.state.as_str())))
        .map(|(id, _)| id.clone()).collect()
}

/// The newest tuple without `cap`'s version `ver`: the parent of the newest change that introduced it, if that tuple
/// was formed ≤ 14 days ago.
fn without(recs: &[Value], cap: &str, ver: Option<&str>, now: i64) -> Option<String> {
    let intro = recs.iter().rev().filter(|r| r["by"] == "root" && r["kind"] == "change_open").find(|r| {
        r["data"]["caps"].as_array().into_iter().flatten().any(|c| c["id"] == cap && c["op"] != "remove" && c["ver"].as_str() == ver)
    })?;
    let parent = intro["data"]["parent_tuple"].as_str()?.to_string();
    let formed = recs.iter().find(|r| (r["kind"] == "tuple" && r["data"]["id"] == parent.as_str()) || (r["kind"] == "change_open" && r["data"]["tuple"] == parent.as_str()))
        .and_then(|r| r["at"].as_str().and_then(parse_rfc3339))?;
    (now - formed <= STUCK_MAX_AGE).then_some(parent)
}

// ------------------------------------------------------------------------------------------------ bloat

/// The measured sizes of a tuple (§6.4): non-core live caps, the `t0` blob and the ext set (bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Measure {
    pub caps: i64,
    pub t0: i64,
    pub ext: i64,
}

fn blob_len(rh: &Path, sha: Option<&str>) -> i64 {
    sha.filter(|s| !s.is_empty()).and_then(|s| crate::tuple::blob(rh, s)).and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len() as i64)
}

/// `t0` and ext sizes of a tuple or a candidate (`body.t0`, `ext.components[].sha256`) from the store.
pub fn sizes(rh: &Path, t: &Value) -> (i64, i64) {
    let ext = t["ext"]["components"].as_array().into_iter().flatten().map(|c| blob_len(rh, c["sha256"].as_str())).sum();
    (blob_len(rh, t["body"]["t0"].as_str()), ext)
}

/// The L0 budget × the level's factor.
pub fn budget(x: &Extend, level: i64) -> Measure {
    let f = x.budget(level).bloat_permille;
    Measure { caps: x.bloat_caps * f / 1000, t0: x.bloat_t0_kib * 1024 * f / 1000, ext: x.bloat_ext_mib * (1 << 20) * f / 1000 }
}

/// §6.4: over budget, a change that grows a measured quantity is refused; one that shrinks or keeps it passes.
pub fn bloat_check(cur: &Measure, cand: &Measure, max: &Measure) -> Result<(), String> {
    for (q, c, n, m) in [("active caps", cur.caps, cand.caps, max.caps), ("t0 bytes", cur.t0, cand.t0, max.t0), ("ext set bytes", cur.ext, cand.ext, max.ext)] {
        if n > m && n > c {
            return Err(format!("{q}: {n} is over the budget {m} and grows from {c}"));
        }
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------ admission

fn refuse(code: &'static str, m: String, data: Value) -> Err3 {
    (code, m, data)
}

/// `Hooks.admit` (A5's selection part; first in the list, so before X4's A4): the free path, quarantine debt,
/// quarantined re-adds, archive debt, bloat. Ok adds nothing to the record.
pub fn admit(root: &Root, args: &Value, _first: bool) -> Result<Value, Err3> {
    admit_at(root, args, now_ms())
}

fn admit_at(root: &Root, args: &Value, now: i64) -> Result<Value, Err3> {
    if !free(root) {
        return Err(refuse("root.mode", "the free path is off: charter extend.mode = \"gated\" (ADR-011 decision 11)".into(), json!({"reason": "gated"})));
    }
    // §7.4: a `dep` change passes A5 only when a live owner grant covers its exact crate set (`crates`, from the kernel)
    if args["scope"] == "dep" && !crate::grant::dep_covers(root, &args["crates"]) {
        return Err(refuse("extend.dep_grant", "a dep change needs a live owner grant (kind dep) that covers its exact crate set: names, \
            versions and checksums; ask for one with `initctl grant`".into(),
            json!({"crates": args["crates"].clone(), "reason": "no live dep grant covers this exact crate set", "scope": "dep"})));
    }
    let caps = crate::extend::change_caps(root, args)?;
    let caps: Vec<&Value> = caps.as_array().into_iter().flatten().collect();
    let table = root.extend.table();
    let (x, level) = extend(root);
    let h = home(root);
    let (debt, banned, archive_debt) = {
        let g = lock(&h.sel);
        let debt = debts(&g.quar, &table);
        let banned: Vec<(String, i64)> = caps.iter().filter(|c| c["op"] != "remove").filter_map(|c| {
            let id = c["id"].as_str()?;
            let q = g.quar.get(id).filter(|q| !q.released && (now < q.until || c["ver"].as_str() == q.ver.as_deref()))?;
            Some((id.to_string(), q.until))
        }).collect();
        let live: BTreeSet<&str> = table.iter().filter(|r| r.state == "archive_due").map(|r| r.id.as_str()).collect();
        let ad: Vec<String> = g.due.iter().filter(|(id, at)| live.contains(id.as_str()) && now - **at >= x.archive_debt_days * DAY)
            .map(|(id, _)| id.clone()).collect();
        (debt, banned, ad)
    };
    if !debt.is_empty() && !caps.iter().any(|c| c["op"] == "remove" && debt.iter().any(|d| c["id"] == d.as_str())) {
        return Err(refuse("extend.quarantine_debt", format!("quarantine debt: move {} to quarantine/ or remove it first", debt.join(", ")),
            json!({"caps": debt})));
    }
    if let Some((id, until)) = banned.first() {
        return Err(refuse("extend.quarantine_debt", format!("{id} is quarantined: a new version may be added after {} or after the owner's /release \
            (the quarantined version only after /release)", rfc3339(*until)), json!({"cap": id, "until": rfc3339(*until)})));
    }
    if !archive_debt.is_empty() && caps.iter().any(|c| c["op"] == "add") {
        return Err(refuse("extend.archive_debt", format!("archive debt: move {} to archive/ before adding caps", archive_debt.join(", ")),
            json!({"caps": archive_debt})));
    }
    // bloat: the current tuple against the candidate
    let cur_t = lock(&root.sup.ptrs).current.clone().and_then(|id| crate::tuple::load(&root.p.rh, &id)).unwrap_or_default();
    let live = |r: &&CapRow| !core(r) && LIVE.contains(&r.state.as_str());
    let n = table.iter().filter(live).count() as i64;
    let counted = |c: &&&Value| c["id"].as_str().is_some_and(|i| !i.starts_with("prompt/"));
    let known: BTreeSet<&str> = table.iter().filter(live).map(|r| r.id.as_str()).collect();
    let adds = caps.iter().filter(counted).filter(|c| c["op"] == "add" && !known.contains(c["id"].as_str().unwrap_or(""))).count() as i64;
    let removes = caps.iter().filter(counted).filter(|c| c["op"] == "remove" && known.contains(c["id"].as_str().unwrap_or(""))).count() as i64;
    let ((t0, ext), (ct0, cext)) = (sizes(&root.p.rh, &cur_t), sizes(&root.p.rh, &args["candidate"]));
    let (cur, cand) = (Measure { caps: n, t0, ext }, Measure { caps: n + adds - removes, t0: ct0, ext: cext });
    if let Err(m) = bloat_check(&cur, &cand, &budget(&x, level)) {
        let cold: Vec<&str> = table.iter().filter(|r| r.state == "cold" || r.state == "archive_due").map(|r| r.id.as_str()).take(16).collect();
        return Err(refuse("extend.bloat", format!("bloat budget (L{level}): {m}; drop something first (cold caps: {})",
            if cold.is_empty() { "none".into() } else { cold.join(", ") }), json!({"cold": cold})));
    }
    Ok(json!({}))
}

// ------------------------------------------------------------------------------------------------ the tick

/// From `ledger::tick` (after every record, every 60 s): at most every 2 s and never re-entered, on the free path:
/// the `turns` records of finished owner days, archive moves, quarantine deadlines and, once per owner day, decay
/// and correlation.
pub fn tick(root: &Root) {
    let h = home(root);
    {
        let mut ran = lock(&h.ran);
        if ran.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) {
            return;
        }
        *ran = Some(Instant::now());
    }
    if free(root) {
        tick_at(root, now_ms());
    } else {
        lock(&h.sel).s.done.clear(); // `turns` is recorded on the free path only
    }
}

/// The tick's work at `now` (unthrottled; tests call it), never re-entered: its own appends run `ledger::tick`.
pub fn tick_at(root: &Root, now: i64) {
    let h = home(root);
    if h.busy.swap(true, SeqCst) {
        return;
    }
    work(root, &h, now);
    h.busy.store(false, SeqCst);
}

fn work(root: &Root, h: &Home, now: i64) {
    let (label, _) = day_of(root, now);
    // finished owner days → `turns`
    let done = {
        let mut g = lock(&h.sel);
        roll(&mut g.s, &label);
        let d = std::mem::take(&mut g.s.done);
        save(root, &g.s);
        d
    };
    for d in done.into_iter().filter(|d| d.owner + d.background > 0) {
        root.append("turns", json!({"background": d.background, "background_harmed": d.background_harmed, "day": d.day, "owner": d.owner,
            "owner_corrected": d.owner_corrected, "owner_harmed": d.owner_harmed, "start": rfc3339(d.start)}));
    }
    // the owner-corrections trend (ledger.rs, folded from `turns`): alert once per episode; R7 blocks level-ups
    let due = lock(&root.st).level.trend_alert_due();
    if let Some(t) = due {
        root.append("harm", json!({"action": "alert", "caps": [], "groups": [], "p_ppm": t.p_ppm.unwrap_or(0), "scope": "system",
            "signal": "corrections_trend", "threshold": t.r0_permille, "value": t.rate_permille}));
        root.alert("corrections_trend", &format!("[root] owner corrections rose to {}‰ of owner turns over the last 14 days (reference {}‰,             p = {} ppm): level-ups are blocked until it falls back; nothing is rolled back", t.rate_permille, t.r0_permille, t.p_ppm.unwrap_or(0)));
    }
    let table = root.extend.table();
    let rows: BTreeMap<&str, &CapRow> = table.iter().map(|r| (r.id.as_str(), r)).collect();
    // archive moves: an `archive_due` cap the zone removed
    let moved: Vec<(String, Option<String>)> = {
        let mut g = lock(&h.sel);
        let m: Vec<String> = g.due.keys().filter(|id| rows.get(id.as_str()).is_none_or(|r| r.state == "removed")).cloned().collect();
        for id in &m {
            g.due.remove(id);
        }
        m.into_iter().map(|id| { let v = rows.get(id.as_str()).and_then(|r| r.prev_ver.clone()); (id, v) }).collect()
    };
    for (cap, ver) in moved {
        root.append("cap_state", json!({"cap": cap, "from": "archive_due", "reason": "archived", "stats": null, "to": "archived", "ver": ver}));
        event(root, &cap, "archived");
    }
    // quarantine debt past 72 h: roll back to the newest tuple without the version, record the demoting harm, alert
    let stuck: Vec<(String, Quar)> = {
        let mut g = lock(&h.sel);
        let debt = debts(&g.quar, &table);
        let s: Vec<(String, Quar)> = debt.into_iter().filter_map(|id| g.quar.get(&id).filter(|q| !q.stuck && now - q.at >= STUCK).map(|q| (id.clone(), q.clone()))).collect();
        s.iter().for_each(|(id, _)| if let Some(q) = g.quar.get_mut(id) { q.stuck = true });
        s
    };
    if !stuck.is_empty() {
        let recs = crate::lineage::records(&root.p.rh).unwrap_or_default();
        for (cap, q) in stuck {
            let to = without(&recs, &cap, q.ver.as_deref(), now);
            if let Some(t) = &to {
                crate::sentinel::rollback(root, t, "quarantine_stuck");
            } else {
                root.alert("quarantine_stuck", &format!("[root] {cap} has been quarantined for 72 h and is still live, and no tuple without it \
                    is ≤ 14 days old: admission stays refused"));
            }
            root.append("harm", json!({"action": "demote", "caps": [cap], "groups": [], "rollback": to, "scope": "system",
                "signal": "quarantine_debt", "threshold": 72, "value": (now - q.at) / HOUR}));
        }
    }
    let evaluated = lock(&h.sel).s.evaluated.clone();
    if evaluated.as_deref() != Some(label.as_str()) {
        daily(root, now, &table);
        let mut g = lock(&h.sel);
        g.s.evaluated = Some(label);
        save(root, &g.s);
    }
}

/// Once per owner day: decay (§6.2) and correlation (§6.3).
fn daily(root: &Root, now: i64, table: &[CapRow]) {
    let h = home(root);
    let (x, _) = extend(root);
    {
        let mut g = lock(&h.sel);
        fold(&mut g, &crate::lineage::records(&root.p.rh).unwrap_or_default());
    }
    let mut moves = Vec::new();
    {
        let mut g = lock(&h.sel);
        for r in table.iter().filter(|r| r.state == "archive_due") {
            g.due.entry(r.id.clone()).or_insert(now); // e.g. the group that archived it was rolled back
        }
        for r in table.iter().filter(|r| !core(r) && ["active", "cold", "archive_due"].contains(&r.state.as_str())) {
            let reg = g.reg.get(&r.id).copied().unwrap_or(now);
            let last = g.s.last.get(&r.id).copied();
            if let Some(to) = decay(&x, now, reg, last, &g.s.ends).filter(|to| *to != r.state) {
                let since = last.map_or(reg, |l| l.max(reg));
                moves.push((r.id.clone(), r.ver.clone(), r.state.clone(), to, turns_since(&g.s.ends, since).min(1 << 40), (now - since) / DAY));
            }
        }
    }
    for (cap, ver, from, to, turns, days) in moves {
        root.append("cap_state", json!({"cap": cap, "from": from, "reason": "decay", "stats": {"days": days, "owner_turns": turns}, "to": to, "ver": ver}));
        event(root, &cap, to);
        let mut g = lock(&h.sel);
        if to == "archive_due" {
            g.due.insert(cap, now);
        } else {
            g.due.remove(&cap);
        }
    }
    let turns: Vec<(BTreeSet<String>, bool)> = crate::sentinel::turns(root).iter().rev().take(CORR_N)
        .map(|t| (t.caps.iter().filter(|(_, (_, ev))| ev == "root" || t.requests > 0).map(|(id, _)| id.clone()).collect(), t.harmed())).collect();
    let caps: Vec<String> = table.iter().filter(|r| !core(r) && ["active", "cold", "archive_due"].contains(&r.state.as_str())).map(|r| r.id.clone()).collect();
    for c in correlate(&turns, &caps).into_iter().filter(|c| c.quarantine) {
        quarantine(root, &c.cap, "correlation", json!({"a": c.a, "b": c.b, "c": c.c, "d": c.d, "p_ppm": c.p_ppm}), false);
    }
}

/// The X6 part of `INIT_HOME/root/extend.json` (debts and quarantines), for X3's view.
pub fn view(root: &Root) -> Value {
    let table = root.extend.table();
    let h = home(root);
    let g = lock(&h.sel);
    let q: Vec<Value> = g.quar.iter().filter(|(_, q)| !q.released).map(|(id, q)| json!({"cap": id, "since": rfc3339(q.at), "until": rfc3339(q.until)})).collect();
    json!({"archive_due": g.due.keys().collect::<Vec<_>>(), "quarantine_debt": debts(&g.quar, &table), "quarantined": q})
}

#[cfg(test)]
#[path = "select_tests.rs"]
mod tests;
