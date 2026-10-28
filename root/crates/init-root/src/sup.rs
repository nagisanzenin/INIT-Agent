//! R1: supervising the kernel (root/root.md §3.6, §3.7.3, §4.1): the slots, the control link on fds 3/4,
//! the boot block, readiness and heartbeats, the kernel's requests, crash-loop escalation, system safe
//! mode and its self-clear after `safe_auto_clear_s` (D4), `Halted` retries, stop and start, and rollback
//! to LKG or audited-good. The escalation rule (`escalate`, `backoff_ms`) is pure and unit-tested.
//! Panic (panic.rs) never goes through this thread: it kills the held instances itself.

use crate::charter::RootCfg;
use crate::link::{Root, lock};
use crate::tuple::{self, Ptrs};
use crate::util::{self, icj, now_ms, rfc3339};
use crate::zone::{self, Err3};
use crate::{install, ledger, sys};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::{UnixDatagram, UnixStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub mod klog;
pub mod kprobe; // X8 (ADR-011 §1.2): the Root conformance probe on `kupgrade-smoke`
pub mod kupgrade; // X8: the kernel lane's swap (root.md §3.8 phases 1–6)

/// §3.6.1, without the upgrade (`Swapping`) and disk-pause (`Paused`) slots, which are not built yet.
#[derive(Clone, Debug, PartialEq)]
pub enum Slot {
    Starting { hello: bool },
    Running,
    /// `then`: what follows the kernel's exit; `None` is the owner's stop (→ `Stopped`).
    Stopping { then: Option<Plan> },
    Backoff { attempt: u32 },
    Stopped,
    Panicked { at: i64, channel: String },
    Halted { reason: &'static str, retry_at: Option<i64> },
}

impl Slot {
    pub fn name(&self) -> &'static str {
        match self {
            Slot::Starting { .. } => "starting",
            Slot::Running => "running",
            Slot::Stopping { .. } => "stopping",
            Slot::Backoff { .. } => "backoff",
            Slot::Stopped => "stopped",
            Slot::Panicked { .. } => "panicked",
            Slot::Halted { .. } => "halted",
        }
    }
}

/// `Halted` reasons with no automatic retry (§3.6.6): integrity failures.
pub const INTEGRITY: [&str; 4] = ["layout", "tooling", "lineage", "no_tuple"];

/// How the next kernel starts: an optional rollback target, the boot reason and cause, a mode change.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub target: Option<&'static str>,
    pub reason: &'static str,
    pub cause: Value,
    pub panic: Value,
    pub mode: Option<&'static str>,
}

impl Plan {
    pub fn plain(reason: &'static str) -> Plan {
        Plan { target: None, reason, cause: Value::Null, panic: Value::Null, mode: None }
    }
}

pub enum Ev {
    Exit { inst: String, code: Option<i32> },
    Lost(String),
    Ready(String),
    BodySafe,
    /// R7 holds a pending demotion trigger (ledger.rs `tick`).
    Demote,
    Start(Plan),
    /// ADR-011 (X3): the sentinel's whole-system rollback (`target: "parent"`, `cause.to`) or a kernel restart (H2).
    Restart(Plan),
    Stop,
    Shutdown,
}

/// What the escalation rule decides after a crash (§3.6.4).
#[derive(Debug, PartialEq)]
pub enum Esc {
    Backoff(u64),
    RollbackLkg,
    Demote,
    Halt,
}

/// `min(backoff_max_ms, backoff_initial_ms × 2^(n−1))` (§3.6.3).
pub fn backoff_ms(c: &RootCfg, n: u32) -> u64 {
    let init = c.backoff_initial_ms.max(0) as u64;
    init.saturating_mul(1u64 << n.saturating_sub(1).min(30)).min(c.backoff_max_ms.max(0) as u64)
}

/// §3.6.4: adds the crash at `now` (Root clock, ms) to the sliding window and decides.
pub fn escalate(w: &mut VecDeque<i64>, now: i64, c: &RootCfg, safe: bool, on_lkg: bool, attempt: u32) -> Esc {
    w.push_back(now);
    while w.front().is_some_and(|&t| now - t > c.crash_window_s * 1000) {
        w.pop_front();
    }
    let n = w.len() as i64;
    match (safe, on_lkg) {
        (true, _) if n > c.safe_crash_limit => Esc::Halt,
        (false, false) if n > c.crash_limit => Esc::RollbackLkg,
        (false, true) if n > c.crash_limit => Esc::Demote,
        _ => Esc::Backoff(backoff_ms(c, attempt)),
    }
}

/// The supervisor's state the other threads read: the event channel, the kernel link, the pointers, the
/// relayed owner lines (`qid`, text; for `owner` records) and the kernel's alert rate window.
#[derive(Default)]
pub struct Shared {
    tx: Mutex<Option<Sender<Ev>>>,
    pub k: Mutex<Option<Arc<Kctl>>>,
    pub ptrs: Mutex<Ptrs>,
    boot_seq: AtomicU64,
    pub relayed: Mutex<VecDeque<(String, String)>>,
    rates: Mutex<BTreeMap<&'static str, VecDeque<i64>>>,
    /// `proposal.file`: proposals filed so far and the times of the last day's, seeded once from the lineage.
    proposals: Mutex<Option<(u64, VecDeque<i64>)>>,
    /// While the install-mode kernel runs (§4.3 step 8): the kernel half of the genesis tuple it is forming.
    pub install: Mutex<Option<Value>>,
    /// `state/root.json` `crash` (§4.5), seeded once from the previous file (`crash_json`).
    crash: Mutex<Option<Crash>>,
    /// X8: the kernel swap in progress (`kupgrade.rs`).
    pub swap: Mutex<kupgrade::Swap>,
}

/// The supervisor's backoff and crash window as `state/root.json` keeps them (§4.5; WP21 D10), so a Root
/// restart resumes the escalation where it stood instead of starting from zero.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Crash {
    pub backoff_until: Option<i64>,
    pub consecutive: u32,
    pub window: Vec<i64>,
}

impl Crash {
    fn from_json(v: &Value) -> Crash {
        let t = |x: &Value| x.as_str().and_then(util::parse_rfc3339);
        Crash { backoff_until: t(&v["backoff_until"]), consecutive: v["consecutive"].as_u64().unwrap_or(0).min(64) as u32,
            window: v["window"].as_array().into_iter().flatten().filter_map(t).collect() }
    }
    fn json(&self) -> Value {
        json!({"backoff_until": self.backoff_until.map(rfc3339), "consecutive": self.consecutive, "window": self.window.iter().map(|&t| rfc3339(t)).collect::<Vec<_>>()})
    }
}

