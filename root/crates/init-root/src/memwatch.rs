//! MEM-RUNAWAY (RELEASE_PLAN.md; measurements/m7-results.md DV-4, DV-6): the per-workload memory watchdog and the
//! workload disk floor (root/root.md §3.14.1), macOS and Linux.
//!
//! Seatbelt has no memory limit and macOS does not enforce `RLIMIT_AS`, so one sandboxed process can grow through RAM
//! into swap, and swap grows on the system volume until the disk is full (M7 D: one python3.9 reached 43.9 GB
//! compressed and free disk fell to 0.24 GB in about two minutes). So the Root looks at its workloads every second:
//! - a *leader* is a process the Root holds: a zone instance (zone.rs) or a sealed run (sealed/run.rs);
//! - a *unit* is the process tree of one direct child of a leader (one tool call of a body, one rustc of a build).
//!   A member stays in its unit after its parent exits (it is remembered by pid and start time), and an orphan still
//!   in the unit root's process group joins it;
//! - a unit whose summed physical footprint (resident + compressed) passes `sandbox.mem_cap_mib` is stopped,
//!   recorded and killed, member by member; a leader whose own footprint passes the cap is killed as a whole
//!   instance, except the kernel, which never is (only its child units are);
//! - below `sandbox.disk_floor_gib` of free disk the Root refuses new workloads (zone.rs, sealed/run.rs) and kills
//!   the running ones (every unit, and every leader that is a workload itself: build, check, git-read, sealed runs).
//!
//! Each kill is appended to the events file the instance's body reads (`INIT_MEM_EVENTS`), before the signal, so the
//! body's tool call reports "process exceeded memory cap N MiB, killed"; the owner gets an alert (a notice, a line in
//! `alerts.jsonl`, `notify.cmd`). Signals reach the leaders only through `sys::signal` (held pids) and the processes
//! observed below them only through `sys::signal_tracked`, which re-checks each identity right before the syscall
//! (the ADR-008 amendment). Linux (LINUX lane): /proc RSS+swap polling through `launcher_view` (the bwrap hops are not
//! units), pidfd-based signaling, records that name the tool call by its pid-namespace pid, and, where the Root runs in
//! a delegated cgroup v2 subtree (the systemd user unit's `Delegate=memory`), a hard `memory.max` per workload instance
//! as a backstop the 1 s poll cannot be outrun past (cgroup.rs).

use crate::charter::Charter;
use crate::link::{Root, lock};
use crate::sys::{self, Proc};
use crate::util::{self, icj, rfc3339, write_atomic};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant};

/// How often a leader's units are measured (the task bound is ≤ 2 s).
pub const POLL: Duration = Duration::from_secs(1);
pub const MIB: u64 = 1 << 20;
/// The zone's events file, `INIT_HOME/root/mem-events.jsonl`: Root-written, readable by every body instance (`PUB`).
pub const ZONE_EVENTS: &str = "root/mem-events.jsonl";
/// The events file is cut to its last lines when it passes this size.
const EVENTS_MAX: u64 = 256 << 10;

/// The cap and the floor in bytes (0 floor: off, test mode only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub cap: u64,
    pub floor: u64,
}

impl Limits {
    pub fn of(ch: &Charter) -> Limits {
        Limits { cap: (ch.sandbox.mem_cap_mib.max(1) as u64) * MIB, floor: (ch.sandbox.disk_floor_gib.max(0) as u64) << 30 }
    }
    pub fn cap_mib(&self) -> u64 {
        self.cap / MIB
    }
    /// The free bytes on the volume holding `path` when they are below the floor; None when there is enough, when
    /// the floor is off, or when statvfs fails (a failed reading never kills anything).
    pub fn disk_low(&self, path: &Path) -> Option<u64> {
        (self.floor > 0).then(|| sys::free_bytes(path).ok()).flatten().filter(|&b| b < self.floor)
    }
    /// The refusal text (zone spawns, sealed runs) for `free` bytes left.
    pub fn disk_msg(&self, free: u64) -> String {
        format!("free disk {} MiB is below the workload floor sandbox.disk_floor_gib = {} GiB: no new workloads until disk space is freed",
            free / MIB, self.floor >> 30)
    }
}

