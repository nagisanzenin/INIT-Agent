//! The Root harm sentinel (ADR-011 §3, lane X3): owner and background turns, the baseline rings, the exact
//! exceedance and binomial tests, the generic harm signals H1–H12, change groups and their windows, attribution
//! (`direct`, `suspect`, `collateral`), re-admission, `extend.rollback` with the whole-system fallback of
//! root.md §3.7.3, the owner's `/rollback`, and the deafness monitor. Every signal is the Root's own observation;
//! a zone report (`change.veto`) can only ask for a rollback. No floats: money is micro-USD, time is Root-clock ms,
//! and every tail is exact integer combinatorics (`stats.rs`).
//! This file is the adapter: hooks the core files call, the `change.open` request, persistence
//! (`ROOT_HOME/state/sentinel.json`, groups re-folded from the lineage) and the actions of the pure `engine.rs`.
//! Lock order: the sentinel lock is a leaf; it is never held while appending, alerting or reading `st`/`ptrs`.

use crate::link::{Root, lock};
use crate::sup::{Ev, Plan};
use crate::util::{self, now_ms, rfc3339};
use crate::zone::Err3;
use engine::{Act, Ctx, Engine, Open};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub mod engine;
pub mod stats;
#[cfg(test)]
mod tests;

/// One closed turn (ADR-011 §3.1): an owner turn starts when the Root relays an owner line; a background turn is a
/// maximal span of zone egress outside owner turns. Both end after 30 s with no zone request in flight, or at 15 min.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub seq: u64,
    /// `owner` or `background`.
    pub kind: String,
    pub start: i64,
    pub end: i64,
    pub cost_musd: u64,
    pub wall_ms: u64,
    pub requests: u32,
    pub req_bytes: u64,
    pub provider_errors: u32,
    pub correction: bool,
    pub crash: bool,
    pub kill: bool,
    pub refusal: bool,
    /// Ended by the 15-minute cap.
    pub capped: bool,
    /// Owner turns: the turn's cost plus the background spend until the next owner turn (S1's zone spend per turn).
    pub zspend_musd: u64,
    /// Capabilities used (X5's tap and `x-egress-caps`): id → (uses, evidence `root`|`zone`).
    pub caps: BTreeMap<String, (u32, String)>,
    /// The groups open when the turn started.
    pub groups: Vec<u64>,
    /// The current tuple when the turn started.
    pub tuple: Option<String>,
    /// Owner turns: the unit (consecutive turns of one owner session, ≤ 6) its statistics are pooled in.
    #[serde(default)]
    pub unit: u64,
}

impl Turn {
    /// A harmed turn (§3.1, for X6's correlation): a correction, a crash, a kill, a provider error, the 15-minute
    /// cap, or a demoting refusal.
    pub fn harmed(&self) -> bool {
        self.correction || self.crash || self.kill || self.provider_errors > 0 || self.capped || self.refusal
    }
}

/// X4's admission hook: `(root, change.open args with group and parent_tuple, first)`.
pub type AdmitFn = fn(&Root, &Value, bool) -> Result<Value, Err3>;
/// X5's caps of a `change.open`.
pub type CapsFn = fn(&Root, &Value) -> Result<Value, Err3>;
/// X4's R2: `(root, snapshot id, reason)`.
pub type RestoreFn = fn(&Root, &str, &str) -> Result<(), String>;

/// Calls into other lanes' modules without a compile-time dependency (build/status/X3.md).
#[derive(Default)]
pub struct Hooks {
    /// X4: A4 and the system snapshot, once per `change.open` after X3's checks; `first`: the change opens a group.
    pub admit: Vec<AdmitFn>,
    /// X5: the change's caps `[{id, ver, prev_ver, op, evidence}]`.
    pub caps: Option<CapsFn>,
    /// X5: one closed turn.
    pub turn_closed: Vec<fn(&Root, &Turn)>,
    /// X6: a `direct` rollback's caps, and whether the repeat ban (30 days) applies.
    pub direct: Vec<fn(&Root, &[String], bool)>,
    /// X4: R2, restore a group's system snapshot with the zone stopped.
    pub restore: Option<RestoreFn>,
}

static HOOKS: OnceLock<Mutex<Hooks>> = OnceLock::new();

pub fn hooks() -> &'static Mutex<Hooks> {
    HOOKS.get_or_init(Mutex::default)
}

