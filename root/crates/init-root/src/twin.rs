//! ADR-011 Evaluation plan (lane X9): the frozen twin and the nightly shadow replay. **Report-only**: nothing here
//! admits, rolls back, quarantines or sets a level. It writes `twin` (the pointer), `twin_report` (weekly
//! aggregates; R7 reads its `regression` flag, ledger.rs) and `INIT_HOME/root/twin.json`.
//!
//! - **The twin** is the `lkg` tuple when `extend.mode` first becomes `"free"`, with a system snapshot of the agent
//!   state taken then (snapshot.rs; `keep_set` pins it by id). Only the owner re-freezes it (`/twin refreeze`, with
//!   the typed confirmation, on `owner.sock`); the Root suggests a re-freeze after 90 days. No control-link request,
//!   grant or record moves it.
//! - **The replay store** `ROOT_HOME/extend/replay/<id>/` (0700; never mirrored, never in the lineage, 7 days, at
//!   most 64): the first line of an owner session that has a workspace snapshot (ADR-009's `session` snapshot, the
//!   state that turn started from), an APFS clone of that snapshot, and the caps its turn used.
//! - **The night**: in the off-peak window, up to `extend.twin.k` stored tasks (favouring tasks whose turns used
//!   caps that are flagged, new since the last report, or near decay) each run in two Root-owned sealed run roots
//!   (sealed/run.rs; the sealed profile, no kernel, no `INIT_HOME`, no IP) on sealed egress lanes: *current* = the
//!   `lkg` tuple on its newest system snapshot, *twin* = the twin tuple on its frozen state, both on the current
//!   model, in random order. Each arm gets an APFS clone of the workspace snapshot and the original line, and runs
//!   until it stops by itself or hits its caps. Run roots and lane sockets are removed by exact path.
//! - **Money** (observe-only, owner policy 2026-10-23): USD and tokens remain in the durable
//!   ledger by run/night. Legacy dollar allowances do not stop either arm; time, request,
//!   window and user cancellation gates remain active. External benchmark caps are separate.
//! - **The window** is `extend.twin.window_utc` (default 17:00–00:30 UTC).
//! - **Statistics** (exact integers, sentinel/stats.rs): McNemar exact on `ended`, the exact sign test on cost and
//!   wall, medians of the per-task ratios; p-values are rounded **up** to ppm, so `p_ppm ≤ 50000` ⇔ p ≤ 0.05.

use crate::charter::{self, Charter};
use crate::link::{Root, lock};
use crate::memwatch::{self, Limits};
use crate::sealed::{Env, RunRoot};
use crate::sentinel::stats::{Big, Tail, binom_tail, median2};
use crate::sup::Slot;
use crate::util::{self, icj, now_ms, rfc3339, write_atomic};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const DAY: i64 = 86_400_000;
const KEEP_MS: i64 = 7 * DAY;
const STORE_MAX: usize = 64;
const PAIRS_KEEP_MS: i64 = 120 * DAY;
/// "No regression" (§7.2): the last 14 nights, at least 40 pairs, `ended` not worse at p ≤ 0.05, median cost ≤ 1.5×.
const REG_NIGHTS: i64 = 14;
const REG_PAIRS: usize = 40;
const REG_COST_PERMILLE: u64 = 1500;
pub const SIG_PPM: u64 = 50_000;
const SIG: (u64, u64) = (5, 100);
/// The signed-rank test's exact u128 null holds up to 120 pairs (k ≤ 8 a night never reaches it).
const SR_MAX: usize = 120;
const REFREEZE_MS: i64 = 90 * DAY;
const TICK_MS: i64 = 30_000;
/// The lineage-reading work (retention, the re-freeze suggestion, the weekly report) runs every 10 minutes.
const SLOW_MS: i64 = 600_000;
/// A night is skipped below the workload disk floor plus 2 GiB.
const ROOM: u64 = 2 << 30;

static LAST_TICK: AtomicI64 = AtomicI64::new(i64::MIN);
static LAST_SLOW: AtomicI64 = AtomicI64::new(i64::MIN);
static RUNNING: AtomicBool = AtomicBool::new(false);
/// The owner session waiting for its first line, per ROOT_HOME: (workspace snapshot id).
static PENDING: Mutex<BTreeMap<PathBuf, String>> = Mutex::new(BTreeMap::new());

/// `[extend]` and `[extend.twin]` of the effective charter (grants cannot name the twin, grant.rs).
#[derive(Clone, Debug, PartialEq)]
pub struct Cfg {
    pub free: bool,
    pub enabled: bool,
    pub k: usize,
    pub day_musd: u64,
    pub run_musd: u64,
    pub run_requests: u64,
    pub run_ms: u64,
    /// `extend.twin.window_utc` (`charter::twin_window`): the off-peak window, ms from 00:00 UTC of the night's date.
    /// A pair starts only if both arms can finish inside it.
    pub win: (i64, i64),
}

/// The default off-peak window (`charter::TWIN_WINDOW_UTC`, 17:00–00:30 UTC): inside DeepSeek's discounted span every
/// day (weekdays 10:15–00:45 with egress.md's margins, weekends all day), and 00:00–07:30 for an owner at +07:00.
pub const WIN: (i64, i64) = (17 * 3_600_000, DAY + 1_800_000);

impl Cfg {
    pub fn of(ch: &Charter) -> Cfg {
        let t = &ch.extend.twin;
        Cfg { free: ch.extend.free(), enabled: t.enabled, k: t.k.clamp(0, 24) as usize, day_musd: charter::musd(&t.usd_day).unwrap_or(0),
            run_musd: charter::musd(&t.run_usd).unwrap_or(0), run_requests: t.run_requests.max(0) as u64, run_ms: t.run_minutes.max(0) as u64 * 60_000,
            win: charter::twin_window(&t.window_utc).unwrap_or(WIN) }
    }
}

/// `HH:MM-HH:MM` of a window (for `twin.json`).
fn win_text((a, b): (i64, i64)) -> String {
    let hm = |x: i64| format!("{:02}:{:02}", x.rem_euclid(DAY) / 3_600_000, x.rem_euclid(3_600_000) / 60_000);
    format!("{}-{}", hm(a), hm(b))
}

fn cfg_of(root: &Root) -> Option<Cfg> {
    lock(&root.st).charter.as_ref().map(|l| Cfg::of(&l.ch))
}

/// The twin as its last `twin` record names it.
#[derive(Clone, Debug, PartialEq)]
pub struct Pin {
    pub tuple: String,
    pub snapshot: Option<String>,
    pub at: i64,
    pub reason: String,
}

pub fn pin_of(records: &[Value]) -> Option<Pin> {
    let r = records.iter().rev().find(|r| r["kind"] == "twin")?;
    let d = &r["data"];
    Some(Pin { tuple: d["tuple"].as_str()?.to_string(), snapshot: d["snapshot"].as_str().map(String::from),
        at: r["at"].as_str().and_then(util::parse_rfc3339).unwrap_or(0), reason: d["reason"].as_str().unwrap_or("").to_string() })
}