/// A process the Root holds, and what it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leader {
    /// The zone instance id, or `sealed:<run id>`.
    pub key: String,
    /// `kernel`, `body`, `probe`, `build`, `check`, `git_read`, or `sealed`.
    pub kind: String,
    pub pid: i32,
}

impl Leader {
    /// The kernel is never killed by the watchdog; only its child units are.
    pub fn protected(&self) -> bool {
        self.kind == "kernel"
    }
    /// The kernel and bodies stay up on the disk floor (their units go); every other leader is a workload itself.
    pub fn hosts_tools(&self) -> bool {
        matches!(self.kind.as_str(), "kernel" | "body" | "probe")
    }
}

/// A process identity: (pid, start time in µs).
pub type Ident = (i32, u64);

fn ident(p: &Proc) -> Ident {
    (p.pid, p.start_us)
}

/// A unit: the leader it started under and the identity of its root (the leader's child).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UnitKey {
    pub leader: String,
    pub root: Ident,
}

#[derive(Clone, Debug)]
pub struct Unit {
    pub key: UnitKey,
    /// Root first (when it still runs), then breadth-first: the order processes are stopped in.
    pub members: Vec<Proc>,
    pub bytes: u64,
}

impl Unit {
    /// The unit root's pid. A body spawns each tool call as its own process group, so this is the call's pgid,
    /// which is what the body matches a record against.
    pub fn pgid(&self) -> i32 {
        self.key.root.0
    }
}

#[derive(Clone, Debug)]
pub enum Verdict {
    /// One unit over the cap: it goes, its leader stays.
    Unit { leader: Option<Leader>, unit: Unit },
    /// A leader's own footprint over the cap: the whole instance goes.
    Leader { leader: Leader, bytes: u64, units: Vec<Unit> },
}

/// One poll: every unit with its leader (None: the leader is gone), each leader's own footprint, and the verdicts.
#[derive(Debug, Default)]
pub struct Plan {
    pub units: Vec<(Option<Leader>, Unit)>,
    pub leader_bytes: BTreeMap<String, u64>,
    pub verdicts: Vec<Verdict>,
}

/// The watchdog's memory between polls: every member seen, by identity, with its unit.
#[derive(Default)]
pub struct Watch {
    seen: HashMap<Ident, UnitKey>,
}