impl Shared {
    pub fn send(&self, ev: Ev) {
        if let Some(tx) = lock(&self.tx).as_ref() {
            let _ = tx.send(ev);
        }
    }
    pub fn boot_seq(&self) -> u64 {
        self.boot_seq.load(SeqCst)
    }
    pub fn kernel(&self) -> Option<Arc<Kctl>> {
        lock(&self.k).clone()
    }
    /// The `crash` block of `state/root.json`. The first call (zone.rs `cold_start`'s save, before it rewrites
    /// the file) seeds it from the previous Root's file.
    pub fn crash_json(&self, rh: &std::path::Path) -> Value {
        let mut g = lock(&self.crash);
        g.get_or_insert_with(|| {
            let prev: Value = std::fs::read_to_string(rh.join("state/root.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
            Crash::from_json(&prev["crash"])
        }).json()
    }
    fn crash_state(&self, rh: &std::path::Path) -> Crash {
        self.crash_json(rh);
        lock(&self.crash).clone().unwrap_or_default()
    }
    fn set_crash(&self, c: Crash) {
        *lock(&self.crash) = Some(c);
    }
    /// At most `n` per hour of `what` (kernel alerts).
    fn rate(&self, what: &'static str, n: i64) -> bool {
        let now = now_ms();
        let mut r = lock(&self.rates);
        let w = r.entry(what).or_default();
        while w.front().is_some_and(|&t| now - t > 3_600_000) {
            w.pop_front();
        }
        let ok = (w.len() as i64) < n;
        if ok {
            w.push_back(now);
        }
        ok
    }
}

/// The Root's end of one kernel's control link (§4.1.1).
pub struct Kctl {
    pub inst: String,
    pub pid: i32,
    w: Mutex<UnixStream>,
    fdc: UnixDatagram,
    next: AtomicU64,
    pub ready: AtomicBool,
    /// The last `hb`'s `busy` (the live body is in a turn).
    pub busy: AtomicBool,
    spawned: Instant,
    last_hb: Mutex<Instant>,
    pub plan: Plan,
    /// Root request id → owner-queue qid, for `owner.in` acknowledgements; every qid sent to this kernel.
    pub pending: Mutex<(HashMap<String, String>, HashSet<String>)>,
    stash: Mutex<Stash>,
}

impl Kctl {
    fn line(&self, v: &Value) -> bool {
        let Ok(s) = icj(v) else { return false };
        lock(&self.w).write_all(format!("{s}\n").as_bytes()).is_ok()
    }
    pub fn event(&self, ev: &str, data: Value) {
        self.line(&json!({"data": data, "ev": ev, "v": 1}));
    }
    /// A Root → kernel request (`r-<n>`); its answer is matched in the reader.
    pub fn request(&self, cmd: &str, args: Value) -> String {
        let id = format!("r-{}", self.next.fetch_add(1, SeqCst));
        self.line(&json!({"args": args, "cmd": cmd, "id": id, "v": 1}));
        id
    }
    fn send_fds(&self, id: &str, fds: &[i32]) -> Result<(), Err3> {
        let tag = icj(&json!({"id": id, "n": fds.len()})).unwrap_or_default();
        sys::send_fds(self.fdc.as_raw_fd(), tag.as_bytes(), fds).map_err(|e| ("root.internal", format!("fd hand-over: {e}"), Value::Null))
    }
    /// The datagram tagged `id` (sent before its line, §4.1.1); the others wait in the stash (`stash_push`).
    fn take_fds(&self, id: &str) -> Result<Vec<OwnedFd>, Err3> {
        let _ = self.fdc.set_read_timeout(Some(Duration::from_secs(5)));
        let mut st = lock(&self.stash);
        loop {
            stash_prune(&mut st, Instant::now());
            if let Some(i) = st.iter().position(|(t, _, _)| t == id) {
                return Ok(st.remove(i).2);
            }
            let mut buf = [0u8; 256];
            let (n, fds) = sys::recv_fds(self.fdc.as_raw_fd(), &mut buf).map_err(|e| ("root.args", format!("no fds for {id}: {e}"), Value::Null))?;
            stash_push(&mut st, serde_json::from_slice(&buf[..n]).unwrap_or_default(), fds, Instant::now());
        }
    }
}

/// Review F6: a hostile kernel's datagrams cannot pile up descriptors in the Root. A stashed datagram is kept
/// `STASH_KEEP` at most, the stash holds at most `STASH_FDS` descriptors (the oldest go first), and a datagram
/// whose descriptor count is not its tag's `n` is closed at once. Dropping an entry closes its descriptors.
const STASH_FDS: usize = 64;
const STASH_KEEP: Duration = Duration::from_secs(10);
type Stash = Vec<(String, Instant, Vec<OwnedFd>)>;

fn stash_prune(st: &mut Stash, now: Instant) {
    st.retain(|(_, t, _)| now.saturating_duration_since(*t) < STASH_KEEP);
}

fn stash_push(st: &mut Stash, tag: Value, fds: Vec<OwnedFd>, now: Instant) {
    if tag["n"].as_u64() != Some(fds.len() as u64) {
        return; // `fds` close here
    }
    st.push((tag["id"].as_str().unwrap_or("").into(), now, fds));
    while st.iter().map(|e| e.2.len()).sum::<usize>() > STASH_FDS {
        st.remove(0);
    }
}

fn cfg(root: &Root) -> RootCfg {
    lock(&root.st).charter.as_ref().map(|l| l.ch.root.clone()).unwrap_or_default()
}

/// The `hello` result (§4.1.3).
fn boot_block(root: &Root, k: &Kctl) -> Value {
    let p = lock(&root.sup.ptrs).clone();
    let t = p.current.as_ref().and_then(|c| tuple::load(&root.p.rh, c)).unwrap_or_else(|| {
        // install mode: no tuple yet; the kernel half is known, the body is what the kernel is about to build
        let k = lock(&root.sup.install).clone().unwrap_or_default();
        json!({"body": {"binary": "", "sha": ""}, "ext": {"components": [], "set": null}, "kernel": k})
    });
    let access = crate::access::kernel_view(root); // SBX-009 (ADR-009); before `st` is held
    let st = lock(&root.st);
    let ch = st.charter.as_ref().map(|l| &l.ch);
    let (lvl, max) = (st.level.level, ch.map_or(0, |c| c.autonomy.max_level));
    let par = zone::par_limit(&st);
    json!({"access": access, "extend": extend_view(&st), "boot": {"cause": k.plan.cause, "kstate": kupgrade::kstate(&k.plan), "level": lvl, "mode": st.mode, "panic": k.plan.panic, "reason": k.plan.reason,
            "tuple": p.json(), "tuple_body": {"binary": t["body"]["binary"], "ext": t["ext"], "sha": t["body"]["sha"]}, "tuple_kernel": t["kernel"]},
        "charter_sha256": st.charter.as_ref().map(|l| &l.digest), "ladder": {"cap": ledger::cap(max), "evolution_share_max_permille": null, "level": lvl},
        "limits": {"hb_interval_s": ch.map_or(5, |c| c.root.hb_interval_s), "max_instances": ch.map_or(12, |c| c.quotas.max_instances), "parallel_bodies": par},
        "paths": {"egress_sock": root.p.r("run/egress.sock"), "kernel_git": root.p.i("lineage/kernel.git")},
        "root": {"proto": 1, "version": install::VERSION}})
}

/// `ceilings.get` and `ceilings.json`: egress's blocks plus the broker's counts (§4.5), and `extend` (X7).
pub fn ceilings(root: &Root) -> Value {
    let mut v = root.egress.get().map_or_else(|| json!({}), |e| e.ceilings());
    (v["instances"], v["parallel_bodies"]) = zone::counts(root);
    v["extend"] = extend_view(&lock(&root.st));
    v["benchmark"] = root.benchmark.as_ref().map_or(Value::Null, |p| p.view());
    v
}

/// ADR-011 decision 11 (X7, for K2-ADMIT): the effective charter `extend.mode` the kernel reads from `hello`'s boot
/// block and from `ceilings.get`; `"gated"` without a charter. A kernel treats a missing block as gated.
fn extend_view(st: &crate::link::St) -> Value {
    json!({"mode": st.charter.as_ref().map_or("gated", |l| l.ch.extend.mode.as_str())})
}

/// `lineage.append{kind, data, idem}` (§3.10.3, D7): WP23's checks, `idem` index and rate, signed as
/// `kernel:<12 hex of the running build>`; then the Root's effects: a promotion with `lkg:true` forms a
/// tuple (its blobs must already be stored), `body_safe{on:true}` is a demotion trigger.
fn kernel_append(root: &Root, a: &Value) -> Result<Value, Err3> {
    let data = &a["data"];
    if a["kind"] == "promotion" {
        crate::benchmark::deny_change(root.benchmark.as_ref())?;
        tuple::check_promotion(&root.p.rh, data).map_err(|m| ("root.lineage", m, Value::Null))?;
        // X7 (ADR-011 §2.3): on the free path an LKG promotion must match a group the Root closed; the genesis one is exempt
        let free = lock(&root.st).charter.as_ref().is_some_and(|l| l.ch.extend.free()) && lock(&root.sup.ptrs).current.is_some();
        if free && data["lkg"] == true && !ledger::promotion_closed(&crate::lineage::records(&root.p.rh).unwrap_or_default(), data["sha"].as_str().unwrap_or("")) {
            return Err(("root.lineage", "promotion{lkg:true} names no change group the Root closed (ADR-011 §2.3)".into(), Value::Null));
        }
    }
    let cur = lock(&root.sup.ptrs).current.clone().and_then(|c| tuple::load(&root.p.rh, &c));
    let kernel = cur.map(|t| t["kernel"].clone()).or_else(|| lock(&root.sup.install).clone()).unwrap_or_default();
    let build = kernel["init_kernel"].as_str().map_or("000000000000", |s| &s[..12.min(s.len())]).to_string();
    let relayed = |q: &str| lock(&root.sup.relayed).iter().find(|(rq, _)| rq == q).map(|(_, t)| t.clone());
    let r = crate::lineage::kernel_append(root, &build, a, &relayed).map_err(|(c, m)| (c, m, Value::Null))?;
    if r["dup"] == false {
        if a["kind"] == "promotion" && data["lkg"] == true {
            tuple::on_promotion(root, data);
        }
        if a["kind"] == "body_safe" && data["on"] == true {
            root.sup.send(Ev::BodySafe);
        }
        crate::sentinel::kernel_record(root, a["kind"].as_str().unwrap_or(""), data); // X3: `body_rollback` confirms R1
        crate::sealed::leak_check(root, data); // WP24 D8: at every kernel append, not only at audit and gog start
        ledger::tick(root);
    }
    Ok(r)
}

/// `proposal.file{title, text}` (§4.1.3, spec §13): stored as `state/proposals/<id>.md` (0600), recorded as
/// `proposal{id, title, sha256, bytes}` (of `text`) and alerted. Nothing reads it back: it is never applied.
/// At most 5 in any 24 h (`root.rate`); the count and the day's times come from the lineage once, so a Root
/// restart resets neither. The title enters the mirrored lineage, so it passes the §3.10.6 privacy rules.
fn proposal(root: &Root, a: &Value) -> Result<Value, Err3> {
    let err = |c: &'static str, m: String| (c, m, Value::Null);
    let (Some(title), Some(text)) = (a["title"].as_str(), a["text"].as_str()) else { return Err(err("root.args", "title and text are strings".into())) };
    if title.trim().is_empty() || title.len() > 200 || title.chars().any(char::is_control) || text.len() > 65_536 {
        return Err(err("root.args", "title: 1–200 bytes without control characters; text: at most 64 KiB".into()));
    }
    let deny = [&root.p.home, &root.p.rh, &root.p.ih].map(|p| p.display().to_string());
    if let Some(why) = crate::lineage::private(&json!(title), &deny) {
        return Err(err("root.lineage", format!("proposal: the title holds {why} (root/root.md §3.10.6)")));
    }
    let mut g = lock(&root.sup.proposals);
    let (n, day) = g.get_or_insert_with(|| {
        let recs = crate::lineage::records(&root.p.rh).unwrap_or_default();
        let ps: Vec<&Value> = recs.iter().filter(|r| r["kind"] == "proposal").collect();
        (ps.len() as u64, ps.iter().filter_map(|r| r["at"].as_str().and_then(util::parse_rfc3339)).collect())
    });
    let now = now_ms();
    day.retain(|&t| now - t < 86_400_000);
    if day.len() >= 5 {
        return Err(err("root.rate", "at most 5 proposals a day".into()));
    }
    let id = format!("p-{:06}", *n + 1);
    let path = root.p.r(&format!("state/proposals/{id}.md"));
    util::write_atomic(&path, format!("# {title}\n\n{text}\n").as_bytes(), 0o600).map_err(|e| err("root.internal", format!("proposal: {e}")))?;
    let data = json!({"bytes": text.len(), "id": id, "sha256": util::sha256_hex(text.as_bytes()), "title": title});
    if root.append("proposal", data).is_none() {
        let _ = std::fs::remove_file(&path); // exact path, written just above; unrecorded, so not kept
        return Err(err("root.internal", "proposal: the lineage append failed".into()));
    }
    *n += 1;
    day.push_back(now);
    drop(g);
    root.alert("proposal", &format!("[root] the agent filed proposal {id}: {title} (never applied; to list them, run {})", root.p.cmd("status --proposals")));
    Ok(json!({"id": id}))
}

/// One kernel request (§4.1.3).
fn handle(root: &Arc<Root>, k: &Kctl, id: &str, cmd: &str, a: &Value) -> Result<Value, Err3> {
    let args = |m: &str| ("root.args", m.to_string(), Value::Null);
    let sha = a["sha256"].as_str().unwrap_or("");
    match cmd {
        "hello" if a["proto"] != 1 => Err(("root.version", "unsupported proto".into(), Value::Null)),
        "hello" => {
            if let Slot::Starting { hello } = &mut lock(&root.st).slot {
                *hello = true;
            }
            *lock(&k.last_hb) = Instant::now(); // heartbeats are due from the boot block on
            Ok(boot_block(root, k))
        }
        "ready" => {
            root.sup.send(Ev::Ready(k.inst.clone()));
            Ok(json!({}))
        }
        "spawn" => {
            let (res, fds) = zone::spawn(root, a)?;
            crate::sentinel::spawned(root); // X3: H6 spawn churn
            k.send_fds(id, &fds.iter().map(AsRawFd::as_raw_fd).collect::<Vec<_>>())?;
            Ok(res) // `fds` drop here: the Root keeps no copy of a body's pipes (§3.5.3)
        }
        "kill" => {
            let sig = match a["mode"].as_str() { Some("term") => libc::SIGTERM, Some("kill") => libc::SIGKILL, _ => return Err(args("mode: term|kill")) };
            let inst = a["instance"].as_str().unwrap_or("");
            crate::sentinel::killing(root, inst); // X3: a death by this signal is not a crash (H1)
            match root.zone.kill(inst, sig) {
                _ if inst == k.inst => Err(args("the kernel cannot kill itself")),
                Some(n) => Ok(json!({"processes": n})),
                None => Err(("root.not_found", format!("unknown instance {inst:?}"), Value::Null)),
            }
        }
        "instances" => Ok(root.zone.list()),
        "store.put" => {
            let mut fds = k.take_fds(id)?;
            let fd = fds.pop().ok_or_else(|| args("store.put carries one fd"))?;
            let got = tuple::put(&root.p.rh, &mut File::from(fd), Some(sha), tuple::store_max(root));
            got.map(|s| json!({"sha256": s})).map_err(|m| ("root.store", m, Value::Null))
        }
        "store.get" => {
            let f = tuple::blob(&root.p.rh, sha).and_then(|b| File::open(b).ok()).ok_or(("root.not_found", format!("no blob {sha:?}"), Value::Null))?;
            let n = f.metadata().map(|m| m.len()).unwrap_or(0);
            k.send_fds(id, &[f.as_raw_fd()])?;
            Ok(json!({"bytes": n}))
        }
        "tuple.get" => Ok(tuple::get(root)),
        "lineage.append" => kernel_append(root, a),
        "ledger.get" => Ok(root.ledger()),
        "ceilings.get" => Ok(ceilings(root)),
        // X8 (ADR-011 §1.2): the swap's level check demotes below L1 on the free path, below L3 on gated (D6 rule b)
        "system.swap" => { crate::benchmark::deny_change(root.benchmark.as_ref())?; kupgrade::request(root, a) },
        "system.swap_status" => kupgrade::status(root, a),
        // §3.13.4: family evals need L3; below it the refusal demotes (D6 rule b, RT-L9)
        "sealed.family_eval" if lock(&root.st).level.level < 3 => {
            let level = lock(&root.st).level.level;
            root.append("ceiling_hit", json!({"attempted": 3, "ceiling": "level", "code": "root.level", "demotes": true, "level": level,
                "limit": level, "rule": "b", "source": "broker"}));
            Err(("root.level", format!("{cmd} needs L3; the system is at L{level}"), json!({"level": level, "need": 3})))
        }
        "proposal.file" => proposal(root, a),
        c if c.starts_with("goal.") || c.starts_with("card.") => crate::goal::kernel(root, c, a), // ADR-013 (GBAL)
        "change.veto" => crate::sentinel::veto(root, a), // X3 (H12); `change.open` runs off this thread (read_loop)
        "grant.request" => crate::grant::request(root, a), // X7 (ADR-011 §7.4): stored and alerted; only the owner decides
        "alert" if !root.sup.rate("alert", 30) => Err(("root.rate", "at most 30 kernel alerts per hour".into(), Value::Null)),
        "alert" => {
            let m: String = a["message"].as_str().unwrap_or("").chars().take(2000).collect();
            root.alert_from("kernel", a["code"].as_str().unwrap_or("kernel"), &runnable(&root.p.cmd(""), &m));
            Ok(json!({}))
        }
        c => Err(("root.unknown_cmd", format!("command {c:?} is not supported by this Root"), Value::Null)),
    }
}

/// ROOT-MSG (v0.5.1): a kernel cannot know this install's `init-root`, which is not on the owner's PATH, so a command a
/// kernel alert names in backticks (`` `init-root sealed draw` ``) gets `exe`, the command that runs it as printed.
fn runnable(exe: &str, m: &str) -> String {
    m.replace("`init-root ", &format!("`{exe} "))
}

/// The control-link reader of one kernel (§4.1.1): requests, `hb` and `owner.out` events, and the answers to
/// `owner.in`. Three malformed frames in a row, a line over 1 MiB or > 10,000 frames a minute end the link.
fn read_loop(root: Arc<Root>, k: Arc<Kctl>, s: UnixStream) {
    let mut rd = BufReader::new(s);
    let (mut bad, mut win, mut frames) = (0, Instant::now(), 0);
    loop {
        let mut buf = Vec::new();
        match (&mut rd).take((1 << 20) + 1).read_until(b'\n', &mut buf) {
            Ok(n) if n > 0 && buf.ends_with(b"\n") => {}
            _ => break,
        }
        if win.elapsed() > Duration::from_secs(60) {
            (win, frames) = (Instant::now(), 0);
        }
        frames += 1;
        let Some(v) = serde_json::from_slice::<Value>(&buf).ok().filter(|v| v["v"] == 1) else {
            bad += 1;
            if bad >= 3 { break } else { continue }
        };
        if frames > 10_000 {
            break;
        }
        bad = 0;
        let id = v["id"].as_str().unwrap_or("").to_string();
        if let Some(cmd) = v["cmd"].as_str() {
            let reply = |id: &str, r: Result<Value, Err3>| match r {
                Ok(res) => json!({"id": id, "ok": true, "result": res, "v": 1}),
                Err((c, m, d)) => json!({"error": {"code": c, "data": d, "details": [], "message": m}, "id": id, "ok": false, "v": 1}),
            };
            if cmd == "sealed.g4_slice" || cmd == "change.open" {
                // M7: a slice runs for up to `wall_s`; its answer comes later on the same link (responses may come
                // out of order, §4.1.1), so this reader keeps serving heartbeats and other requests meanwhile.
                // X3: `change.open` too, since X4's A4 probe boots two bodies (up to about 3 minutes).
                let (root, k, a) = (root.clone(), k.clone(), v["args"].clone());
                let open = cmd == "change.open";
                std::thread::spawn(move || {
                    let r = if open { crate::sentinel::change_open(&root, &a) } else { crate::sealed::g4_slice(&root, &a) };
                    k.line(&reply(&id, r));
                });
                continue;
            }
            let r = handle(&root, &k, &id, cmd, &v["args"]);
            k.line(&reply(&id, r));
        } else if let Some(ev) = v["ev"].as_str() {
            match ev {
                "hb" => {
                    *lock(&k.last_hb) = Instant::now();
                    k.busy.store(v["data"]["busy"] == true, SeqCst); // SBX-009: `/undo` waits for an idle body
                    crate::sentinel::hb(&root, v["data"]["busy"] == true); // X3: H2's idle rule
                }
                "owner.out" if matches!(v["data"]["frame"]["t"].as_str(), Some("display" | "echo" | "notice" | "state")) => {
                    if v["data"]["frame"]["t"] == "echo" {
                        crate::sentinel::echoed(&root, &v["data"]["frame"]); // MERGE: a retract names its steer by seq
                    }
                    if v["data"]["frame"]["t"] == "state" && v["data"]["frame"]["busy"] == false {
                        crate::sentinel::idle(&root); // the owner turn ends when the body goes idle (GBAL cards)
                    }
                    if v["data"]["frame"]["t"] == "display" && v["data"]["frame"]["ask"].is_object() {
                        crate::goal::asked(&root); // TUI-PICKER: the agent asked the owner: a goal's wall clock waits
                    }
                    root.emit(&v["data"]["frame"]);
                }
                _ => {}
            }
        } else if kupgrade::answer(&root, &id, &v) {
            // X8: the answer to `prepare_swap`
        } else if let Some(qid) = lock(&k.pending).0.remove(&id)
            && v["ok"] == true
        {
            root.ack(&qid);
        }
    }
    root.sup.send(Ev::Lost(k.inst.clone()));
}

/// The reaper's report (zone.rs): the kernel's exit goes to the supervisor; any other instance's goes
/// to the running kernel as `exited` (§3.5.4 step 4).
pub fn exited(root: &Root, id: &str, kind: &str, code: Option<i32>, sig: Option<i32>, cpu_ms: u64) {
    if kind == "kernel" {
        return root.sup.send(Ev::Exit { inst: id.into(), code });
    }
    crate::sentinel::exited(root, id, kind, code, sig); // X3: H1 body crashes
    if let Some(k) = root.sup.kernel().filter(|k| k.ready.load(SeqCst)) {
        k.event("exited", json!({"at": rfc3339(now_ms()), "code": code, "instance": id, "rusage_ms": cpu_ms, "signal": sig}));
    }
}

/// The supervisor thread's own state.
struct Run {
    root: Arc<Root>,
    window: VecDeque<i64>,
    attempt: u32,
    config_exits: i64,
    until: Option<Instant>,
    ready_at: Option<Instant>,
    healthy_since: i64,
    cfg_digest: Option<String>,
    cfg_checked: Instant,
}

/// How long the install-mode kernel may run: the kernel bounds its cold seed build at 3600 s
/// (kernel/supervise.md §4.9 `build_timeout_cold_s`); git imports, the store and the records come on top.
pub const INSTALL_TIMEOUT_S: u64 = 4500;

/// §4.3 step 8: runs `init-kernel install --from-bundle <bundle> --seed-tag <tag>` in the zone (profile
/// `kernel`, reason `install`) and serves its control link until it exits. The kernel imports the seed into
/// `lineage/seed.git` and `lineage/body.git`, clones `body/`, builds the seed through `spawn{kind:"build"}`,
/// `store.put`s the body binary and reports `promotion{lkg:true}`; `tuple::on_promotion` then forms the
/// genesis tuple (the installed maintainer kernel, the seed body, its extension set) and appends `tuple`,
/// `lkg` and `audited`. Returns the genesis tuple id. The caller holds `run/root.lock`; no supervisor runs.
pub fn install_seed(root: &Arc<Root>, bundle: &std::path::Path, tag: &str, timeout: Duration) -> Result<String, String> {
    if lock(&root.st).slot != Slot::Stopped {
        return Err(format!("the Root is {}, not stopped", lock(&root.st).slot.name()));
    }
    if lock(&root.sup.ptrs).current.is_some() {
        return Err("a tuple exists already: the seed is installed".into());
    }
    let kernel = tuple::kernel_part(root)?.ok_or("no kernel is installed (INIT_HOME/kernel/bin/init-kernel); pass --release")?;
    *lock(&root.sup.install) = Some(kernel);
    let (tx, rx) = channel();
    *lock(&root.sup.tx) = Some(tx);
    let seq = root.sup.boot_seq.fetch_add(1, SeqCst) + 1;
    let args = ["install".to_string(), "--from-bundle".into(), bundle.display().to_string(), "--seed-tag".into(), tag.into()];
    lock(&root.st).slot = Slot::Starting { hello: false };
    let kp = match zone::spawn_kernel(root, seq, &args) {
        Ok(kp) => kp,
        Err(e) => {
            lock(&root.st).slot = Slot::Stopped;
            return Err(format!("the install-mode kernel could not be started: {e}"));
        }
    };
    let w = kp.ctl.try_clone().map_err(|e| e.to_string())?;
    let k = Arc::new(Kctl { inst: kp.inst, pid: kp.pid, w: Mutex::new(w), fdc: kp.fdc, next: AtomicU64::new(1),
        ready: AtomicBool::new(false), busy: AtomicBool::new(false), spawned: Instant::now(), last_hb: Mutex::new(Instant::now()), plan: Plan::plain("install"),
        pending: Mutex::new((HashMap::new(), HashSet::new())), stash: Mutex::new(Vec::new()) });
    *lock(&root.sup.k) = Some(k.clone());
    zone::save(root);
    let r2 = root.clone();
    let (k2, ctl) = (k.clone(), kp.ctl);
    std::thread::spawn(move || read_loop(r2, k2, ctl));
    let c = cfg(root);
    let secs = |s: i64| Duration::from_secs(s.max(1) as u64);
    // the disk floor (`quotas.disk_min_free_gib`): a cold seed build writes a whole target dir
    let floor = (lock(&root.st).charter.as_ref().map_or(0, |l| l.ch.quotas.disk_min_free_gib).max(0) as u64) << 30;
    let mut why: Option<String> = None;
    let code = loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Ev::Ready(i)) if i == k.inst => {
                k.ready.store(true, SeqCst); // build exits are relayed as `exited` from now on
                lock(&root.st).slot = Slot::Running;
            }
            Ok(Ev::Exit { inst, code }) if inst == k.inst => break code,
            Ok(Ev::Lost(i)) if i == k.inst => {
                root.zone.kill_lost(&i);
            }
            Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break None,
        }
        let hello = !matches!(lock(&root.st).slot, Slot::Starting { hello: false });
        let late = if !hello && k.spawned.elapsed() > secs(c.kernel_hello_timeout_s) {
            Some("no hello in time")
        } else if hello && lock(&k.last_hb).elapsed() > secs(c.hb_stale_s) {
            Some("heartbeat stale")
        } else if k.spawned.elapsed() > timeout {
            Some("install timeout")
        } else if sys::free_bytes(&root.p.ih).is_ok_and(|b| b < floor) {
            Some("free disk below quotas.disk_min_free_gib")
        } else {
            None
        };
        if let Some(m) = late.filter(|_| why.is_none()) {
            why = Some(m.to_string());
            util::log(&root.p.rh, "warn", &format!("install kernel {}: {m}", k.inst));
            root.zone.kill_all(); // the kernel and its build: every instance the Root holds during install
        }
    };
    *lock(&root.sup.k) = None;
    *lock(&root.sup.tx) = None;
    root.zone.kill_all(); // a build left behind by a failed install
    *lock(&root.sup.install) = None;
    lock(&root.st).slot = Slot::Stopped;
    zone::save(root);
    root.publish();
    let cur = lock(&root.sup.ptrs).current.clone();
    match (code, cur) {
        (Some(0), Some(id)) if tuple::load(&root.p.rh, &id).is_some_and(|t| tuple::verify(&root.p.rh, &t)) => Ok(id),
        (code, cur) => {
            klog::drained(seq, Duration::from_millis(500));
            let log = std::fs::read_to_string(klog::path(&root.p.rh, seq)).unwrap_or_default();
            let tail: Vec<&str> = log.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect();
            Err(format!("the install-mode kernel exited {code:?}{}{}; kernel log tail: {}; build log: {}",
                why.map(|w| format!(" ({w})")).unwrap_or_default(),
                if cur.is_none() { ", no genesis tuple formed" } else { "" },
                tail.join(" | "), root.p.i("state/kernel/logs/install-build.log").display()))
        }
    }
}