/// Freezes the `lkg` tuple with a fresh system snapshot and appends `twin{tuple, snapshot, reason}`. On `enable` a
/// failed snapshot still freezes the tuple (alerted: the twin runs on an empty state); an owner re-freeze is refused.
fn freeze(root: &Root, reason: &str) -> Result<Pin, String> {
    let lkg = lock(&root.sup.ptrs).lkg.clone().ok_or("there is no lkg tuple yet")?;
    let snapshot = match crate::snapshot::take(root, None) {
        Ok(v) => v["id"].as_str().map(String::from),
        Err((c, m, _)) if reason == "enable" => {
            root.alert("twin_no_state", &format!("[root] the twin is frozen without agent state ({c}: {m}); its replays start from an empty state"));
            None
        }
        Err((c, m, _)) => return Err(format!("no system snapshot ({c}: {m}); the twin is unchanged")),
    };
    let rec = root.append("twin", json!({"reason": reason, "snapshot": snapshot, "tuple": lkg})).ok_or("the lineage is unavailable")?;
    publish(root);
    Ok(Pin { tuple: lkg, snapshot, at: rec["at"].as_str().and_then(util::parse_rfc3339).unwrap_or_else(now_ms), reason: reason.into() })
}

// ---------------------------------------------------------------------------------------------
// The replay store

/// One stored task: the session's first owner line and its workspace snapshot (`ws/` beside `entry.json`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub at: i64,
    pub snapshot: String,
    pub line: String,
    pub turn: Option<u64>,
    pub caps: Vec<String>,
    pub replayed: Option<String>,
}

pub fn store_dir(rh: &Path) -> PathBuf {
    rh.join("extend/replay")
}

fn safe(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
}

fn save_entry(store: &Path, e: &Entry) -> Result<(), String> {
    let v = serde_json::to_value(e).map_err(|x| x.to_string())?;
    write_atomic(&store.join(&e.id).join("entry.json"), format!("{}\n", icj(&v).map_err(|x| x.to_string())?).as_bytes(), 0o600)
        .map_err(|x| x.to_string())
}

/// Every stored task with its workspace, oldest first.
pub fn entries(store: &Path) -> Vec<Entry> {
    let mut v: Vec<Entry> = fs::read_dir(store).into_iter().flatten().flatten().filter_map(|d| {
        let id = d.file_name().to_string_lossy().into_owned();
        let e: Entry = serde_json::from_str(&fs::read_to_string(d.path().join("entry.json")).ok()?).ok()?;
        (e.id == id && safe(&id) && fs::symlink_metadata(d.path().join("ws")).is_ok_and(|m| m.is_dir())).then_some(e)
    }).collect();
    v.sort_by(|a, b| (a.at, &a.id).cmp(&(b.at, &b.id)));
    v
}

/// The first line of a session with a workspace snapshot: an APFS clone of `snap` (never followed if it is a link)
/// into `<store>/<id>/ws`, then `entry.json`. Nothing is left on failure.
pub fn capture_at(store: &Path, snap: &Path, snapshot_id: &str, line: &str, at: i64) -> Result<Entry, String> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(store).map_err(|e| format!("extend/replay: {e}"))?;
    let id = util::hex(&util::random::<6>().map_err(|e| e.to_string())?);
    let dir = store.join(&id);
    fs::DirBuilder::new().mode(0o700).create(&dir).map_err(|e| e.to_string())?;
    let e = Entry { id: id.clone(), at, snapshot: snapshot_id.into(), line: line.into(), ..Entry::default() };
    let r = (|| {
        crate::sys::clonefile(snap, &dir.join("ws")).map_err(|x| format!("clonefile: {x}"))?;
        if !fs::symlink_metadata(dir.join("ws")).is_ok_and(|m| m.is_dir()) {
            return Err("the workspace snapshot is not a directory".to_string());
        }
        save_entry(store, &e)
    })();
    if r.is_err() {
        let _ = crate::snapshot::remove(&dir); // exact path, created above
    }
    r.map(|()| e)
}

/// Retention: tasks older than 7 days, beyond the newest 64, and torn captures older than an hour go, by exact path.
pub fn prune_store(store: &Path, now: i64) {
    let all = entries(store);
    let keep: BTreeSet<String> = all.iter().rev().filter(|e| now - e.at < KEEP_MS).take(STORE_MAX).map(|e| e.id.clone()).collect();
    for d in fs::read_dir(store).into_iter().flatten().flatten() {
        let id = d.file_name().to_string_lossy().into_owned();
        let torn = !all.iter().any(|e| e.id == id) && d.metadata().ok().and_then(|m| m.modified().ok()).is_some_and(|t| t.elapsed().is_ok_and(|x| x.as_secs() > 3600));
        if safe(&id) && !keep.contains(&id) && (all.iter().any(|e| e.id == id) || torn) {
            let _ = crate::snapshot::remove(&d.path()); // exact path: a task this Root stored
        }
    }
}

/// access.rs `workspace_set`: a new owner session (`None` first, then its snapshot when one was taken).
pub fn session_started(root: &Root, snap: Option<&Value>) {
    let mut p = lock(&PENDING);
    match snap.and_then(|s| s["id"].as_str()) {
        Some(id) => drop(p.insert(root.p.rh.clone(), id.to_string())),
        None => drop(p.remove(&root.p.rh)),
    }
}

/// link.rs `deliver`: the first line relayed in a session that has a workspace snapshot is stored (on the free path
/// with the twin enabled). Slash lines are not tasks; a redelivery finds nothing pending.
pub fn relayed(root: &Root, _qid: &str, frame: &Value) {
    if frame["t"] != "line" {
        return;
    }
    let Some(snap) = lock(&PENDING).remove(&root.p.rh) else { return };
    let text = frame["text"].as_str().unwrap_or("").trim().to_string();
    if text.is_empty() || text.starts_with('/') || !cfg_of(root).is_some_and(|c| c.free && c.enabled) {
        return;
    }
    let (rh, at) = (root.p.rh.clone(), now_ms());
    std::thread::spawn(move || {
        let src = rh.join("snapshots").join(&snap);
        if let Err(m) = capture_at(&store_dir(&rh), &src, &snap, &text, at) {
            util::log(&rh, "warn", &format!("twin: the session's first line was not stored: {m}"));
        }
    });
}

/// `sentinel::Hooks.turn_closed`: the caps of the owner turn a stored line started.
fn turn_closed(root: &Root, t: &crate::sentinel::Turn) {
    if t.kind == "owner" && cfg_of(root).is_some_and(|c| c.free && c.enabled) {
        tag_turn(&store_dir(&root.p.rh), t.seq, t.start, t.end, t.caps.keys().cloned().collect());
    }
}

pub fn tag_turn(store: &Path, seq: u64, start: i64, end: i64, caps: Vec<String>) {
    if let Some(mut e) = entries(store).into_iter().rev().find(|e| e.turn.is_none() && e.at >= start - 2_000 && e.at <= end) {
        (e.turn, e.caps) = (Some(seq), caps);
        let _ = save_entry(store, &e);
    }
}

/// Registers the closed-turn hook with the sentinel (X3).
pub fn register() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| lock(crate::sentinel::hooks()).turn_closed.push(turn_closed));
}

/// Tonight's sample: unreplayed tasks of the last 7 days, those whose turn used a favoured cap first, then random.
pub fn sample(all: &[Entry], now: i64, k: usize, favoured: &BTreeSet<String>, rnd: &mut dyn FnMut() -> u64) -> Vec<Entry> {
    let mut v: Vec<(bool, u64, &Entry)> = all.iter().filter(|e| e.replayed.is_none() && e.at <= now && now - e.at < KEEP_MS)
        .map(|e| (!e.caps.iter().any(|c| favoured.contains(c)), rnd(), e)).collect();
    v.sort_by(|a, b| (a.0, a.1, &a.2.id).cmp(&(b.0, b.1, &b.2.id)));
    v.into_iter().take(k).map(|x| x.2.clone()).collect()
}