/// The sentinel's state in `Root` (`Root.sentinel`).
#[derive(Default)]
pub struct Shared {
    eng: Mutex<Option<Engine>>,
    /// `change.open` requests one at a time (the hooks run without the engine lock).
    open: Mutex<()>,
    /// Owner-line qids already seen (a redelivery is not a new line) and instances the Root signalled itself.
    seen: Mutex<VecDeque<String>>,
    killed: Mutex<VecDeque<(String, i64)>>,
    /// The last save of `state/sentinel.json` and of `extend.json`.
    saved: Mutex<Option<Instant>>,
    /// A group closed or `lkg` moved: X4's `settled_tick` and `prune` are due (`after_close` runs one at a time).
    closed: AtomicBool,
    after_close: AtomicBool,
}

/// The Root's view of the world for one engine call, read before the engine lock is taken.
fn ctx(root: &Root) -> Ctx {
    let now = now_ms();
    let (level, running, day, free_mode) = {
        let st = lock(&root.st);
        let day = st.charter.as_ref().map_or(now.div_euclid(86_400_000) * 86_400_000, |l| crate::egress::owner_day(&l.ch, now));
        let free = st.charter.as_ref().is_some_and(|l| crate::charter::effective(&l.ch)["extend"]["mode"] == "free");
        (st.level.level, st.slot == crate::sup::Slot::Running, day, free)
    };
    let running = running && root.sup.kernel().is_some_and(|k| k.ready.load(std::sync::atomic::Ordering::SeqCst));
    let current = lock(&root.sup.ptrs).current.clone();
    Ctx { now, level, current, running, day, free: crate::sys::free_bytes(&root.p.ih).ok(), free_mode }
}

/// Charter `[extend]` with the owner's grant overlay (X7's `grant`), read before the engine lock (`grant` takes `st`).
fn limits(root: &Root, level: i64) -> engine::Limits {
    let b = crate::grant::budget(root, level);
    let w = |k: bool| {
        let (turns, max_ms, min_ms) = crate::grant::win(root, k);
        engine::Win { turns: turns.max(1) as usize, max_ms, min_ms, quiet: engine::WIN.quiet }
    };
    engine::Limits { budget: (b.open_groups.max(0) as usize, b.changes_day.max(0) as u32, b.group_max.max(1) as usize, b.kernel_day.max(0) as u32),
        win: w(false), kwin: w(true), burst_floor: crate::grant::burst_floor_musd(root) }
}

/// One engine call: the context first, then the engine under its lock, then the actions with no lock held.
fn with<T>(root: &Root, f: impl FnOnce(&mut Engine, &Ctx) -> (T, Vec<Act>)) -> T {
    let cx = ctx(root);
    let lim = limits(root, cx.level);
    let (t, acts) = {
        let mut g = lock(&root.sentinel.eng);
        let e = g.get_or_insert_with(|| load(root, &cx));
        e.lim = lim;
        f(e, &cx)
    };
    run(root, acts);
    t
}

fn go(root: &Root, f: impl FnOnce(&mut Engine, &Ctx) -> Vec<Act>) {
    with(root, |e, cx| ((), f(e, cx)));
}

fn run(root: &Root, acts: Vec<Act>) {
    let (mut save, mut closed) = (false, false);
    for a in acts {
        match a {
            Act::Append(kind, data) => {
                closed |= matches!(kind, "change_close" | "lkg");
                drop(root.append(kind, data));
            }
            Act::Event(ev, data) => {
                if let Some(k) = root.sup.kernel() {
                    k.event(ev, data);
                }
            }
            Act::Alert(code, msg) => root.alert(code, &msg),
            Act::Restart { to, reason, mut cause, restore } => {
                cause["restore"] = json!(restore);
                root.sup.send(Ev::Restart(Plan { target: to.map(|_| "parent"), reason, cause, panic: Value::Null, mode: None }));
            }
            Act::TurnClosed(t) => {
                let fs = lock(hooks()).turn_closed.clone();
                fs.iter().for_each(|f| f(root, &t));
            }
            Act::Direct(caps, repeat) => {
                let fs = lock(hooks()).direct.clone();
                fs.iter().for_each(|f| f(root, &caps, repeat));
            }
            Act::Save => save = true,
        }
    }
    if save {
        persist(root);
    }
    if closed {
        root.sentinel.closed.store(true, SeqCst); // `tick` runs X4's `settled_tick` and `prune` off this thread
    }
}