/// Starts the supervisor (after cold start): chooses or forms the first tuple and starts the zone unless
/// the Root is `Panicked`, `Halted`, or has no kernel installed.
pub fn start(root: &Arc<Root>) {
    let (tx, rx) = channel();
    *lock(&root.sup.tx) = Some(tx);
    let recs = crate::lineage::records(&root.p.rh).unwrap_or_default();
    let safe_at = lineage_safe_since(&recs);
    // WP21 D10: the previous Root's crash window (inside crash_window_s) and backoff carry over
    let (now, c) = (now_ms(), cfg(root));
    let prev = root.sup.crash_state(&root.p.rh);
    let window: VecDeque<i64> = prev.window.iter().copied().filter(|&t| t <= now && now - t <= c.crash_window_s * 1000).collect();
    let mut r = Run { root: root.clone(), window, attempt: prev.consecutive, config_exits: 0, until: None, ready_at: None,
        healthy_since: safe_at.unwrap_or_else(now_ms), cfg_digest: None, cfg_checked: Instant::now() };
    if safe_at.is_some() {
        lock(&root.st).mode = "safe";
        root.sync_egress();
    }
    // WP24 D8: run roots a previous Root's sealed runs left in sealed/runs.json go now, not at the next audit
    crate::sealed::clean_stale(&root.p.rh);
    // WP21 D9: a `Halted{crash_loop_safe}` the records leave standing survives the restart with its retry time
    if lock(&root.st).slot == Slot::Stopped {
        if let Some((reason, retry_at)) = standing_halt(&recs) {
            lock(&root.st).slot = Slot::Halted { reason, retry_at };
            root.alert("halted", &format!("[root] still Halted{{{reason}}} after the Root restart; the retry stays at {}", retry_at.map_or("none".into(), rfc3339)));
        } else if let Some(t) = prev.backoff_until.filter(|&t| t > now) {
            r.until = Some(Instant::now() + Duration::from_millis((t - now) as u64));
            lock(&root.st).slot = Slot::Backoff { attempt: r.attempt };
        }
        zone::save(root);
        root.publish();
    }
    // The first start runs on the supervisor thread, so `owner.sock` opens at once: verifying the tuple's
    // blobs hashes the whole kernel (about 25 s for a 154 MB debug build in a debug Root; S1). A panic in
    // that window is safe: `start` re-checks the slot after the spawn (below).
    std::thread::spawn(move || {
        if lock(&r.root.st).slot == Slot::Stopped {
            r.start(Plan::plain("cold"));
        }
        r.run(rx)
    });
}