/// Caps to favour: added or modified since `since` (the last report or the freeze), in a group closed `flagged` in
/// the last 14 days, or near decay (`near`: the cap table's `cold` and `archive_due` rows).
pub fn favoured_of(records: &[Value], near: &[String], since: i64, now: i64) -> BTreeSet<String> {
    let mut groups: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    let mut out: BTreeSet<String> = near.iter().cloned().collect();
    for r in records {
        let (d, at) = (&r["data"], r["at"].as_str().and_then(util::parse_rfc3339).unwrap_or(0));
        match r["kind"].as_str() {
            Some("change_open") => {
                let caps: Vec<String> = d["caps"].as_array().into_iter().flatten().filter(|c| c["op"] != "remove")
                    .filter_map(|c| c["id"].as_str().map(String::from)).collect();
                if at >= since {
                    out.extend(caps.iter().cloned());
                }
                if let Some(g) = d["group"].as_u64() {
                    groups.entry(g).or_default().extend(caps);
                }
            }
            Some("change_close") if d["outcome"] == "flagged" && now - at <= 14 * DAY => {
                out.extend(d["group"].as_u64().and_then(|g| groups.get(&g)).into_iter().flatten().cloned());
            }
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// The night

/// The off-peak night `now` falls in under window `(from, to)` (`Cfg.win`): (its date, the window's end), or None
/// outside the window. A night is named by the UTC date its window starts on.
pub fn night_of(now: i64, (from, to): (i64, i64)) -> Option<(String, i64)> {
    let day = now.div_euclid(DAY) * DAY;
    let tod = now - day;
    if tod >= from && tod < to {
        Some((date(day), day + to))
    } else if tod + DAY < to && tod + DAY >= from {
        Some((date(day - DAY), day - DAY + to))
    } else {
        None
    }
}

fn date(ms: i64) -> String {
    rfc3339(ms)[..10].to_string()
}

fn lane_run(night: &str, run: &str) -> String {
    format!("twin-{}-{run}", night.replace('-', ""))
}

/// Σ the night's replay spend in the egress usage ledger: each `t:"sealed"` line of a `twin-<night>-…` run at its
/// cost, and a `start` without its line (the Root died mid-request) at its reservation.
pub fn night_spent(rh: &Path, night: &str) -> u64 {
    let prefix = lane_run(night, "");
    let next = util::parse_rfc3339(&format!("{night}T00:00:00Z")).map_or_else(String::new, |t| date(t + DAY));
    let (mut open, mut spent) = (BTreeMap::<String, u64>::new(), 0u64);
    for d in [night.to_string(), next] {
        for v in fs::read_to_string(rh.join("egress/usage").join(format!("{d}.jsonl"))).unwrap_or_default().lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["run"].as_str().is_some_and(|r| r.starts_with(&prefix))) {
            let id = v["id"].as_str().unwrap_or("").to_string();
            match v["t"].as_str() {
                Some("start") => drop(open.insert(id, v["reserved_musd"].as_u64().unwrap_or(0))),
                Some("sealed") => {
                    open.remove(&id);
                    spent = spent.saturating_add(v["cost_musd"].as_u64().unwrap_or(0));
                }
                _ => {}
            }
        }
    }
    open.values().fold(spent, |a, r| a.saturating_add(*r))
}

/// What one arm ran on: a tuple and an agent-state snapshot directory (its `state/` is used), if any.
#[derive(Clone, Debug)]
pub struct Src {
    pub tuple: Value,
    pub state: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Which {
    Cur,
    Twin,
}

/// One arm's generic, Root-observed outcome.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Out {
    /// `ended` (stopped by itself), `capped` (money, requests or wall) or `crashed`.
    pub outcome: String,
    pub cost_musd: u64,
    pub wall_ms: u64,
    pub requests: u64,
    pub provider_errors: u64,
    pub mem_cap: u64,
    pub diff_files: u64,
    pub diff_bytes: u64,
    /// The lane refused every request (the twin pool or a cap): not a comparable run.
    #[serde(skip)]
    pub refused_only: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pair {
    pub night: String,
    pub task: String,
    pub model: String,
    pub order: String,
    pub cur: Out,
    pub twin: Out,
}

pub struct NightOut {
    pub pairs: Vec<Pair>,
    pub spent: u64,
    pub stop: &'static str,
    pub error: Option<String>,
}

/// The night's loop: per task, both arms in the order `coin` picks; dollar fields are metadata.
/// It stops when `gate(ms needed)` says so (`window`,
/// `disk`, `cancelled`), when a lane refused every request (`refused`), or on an arm error (`busy`, `error`).
/// An incomplete pair is discarded; its spend still counts.
#[allow(clippy::too_many_arguments)] // three injected closures (coin, gate, arm) keep the loop testable at $0
pub fn run_night(cfg: &Cfg, night: &str, model: &str, tasks: &[Entry], spent0: u64, coin: &mut dyn FnMut() -> bool,
    gate: &mut dyn FnMut(i64) -> Option<&'static str>, arm: &mut dyn FnMut(Which, &Entry, u64) -> Result<Out, String>) -> NightOut {
    let mut o = NightOut { pairs: Vec::new(), spent: spent0, stop: "done", error: None };
    for t in tasks {
        if let Some(why) = gate(2 * cfg.run_ms as i64) {
            o.stop = why;
            return o;
        }
        let cap = cfg.run_musd; // legacy metadata only; Root observes USD and the user controls stopping
        let order = if coin() { [Which::Cur, Which::Twin] } else { [Which::Twin, Which::Cur] };
        let mut got: Vec<(Which, Out)> = Vec::new();
        for (i, w) in order.into_iter().enumerate() {
            if i == 1 && let Some(why) = gate(cfg.run_ms as i64) {
                o.stop = why;
                return o;
            }
            match arm(w, t, cap) {
                Ok(a) => {
                    o.spent = o.spent.saturating_add(a.cost_musd);
                    got.push((w, a));
                }
                Err(e) => {
                    o.stop = match e.as_str() { "busy" => "busy", "disk" => "disk", "cancelled" => "cancelled", _ => "error" };
                    o.error = Some(e);
                    return o;
                }
            }
        }
        if got.iter().any(|(_, a)| a.refused_only) {
            o.stop = "refused";
            return o;
        }
        let take = |w: Which| got.iter().find(|x| x.0 == w).map(|x| x.1.clone()).unwrap_or_default();
        let first = if order[0] == Which::Cur { "cur_first" } else { "twin_first" };
        o.pairs.push(Pair { night: night.into(), task: t.id.clone(), model: model.into(), order: first.into(), cur: take(Which::Cur), twin: take(Which::Twin) });
    }
    o
}

/// Files and links under `base` (relative path → (is link, size, mtime ns)), never following links.
fn walk(base: &Path) -> BTreeMap<String, (bool, u64, i128)> {
    let (mut out, mut stack) = (BTreeMap::new(), vec![base.to_path_buf()]);
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            let Ok(m) = fs::symlink_metadata(e.path()) else { continue };
            if m.is_dir() {
                stack.push(e.path());
                continue;
            }
            let rel = e.path().strip_prefix(base).map(|r| r.to_string_lossy().into_owned()).unwrap_or_default();
            out.insert(rel, (m.file_type().is_symlink(), m.len(), i128::from(m.mtime()) * 1_000_000_000 + i128::from(m.mtime_nsec())));
        }
    }
    out
}

/// The workspace diff: (files added, removed or changed by size or mtime; their bytes).
pub fn diff(before: &Path, after: &Path) -> (u64, u64) {
    let (a, b) = (walk(before), walk(after));
    let (mut files, mut bytes) = (0u64, 0u64);
    for (k, x) in &a {
        match b.get(k) {
            None => (files, bytes) = (files + 1, bytes + x.1),
            Some(y) if y != x => (files, bytes) = (files + 1, bytes + y.1),
            _ => {}
        }
    }
    for (k, y) in &b {
        if !a.contains_key(k) {
            (files, bytes) = (files + 1, bytes + y.1);
        }
    }
    (files, bytes)
}

fn io(what: &str) -> impl Fn(std::io::Error) -> String + '_ {
    move |e| format!("{what}: {e}")
}