/// The state file (0600): rings, turns, windows. The open groups and the rollback history are re-folded from the
/// lineage at load, which stays authoritative.
fn persist(root: &Root) {
    let body = lock(&root.sentinel.eng).as_ref().and_then(|e| serde_json::to_string(e).ok());
    if let Some(b) = body
        && let Err(e) = util::write_atomic(&root.p.r("state/sentinel.json"), b.as_bytes(), 0o600)
    {
        util::log(&root.p.rh, "error", &format!("sentinel state: {e}"));
    }
    let mut v = view(root);
    if let (Some(o), Value::Object(sel)) = (v.as_object_mut(), crate::select::view(root)) {
        o.extend(sel); // X6: archive_due, quarantine_debt, quarantined
    }
    if v["groups"].as_array().is_some_and(|g| !g.is_empty()) || ctx(root).free_mode || root.p.i("root/extend.json").exists() {
        let _ = util::write_atomic(&root.p.i("root/extend.json"), format!("{}\n", util::icj(&v).unwrap_or_default()).as_bytes(), 0o644);
    }
    *lock(&root.sentinel.saved) = Some(Instant::now());
}

/// The engine at first use: `state/sentinel.json` for the statistics, the lineage for groups and history.
fn load(root: &Root, cx: &Ctx) -> Engine {
    let mut e: Engine = std::fs::read_to_string(root.p.r("state/sentinel.json")).ok().and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| Engine::new(cx.current.as_deref()));
    fold(&mut e, &crate::lineage::records(&root.p.rh).unwrap_or_default());
    e
}

/// Re-folds the open groups, the outcomes, the re-admission history and the per-day counts from `change_open` and
/// `change_close` records, keeping the saved window statistics of groups that are still open.
pub fn fold(e: &mut Engine, recs: &[Value]) {
    let saved: BTreeMap<u64, engine::Group> = e.groups.drain(..).map(|g| (g.seq, g)).collect();
    let mut open: BTreeMap<u64, engine::Group> = BTreeMap::new();
    let mut shas: BTreeMap<u64, Vec<(String, Vec<String>)>> = BTreeMap::new();
    for r in recs {
        let (d, at) = (&r["data"], r["at"].as_str().and_then(util::parse_rfc3339).unwrap_or(0));
        match r["kind"].as_str() {
            Some("change_open") => {
                let Some(seq) = d["group"].as_u64() else { continue };
                e.next_group = e.next_group.max(seq + 1);
                let caps: Vec<String> = d["caps"].as_array().into_iter().flatten().filter(|c| c["op"] != "remove")
                    .filter_map(|c| c["id"].as_str().map(String::from)).collect();
                let sha = d["sha"].as_str().unwrap_or("").to_string();
                shas.entry(seq).or_default().push((sha.clone(), caps.clone()));
                let ch = engine::Change { sha, scope: d["scope"].as_str().unwrap_or("").into(), caps, readmit: d["readmit"] == true };
                let g = open.entry(seq).or_insert_with(|| engine::Group { seq, kernel: d["scope"] == "k", opened: at,
                    parent: d["parent_tuple"].as_str().unwrap_or("").into(), snapshot: d["snapshot"].as_str().map(String::from), ..Default::default() });
                g.changes.retain(|c| !c.caps.iter().any(|x| ch.caps.contains(x)));
                g.changes.push(ch);
                (g.last_join, g.tuple) = (at, d["tuple"].as_str().unwrap_or("").into());
            }
            Some("change_close") => {
                let Some(seq) = d["group"].as_u64() else { continue };
                open.remove(&seq);
                let outcome = d["outcome"].as_str().unwrap_or("").to_string();
                let status = d["attribution"].as_str().map(String::from).or_else(|| match outcome.as_str() {
                    "collateral" => Some("collateral".into()),
                    "rolled_back" => Some("vetoed".into()), // H12: unattributed, re-admissible once
                    _ => None,
                });
                for (sha, caps) in shas.get(&seq).into_iter().flatten() {
                    if let Some(st) = &status {
                        let readmitted = e.hist.get(sha).is_some_and(|h| h.readmitted) || recs.iter().any(|x| x["kind"] == "change_open" && x["data"]["sha"] == sha.as_str() && x["data"]["readmit"] == true);
                        e.hist.insert(sha.clone(), engine::Hist { status: st.clone(), readmitted });
                    }
                    if status.as_deref() == Some("direct") {
                        caps.iter().for_each(|c| e.directs.entry(c.clone()).or_default().push(at));
                    }
                }
                e.outcomes.insert(seq, outcome);
            }
            Some("kernel_upgrade") if d["stage"] == "probation_pass" => {
                if let Some(g) = d["group"].as_u64().and_then(|s| open.get_mut(&s)) {
                    g.ksettled = true; // X8: body groups may open beside it
                }
            }
            _ => {}
        }
    }
    for (seq, mut g) in open {
        if let Some(s) = saved.get(&seq).filter(|s| s.last_join == g.last_join) {
            (g.win, g.reqs, g.looks, g.turn, g.free_at_open) = (s.win.clone(), s.reqs, s.looks, s.turn, s.free_at_open);
        }
        e.groups.push(g);
    }
    for v in e.directs.values_mut() {
        v.sort_unstable();
        v.dedup();
    }
}