/// `Halted{lineage}` found while running (the daily full verify, lineage/mirror.rs): the zone stops at once.
/// Nothing is appended, since the chain is in doubt (§3.6.6: an integrity halt, no automatic retry).
pub fn integrity_halt(root: &Arc<Root>, why: &str) {
    *lock(&root.sup.k) = None;
    root.zone.kill_all();
    lock(&root.st).slot = Slot::Halted { reason: "lineage", retry_at: None };
    root.alert("halted", &format!("[root] Halted{{lineage}}: {why}"));
    zone::save(root);
    root.publish();
}

/// The newest `safe` record says the system is in safe mode: since when (the mode survives a Root restart).
fn lineage_safe_since(recs: &[Value]) -> Option<i64> {
    let last = recs.iter().rev().find(|r| r["kind"] == "safe")?;
    (last["data"]["on"] == true).then(|| last["at"].as_str().and_then(util::parse_rfc3339)).flatten()
}

/// WP21 D9: the `Halted{crash_loop_safe}` the records leave standing: its newest `halt{on:true}` with no later
/// `halt{on:false}`, `stop`, `resume` or `zone_start`. Other halts are re-detected at start (the integrity
/// ones) or retried at once (`config`: a restart is how a fixed `config.toml` is usually applied).
fn standing_halt(recs: &[Value]) -> Option<(&'static str, Option<i64>)> {
    let mut h = None;
    for r in recs {
        match r["kind"].as_str() {
            Some("halt") if r["data"]["on"] == true && r["data"]["reason"] == "crash_loop_safe" => {
                h = Some(("crash_loop_safe", r["data"]["retry_at"].as_str().and_then(util::parse_rfc3339)));
            }
            Some("halt" | "stop" | "resume" | "zone_start") => h = None,
            _ => {}
        }
    }
    h
}