impl Watch {
    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }

    /// Pure: the units of `leaders` in `table`, measured with `fp`, and what is over `cap`. Reads nothing else.
    pub fn plan(&mut self, leaders: &[Leader], table: &[Proc], cap: u64, fp: &dyn Fn(i32) -> Option<u64>) -> Plan {
        let by_pid: HashMap<i32, &Proc> = table.iter().map(|p| (p.pid, p)).collect();
        let mut kids: HashMap<i32, Vec<&Proc>> = HashMap::new();
        for p in table {
            kids.entry(p.ppid).or_default().push(p);
        }
        kids.values_mut().for_each(|v| v.sort_by_key(|p| p.pid));
        let me = std::process::id() as i32;
        // a leader is never a member of a unit, nor is this process or pid ≤ 1
        let mut taken: HashSet<i32> = leaders.iter().map(|l| l.pid).chain([me, 0, 1]).collect();
        let mut groups: BTreeMap<UnitKey, Vec<Proc>> = BTreeMap::new();
        let grow = |start: &Proc, taken: &mut HashSet<i32>, out: &mut Vec<Proc>| {
            let mut q = VecDeque::from([*start]);
            taken.insert(start.pid);
            while let Some(p) = q.pop_front() {
                out.push(p);
                for c in kids.get(&p.pid).into_iter().flatten() {
                    if taken.insert(c.pid) {
                        q.push_back(**c);
                    }
                }
            }
        };
        // 1. the live trees: each child of a present leader roots a unit; orphans left in its process group join it
        for l in leaders.iter().filter(|l| by_pid.contains_key(&l.pid)) {
            for c in kids.get(&l.pid).cloned().unwrap_or_default() {
                if taken.contains(&c.pid) {
                    continue;
                }
                let mut m = Vec::new();
                grow(c, &mut taken, &mut m);
                if c.pgid == c.pid {
                    for q in table.iter().filter(|q| q.pgid == c.pid && q.ppid == 1) {
                        if !taken.contains(&q.pid) {
                            grow(q, &mut taken, &mut m);
                        }
                    }
                }
                groups.insert(UnitKey { leader: l.key.clone(), root: ident(c) }, m);
            }
        }
        // 2. members seen before whose link to the tree is gone (their parent exited): back into their old unit
        let mut old: Vec<(&Ident, &UnitKey)> = self.seen.iter().collect();
        old.sort();
        for (id, key) in old {
            if let Some(p) = by_pid.get(&id.0).filter(|p| p.start_us == id.1 && !taken.contains(&p.pid)) {
                let mut m = Vec::new();
                grow(p, &mut taken, &mut m);
                groups.entry(key.clone()).or_default().extend(m);
            }
        }
        // 3. measure and judge
        let mut plan = Plan::default();
        self.seen = groups.iter().flat_map(|(k, m)| m.iter().map(move |p| (ident(p), k.clone()))).collect();
        let present: Vec<&Leader> = leaders.iter().filter(|l| by_pid.contains_key(&l.pid)).collect();
        for l in &present {
            plan.leader_bytes.insert(l.key.clone(), fp(l.pid).unwrap_or(0));
        }
        for (key, members) in groups {
            let bytes = members.iter().map(|p| fp(p.pid).unwrap_or(0)).sum();
            let owner = leaders.iter().find(|l| l.key == key.leader).cloned();
            plan.units.push((owner, Unit { key, members, bytes }));
        }
        for l in &present {
            let lb = plan.leader_bytes[&l.key];
            let mine = plan.units.iter().filter(|(o, _)| o.as_ref().is_some_and(|o| o.key == l.key)).map(|(_, u)| u.clone());
            if !l.protected() && lb > cap {
                plan.verdicts.push(Verdict::Leader { leader: (*l).clone(), bytes: lb, units: mine.collect() });
            }
        }
        // a leader that goes whole takes its units with it
        let gone: HashSet<String> = plan.verdicts.iter().filter_map(|v| match v {
            Verdict::Leader { leader, .. } => Some(leader.key.clone()),
            Verdict::Unit { .. } => None,
        }).collect();
        let units: Vec<Verdict> = plan.units.iter().filter(|(o, u)| u.bytes > cap && !o.as_ref().is_some_and(|o| gone.contains(&o.key)))
            .map(|(o, u)| Verdict::Unit { leader: o.clone(), unit: u.clone() }).collect();
        plan.verdicts.extend(units);
        plan
    }
}

/// LINUX: the Root's leader is the outer bwrap (`exec-clean` → `unshare` → `bwrap`, one pid); bwrap forks the pid
/// namespace's init, which forks the payload (the kernel, a body, a build, a sealed body). Without this view a leader
/// would have one child and so one unit: the whole instance, kernel included. `launchers` are those bwrap processes
/// (on Linux, `/proc/<pid>/comm` is `bwrap`): from each leader the chain of launchers is followed (earliest-started
/// child first) to the payload, which stands in for the leader; the launchers leave the table, and any other child
/// of a launcher (an orphan reparented to the namespace's init) becomes a child of the payload, so it stays in a unit.
/// A leader that is not a launcher (macOS, tests) or has no payload yet is left as it is. Pure.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn launcher_view(leaders: &[Leader], table: &[Proc], launcher: &dyn Fn(i32) -> bool) -> (Vec<Leader>, Vec<Proc>) {
    let mut kids: HashMap<i32, Vec<&Proc>> = HashMap::new();
    for p in table {
        kids.entry(p.ppid).or_default().push(p);
    }
    let (mut out, mut hidden, mut adopt) = (Vec::new(), HashSet::new(), HashMap::new());
    for l in leaders {
        let (mut at, mut chain) = (l.pid, Vec::new());
        while launcher(at) && chain.len() < 4 {
            chain.push(at);
            match kids.get(&at).and_then(|k| k.iter().min_by_key(|p| (p.start_us, p.pid))) {
                Some(next) => at = next.pid,
                None => break,
            }
        }
        if chain.is_empty() || chain.contains(&at) {
            out.push(l.clone());
            continue;
        }
        for c in &chain {
            hidden.insert(*c);
            for k in kids.get(c).into_iter().flatten().filter(|k| k.pid != at && !chain.contains(&k.pid)) {
                adopt.insert(k.pid, at);
            }
        }
        out.push(Leader { pid: at, ..l.clone() });
    }
    let table = table.iter().filter(|p| !hidden.contains(&p.pid)).map(|p| Proc { ppid: adopt.get(&p.pid).copied().unwrap_or(p.ppid), ..*p }).collect();
    (out, table)
}