// ------------------------------------------------------------------------------------------------ hooks

/// `link.rs` `deliver`: a frame the Root relayed to a ready kernel (`line` starts or steers a turn; `interrupt`).
pub fn relayed(root: &Root, qid: &str, frame: &Value) {
    match frame["t"].as_str() {
        Some("interrupt") => go(root, |e, cx| {
            e.interrupt(cx);
            Vec::new()
        }),
        Some("line") => {
            {
                let mut seen = lock(&root.sentinel.seen);
                if seen.iter().any(|q| q == qid) {
                    return; // a redelivery after a restart
                }
                seen.push_back(qid.into());
                if seen.len() > 256 {
                    seen.pop_front();
                }
            }
            let text = frame["text"].as_str().unwrap_or("").to_string();
            go(root, |e, cx| e.line(cx, qid, &text));
        }
        _ => {}
    }
}

/// `access.rs` `workspace.set`: a new owner session (each `init` launch). The sentinel's units never span sessions.
pub fn session_started(root: &Root) {
    go(root, |e, _| {
        e.session_started();
        Vec::new()
    });
}

/// `link.rs` `ack`: the kernel holds the line.
pub fn acked(root: &Root, qid: &str) {
    go(root, |e, _| {
        e.ack(qid);
        Vec::new()
    });
}

/// `link.rs` `line`, before anything else but `/panic`'s own path: the Root lines of ADR-011. True: consumed.
pub fn owner_text(root: &Root, text: &str) -> bool {
    let mut w = text.split_whitespace();
    match w.next() {
        Some("/rollback") => {
            let arg = w.next().map(String::from);
            rollback_line(root, arg.as_deref());
            true
        }
        Some("/undo") => {
            go(root, |e, cx| {
                e.interrupt(cx); // a correction of the current or just-ended owner turn
                Vec::new()
            });
            false
        }
        _ => false,
    }
}

/// `/rollback [sha|cap|settled|lkg|twin]` (ADR-011 §3.3 H3, Threat model "drift").
fn rollback_line(root: &Root, arg: Option<&str>) {
    if let Some(name @ ("settled" | "lkg" | "twin")) = arg {
        let to = crate::tuple::get(root)[name]["id"].as_str().map(String::from).or_else(|| {
            let p = lock(&root.sup.ptrs).clone();
            match name { "settled" => p.settled, "lkg" => p.lkg, "twin" => p.twin, _ => None } // twin: X9
        });
        let Some(to) = to else {
            return root.notice("warn", &format!("[root] /rollback {name}: there is no {name} tuple yet"));
        };
        go(root, |e, cx| {
            e.interrupt(cx);
            let mut acts = e.trigger(cx, "H3", 1, 1, engine::Pick::Collateral, "rollback_all");
            acts.retain(|a| !matches!(a, Act::Event("extend.rollback", _)));
            e.rollback = None;
            acts.push(Act::Restart { to: Some(to.clone()), reason: "rollback_system", cause: json!({"to": to, "trigger": "owner"}), restore: None });
            acts
        });
        return root.notice("info", &format!("[root] rolling the whole system back to the {name} tuple"));
    }
    go(root, |e, cx| {
        e.interrupt(cx);
        e.reversal(cx, arg, "/rollback")
    });
}