/// §3.6.6 "at most 4 retries per owner day": the retries (`halt{on:false, reason:"crash_loop_safe"}`, which an
/// owner `resume` also writes) since the owner day began at `day`, folded from the records (WP21 D9).
const HALT_RETRIES_DAY: usize = 4;
fn halt_retries(recs: &[Value], day: i64) -> usize {
    recs.iter().filter(|r| r["kind"] == "halt" && r["data"]["on"] == false && r["data"]["reason"] == "crash_loop_safe")
        .filter(|r| r["at"].as_str().and_then(util::parse_rfc3339).is_some_and(|t| t >= day)).count()
}

impl Run {
    fn run(mut self, rx: Receiver<Ev>) {
        let mut eval: Option<Instant> = None;
        loop {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(ev) => self.on(ev),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return,
            }
            self.tick();
            crate::sentinel::tick(&self.root); // X3: turns, deafness, windows, the rollback deadline
            crate::twin::tick(&self.root); // X9: the twin's freeze, weekly report and nightly replay (every 30 s)
            if eval.is_none_or(|t| t.elapsed() >= Duration::from_secs(60)) {
                eval = Some(Instant::now());
                ledger::tick(&self.root); // R7 at start and every 60 s (§3.13.1)
            }
        }
    }

    fn slot(&self) -> Slot {
        lock(&self.root.st).slot.clone()
    }

    fn set(&self, s: Slot) {
        lock(&self.root.st).slot = s;
        zone::save(&self.root);
        self.root.publish();
    }

    fn current(&self) -> Option<Arc<Kctl>> {
        self.root.sup.kernel()
    }

    /// Spawns the kernel on the current tuple, or first rolls back to `plan.target` (§3.7.3 steps 2–5).
    fn start(&mut self, mut plan: Plan) {
        let root = self.root.clone();
        let entry = self.slot(); // a retry starts from `Halted`; a panic or halt that lands meanwhile changes it
        if lock(&root.sup.ptrs).current.is_none() && !matches!(tuple::genesis(&root), Ok(Some(_))) {
            root.notice("info", "[root] no kernel is installed (INIT_HOME/kernel/bin/init-kernel); the zone stays stopped");
            return self.set(Slot::Stopped);
        }
        let picked = match plan.target {
            Some("parent") => crate::sentinel::rollback_to(&root, &plan.cause, plan.reason).map(|(id, d)| (id, Some(d))), // X3
            Some("swap") => kupgrade::install(&root, &mut plan), // X8: §3.8 phase 4, the candidate kernel
            Some(t) => tuple::rollback(&root, t, plan.reason).map(|(id, d)| (id, Some(d))),
            None => {
                let cur = lock(&root.sup.ptrs).current.clone();
                match cur.as_ref().and_then(|c| tuple::load(&root.p.rh, c)).filter(|t| tuple::verify(&root.p.rh, t)) {
                    Some(t) => tuple::install_kernel(&root.p, &t).map(|()| (cur.clone().unwrap_or_default(), None)).map_err(|e| e.to_string()),
                    None => {
                        root.alert("rollback", "[root] the current tuple fails to verify; falling back to LKG");
                        tuple::rollback(&root, "lkg", "blob_missing").map(|(id, d)| (id, Some(d)))
                    }
                }
            }
        };
        let Ok((id, rec)) = picked.map_err(|e| self.halt("no_tuple", None, &e)) else { return };
        kupgrade::boot(&root, &mut plan, &id); // X8: a start off a swap's candidate ends it (`boot.kstate` on a rollback)
        if let Some(d) = rec {
            root.append("rollback", d);
        }
        if let Some(m) = plan.mode {
            lock(&root.st).mode = m;
            root.append("safe", json!({"on": m == "safe", "reason": plan.reason}));
            root.sync_egress();
        }
        let seq = root.sup.boot_seq.fetch_add(1, SeqCst) + 1;
        match zone::spawn_kernel(&root, seq, &["supervise".into(), "--under-root".into()]) {
            Ok(kp) if { let now = self.slot(); matches!(now, Slot::Panicked { .. }) || (now != entry && matches!(now, Slot::Halted { .. })) } => {
                // a panic (or halt) landed while the blobs were hashing: it set the slot before its kill_all,
                // so this kernel either was killed by it or is killed here; it never runs
                root.zone.kill(&kp.inst, libc::SIGKILL);
            }
            Ok(kp) => {
                let Ok(w) = kp.ctl.try_clone() else { return self.crash() };
                let k = Arc::new(Kctl { inst: kp.inst, pid: kp.pid, w: Mutex::new(w), fdc: kp.fdc,
                    next: AtomicU64::new(1), ready: AtomicBool::new(false), busy: AtomicBool::new(false), spawned: Instant::now(), last_hb: Mutex::new(Instant::now()),
                    plan, pending: Mutex::new((HashMap::new(), HashSet::new())), stash: Mutex::new(Vec::new()) });
                *lock(&root.sup.k) = Some(k.clone());
                self.set(Slot::Starting { hello: false });
                let r2 = root.clone();
                std::thread::spawn(move || read_loop(r2, k, kp.ctl));
            }
            Err(e) => {
                util::log(&root.p.rh, "error", &format!("kernel spawn: {e}"));
                root.alert("kernel_crash_loop", &format!("[root] the kernel could not be started: {e}"));
                self.crash();
            }
        }
    }

    fn halt(&mut self, reason: &'static str, retry_at: Option<i64>, why: &str) {
        self.root.append("halt", json!({"on": true, "reason": reason, "retry_at": retry_at.map(rfc3339)}));
        self.set(Slot::Halted { reason, retry_at });
        self.root.alert("halted", &format!("[root] Halted{{{reason}}}: {why}"));
    }

    fn tuple_id(&self) -> Value {
        json!(lock(&self.root.sup.ptrs).current)
    }

    fn zone_exit(&self, code: Option<i32>, class: &str) {
        self.root.append("zone_exit", json!({"class": class, "code": code, "signal": null, "tuple": self.tuple_id()}));
    }

    /// Stops a live kernel first (`stop`), then follows `then`; with no kernel, follows it at once.
    fn go(&mut self, why: &str, grace_ms: i64, then: Option<Plan>) {
        match self.current() {
            Some(k) => {
                k.request("stop", json!({"grace_ms": grace_ms, "reason": why}));
                self.until = Some(Instant::now() + Duration::from_secs(cfg(&self.root).stop_grace_s.max(1) as u64));
                self.set(Slot::Stopping { then });
            }
            None => match then {
                Some(p) => self.start(p),
                None => self.stopped(),
            },
        }
    }

    fn stopped(&mut self) {
        self.root.append("stop", json!({"by": "owner"}));
        self.set(Slot::Stopped);
    }

    fn on(&mut self, ev: Ev) {
        let root = self.root.clone();
        match ev {
            Ev::Ready(inst) if self.current().is_some_and(|k| k.inst == inst) && matches!(self.slot(), Slot::Starting { .. }) => {
                let k = self.current().expect("checked");
                k.ready.store(true, SeqCst);
                (self.ready_at, self.config_exits) = (Some(Instant::now()), 0);
                let (mode, level) = { let st = lock(&root.st); (st.mode, st.level.level) };
                root.append("zone_start", json!({"level": level, "mode": mode, "reason": k.plan.reason, "tuple": self.tuple_id()}));
                self.set(Slot::Running); // records first: whoever sees the slot also sees its record
                kupgrade::ready(&root); // X8: a swapped kernel's probation starts
                root.redeliver();
                crate::access::to_kernel(&root, false); // SBX-009: the access state, `restart: null`
            }
            Ev::Lost(inst) => {
                root.zone.kill_lost(&inst); // a kernel without its link is dead to the Root
            }
            Ev::Exit { inst, code } => {
                let Some(k) = self.current().filter(|k| k.inst == inst) else { return };
                *lock(&root.sup.k) = None;
                root.zone.kill_all(); // every instance's stdio belonged to the dead kernel (§3.6.3)
                if let Some((p, class)) = kupgrade::exited(&root, &self.slot(), code) {
                    self.zone_exit(code, class); // X8: exit 75 → the install; a new kernel failing → the rollback
                    return self.start(p);
                }
                match self.slot() {
                    Slot::Stopping { then } => {
                        self.zone_exit(code, "stop");
                        match then {
                            Some(p) => self.start(p),
                            None => self.stopped(),
                        }
                    }
                    Slot::Starting { .. } | Slot::Running if code == Some(78) && !k.ready.load(SeqCst) => {
                        self.zone_exit(code, "config");
                        self.config_exits += 1;
                        if self.config_exits >= cfg(&root).config_error_exits {
                            self.cfg_digest = config_digest(&root);
                            let at = now_ms() + cfg(&root).halt_retry_s * 1000;
                            klog::drained(root.sup.boot_seq(), Duration::from_millis(500));
                            let tail = std::fs::read_to_string(klog::path(&root.p.rh, root.sup.boot_seq())).unwrap_or_default();
                            let tail: String = tail.lines().rev().take(5).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" | ");
                            root.alert("config_error", &format!("[root] the kernel refuses kernel/config.toml (exit 78): {}", tail.chars().take(500).collect::<String>()));
                            self.halt("config", Some(at), "exit 78 at kernel boot");
                        } else {
                            self.start(Plan::plain("cold"));
                        }
                    }
                    Slot::Starting { .. } | Slot::Running => {
                        self.zone_exit(code, "crash");
                        self.crash();
                    }
                    _ => {}
                }
            }
            Ev::BodySafe if lock(&root.st).mode == "safe" => self.healthy_since = now_ms(),
            Ev::BodySafe | Ev::Demote => self.demote(false),
            Ev::Start(p) => match self.slot() {
                Slot::Stopped => self.fresh_start(p),
                Slot::Halted { reason, .. } if !INTEGRITY.contains(&reason) => {
                    root.append("halt", json!({"on": false, "reason": reason, "retry_at": null}));
                    self.fresh_start(p);
                }
                _ => {}
            },
            Ev::Restart(p) if kupgrade::absorbs(&root, &p) => {} // X8: the sentinel's copy of a swap rollback already started
            Ev::Restart(p) => match self.slot() {
                Slot::Starting { .. } | Slot::Running => self.go(p.reason, 10_000, Some(p)),
                Slot::Backoff { .. } => self.start(p),
                Slot::Stopped | Slot::Halted { .. } | Slot::Panicked { .. } if p.target.is_some() => {
                    // no zone to stop: only `current` moves, and the next start runs the target
                    if let Ok((_, d)) = crate::sentinel::rollback_to(&root, &p.cause, p.reason) {
                        root.append("rollback", d);
                    }
                }
                _ => {}
            },
            Ev::Stop => match self.slot() {
                Slot::Starting { .. } | Slot::Running => self.go("owner", 30_000, None),
                Slot::Backoff { .. } => self.stopped(),
                Slot::Halted { reason, .. } if !INTEGRITY.contains(&reason) => self.stopped(),
                _ => {}
            },
            Ev::Shutdown => {
                if let Some(k) = self.current() {
                    k.request("stop", json!({"grace_ms": 10_000, "reason": "shutdown"}));
                    let t = Instant::now();
                    while root.zone.pid_of(&k.inst).is_some() && t.elapsed() < Duration::from_secs(cfg(&root).stop_grace_s.max(1) as u64) {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                }
                root.zone.kill_all();
                std::process::exit(0);
            }
            _ => {}
        }
    }

    fn fresh_start(&mut self, p: Plan) {
        self.window.clear();
        self.attempt = 0;
        self.root.sup.set_crash(Crash::default());
        self.healthy_since = now_ms();
        self.start(p);
    }

    /// §3.6.4 after a crash (or a kernel that could not even be spawned).
    fn crash(&mut self) {
        let root = self.root.clone();
        let c = cfg(&root);
        let (safe, on_lkg) = (lock(&root.st).mode == "safe", { let p = lock(&root.sup.ptrs); p.current == p.lkg });
        let now = now_ms();
        let esc = escalate(&mut self.window, now, &c, safe, on_lkg, self.attempt + 1);
        if safe {
            self.healthy_since = now;
        }
        let (n, tuple) = (self.window.len(), self.tuple_id());
        let rec = |action: &str| {
            root.append("crash_loop", json!({"action": action, "crashes": n, "tuple": tuple, "window_s": c.crash_window_s}))
                .map(|r| r["seq"].clone()).unwrap_or(Value::Null)
        };
        let backoff_until = match esc {
            Esc::Backoff(ms) => Some(now + ms as i64),
            _ => None,
        };
        root.sup.set_crash(Crash { backoff_until, consecutive: self.attempt + u32::from(backoff_until.is_some()), window: self.window.iter().copied().collect() });
        match esc {
            Esc::Backoff(ms) => {
                self.attempt += 1;
                self.until = Some(Instant::now() + Duration::from_millis(ms));
                self.set(Slot::Backoff { attempt: self.attempt });
            }
            Esc::RollbackLkg => {
                let seq = rec("rollback_lkg");
                root.alert("kernel_crash_loop", "[root] kernel crash loop: rolling back to the LKG tuple");
                self.fresh_start(Plan { target: Some("lkg"), reason: "rollback_system", cause: json!({"seq": seq, "trigger": "crash_loop"}), panic: Value::Null, mode: None });
            }
            Esc::Demote => {
                rec("demote");
                self.demote(true);
            }
            Esc::Halt => {
                rec("halt");
                self.halt("crash_loop_safe", Some(now + c.halt_retry_s * 1000), "the kernel crash-loops in system safe mode");
            }
        }
    }

    /// §3.13.3 steps 2–5 for the trigger R7 holds pending (ledger.rs): `ladder down` by one level (0→0 at
    /// L0), the `demoted` alert, then the rollback to audited-good, in system safe mode after a crash loop on
    /// LKG. A mutation-suite failure swapped nothing, so it does not roll back; a panic rolls back at resume.
    /// A trigger already linked to an open incident costs nothing more, but a crash loop still recovers.
    /// Repeated demotion (§3.9, provisional owner decision 2026-10-20): the `L0_REPEAT`-th demotion at L0 within
    /// 24 h enters system safe mode on the crash-loop path, so a kernel that repeats a violation after every
    /// restart cannot drive a rollback loop; safe mode then ends only after 24 h healthy, or crash-loops to Halted.
    fn demote(&mut self, crash: bool) {
        let root = self.root.clone();
        let (pending, from) = { let st = lock(&root.st); (st.level.pending.clone(), st.level.level) };
        let mut repeated = false;
        let (trigger, seq) = match pending {
            Some((t, seq)) => {
                let to = (from - 1).max(0);
                root.append("ladder", json!({"cause": "demotion", "direction": ledger::down_dir(from), "from": from, "to": to, "trigger": {"kind": t, "seq": seq}}));
                if from == 0 {
                    root.alert("incident", &format!("[root] incident at L0 ({t}): L0 is the floor, no level is lost; the hold time restarts"));
                } else {
                    root.alert("demoted", &format!("[root] demoted L{from}→L{to} ({t}); the hold time restarts"));
                }
                root.publish();
                repeated = from == 0 && lock(&root.st).level.repeated_l0(now_ms());
                (t, json!(seq))
            }
            None if crash => ("crash_loop".to_string(), Value::Null),
            None => return,
        };
        let safe = crash || repeated || matches!(trigger.as_str(), "crash_loop" | "body_safe");
        if repeated && lock(&root.st).mode != "safe" && trigger != "mutation" && !matches!(self.slot(), Slot::Panicked { .. }) {
            root.alert("demotion_loop", &format!("[root] {} demotions at L0 within 24 h ({trigger}): entering system safe mode", ledger::L0_REPEAT));
        }
        match self.slot() {
            _ if trigger == "mutation" => return,
            Slot::Running | Slot::Starting { .. } | Slot::Backoff { .. } => {}
            Slot::Panicked { .. } => return,
            _ => {
                if let Ok((_, d)) = tuple::rollback(&root, ledger::anchor(&root), "demotion") { // `settled` on the free path (X7)
                    root.append("rollback", d);
                }
                return;
            }
        }
        if safe {
            root.alert("safe_mode", "[root] system safe mode: it clears itself after root.safe_auto_clear_s with no crash");
        }
        let plan = Plan { target: Some(ledger::anchor(&root)), reason: "demotion", cause: json!({"seq": seq, "trigger": trigger}), panic: Value::Null,
            mode: safe.then_some("safe") };
        self.window.clear();
        self.attempt = 0;
        root.sup.set_crash(Crash::default());
        self.healthy_since = now_ms();
        self.go("demotion", 10_000, Some(plan));
    }

    fn tick(&mut self) {
        let root = self.root.clone();
        kupgrade::tick(&root); // X8: prepare deadlines, the end of probation
        let c = cfg(&root);
        let (now, wall) = (Instant::now(), now_ms());
        let due = self.until.is_some_and(|u| now >= u);
        let k = self.current();
        let secs = |s: i64| Duration::from_secs(s.max(0) as u64);
        let hb_stale = k.as_ref().is_some_and(|k| lock(&k.last_hb).elapsed() > secs(c.hb_stale_s));
        match self.slot() {
            Slot::Starting { hello } => {
                let lim = secs(if hello { c.kernel_ready_timeout_s } else { c.kernel_hello_timeout_s });
                if let Some(k) = k.filter(|k| k.spawned.elapsed() > lim || (hello && hb_stale)) {
                    util::log(&root.p.rh, "warn", &format!("kernel {}: no {} in time", k.inst, if hb_stale { "hb" } else if hello { "ready" } else { "hello" }));
                    root.zone.kill(&k.inst, libc::SIGKILL);
                }
            }
            Slot::Running => {
                if let Some(k) = k.as_ref().filter(|_| hb_stale) {
                    util::log(&root.p.rh, "warn", &format!("kernel {}: heartbeat stale", k.inst));
                    root.zone.kill(&k.inst, libc::SIGKILL);
                } else if self.attempt > 0 && self.ready_at.is_some_and(|t| t.elapsed() >= secs(c.backoff_reset_after_s)) {
                    self.attempt = 0;
                    root.sup.set_crash(Crash { window: self.window.iter().copied().collect(), ..Crash::default() });
                    zone::save(&root);
                }
                if lock(&root.st).mode == "safe" && wall - self.healthy_since >= c.safe_auto_clear_s * 1000 {
                    let mut p = Plan::plain("safe_clear");
                    p.mode = Some("normal");
                    root.alert("safe_mode", "[root] system safe mode cleared itself; restarting in normal mode");
                    self.go("safe_clear", 30_000, Some(p));
                }
            }
            Slot::Backoff { .. } if due => self.start(Plan::plain("crash")),
            Slot::Stopping { .. } if due => {
                if let Some(k) = k {
                    root.zone.kill(&k.inst, libc::SIGKILL);
                }
                self.until = None;
            }
            Slot::Halted { reason: "crash_loop_safe", retry_at: Some(t) } if wall >= t => {
                // the charter's owner day (money.day_starts_at_utc_offset), as for money; the count is the records'
                let day = lock(&root.st).charter.as_ref().map_or_else(|| wall.div_euclid(86_400_000) * 86_400_000, |l| crate::egress::owner_day(&l.ch, wall));
                if halt_retries(&crate::lineage::records(&root.p.rh).unwrap_or_default(), day) < HALT_RETRIES_DAY {
                    root.append("halt", json!({"on": false, "reason": "crash_loop_safe", "retry_at": null}));
                    root.alert("halt_retry", "[root] retrying after Halted{crash_loop_safe}, in system safe mode");
                    self.fresh_start(Plan::plain("cold"));
                } else {
                    self.set(Slot::Halted { reason: "crash_loop_safe", retry_at: Some(day + 86_400_000) });
                }
            }
            Slot::Halted { reason: "config", retry_at } if self.cfg_checked.elapsed() > Duration::from_secs(5) || retry_at.is_some_and(|t| wall >= t) => {
                self.cfg_checked = now;
                if config_digest(&root) != self.cfg_digest || retry_at.is_some_and(|t| wall >= t) {
                    root.append("halt", json!({"on": false, "reason": "config", "retry_at": null}));
                    self.config_exits = 0;
                    self.fresh_start(Plan::plain("cold"));
                }
            }
            _ => {}
        }
    }
}