/// The process table and leaders the watchdog judges: the platform's table, through `launcher_view` on Linux.
pub fn observed(leaders: &[Leader]) -> (Vec<Leader>, Vec<Proc>) {
    let table = sys::proc_table();
    #[cfg(target_os = "linux")]
    return launcher_view(leaders, &table, &sys::is_bwrap);
    #[cfg(not(target_os = "linux"))]
    (leaders.to_vec(), table)
}

/// Stops every member top-down (nothing forks meanwhile), looks once more for children born before their parent
/// stopped, then kills bottom-up. Only through `sys::signal_tracked`, which re-checks each identity. Returns the pids
/// that took the SIGKILL.
pub fn kill_members(members: &[Proc]) -> Vec<i32> {
    let mut all: Vec<Proc> = members.to_vec();
    for p in &all {
        sys::signal_tracked(p, libc::SIGSTOP);
    }
    for _ in 0..4 {
        let known: HashSet<Ident> = all.iter().map(ident).collect();
        let parents: HashSet<i32> = all.iter().map(|p| p.pid).collect();
        let new: Vec<Proc> = sys::proc_table().into_iter().filter(|q| parents.contains(&q.ppid) && !known.contains(&ident(q))).collect();
        if new.is_empty() {
            break;
        }
        for q in &new {
            sys::signal_tracked(q, libc::SIGSTOP);
        }
        all.extend(new);
    }
    all.iter().rev().filter(|p| sys::signal_tracked(p, libc::SIGKILL)).map(|p| p.pid).collect()
}

/// A wall-clock ms stamp that ignores the test clock: the body compares it with its own spawn time.
fn wall_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// One events record (ICJ: integers only). `event`: `mem_cap`, `disk_floor` (a kill), `disk_low`, `disk_ok`.
/// `pgid` is the unit root's pid (the body's tool call), or the leader's for a whole instance.
#[allow(clippy::too_many_arguments)]
pub fn record(event: &str, leader: Option<&Leader>, pgid: Option<i32>, scope: &str, bytes: u64, lim: &Limits, free: Option<u64>, pids: &[i32])
    -> Value {
    json!({"v": 1, "event": event, "wall_ms": wall_ms(), "instance": leader.map(|l| l.key.clone()), "kind": leader.map(|l| l.kind.clone()),
        "pgid": pgid, "scope": scope, "footprint_mib": bytes / MIB, "cap_mib": lim.cap_mib(), "free_mib": free.map(|f| f / MIB),
        "floor_mib": lim.floor / MIB, "pids": pids})
}

/// Appends one record to an events file the Root opened itself (a sealed run's, held since before the spawn).
pub fn append(f: &mut File, v: &Value) {
    if let Ok(s) = icj(v) {
        let _ = f.write_all(format!("{s}\n").as_bytes());
    }
}

/// Appends one record to the zone's events file (a Root-owned directory the zone may only read), cutting it to its
/// last 200 lines past 256 KiB.
pub fn append_path(path: &Path, v: &Value) {
    if fs::symlink_metadata(path).is_ok_and(|m| m.len() > EVENTS_MAX) {
        let s = fs::read_to_string(path).unwrap_or_default();
        let lines: Vec<&str> = s.lines().collect();
        let keep = lines[lines.len().saturating_sub(200)..].join("\n") + "\n";
        let _ = write_atomic(path, keep.as_bytes(), 0o644);
    }
    let f = OpenOptions::new().create(true).append(true).mode(0o644).custom_flags(libc::O_NOFOLLOW).open(path);
    if let Ok(mut f) = f {
        append(&mut f, v);
    }
}

/// The daemon's Root, for alerts raised where no `Root` is passed (the sealed runs). Without one (the CLI, unit
/// tests) an alert goes to `ROOT_HOME/alerts.jsonl` and the Root log only.
static ROOT: OnceLock<Weak<Root>> = OnceLock::new();