/// Count of a sealed run's lines in the usage ledger with an upstream 400 or 422 (H9's malformed-request class;
/// provider 429/5xx and lane refusals excluded).
fn provider_errors(rh: &Path, run: &str, dates: &[String]) -> u64 {
    let mut n = 0;
    for d in dates.iter().collect::<BTreeSet<_>>() {
        n += fs::read_to_string(rh.join("egress/usage").join(format!("{d}.jsonl"))).unwrap_or_default().lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|v| v["t"] == "sealed" && v["run"] == run && matches!(v["status"].as_u64(), Some(400 | 422))).count() as u64;
    }
    n
}

/// One replay arm in a fresh sealed run root: the tuple's body, ext set and T0; a clone of the state snapshot's
/// `state/`; a clone of the task's workspace snapshot as `INIT_WORKSPACE`; the live `kernel/config.toml` and boot
/// record's model (both arms on the current model); a sealed lane with `cap_musd`, `run_requests` and `run_ms`.
/// The run root and the lane socket are removed by exact path when this returns, whatever happened.
pub fn run_arm(env: &Env, night: &str, src: &Src, e: &Entry, store: &Path, cap_musd: u64, cfg: &Cfg) -> Result<Out, String> {
    let a = crate::snapshot::arm_of(&src.tuple).map_err(|m| format!("tuple: {m}"))?;
    let run = RunRoot::new(&env.rh)?;
    let r = run.dir.clone();
    let st = r.join("state");
    if let Some(s) = src.state.as_ref().map(|s| s.join("state")).filter(|s| fs::symlink_metadata(s).is_ok_and(|m| m.is_dir())) {
        crate::sys::clonefile(&s, &st).map_err(io("clone the arm's state"))?;
    }
    for x in ["run/ready", "run/heartbeat", "session", "spill", "sessions"] {
        crate::snapshot::remove(&st.join(x)).map_err(io("state"))?;
    }
    crate::snapshot::place_t0(&env.ih, &env.rh, &r, &a)?;
    let bin = r.join("bin/init-body");
    fs::copy(crate::snapshot::store_blob(&env.rh, &a.binary)?, &bin).and_then(|_| fs::set_permissions(&bin, fs::Permissions::from_mode(0o500)))
        .map_err(io("bin/init-body"))?;
    for d in ["bin/ext", "kernel", "state/kernel", "state/sessions", "state/run", "work/tmp", "work/home", "work/cargo-home", "run"] {
        fs::DirBuilder::new().recursive(true).mode(0o700).create(r.join(d)).map_err(io(d))?;
    }
    for (name, sha) in &a.ext {
        fs::copy(crate::snapshot::store_blob(&env.rh, sha)?, r.join(format!("bin/ext/{name}.wasm"))).map_err(io("bin/ext"))?;
    }
    for f in ["CONSTITUTION.md", "config.toml"] {
        let s = env.ih.join("kernel").join(f);
        if fs::symlink_metadata(&s).is_ok_and(|m| m.is_file()) {
            fs::copy(&s, r.join("kernel").join(f)).map_err(io(f))?;
        }
    }
    let ws = r.join("work/ws");
    crate::sys::clonefile(&store.join(&e.id).join("ws"), &ws).map_err(io("clone the workspace snapshot"))?;
    let rnd = |n: usize| util::random::<8>().map(|b| util::hex(&b)[..n].to_string()).map_err(|x| x.to_string());
    let t = rfc3339(now_ms());
    let session = format!("{}{}{}-{}{}{}-{}", &t[0..4], &t[5..7], &t[8..10], &t[11..13], &t[14..16], &t[17..19], rnd(6)?);
    let boot_id = rnd(16)?;
    let mut b: Value = fs::read(env.ih.join("state/kernel/boot.json")).ok().and_then(|x| serde_json::from_slice(&x).ok()).filter(Value::is_object)
        .unwrap_or_else(|| json!({"v": 1}));
    for (k, v) in [("body_id", json!("twin")), ("boot_id", json!(boot_id)), ("boot_seq", json!(0)), ("branch", Value::Null), ("cause", Value::Null),
        ("crash", Value::Null), ("from_sha", Value::Null), ("mode", json!("normal")), ("reason", json!("cold")), ("provisional", Value::Null),
        ("safe", Value::Null), ("session", json!(session)), ("sha", json!(a.sha)), ("spawned_at", json!(t)), ("update_failed", Value::Null)] {
        b[k] = v;
    }
    b["env"]["init_home"] = json!(r);
    let kv = b["env"]["kernel_version"].as_str().unwrap_or("0.1.0").to_string();
    write_atomic(&r.join("state/kernel/boot.json"), format!("{b}\n").as_bytes(), 0o600).map_err(io("boot.json"))?;
    let name = lane_run(night, &run.id);
    let lane = crate::egress::twin_lane(&env.rh, &env.ch, &name, &crate::sealed::lane_sock(&env.rh, &run), cap_musd, env.level, night)
        .map_err(|x| format!("sealed lane: {x}"))?;
    lane.set_caps(crate::egress::Caps { turns: cfg.run_requests, tokens_out: 0, tokens_in: 0 });
    let s = |p: &Path| p.to_string_lossy().into_owned();
    let lim = Limits::of(&env.ch);
    let mut c = crate::sealed::sandboxed(env, &run, Some(&lane.sock), &[bin.into()]);
    c.envs([("INIT_HOME", s(&r)), ("INIT_SESSION", session), ("INIT_SHA", a.sha.clone()), ("INIT_BOOT_ID", boot_id), ("INIT_BOOT_REASON", "cold".into()),
        ("INIT_BODY_ID", "twin".into()), ("INIT_MODE", "normal".into()), ("INIT_UI", "stdio-jsonl/1".into()), ("INIT_KERNEL_VERSION", kv),
        ("INIT_KERNEL_SOCK", s(&env.rh.join("run").join(format!("sk-{}.sock", run.id)))), ("HOME", s(&r.join("work/home"))),
        ("TMPDIR", format!("{}/", s(&r.join("work/tmp")))), ("CARGO_HOME", s(&r.join("work/cargo-home"))), ("CARGO_TARGET_DIR", s(&r.join("work/target"))),
        ("CARGO_NET_OFFLINE", "true".into()), ("GIT_AUTHOR_NAME", "INIT Agent".into()), ("GIT_COMMITTER_NAME", "INIT Agent".into()),
        ("GIT_AUTHOR_EMAIL", "agent@init.local".into()), ("GIT_COMMITTER_EMAIL", "agent@init.local".into()), ("INIT_WORKSPACE", s(&ws)),
        ("INIT_ACCESS", "workspace".into())]).envs(memwatch::body_env(&lim, &run.events_path())).current_dir(&ws);
    let input = json!({"id": rnd(16)?, "mode": "steer", "seq": 1, "t": "input", "text": e.line, "ts": t});
    let (t0, day0) = (Instant::now(), date(now_ms()));
    let res = crate::sealed::supervise(c, &run, &lim, Duration::from_millis(cfg.run_ms), Some(icj(&input).map_err(|x| x.to_string())?), || {
        let s = lane.status();
        let idle = s["idle_ms"].as_i64().unwrap_or(0);
        match (s["last_finish"].as_str(), s["inflight"].as_u64(), s["capped"].as_u64().unwrap_or(0) > 0) {
            (_, Some(0), true) if idle > 10_000 => Some("cost_capped"),
            (Some("stop"), Some(0), _) if idle > 5_000 => Some("done"),
            _ => None,
        }
    });
    let wall_ms = t0.elapsed().as_millis() as u64;
    let ls = lane.close();
    let (_, why, _) = res.map_err(|m| if m.contains("floor") { "disk".to_string() } else { m })?;
    if why == "disk_floor" {
        return Err("disk".into());
    }
    if crate::sealed::cancelled() {
        return Err("cancelled".into());
    }
    let mem_cap = fs::read_to_string(run.events_path()).unwrap_or_default().lines().filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["event"] == "mem_cap").count() as u64;
    let n = |k: &str| ls[k].as_u64().unwrap_or(0);
    let outcome = match why {
        "timeout" | "cost_capped" => "capped",
        "crash" | "mem_cap" => "crashed",
        _ if n("capped") > 0 => "capped",
        _ => "ended",
    };
    let (diff_files, diff_bytes) = diff(&store.join(&e.id).join("ws"), &ws);
    Ok(Out { outcome: outcome.into(), cost_musd: n("spent_musd"), wall_ms, requests: n("requests"),
        provider_errors: provider_errors(&env.rh, &name, &[day0, date(now_ms())]), mem_cap, diff_files, diff_bytes,
        refused_only: n("requests") == 0 && n("refused") > 0 })
}