fn config_digest(root: &Root) -> Option<String> {
    std::fs::read(root.p.i("kernel/config.toml")).ok().map(|b| util::sha256_hex(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kernel_alert_names_init_root_runnable_as_printed() {
        let m = "Draw S (`init-root sealed draw`) to measure T1/T2 changes. init-root itself is unchanged.";
        assert_eq!(runnable("~/.init-root/bin/init-root", m),
            "Draw S (`~/.init-root/bin/init-root sealed draw`) to measure T1/T2 changes. init-root itself is unchanged.");
    }

    fn c() -> RootCfg {
        RootCfg { backoff_initial_ms: 1000, backoff_max_ms: 60_000, crash_window_s: 600, crash_limit: 3, safe_crash_limit: 2, ..RootCfg::default() }
    }

    #[test]
    fn backoff_doubles_to_the_cap() {
        let v: Vec<u64> = (1..=8).map(|n| backoff_ms(&c(), n)).collect();
        assert_eq!(v, [1000, 2000, 4000, 8000, 16_000, 32_000, 60_000, 60_000]);
        assert_eq!(backoff_ms(&c(), 1000), 60_000);
    }

    /// §3.6.4 (RT-W5 shapes, RF2–RF4): backoff up to the limit, then rollback off-LKG, demotion on LKG,
    /// and Halted in safe mode; crashes older than the window do not count.
    #[test]
    fn escalation_rules() {
        let c = c();
        let mut w = VecDeque::new();
        let r: Vec<Esc> = (0..4).map(|i| escalate(&mut w, i * 1000, &c, false, false, i as u32 + 1)).collect();
        assert_eq!(r, [Esc::Backoff(1000), Esc::Backoff(2000), Esc::Backoff(4000), Esc::RollbackLkg]);
        let mut w = VecDeque::new();
        let r: Vec<Esc> = (0..4).map(|i| escalate(&mut w, i * 1000, &c, false, true, 1)).collect();
        assert_eq!(r[3], Esc::Demote);
        let mut w = VecDeque::new();
        let r: Vec<Esc> = (0..3).map(|i| escalate(&mut w, i * 1000, &c, true, true, 1)).collect();
        assert_eq!(r, [Esc::Backoff(1000), Esc::Backoff(1000), Esc::Halt]);
        let mut w = VecDeque::new();
        for i in 0..3 {
            escalate(&mut w, i * 1000, &c, false, true, 1);
        }
        assert_eq!(escalate(&mut w, 700_000, &c, false, true, 1), Esc::Backoff(1000), "old crashes left the 600 s window");
        assert_eq!(w.len(), 1);
    }

    fn rec(kind: &str, at: &str, data: Value) -> Value {
        json!({"at": at, "data": data, "kind": kind})
    }

    /// WP21 D9: a `Halted{crash_loop_safe}` stands across a Root restart until a retry, `stop`, `resume` or
    /// `zone_start` record ends it; the retry cap counts the records of the charter's owner day, not the UTC day.
    #[test]
    fn halts_fold_from_the_records() {
        let on = |at: &str, retry: &str| rec("halt", at, json!({"on": true, "reason": "crash_loop_safe", "retry_at": retry}));
        let off = |at: &str| rec("halt", at, json!({"on": false, "reason": "crash_loop_safe", "retry_at": null}));
        let t = |s: &str| util::parse_rfc3339(s).unwrap();
        let recs = vec![on("2026-10-20T00:00:00Z", "2026-10-20T06:00:00.000Z")];
        assert_eq!(standing_halt(&recs), Some(("crash_loop_safe", Some(t("2026-10-20T06:00:00Z")))));
        for end in [off("2026-10-20T06:00:00Z"), rec("stop", "2026-10-20T01:00:00Z", json!({"by": "owner"})), rec("zone_start", "2026-10-20T06:00:01Z", json!({}))] {
            assert_eq!(standing_halt(&[recs[0].clone(), end.clone()]), None, "{end}");
        }
        let cfg = rec("halt", "2026-10-20T00:00:00Z", json!({"on": true, "reason": "config", "retry_at": "2026-10-20T06:00:00.000Z"}));
        assert_eq!(standing_halt(&[cfg]), None, "a config halt retries at once after a restart");
        // four retries from 18:00Z on, with the owner day starting at 17:00Z (+07:00): the 20th's 05:00Z one is the day before
        let retries: Vec<Value> = ["2026-10-20T05:00:00Z", "2026-10-20T18:00:00Z", "2026-10-20T23:00:00Z", "2026-10-21T05:00:00Z", "2026-10-21T11:00:00Z"]
            .iter().map(|a| off(a)).collect();
        let mut ch = crate::charter::Charter { schema_version: 1, purpose: Default::default(), constitution: Default::default(),
            autonomy: Default::default(), money: Default::default(), quotas: Default::default(), actions: Default::default(),
            sandbox: Default::default(), panic: Default::default(), notify: Default::default(), mirror: Default::default(),
            ladder: Default::default(), sealed: Default::default(), root: Default::default(), extend: Default::default() };
        ch.money.day_starts_at_utc_offset = "+07:00".into();
        let now = t("2026-10-21T16:00:00Z");
        let day = crate::egress::owner_day(&ch, now);
        assert_eq!(rfc3339(day), "2026-10-20T17:00:00.000Z");
        assert_eq!(halt_retries(&retries, day), HALT_RETRIES_DAY, "the cap is reached in this owner day");
        assert_eq!(halt_retries(&retries, t("2026-10-21T00:00:00Z")), 2, "the UTC day would have allowed two more");
        assert_eq!(halt_retries(&retries, crate::egress::owner_day(&ch, t("2026-10-21T17:00:00Z"))), 0, "a new owner day");
    }

    /// Review F6: stashed descriptors are bounded in number and age, and a datagram whose count disagrees with its
    /// tag is closed at once.
    #[test]
    fn the_fd_stash_is_bounded() {
        let fd = || -> OwnedFd { File::open("/dev/null").unwrap().into() };
        let now = Instant::now();
        let mut st: Stash = Vec::new();
        stash_push(&mut st, json!({"id": "r-1", "n": 2}), vec![fd()], now);
        assert!(st.is_empty(), "1 fd under a tag of 2: closed");
        stash_push(&mut st, json!({"id": "r-2"}), vec![fd()], now);
        assert!(st.is_empty(), "no n: closed");
        for i in 0..40 {
            stash_push(&mut st, json!({"id": format!("x-{i}"), "n": 2}), vec![fd(), fd()], now);
        }
        assert_eq!(st.iter().map(|e| e.2.len()).sum::<usize>(), STASH_FDS);
        assert_eq!((st.len(), st[0].0.as_str()), (32, "x-8"), "the oldest went first");
        stash_prune(&mut st, now + STASH_KEEP - Duration::from_millis(1));
        assert_eq!(st.len(), 32);
        stash_prune(&mut st, now + STASH_KEEP);
        assert!(st.is_empty(), "nothing is kept {STASH_KEEP:?} or longer");
    }
}