/// `link.rs` `after`: every record the Root appended (its own and the kernel's).
pub fn record(root: &Root, rec: &Value) {
    let d = &rec["data"];
    match rec["kind"].as_str().unwrap_or("") {
        "zone_exit" if d["class"] == "crash" => go(root, |e, cx| e.kernel_crash(cx)),
        "zone_exit" | "stop" | "resume" => go(root, |e, cx| {
            e.zone_stopping(cx);
            Vec::new()
        }),
        "ceiling_hit" if d["demotes"] == true => go(root, |e, cx| e.scope(cx)),
        "kernel_upgrade" if d["stage"] == "probation_pass" => {
            if let Some(g) = d["group"].as_u64() {
                go(root, |e, _| e.ksettle(g)); // X8: the kernel change passed probation
            }
        }
        "escapee" => go(root, |e, cx| e.scope(cx)),
        "body_rollback" => {
            let to_sha = d["to_sha"].as_str().unwrap_or("").to_string();
            let rh = root.p.rh.clone();
            // X8: a kernel confirms only a rollback that keeps it; a target with another kernel is the Root's to perform
            let kernel = lock(&root.sup.ptrs).current.clone().and_then(|c| crate::tuple::load(&rh, &c)).map(|t| t["kernel"].clone());
            let body = move |id: &str| crate::tuple::load(&rh, id).filter(|t| kernel.as_ref().is_none_or(|k| *k == t["kernel"]))
                .and_then(|t| t["body"]["sha"].as_str().map(String::from));
            go(root, |e, cx| e.confirmed(cx, &to_sha, &body));
        }
        "rollback" => {
            let to = d["to"].as_str().unwrap_or("").to_string();
            go(root, |e, _| {
                if e.rollback.as_ref().is_some_and(|p| p.to == to) {
                    e.rollback = None; // the Root's own whole-system rollback reached the target
                }
                Vec::new()
            });
        }
        _ => {}
    }
}

/// `sup.rs` read loop: the kernel's `hb{busy}`.
pub fn hb(root: &Root, busy: bool) {
    go(root, |e, _| {
        e.hb(busy);
        Vec::new()
    });
}

/// `sup.rs` `handle`: a `spawn` the broker granted (H6 churn).
pub fn spawned(root: &Root) {
    go(root, |e, cx| e.spawned(cx));
}

/// An instance the Root is about to signal itself (a kernel `kill` request, a memory-watchdog instance kill): its
/// death by that signal is not a crash.
pub fn killing(root: &Root, inst: &str) {
    let mut k = lock(&root.sentinel.killed);
    k.push_back((inst.into(), now_ms()));
    while k.len() > 64 {
        k.pop_front();
    }
}

/// `sup.rs` `exited` (the reaper): a body instance ended. A crash is an exit code ∉ {0, 75, 78} or a signal the Root
/// did not send (H1).
pub fn exited(root: &Root, inst: &str, kind: &str, code: Option<i32>, sig: Option<i32>) {
    if kind != "body" {
        return;
    }
    let ours = lock(&root.sentinel.killed).iter().any(|(i, _)| i == inst);
    let crash = match (code, sig) {
        (Some(c), _) => ![0, 75, 78].contains(&c),
        (None, Some(_)) => !ours,
        (None, None) => false,
    };
    go(root, |e, cx| e.body_exit(cx, crash));
}

/// `memwatch.rs` zone sink: `mem_cap` and `disk_floor` kills (H6).
pub fn mem_event(root: &Root, v: &Value) {
    let ev = v["event"].as_str().unwrap_or("").to_string();
    if !matches!(ev.as_str(), "mem_cap" | "disk_floor") {
        return;
    }
    if let Some(i) = v["instance"].as_str().filter(|_| v["scope"] == "instance") {
        killing(root, i);
    }
    go(root, |e, cx| e.mem_event(cx, &ev));
}

/// `egress.rs` `forward`, after the live `start` record: a zone request begins.
pub fn egress_start(root: &Root, id: &str, req_bytes: u64) {
    go(root, |e, cx| e.req_start(cx, id, req_bytes));
}

/// `egress.rs` `finish`: a zone request ended. H9 counts finish class `error` and upstream 400/422 (a malformed
/// request the zone built); provider 429/5xx and Root refusals are not the zone's.
pub fn egress_end(root: &Root, id: &str, cost_musd: u64, status: u16, finish: Option<&str>) {
    let error = finish == Some("error") || status == 400 || status == 422;
    go(root, |e, cx| e.req_end(cx, id, cost_musd, error));
}