// ---------------------------------------------------------------------------------------------
// Statistics (exact integers)

/// `num/den` rounded up to parts per million (so `≤ 50000` ⇔ p ≤ 0.05).
fn ppm_up(t: &Tail) -> u64 {
    let f = t.ppm();
    let (mut l, mut r) = (t.den.clone(), t.num.clone());
    l.mul(f);
    r.mul(1_000_000);
    if l < r { (f + 1).min(1_000_000) } else { f }
}

/// `P(X ≥ k)` for X ~ Bin(n, 1/2), in ppm rounded up.
pub fn upper_ppm(n: u64, k: u64) -> u64 {
    ppm_up(&binom_tail(n, k, 1, 2))
}

/// The exact two-sided test at 1/2 on counts `a` and `b` (McNemar on discordant pairs; the sign test with ties
/// dropped): `min(1, 2·P(X ≥ max(a, b)))`, X ~ Bin(a + b, 1/2), in ppm rounded up.
pub fn two_sided_ppm(a: u64, b: u64) -> u64 {
    let mut t = binom_tail(a + b, a.max(b), 1, 2);
    t.num.mul(2);
    if t.num >= t.den { 1_000_000 } else { ppm_up(&t) }
}

fn ratio(cur: u64, twin: u64) -> u64 {
    cur.saturating_mul(1000) / twin.max(1)
}

/// `{cur_higher, twin_higher, p_ppm, median_ratio_permille}` of one per-arm quantity over the pairs.
fn signed(p: &[&Pair], f: fn(&Out) -> u64) -> Value {
    let hi = p.iter().filter(|x| f(&x.cur) > f(&x.twin)).count() as u64;
    let lo = p.iter().filter(|x| f(&x.cur) < f(&x.twin)).count() as u64;
    let r: Vec<u64> = p.iter().map(|x| if f(&x.cur) == f(&x.twin) { 1000 } else { ratio(f(&x.cur), f(&x.twin)) }).collect();
    let med = if r.is_empty() { Value::Null } else { json!(median2(&r) / 2) };
    json!({"cur_higher": hi, "median_ratio_permille": med, "p_ppm": two_sided_ppm(hi, lo), "twin_higher": lo})
}

/// The paired statistics of a set of pairs.
pub fn stats(p: &[&Pair]) -> Value {
    let e = |a: &Out| a.outcome == "ended";
    let cur_only = p.iter().filter(|x| e(&x.cur) && !e(&x.twin)).count() as u64;
    let twin_only = p.iter().filter(|x| !e(&x.cur) && e(&x.twin)).count() as u64;
    let crashed = |f: fn(&Pair) -> &Out| p.iter().filter(|x| f(x).outcome == "crashed").count();
    json!({"cost": signed(p, |a| a.cost_musd), "crashes": {"cur": crashed(|x| &x.cur), "twin": crashed(|x| &x.twin)},
        "ended": {"cur": p.iter().filter(|x| e(&x.cur)).count(), "discordant": cur_only + twin_only, "p_ppm": two_sided_ppm(cur_only, twin_only),
            "twin": p.iter().filter(|x| e(&x.twin)).count()},
        "pairs": p.len(), "wall": signed(p, |a| a.wall_ms)})
}

/// The exact null of the Wilcoxon signed-rank statistic W⁺ for n pairs with no ties: the counts of the subsets of
/// {1..n} by their sum, out of 2ⁿ (u128, n ≤ 120). The same recursion as X10's `signed_rank_k` (extend_sim.rs).
fn rank_counts(n: u64) -> Vec<u128> {
    let top = (n * (n + 1) / 2) as usize;
    let mut f = vec![0u128; top + 1];
    f[0] = 1;
    for r in 1..=n as usize {
        for s in (r..=top).rev() {
            f[s] += f[s - r];
        }
    }
    f
}

/// The smallest w with `P(W⁺ ≥ w) ≤ a/b` (the critical value of the one-sided test), or None; as X10's.
pub fn signed_rank_k(n: u64, (a, b): (u64, u64)) -> Option<u64> {
    let f = rank_counts(n);
    let (total, mut tail, mut best) = (1u128 << n, 0u128, None);
    for w in (0..f.len()).rev() {
        tail += f[w];
        if tail * u128::from(b) > total * u128::from(a) {
            break;
        }
        best = Some(w as u64);
    }
    best
}

/// `P(W⁺ ≥ w)` for n pairs, in ppm rounded up.
pub fn rank_upper_ppm(n: u64, w: u64) -> u64 {
    let f = rank_counts(n);
    let tail: u128 = f.iter().skip(w as usize).sum();
    ppm_up(&Tail { num: Big::from(tail), den: Big::from(1u128 << n) })
}

/// W⁺ of per-pair (current, twin) amounts on the log-ratio scale: pairs ranked 1…n by |ln(cur/twin)|, compared
/// exactly by cross-multiplying max·min (amounts below 1 count as 1), ties in pair order (X10's stable sort), equal
/// amounts included with the smallest ranks; W⁺ sums the ranks of the pairs where the current arm is higher.
pub fn w_plus(v: &[(u64, u64)]) -> u64 {
    let hl = |&(a, b): &(u64, u64)| (u128::from(a.max(b).max(1)), u128::from(a.min(b).max(1)));
    let mut idx: Vec<usize> = (0..v.len()).collect();
    idx.sort_by(|&i, &j| {
        let ((h1, l1), (h2, l2)) = (hl(&v[i]), hl(&v[j]));
        (h1 * l2).cmp(&(h2 * l1))
    });
    idx.iter().enumerate().filter(|(_, i)| v[**i].0.max(1) > v[**i].1.max(1)).map(|(r, _)| r as u64 + 1).sum()
}