pub fn alert(rh: &Path, code: &str, msg: &str) {
    match ROOT.get().and_then(Weak::upgrade) {
        Some(r) => r.alert(code, msg),
        None => {
            let a = json!({"code": code, "message": msg, "source": "root", "ts": rfc3339(util::now_ms()), "v": 1});
            let _ = util::append_line(&rh.join("alerts.jsonl"), &a);
        }
    }
    util::log(rh, "warn", msg);
}

/// The alert text for a memory kill.
pub fn mem_msg(leader: Option<&Leader>, scope: &str, pgid: i32, n: usize, bytes: u64, lim: &Limits) -> String {
    let what = match (leader, scope) {
        (Some(l), "instance") => format!("{} instance {} (pid {pgid}) itself", l.kind, l.key),
        (Some(l), _) => format!("a workload of {} instance {} (process group {pgid}, {n} process(es))", l.kind, l.key),
        (None, _) => format!("a leftover workload (process group {pgid}, {n} process(es)) of an exited instance"),
    };
    format!("[root] memory cap: {what} reached {} MiB, over sandbox.mem_cap_mib = {} MiB; killed by pid", bytes / MIB, lim.cap_mib())
}

/// One kill as the owner and the body see it: the record goes to `sink` before any signal, then the members die.
/// Returns the killed pids.
pub fn kill_unit(sink: &mut dyn FnMut(&Value), rh: &Path, event: &str, leader: Option<&Leader>, unit: &Unit, lim: &Limits, free: Option<u64>)
    -> Vec<i32> {
    let pids: Vec<i32> = unit.members.iter().map(|p| p.pid).collect();
    // LINUX: the body knows its tool call by the pid it sees, the one in the instance's pid namespace
    let local = sys::ns_pid(unit.pgid()).unwrap_or(unit.pgid());
    sink(&record(event, leader, Some(local), "unit", unit.bytes, lim, free, &pids));
    let killed = kill_members(&unit.members);
    if event == "mem_cap" {
        alert(rh, "mem_cap", &mem_msg(leader, "unit", unit.pgid(), unit.members.len(), unit.bytes, lim));
    }
    killed
}

// ------------------------------------------------------------------------------------------------ the zone

/// The zone watchdog (the daemon, after the supervisor starts): every held zone instance, every second.
pub fn spawn_zone(root: Arc<Root>) {
    let _ = ROOT.set(Arc::downgrade(&root));
    let _ = std::thread::Builder::new().name("memwatch".into()).spawn(move || {
        let mut w = Watch::default();
        let mut disk = DiskState::default();
        loop {
            std::thread::sleep(POLL);
            let lim = lock(&root.st).charter.as_ref().map(|l| Limits::of(&l.ch));
            if let Some(lim) = lim {
                zone_tick(&root, &mut w, &mut disk, &lim);
            }
        }
    });
}

/// Whether the last poll saw the disk below the floor, and when the zone's events file last said so.
#[derive(Default)]
pub struct DiskState {
    low: bool,
    said: Option<Instant>,
}