/// X5's tap: the capabilities one zone egress request used (by its egress id), with their uses and evidence. They
/// land on that request's turn; the same id with both evidences keeps `root` and sums the uses.
pub fn req_caps(root: &Root, req_id: &str, caps: &[(String, u32, &'static str)]) {
    go(root, |e, _| {
        e.req_caps(req_id, caps);
        Vec::new()
    });
}

/// `sup.rs` run loop, every 200 ms: turn ends, deafness, window closes, the rollback deadline. After a close, X4's
/// `settled_tick` and `prune` run on their own thread (they read the whole lineage, and `prune` waits for a concurrent
/// snapshot `take` or `restore`), so the supervisor never waits on them.
pub fn tick(root: &Arc<Root>) {
    go(root, |e, cx| e.tick(cx));
    if root.sentinel.closed.load(SeqCst) && !root.sentinel.after_close.swap(true, SeqCst) {
        root.sentinel.closed.store(false, SeqCst);
        let r = root.clone();
        std::thread::spawn(move || {
            crate::snapshot::settled_tick(&r); // `settled` may move after a close or an `lkg` move
            crate::snapshot::prune(&r); // snapshots of closed groups are released
            r.sentinel.after_close.store(false, SeqCst);
        });
    }
}

// ------------------------------------------------------------------------------------------------ the control link

fn no(code: &'static str, m: impl Into<String>) -> Err3 {
    (code, m.into(), Value::Null)
}

fn hexish(s: &str) -> bool {
    (7..=64).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_hexdigit())
}

/// `change.veto{group, reason}` (H12): always honoured, never counted toward the record.
pub fn veto(root: &Root, a: &Value) -> Result<Value, Err3> {
    let group = a["group"].as_u64().ok_or_else(|| no("root.args", "group is an integer"))?;
    let reason: String = a["reason"].as_str().unwrap_or("").chars().filter(|c| !c.is_control()).take(200).collect();
    util::log(&root.p.rh, "info", &format!("change.veto group {group}: {reason}"));
    with(root, |e, cx| match e.veto(cx, group) {
        Ok(acts) => (Ok(json!({})), acts),
        Err((c, m)) => (Err(no(c, m)), Vec::new()),
    })
}

/// `change.open{sha, from_sha, scope, candidate{body:{binary, sha, t0?}, ext:{components, set}, kernel?}, caps,
/// prefix_change}` → `{seq, group, snapshot, window}` (ADR-011 §2.3 step 3). `sup.rs` runs it on its own thread:
/// X4's A4 probe can take minutes, and the control-link reader keeps serving heartbeats meanwhile.
pub fn change_open(root: &Arc<Root>, a: &Value) -> Result<Value, Err3> {
    crate::benchmark::deny_change(root.benchmark.as_ref())?;
    let _one = lock(&root.sentinel.open);
    let (sha, from) = (a["sha"].as_str().unwrap_or(""), a["from_sha"].as_str().unwrap_or(""));
    let scope = a["scope"].as_str().filter(|s| ["t0", "t1", "t2", "dep", "k"].contains(s)).ok_or_else(|| no("root.args", "scope: t0|t1|t2|dep|k"))?;
    if !hexish(sha) || !(from.is_empty() || hexish(from)) {
        return Err(no("root.args", "sha and from_sha are commit ids (hex)"));
    }
    let c = &a["candidate"];
    let cur = lock(&root.sup.ptrs).current.clone().and_then(|id| crate::tuple::load(&root.p.rh, &id));
    let kernel = if scope == "k" { c["kernel"].clone() } else { cur.as_ref().map(|t| t["kernel"].clone()).unwrap_or_default() };
    if !kernel.is_object() || !c["body"].is_object() {
        return Err(no("root.args", "candidate.body (and for scope k candidate.kernel) is required; no current tuple to take the kernel from"));
    }
    let mut body = json!({"binary": c["body"]["binary"].as_str().unwrap_or(""), "sha": c["body"]["sha"].as_str().unwrap_or(sha)});
    if let Some(t0) = c["body"]["t0"].as_str() {
        body["t0"] = json!(t0);
    }
    let t = json!({"body": body, "ext": {"components": c["ext"]["components"].as_array().cloned().unwrap_or_default(), "set": c["ext"]["set"]},
        "kernel": kernel, "v": 1});
    // A0: the candidate's blobs are in the Root's store with the claimed digests, under the active charter
    let t0_ok = body["t0"].as_str().is_none_or(|s| crate::tuple::blob(&root.p.rh, s).is_some_and(|b| b.is_file()));
    if !crate::tuple::verify(&root.p.rh, &t) || !t0_ok || lock(&root.st).charter.is_none() {
        root.alert("boundary", &format!("[root] change {}: a candidate blob is missing or does not match its digest (A0)", &sha[..sha.len().min(12)]));
        return Err(no("root.boundary", "A0: a candidate blob is missing from the store or does not match its digest"));
    }
    let caps_hook = lock(hooks()).caps;
    let caps = match caps_hook {
        Some(f) => f(root, a)?,
        None => json!(a["caps"].as_array().cloned().unwrap_or_default().into_iter().filter(|c| c["id"].is_string())
            .map(|mut c| { c["evidence"] = json!("zone"); c }).collect::<Vec<_>>()),
    };
    let ids: Vec<String> = caps.as_array().into_iter().flatten().filter(|c| c["op"] != "remove").filter_map(|c| c["id"].as_str().map(String::from)).collect();
    let tid = crate::tuple::id_of(&t);
    let open = Open { sha: sha.into(), scope: scope.into(), caps: ids, tuple: tid.clone() };
    let plan = with(root, |e, cx| (e.plan(cx, &open), Vec::new())).map_err(|(c, m)| no(c, m))?;
    // X4: A4 and the system snapshot (first change of a group only)
    let mut args = a.clone();
    (args["group"], args["parent_tuple"]) = (json!(plan.group), json!(plan.parent));
    let mut checks = json!({"A0": "pass", "A4": "skip", "A5": "pass"});
    let mut snapshot = Value::Null;
    let admit = lock(hooks()).admit.clone();
    for f in admit {
        let r = f(root, &args, plan.first)?;
        for c in ["A4", "K"] {
            if r.get(c).is_some() {
                checks[c] = r[c].clone(); // K: X8's Root conformance probe (scope k)
            }
        }
        if r.get("snapshot").is_some_and(|s| !s.is_null()) {
            snapshot = r["snapshot"].clone();
        }
    }
    if !plan.first {
        snapshot = with(root, |e, _| (e.groups.iter().find(|g| g.seq == plan.group).and_then(|g| g.snapshot.clone()), Vec::new())).map_or(Value::Null, Value::from);
    }
    crate::tuple::save(&root.p.rh, &t).map_err(|e| no("root.internal", format!("tuple: {e}")))?;
    let n_open = with(root, |e, _| (e.groups.len(), Vec::new()));
    let level = lock(&root.st).level.level; // its own statement: `with` takes `st` too
    let rec = root.append("change_open", json!({"caps": caps, "checks": checks, "from_sha": from, "group": plan.group, "level": level,
        "open_groups": n_open + usize::from(plan.first), "parent_tuple": plan.parent, "readmit": plan.readmit, "scope": scope, "sha": sha,
        "snapshot": snapshot, "tuple": tid})).ok_or_else(|| no("root.internal", "change_open: the lineage append failed"))?;
    // X8: a kernel change goes live by its swap (`sup/kupgrade.rs` appends its `tuple`), so `current` stays put here
    if scope != "k" && lock(&root.sup.ptrs).current.as_deref() != Some(tid.as_str()) {
        root.append("tuple", json!({"id": tid, "tuple": t}));
    }
    let snap = snapshot.as_str().map(String::from);
    go(root, |e, cx| e.commit(cx, &open, &plan, snap));
    let level_now = lock(&root.st).level.level; // its own statement: `limits` takes `st` too
    let w = { let l = limits(root, level_now); if scope == "k" { l.kwin } else { l.win } };
    Ok(json!({"group": plan.group, "seq": rec["seq"], "snapshot": snapshot,
        "window": {"max_h": w.max_ms / engine::HOUR_MS, "min_h": w.min_ms / engine::HOUR_MS, "quiet": w.quiet, "turns": w.turns}}))
}

// ------------------------------------------------------------------------------------------------ rollback

/// `sup.rs` `start` with `Plan.target = "parent"`: §3.7.3 steps 2–3 to the tuple in `cause.to` (X4's R2 restore
/// first when `cause.restore` names a snapshot and the zone holds nothing), else LKG's fallbacks.
pub fn rollback_to(root: &Root, cause: &Value, reason: &str) -> Result<(String, Value), String> {
    if let Some(snap) = cause["restore"].as_str() {
        let t = Instant::now();
        while root.zone.count(None) > 0 && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
        let restore = lock(hooks()).restore;
        match restore.map(|f| f(root, snap, "H1")) {
            Some(Ok(())) => {}
            Some(Err(e)) => root.alert("restore", &format!("[root] R2 restore of snapshot {snap} failed: {e}")),
            None => util::log(&root.p.rh, "warn", "R2: no restore hook is registered (X4)"),
        }
    }
    let from = lock(&root.sup.ptrs).current.clone();
    let id = cause["to"].as_str().unwrap_or("");
    match crate::tuple::load(&root.p.rh, id).filter(|t| crate::tuple::verify(&root.p.rh, t)) {
        Some(t) => {
            crate::tuple::install_kernel(&root.p, &t).map_err(|e| format!("install {id}: {e}"))?;
            Ok((id.to_string(), json!({"from": from, "reason": reason, "target": "parent", "to": id})))
        }
        None => crate::tuple::rollback(root, "lkg", reason),
    }
}

/// X6 (the 72 h quarantine rollback) and others: roll back to `to` through the kernel, with the 120 s fallback.
#[allow(dead_code)] // the API for X5, X6 and X7 (build/status/X3.md)
pub fn rollback(root: &Root, to: &str, reason: &str) {
    go(root, |e, cx| e.rollback_to(cx, to, &[], reason, None, false));
}

// ------------------------------------------------------------------------------------------------ views

/// The last 120 closed turns, owner and background (X6's correlation).
#[allow(dead_code)] // the API for X5, X6 and X7 (build/status/X3.md)
pub fn turns(root: &Root) -> Vec<Turn> {
    with(root, |e, _| (e.recent.iter().cloned().collect(), Vec::new()))
}

/// X5: up to 16 cap ids used so far in the open turn (`egress_action` names them, ADR-011 §5.3).
pub fn turn_caps(root: &Root) -> Vec<String> {
    with(root, |e, _| (e.turn.as_ref().map(|t| t.caps.keys().take(16).cloned().collect()).unwrap_or_default(), Vec::new()))
}

/// The open groups.
pub fn open_groups(root: &Root) -> Vec<Value> {
    with(root, |e, _| (e.groups.iter().map(group_json).collect(), Vec::new()))
}

fn group_json(g: &engine::Group) -> Value {
    json!({"caps": g.changes.iter().flat_map(|c| c.caps.clone()).collect::<Vec<_>>(), "changes": g.changes.len(), "kernel": g.kernel,
        "last_join": rfc3339(g.last_join), "opened_at": rfc3339(g.opened), "parent_tuple": g.parent, "seq": g.seq, "snapshot": g.snapshot,
        "tuple": g.tuple, "turns": g.win.len()})
}

/// The X3 part of `INIT_HOME/root/extend.json`: groups and windows, the freeze, the rings' fill, a pending rollback.
pub fn view(root: &Root) -> Value {
    let g = lock(&root.sentinel.eng);
    let Some(e) = g.as_ref() else { return json!({"v": 1}) };
    json!({"freeze_until": e.freeze_until.map(rfc3339), "groups": e.groups.iter().map(group_json).collect::<Vec<_>>(),
        "ring": {"requests": e.req_ring.len(), "requests_m": engine::REQ_RING, "turns": e.ring.len(),
            "units": e.ring.iter().map(|t| t.unit).collect::<std::collections::BTreeSet<_>>().len(), "units_m": engine::RING_M},
        "rollback": e.rollback.as_ref().map(|p| json!({"deadline": rfc3339(p.deadline), "groups": p.groups, "reason": p.reason, "to": p.to})),
        "v": 1})
}

/// `sup.rs` `kernel_append`: a record the kernel reported (`body_rollback` confirms an R1 rollback).
pub fn kernel_record(root: &Root, kind: &str, data: &Value) {
    if kind == "body_rollback" {
        record(root, &json!({"data": data, "kind": kind}));
    }
}

/// `panic.rs`, after the panic records (they are appended under the panic's own lineage lock, so `after` misses them).
pub fn panicked(root: &Root) {
    go(root, |e, cx| e.panic(cx));
}