/// The twin's cost test (X10's proposal, approved 2026-10-22): the exact signed-rank test on per-task log cost ratios
/// over the 14-night span (at most its newest 120 pairs), one-sided: `p_ppm` = P(W⁺ ≥ w), small when the current
/// arm is costlier; `lower_p_ppm` the other side. Plus the span's median ratio and sign counts.
pub fn cost_test(p: &[&Pair]) -> Value {
    let p = &p[p.len().saturating_sub(SR_MAX)..];
    let v: Vec<(u64, u64)> = p.iter().map(|x| (x.cur.cost_musd, x.twin.cost_musd)).collect();
    let n = v.len() as u64;
    let w = w_plus(&v);
    let mut c = signed(p, |a| a.cost_musd);
    c["test"] = json!("signed_rank");
    c["nights"] = json!(REG_NIGHTS);
    c["pairs"] = json!(n);
    c["w_plus"] = json!(w);
    c["k"] = json!(signed_rank_k(n, SIG));
    c["p_ppm"] = json!(rank_upper_ppm(n, w));
    c["lower_p_ppm"] = json!(rank_upper_ppm(n, n * (n + 1) / 2 - w));
    c
}

/// "No regression" (ADR-011 §7.2) over the last 14 nights' pairs: `ended` not significantly worse (one-sided exact
/// McNemar `P(X ≥ twin_only)` > 0.05) and the median cost ratio ≤ 1.5. With fewer than 40 pairs the verdict is
/// `insufficient`, which never blocks a level (orchestrator 2026-10-22: below 3 pairs a night the twin could never
/// give a verdict). Returns (verdict, the one-sided p in ppm, the median cost ratio).
pub fn regression(p: &[&Pair]) -> (&'static str, u64, Option<u64>) {
    let e = |a: &Out| a.outcome == "ended";
    let cur_only = p.iter().filter(|x| e(&x.cur) && !e(&x.twin)).count() as u64;
    let twin_only = p.iter().filter(|x| !e(&x.cur) && e(&x.twin)).count() as u64;
    let worse_p = upper_ppm(cur_only + twin_only, twin_only);
    let med = signed(p, |a| a.cost_musd)["median_ratio_permille"].as_u64();
    let verdict = if p.len() < REG_PAIRS {
        "insufficient"
    } else if worse_p > SIG_PPM && med.is_some_and(|m| m <= REG_COST_PERMILLE) {
        "no_regression"
    } else {
        "regression"
    };
    (verdict, worse_p, med)
}

/// Monday 00:00 UTC of the week holding `ms`.
pub fn week_start(ms: i64) -> i64 {
    let d = ms.div_euclid(DAY);
    (d - (d + 3).rem_euclid(7)) * DAY
}

/// The week to report now, if any: the last complete week (from an hour after it ended, so Sunday's night is over),
/// once, and only if the twin existed during it.
pub fn report_due(records: &[Value], pin_at: i64, now: i64) -> Option<i64> {
    let this = week_start(now);
    let w = this - 7 * DAY;
    if now < this + 3_600_000 || pin_at >= this {
        return None;
    }
    let key = date(w);
    (!records.iter().any(|r| r["kind"] == "twin_report" && r["data"]["week"] == key.as_str())).then_some(w)
}

/// The `twin_report` of week `w` (aggregates only), on the newest model among the week's pairs (reports before and
/// after a model change stay apart): `ended`, `wall` and `crashes` over the week's pairs; `cost` (the signed-rank
/// test) and the verdict over the last 14 nights; `spent_musd`. `regression` is true only for a `regression`
/// verdict, so an `insufficient` twin never blocks a level (ledger.rs reads the flag).
pub fn report_of(pairs: &[Pair], w: i64, spent: u64) -> Value {
    let (from, to, l14) = (date(w), date(w + 7 * DAY), date(w + 7 * DAY - REG_NIGHTS * DAY));
    let model = pairs.iter().rev().find(|p| p.night >= from && p.night < to).map(|p| p.model.clone()).unwrap_or_default();
    let wk: Vec<&Pair> = pairs.iter().filter(|p| p.night >= from && p.night < to && p.model == model).collect();
    let last: Vec<&Pair> = pairs.iter().filter(|p| p.night >= l14 && p.night < to && p.model == model).collect();
    let mut v = stats(&wk);
    let (verdict, worse_p, med) = regression(&last);
    v["cost"] = cost_test(&last);
    v["week"] = json!(from);
    v["spent_musd"] = json!(spent);
    v["model"] = json!(model);
    v["verdict"] = json!(verdict);
    v["regression"] = json!(verdict == "regression");
    v["last14"] = json!({"cost_median_ratio_permille": med, "ended_worse_p_ppm": worse_p, "pairs": last.len()});
    v
}

/// The stop rule's test on one report: the twin significantly better on `ended` (the week's McNemar) or on cost
/// (the 14-night signed-rank test in the "current costlier" direction).
pub fn twin_better(d: &Value) -> bool {
    let u = |p: &str| d.pointer(p).and_then(Value::as_u64).unwrap_or(0);
    let p = |p: &str| d.pointer(p).and_then(Value::as_u64).unwrap_or(1_000_000);
    (u("/ended/twin") > u("/ended/cur") && p("/ended/p_ppm") <= SIG_PPM) || p("/cost/p_ppm") <= SIG_PPM
}

/// Owner-facing words for a report: a result only at p ≤ 0.05, otherwise "not distinguishable at n = …".
pub fn reading(d: &Value) -> Value {
    let n = d["pairs"].as_u64().unwrap_or(0);
    let pv = |x: &Value| format!("p = {}.{:06}", x.as_u64().unwrap_or(0) / 1_000_000, x.as_u64().unwrap_or(0) % 1_000_000);
    let nd = format!("not distinguishable at n = {n}");
    let e = &d["ended"];
    let ended = match (e["p_ppm"].as_u64().unwrap_or(1_000_000) <= SIG_PPM, e["cur"].as_u64() > e["twin"].as_u64()) {
        (false, _) => nd.clone(),
        (true, true) => format!("the current system ended more tasks than the twin ({})", pv(&e["p_ppm"])),
        (true, false) => format!("the twin ended more tasks than the current system ({})", pv(&e["p_ppm"])),
    };
    let sig = |x: &Value| x.as_u64().unwrap_or(1_000_000) <= SIG_PPM;
    let w = &d["wall"];
    let wall = match (sig(&w["p_ppm"]), w["cur_higher"].as_u64() > w["twin_higher"].as_u64()) {
        (false, _) => nd,
        (true, up) => format!("the current system's wall time per task is {} ({}, median ratio {}‰)", if up { "higher" } else { "lower" },
            pv(&w["p_ppm"]), w["median_ratio_permille"]),
    };
    let c = &d["cost"];
    let cost = match (sig(&c["p_ppm"]), sig(&c["lower_p_ppm"])) {
        (true, _) => format!("over the last 14 nights the current system costs more per task than the twin (signed-rank {}, median ratio {}‰)",
            pv(&c["p_ppm"]), c["median_ratio_permille"]),
        (_, true) => format!("over the last 14 nights the current system costs less per task than the twin (signed-rank {}, median ratio {}‰)",
            pv(&c["lower_p_ppm"]), c["median_ratio_permille"]),
        _ => format!("not distinguishable at n = {} (14 nights)", c["pairs"].as_u64().unwrap_or(0)),
    };
    let verdict = match d["verdict"].as_str() {
        Some("insufficient") => format!("insufficient: {} pairs in 14 nights, 40 needed (an insufficient twin blocks no level)", d["last14"]["pairs"]),
        Some(v) => v.replace('_', " "),
        None => String::new(),
    };
    json!({"cost": cost, "ended": ended, "verdict": verdict, "wall": wall})
}