/// One zone poll: the disk floor first (it kills every workload), then the memory cap.
pub fn zone_tick(root: &Root, w: &mut Watch, disk: &mut DiskState, lim: &Limits) {
    let leaders = root.zone.leaders();
    let events = root.p.i(ZONE_EVENTS);
    let rh = root.p.rh.clone();
    let mut sink = |v: &Value| {
        append_path(&events, v);
        crate::sentinel::mem_event(root, v); // X3: H6
    };
    let low = lim.disk_low(&root.p.ih);
    match (low, disk.low) {
        (Some(free), _) => {
            if disk.said.is_none_or(|t| t.elapsed() >= Duration::from_secs(5)) {
                sink(&record("disk_low", None, None, "zone", 0, lim, Some(free), &[]));
                disk.said = Some(Instant::now());
            }
            if !disk.low {
                alert(&rh, "disk_floor", &format!("[root] {}; running workloads are killed", lim.disk_msg(free)));
            }
            disk.low = true;
        }
        (None, true) => {
            sink(&record("disk_ok", None, None, "zone", 0, lim, sys::free_bytes(&root.p.ih).ok(), &[]));
            root.notice_code("info", "disk_floor", "[root] free disk is back above the workload floor; workloads may run again");
            *disk = DiskState::default();
        }
        (None, false) => {}
    }
    if leaders.is_empty() && w.is_empty() {
        return;
    }
    let (leaders, table) = observed(&leaders);
    // LINUX: OOM kills the cgroup backstop made since the last poll (cgroup.rs), one record and alert per instance
    #[cfg(target_os = "linux")]
    for l in &leaders {
        let n = crate::cgroup::oom_kills(&l.key);
        if n > 0 {
            sink(&record("mem_cap", Some(l), None, "cgroup", lim.cap, lim, None, &[]));
            alert(&rh, "mem_cap", &format!("[root] memory cap: {n} process(es) of {} instance {} reached memory.max = sandbox.mem_cap_mib = {} MiB; killed by the kernel's OOM killer",
                l.kind, l.key, lim.cap_mib()));
        }
    }
    let plan = w.plan(&leaders, &table, lim.cap, &sys::footprint);
    if let Some(free) = low {
        for (owner, u) in &plan.units {
            kill_unit(&mut sink, &rh, "disk_floor", owner.as_ref(), u, lim, Some(free));
        }
        for l in leaders.iter().filter(|l| !l.hosts_tools() && !l.protected()) {
            sink(&record("disk_floor", Some(l), Some(l.pid), "instance", 0, lim, Some(free), &[l.pid]));
            root.zone.kill(&l.key, libc::SIGKILL);
        }
        return;
    }
    for v in plan.verdicts {
        match v {
            Verdict::Unit { leader, unit } => {
                kill_unit(&mut sink, &rh, "mem_cap", leader.as_ref(), &unit, lim, None);
            }
            Verdict::Leader { leader, bytes, units } => {
                sink(&record("mem_cap", Some(&leader), Some(leader.pid), "instance", bytes, lim, None, &[leader.pid]));
                root.zone.kill(&leader.key, libc::SIGKILL);
                let n: usize = units.iter().map(|u| kill_members(&u.members).len()).sum();
                alert(&rh, "mem_cap", &mem_msg(Some(&leader), "instance", leader.pid, n + 1, bytes, lim));
            }
        }
    }
}