// ---------------------------------------------------------------------------------------------
// State, the scheduler and the published view

fn twin_dir(rh: &Path) -> PathBuf {
    rh.join("extend/twin")
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    last_night: String,
    suggested_ms: i64,
    night: Value,
}

impl State {
    fn load(rh: &Path) -> State {
        fs::read_to_string(twin_dir(rh).join("state.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    fn save(&self, rh: &Path) {
        let d = twin_dir(rh);
        let _ = fs::DirBuilder::new().recursive(true).mode(0o700).create(&d);
        if let Ok(v) = serde_json::to_value(self) {
            let _ = write_atomic(&d.join("state.json"), format!("{}\n", icj(&v).unwrap_or_default()).as_bytes(), 0o600);
        }
    }
}

pub fn pairs(rh: &Path) -> Vec<Pair> {
    fs::read_to_string(twin_dir(rh).join("pairs.jsonl")).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

fn add_pairs(rh: &Path, new: &[Pair], now: i64) {
    let cut = date(now - PAIRS_KEEP_MS);
    let mut all: Vec<Pair> = pairs(rh).into_iter().filter(|p| p.night >= cut).collect();
    all.extend(new.iter().cloned());
    let body: String = all.iter().filter_map(|p| serde_json::to_value(p).ok()).filter_map(|v| icj(&v).ok()).map(|l| l + "\n").collect();
    let _ = fs::DirBuilder::new().recursive(true).mode(0o700).create(twin_dir(rh));
    let _ = write_atomic(&twin_dir(rh).join("pairs.jsonl"), body.as_bytes(), 0o600);
}

/// `provider/model` from the live boot record: the model both arms run on.
fn model(ih: &Path) -> String {
    let b: Value = fs::read(ih.join("state/kernel/boot.json")).ok().and_then(|x| serde_json::from_slice(&x).ok()).unwrap_or_default();
    format!("{}/{}", b["model"]["provider"].as_str().unwrap_or(""), b["model"]["model"].as_str().unwrap_or(""))
}

fn halted(root: &Root) -> bool {
    matches!(lock(&root.st).slot, Slot::Panicked { .. } | Slot::Halted { .. })
}

/// Rate limit: true (and `at` set to `now`) on the first call and once `every` ms have passed (or the clock moved back).
pub fn due(at: &AtomicI64, now: i64, every: i64) -> bool {
    let last = at.load(SeqCst);
    if last != i64::MIN && now >= last && now.saturating_sub(last) < every {
        return false;
    }
    at.store(now, SeqCst);
    true
}

/// `sup.rs` run loop (every 200 ms; works every 30 s): the freeze when the free path first holds, tonight's replay
/// (on its own thread); every 10 minutes store retention, the 90-day re-freeze suggestion and the weekly report.
pub fn tick(root: &Arc<Root>) {
    let now = now_ms();
    if !due(&LAST_TICK, now, TICK_MS) {
        return;
    }
    let Some(cfg) = cfg_of(root).filter(|c| c.free) else { return };
    if halted(root) {
        return;
    }
    let rh = &root.p.rh;
    let frozen = lock(&root.sup.ptrs).twin.is_some(); // the guard drops here: `freeze` locks the pointers itself
    if !frozen && freeze(root, "enable").is_err() {
        return; // no lkg yet: the next tick tries again
    }
    let st = State::load(rh);
    if due(&LAST_SLOW, now, SLOW_MS) {
        slow(root, now, st.suggested_ms);
    }
    if cfg.enabled && let Some((n, end)) = night_of(now, cfg.win) && st.last_night != n && now + 2 * cfg.run_ms as i64 <= end
        && RUNNING.compare_exchange(false, true, SeqCst, SeqCst).is_ok()
    {
        struct Done;
        impl Drop for Done {
            fn drop(&mut self) {
                RUNNING.store(false, SeqCst); // also if the night's thread panics
            }
        }
        let r = root.clone();
        std::thread::spawn(move || {
            let _d = Done;
            night(&r, &n, end);
        });
    }
}

fn slow(root: &Root, now: i64, suggested: i64) {
    let rh = &root.p.rh;
    prune_store(&store_dir(rh), now);
    let recs = crate::lineage::records(rh).unwrap_or_default();
    let Some(pin) = pin_of(&recs) else { return };
    if now - pin.at >= REFREEZE_MS && now - suggested >= 30 * DAY {
        root.alert("twin_refreeze_suggested", &format!("[root] the twin was frozen {} days ago; /twin refreeze re-freezes it to the current lkg (owner-only)",
            (now - pin.at) / DAY));
        let mut st = State::load(rh);
        st.suggested_ms = now;
        st.save(rh);
    }
    if let Some(w) = report_due(&recs, pin.at, now) {
        report(root, w);
    }
}

/// Appends week `w`'s `twin_report`, applies the stop rule (3 consecutive reports with the twin significantly
/// better → alert `twin_worse`), and publishes `twin.json`.
fn report(root: &Root, w: i64) {
    let rh = &root.p.rh;
    let spent = (0..7).map(|i| night_spent(rh, &date(w + i * DAY))).sum::<u64>();
    let d = report_of(&pairs(rh), w, spent);
    if root.append("twin_report", d).is_none() {
        return;
    }
    let recs = crate::lineage::records(rh).unwrap_or_default();
    let last: Vec<&Value> = recs.iter().rev().filter(|r| r["kind"] == "twin_report").take(3).map(|r| &r["data"]).collect();
    if last.len() == 3 && last.iter().all(|d| twin_better(d)) {
        root.alert("twin_worse", "[root] the frozen twin did significantly better than the current system in 3 weekly reports in a row; \
            consider extend.mode = \"gated\" or /rollback twin");
    }
    publish(root);
}

/// Waits for the sealed operation lock (A4, audits and slices hold it too), at most 10 minutes.
fn sealed_lock(rh: &Path) -> Result<std::sync::MutexGuard<'static, ()>, String> {
    for _ in 0..120 {
        if let Some(g) = crate::sealed::try_begin(rh) {
            return Ok(g);
        }
        std::thread::sleep(Duration::from_secs(5));
    }
    Err("busy".into())
}

/// Tonight's replay on the live Root.
fn night(root: &Arc<Root>, n: &str, end: i64) {
    let (rh, now) = (root.p.rh.clone(), now_ms());
    let Some(cfg) = cfg_of(root) else { return };
    let recs = crate::lineage::records(&rh).unwrap_or_default();
    let lkg = lock(&root.sup.ptrs).lkg.clone(); // its own statement: `Env::of` locks the pointers too
    let (Some(pin), Some(lkg), Ok(env)) = (pin_of(&recs), lkg, Env::of(root)) else { return };
    let load = |id: &str| crate::tuple::load(&rh, id);
    let snap = |id: &str| Some(crate::snapshot::dir(&rh).join(id)).filter(|p| p.is_dir());
    let newest = crate::snapshot::metas(&rh).into_iter().rev().find(|m| m["kind"] == "sys").and_then(|m| m["id"].as_str().and_then(snap));
    let (Some(tt), Some(ct)) = (load(&pin.tuple), load(&lkg)) else {
        root.alert("twin_night", "[root] tonight's twin replay was skipped: a tuple is not in the store");
        return;
    };
    let twin = Src { tuple: tt, state: pin.snapshot.as_deref().and_then(snap) };
    let cur = Src { tuple: ct, state: newest };
    let done = pairs(&rh).iter().filter(|p| p.night == n).count();
    let since = recs.iter().rev().find(|r| r["kind"] == "twin_report").and_then(|r| r["at"].as_str().and_then(util::parse_rfc3339)).unwrap_or(pin.at);
    let near: Vec<String> = root.extend.table().into_iter().filter(|c| c.state == "cold" || c.state == "archive_due").map(|c| c.id).collect();
    let store = store_dir(&rh);
    let mut rnd = || u64::from_le_bytes(util::random::<8>().unwrap_or_default());
    let tasks = sample(&entries(&store), now, cfg.k.saturating_sub(done), &favoured_of(&recs, &near, since, now), &mut rnd);
    let floor = Limits::of(&env.ch).floor;
    let mut gate = |need: i64| -> Option<&'static str> {
        if crate::sealed::cancelled() || halted(root) {
            Some("cancelled")
        } else if now_ms() + need > end {
            Some("window")
        } else if crate::sys::free_bytes(&rh).unwrap_or(u64::MAX) < floor + ROOM {
            Some("disk")
        } else {
            None
        }
    };
    let mut arm = |w: Which, e: &Entry, cap: u64| -> Result<Out, String> {
        let _g = sealed_lock(&rh)?;
        run_arm(&env, n, if w == Which::Cur { &cur } else { &twin }, e, &store, cap, &cfg)
    };
    let mut coin = || util::random::<1>().map(|b| b[0] & 1 == 1).unwrap_or(true);
    let o = run_night(&cfg, n, &model(&root.p.ih), &tasks, night_spent(&rh, n), &mut coin, &mut gate, &mut arm);
    add_pairs(&rh, &o.pairs, now_ms());
    let mut all = entries(&store);
    for e in all.iter_mut().filter(|e| o.pairs.iter().any(|p| p.task == e.id)) {
        e.replayed = Some(n.to_string());
        let _ = save_entry(&store, e);
    }
    if let Some(m) = &o.error {
        util::log(&rh, "warn", &format!("twin night {n}: {} ({m})", o.stop));
    }
    let mut st = State::load(&rh);
    if !matches!(o.stop, "busy" | "disk" | "cancelled") {
        st.last_night = n.to_string(); // a deferred night is retried by a later tick inside the window
    }
    st.night = json!({"night": n, "pairs": done + o.pairs.len(), "sampled": tasks.len(), "spent_musd": o.spent, "stop": o.stop});
    st.save(&rh);
    publish(root);
}

/// `INIT_HOME/root/twin.json` (0644, ICJ): the pointer, the budget, the last night and report, and their reading.
/// Aggregates only: no task text, no path.
pub fn view(root: &Root) -> Value {
    let (rh, now) = (&root.p.rh, now_ms());
    let recs = crate::lineage::records(rh).unwrap_or_default();
    let st = State::load(rh);
    let rep = recs.iter().rev().find(|r| r["kind"] == "twin_report").map(|r| r["data"].clone());
    let budget = lock(&root.st).charter.as_ref().map(|l| serde_json::to_value(&l.ch.extend.twin).unwrap_or_default());
    let cfg = cfg_of(root);
    // X10: 6 pairs a night assume a run costs ≤ $0.025; the measured per-run cost and pairs per night say whether the
    // twin can reach the 40 pairs in 14 nights a verdict needs (an insufficient twin blocks no level)
    let (all, cut) = (pairs(rh), date(now - REG_NIGHTS * DAY));
    let recent: Vec<&Pair> = all.iter().filter(|p| p.night >= cut).collect();
    let nights = recent.iter().map(|p| p.night.as_str()).collect::<BTreeSet<_>>().len();
    let costs: Vec<u64> = recent.iter().flat_map(|p| [p.cur.cost_musd, p.twin.cost_musd]).collect();
    let measured = json!({"insufficient": recent.len() < REG_PAIRS, "nights": nights, "pairs": recent.len(),
        "pairs_per_night_permille": (recent.len() as u64 * 1000).checked_div(nights as u64),
        "run_cost_median_musd": (!costs.is_empty()).then(|| median2(&costs) / 2)});
    json!({"money_enforced": false, "money_policy": "observe", "budget": budget, "last14": measured, "enabled": cfg.as_ref().is_some_and(|c| c.enabled), "mode": if cfg.as_ref().is_some_and(|c| c.free) { "free" } else { "gated" },
        "night": {"last": st.last_night, "summary": st.night, "window_utc": win_text(cfg.as_ref().map_or(WIN, |c| c.win))},
        "reading": rep.as_ref().map(reading), "report": rep, "replay_tasks": entries(&store_dir(rh)).len(),
        "twin": pin_of(&recs).map(|p| json!({"age_days": (now - p.at) / DAY, "frozen_at": rfc3339(p.at), "reason": p.reason,
            "refreeze_suggested": now - p.at >= REFREEZE_MS, "snapshot": p.snapshot, "tuple": p.tuple})), "v": 1})
}

pub fn publish(root: &Root) {
    let dir = root.p.i("root");
    if dir.is_dir() && let Ok(s) = icj(&view(root)) {
        let _ = write_atomic(&dir.join("twin.json"), format!("{s}\n").as_bytes(), 0o644);
    }
}

/// Owner lines (link.rs `line`): `/twin` shows the twin; `/twin refreeze` asks for the typed confirmation and
/// `/twin refreeze confirm` re-freezes it (owner-only: only `owner.sock` carries these lines). True: consumed.
pub fn owner_text(root: &Root, t: &str) -> bool {
    let w: Vec<&str> = t.split_whitespace().collect();
    if w.first() != Some(&"/twin") {
        return false;
    }
    let recs = crate::lineage::records(&root.p.rh).unwrap_or_default();
    let pin = pin_of(&recs);
    let free = cfg_of(root).is_some_and(|c| c.free);
    let desc = |p: &Pin| format!("tuple {}, frozen {} days ago ({})", p.tuple, (now_ms() - p.at) / DAY, p.reason);
    match w[1..] {
        [] => {
            let r = view(root);
            let text = match &pin {
                None => "[root] no twin yet: it is frozen when extend.mode first becomes \"free\"".to_string(),
                Some(p) => format!("[root] twin: {} · last night {} · reading {}", desc(p), r["night"]["last"], icj(&r["reading"]).unwrap_or_default()),
            };
            root.notice_code("info", "twin", &text);
        }
        ["refreeze"] if !free => root.notice_code("warn", "twin", "[root] the twin exists only on the free path (extend.mode = \"free\")"),
        ["refreeze"] => {
            let lkg = lock(&root.sup.ptrs).lkg.clone().unwrap_or_else(|| "(none)".into());
            let old = pin.as_ref().map_or_else(|| "no twin".to_string(), desc);
            root.notice_code("info", "twin", &format!("[root] Re-freezing replaces the twin ({old}) with the current lkg tuple {lkg} and a new \
                snapshot of the agent state; comparisons start over from it, so drift since the last freeze stops counting. \
                Type /twin refreeze {} to do it.", crate::access::PHRASE));
        }
        ["refreeze", c] if c == crate::access::PHRASE && free => match freeze(root, "owner_refreeze") {
            Ok(p) => root.notice_code("info", "twin", &format!("[root] the twin is re-frozen: {}", desc(&p))),
            Err(m) => root.notice_code("warn", "twin", &format!("[root] /twin refreeze: {m}")),
        },
        _ => root.notice_code("info", "twin", "[root] /twin · /twin refreeze"),
    }
    true
}

#[cfg(test)]
mod tests;