/// The environment a body instance gets for its side of the watchdog (tools.md §3.5.8): the cap, for its messages,
/// and the events file it reads after a tool process dies by a signal.
pub fn body_env(lim: &Limits, events: &Path) -> [(&'static str, String); 2] {
    [("INIT_MEM_CAP_MIB", lim.cap_mib().to_string()), ("INIT_MEM_EVENTS", events.display().to_string())]
}

/// The events path of the zone (`INIT_HOME/root/mem-events.jsonl`).
pub fn zone_events(ih: &Path) -> PathBuf {
    ih.join(ZONE_EVENTS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: i32, ppid: i32, pgid: i32) -> Proc {
        Proc { pid, ppid, pgid, start_us: 1000 + pid as u64, uid: 501 }
    }
    fn l(key: &str, kind: &str, pid: i32) -> Leader {
        Leader { key: key.into(), kind: kind.into(), pid }
    }
    const M: u64 = MIB;

    /// LINUX: leader 10 (outer bwrap) → 11 (the namespace's init, bwrap) → 12 (the body) → tool calls 13 and 14 (a
    /// tree with 15); 16 is an orphan the init adopted. Through the view the body stands in for the leader, its tool
    /// calls are separate units, the orphan stays in a unit, and no bwrap is ever a member. A non-launcher leader and a
    /// launcher with no payload yet are left alone.
    #[test]
    fn the_bwrap_hops_are_not_units_and_tool_calls_stay_separate() {
        let t = vec![p(10, 9, 10), p(11, 10, 11), p(12, 11, 12), p(13, 12, 13), p(14, 12, 14), p(15, 14, 14), p(16, 11, 16), p(20, 9, 20), p(21, 20, 21)];
        let bwrap = |pid: i32| [10, 11, 30].contains(&pid);
        let leaders = [l("b", "body", 10), l("x", "build", 20), l("y", "check", 30)];
        let (seen, table) = launcher_view(&leaders, &t, &bwrap);
        assert_eq!(seen.iter().map(|l| l.pid).collect::<Vec<_>>(), vec![12, 20, 30]);
        assert!(!table.iter().any(|p| p.pid == 10 || p.pid == 11), "the launchers leave the table");
        assert_eq!(table.iter().find(|p| p.pid == 16).unwrap().ppid, 12, "the init's orphan is adopted by the payload");
        let fp = |pid: i32| Some(if pid == 15 { 300 * M } else { M });
        let plan = Watch::default().plan(&seen, &table, 256 * M, &fp);
        let mut units: Vec<(String, i32, Vec<i32>)> = plan.units.iter()
            .map(|(o, u)| (o.as_ref().unwrap().key.clone(), u.pgid(), u.members.iter().map(|m| m.pid).collect())).collect();
        units.sort();
        assert_eq!(units, vec![("b".into(), 13, vec![13]), ("b".into(), 14, vec![14, 15]), ("b".into(), 16, vec![16]), ("x".into(), 21, vec![21])]);
        assert_eq!(plan.verdicts.len(), 1);
        assert!(matches!(&plan.verdicts[0], Verdict::Unit { unit, .. } if unit.pgid() == 14), "only the tool call over the cap goes");
        // a protected kernel behind bwrap: its tool calls go, never the kernel itself
        let k = [l("k", "kernel", 10)];
        let (seen, table) = launcher_view(&k, &t, &bwrap);
        let plan = Watch::default().plan(&seen, &table, 256 * M, &|_| Some(300 * M));
        assert!(plan.verdicts.iter().all(|v| matches!(v, Verdict::Unit { unit, .. } if unit.pgid() != 12)), "{:?}", plan.verdicts);
    }

    /// A body with two tool calls (one a tree of three) next to the kernel: units are per child of a leader, summed
    /// over the child's tree; a unit over the cap is the only verdict, its leader and the sibling unit are not.
    #[test]
    fn units_are_child_trees_summed_and_only_the_one_over_cap_goes() {
        let t = [p(10, 1, 10), p(11, 1, 11), p(20, 10, 20), p(21, 20, 20), p(22, 21, 20), p(30, 10, 30), p(40, 11, 40), p(99, 1, 99)];
        let fp = |pid: i32| Some(match pid { 21 => 200 * M, 22 => 100 * M, 30 => 250 * M, 40 => 900 * M, 10 | 11 => 50 * M, _ => M });
        let leaders = [l("body1", "body", 10), l("kern", "kernel", 11)];
        let mut w = Watch::default();
        let plan = w.plan(&leaders, &t, 256 * M, &fp);
        let unit = |root: i32| plan.units.iter().find(|(_, u)| u.key.root.0 == root).map(|(o, u)| (o.clone().unwrap().key, u.clone())).unwrap();
        let (o20, u20) = unit(20);
        assert_eq!((o20.as_str(), u20.members.iter().map(|p| p.pid).collect::<Vec<_>>(), u20.bytes), ("body1", vec![20, 21, 22], 301 * M));
        assert_eq!(unit(30).1.bytes, 250 * M);
        assert_eq!(plan.units.len(), 3, "20, 30 under the body, 40 under the kernel; 99 is nobody's");
        let over: Vec<(i32, Option<String>)> = plan.verdicts.iter().map(|v| match v {
            Verdict::Unit { leader, unit } => (unit.pgid(), leader.as_ref().map(|l| l.key.clone())),
            Verdict::Leader { leader, .. } => (-leader.pid, None),
        }).collect();
        assert_eq!(over, vec![(20, Some("body1".into())), (40, Some("kern".into()))], "the kernel's child unit may go, the kernel never");
    }

    /// The leader itself over the cap: the whole instance is the verdict (with its units), never per unit; the kernel
    /// over the cap is never a verdict.
    #[test]
    fn a_leader_over_the_cap_goes_whole_but_never_the_kernel() {
        let t = [p(10, 1, 10), p(20, 10, 20), p(11, 1, 11)];
        let fp = |pid: i32| Some(match pid { 10 | 11 => 500 * M, 20 => 300 * M, _ => 0 });
        let mut w = Watch::default();
        let plan = w.plan(&[l("b", "check", 10), l("k", "kernel", 11)], &t, 256 * M, &fp);
        assert_eq!(plan.verdicts.len(), 1);
        match &plan.verdicts[0] {
            Verdict::Leader { leader, bytes, units } => {
                assert_eq!((leader.key.as_str(), *bytes, units.len(), units[0].pgid()), ("b", 500 * M, 1, 20));
            }
            v => panic!("{v:?}"),
        }
    }

    /// A member whose parent exits (it is now launchd's child) stays in its unit on later polls, by pid and start
    /// time; a reused pid (another start time) does not; an orphan left in the unit root's group joins at once.
    #[test]
    fn orphans_stay_in_their_unit_by_identity() {
        let leaders = [l("b", "body", 10)];
        let fp = |pid: i32| Some(if pid == 22 { 400 * M } else { M });
        let mut w = Watch::default();
        let t1 = [p(10, 1, 10), p(20, 10, 20), p(21, 20, 20), p(22, 21, 22)];
        assert!(w.plan(&leaders, &t1, 256 * M, &fp).verdicts.len() == 1);
        // 21 exited: 22 (its own group now) is launchd's child; it still counts toward unit 20
        let t2 = [p(10, 1, 10), p(20, 10, 20), p(22, 1, 22)];
        let plan = w.plan(&leaders, &t2, 256 * M, &fp);
        let u = &plan.units.iter().find(|(_, u)| u.pgid() == 20).unwrap().1;
        assert_eq!(u.members.iter().map(|p| p.pid).collect::<Vec<_>>(), vec![20, 22]);
        assert_eq!(plan.verdicts.len(), 1);
        // pid 22 reused by a stranger (other start time): not ours
        let mut stranger = p(22, 1, 22);
        stranger.start_us += 7;
        let plan = w.plan(&leaders, &[p(10, 1, 10), p(20, 10, 20), stranger], 256 * M, &fp);
        assert!(plan.verdicts.is_empty() && plan.units.iter().all(|(_, u)| u.members.iter().all(|m| m.pid != 22)));
        // never seen, but orphaned inside the unit root's process group: joins
        let mut w = Watch::default();
        let plan = w.plan(&leaders, &[p(10, 1, 10), p(20, 10, 20), p(23, 1, 20)], 256 * M, &fp);
        assert_eq!(plan.units[0].1.members.iter().map(|p| p.pid).collect::<Vec<_>>(), vec![20, 23]);
        // the leader exits: its seen units live on as leftovers (no leader) and still face the cap
        let mut w = Watch::default();
        w.plan(&leaders, &t1, 256 * M, &fp);
        let plan = w.plan(&[], &[p(22, 1, 22)], 256 * M, &fp);
        assert!(matches!(&plan.verdicts[..], [Verdict::Unit { leader: None, .. }]), "{:?}", plan.verdicts);
    }

    /// Never a member: this process, pid 1, a leader (another instance), or a process outside every leader's tree.
    #[test]
    fn strangers_and_leaders_are_never_members() {
        let me = std::process::id() as i32;
        let t = [p(10, 1, 10), p(11, 10, 11), p(me, 10, me), p(50, 1, 50), p(51, 50, 50)];
        let mut w = Watch::default();
        let plan = w.plan(&[l("a", "body", 10), l("b", "body", 11)], &t, 0, &|_| Some(M));
        let all: Vec<i32> = plan.units.iter().flat_map(|(_, u)| u.members.iter().map(|p| p.pid)).collect();
        assert!(all.is_empty(), "{all:?}: 11 is a leader, {me} is us, 50/51 belong to nobody");
    }

    #[test]
    fn limits_and_records_are_integers() {
        let lim = Limits { cap: 256 * M, floor: u64::MAX };
        assert!(lim.disk_low(Path::new("/")).is_some(), "an impossible floor reads as low");
        assert!(Limits { cap: M, floor: 0 }.disk_low(Path::new("/")).is_none(), "floor 0 is off");
        assert!(Limits { cap: M, floor: 1 }.disk_low(Path::new("/")).is_none());
        assert!(lim.disk_msg(3 * M).contains("free disk 3 MiB is below the workload floor"));
        let r = record("mem_cap", Some(&l("i", "body", 9)), Some(20), "unit", 300 * M, &lim, None, &[20, 21]);
        assert!(icj(&r).is_ok(), "ICJ: integers only");
        assert_eq!((r["cap_mib"].as_u64(), r["footprint_mib"].as_u64(), r["pgid"].as_i64()), (Some(256), Some(300), Some(20)));
    }
}
