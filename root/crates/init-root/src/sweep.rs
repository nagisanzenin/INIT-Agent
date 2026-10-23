//! The D1 membership rule and the instance sweep (root/root.md §3.5.4), over any process table.
//! Live-system safety (CONVENTIONS §1, D1, ADR-008): on the owner's machine nothing here signals. The live
//! scan (`zone::scan`, which hands D1 members to `sys::signal_member`) exists only with the `live-sweep`
//! feature, refuses to run outside a disposable environment (`disposable`), and cannot be built in a
//! release build. `classify` is the dry-run's view of the same rule (`doctor --sweep-dry-run`).
//! Leaders and process groups the Root holds are signalled by `zone.rs`, never from here.

use std::path::Path;

// ADR-008: a release build never carries the live D1 scan. Lift this only with the ADR's enabling step.
#[cfg(all(feature = "live-sweep", not(debug_assertions)))]
compile_error!("init-root: `live-sweep` is for disposable-environment test builds only (ADR-008); never in a release build");

/// What a sweep may ask about a process. `None` means "cannot tell", which never matches.
pub trait ProcTable {
    /// Every pid of our uid.
    fn pids(&self) -> Vec<i32>;
    /// (a) `sandbox_check(pid, NULL, 0) == 1`; `None` when `sandbox_check` is missing (RF13) or fails.
    fn sandboxed(&self, pid: i32) -> Option<bool>;
    /// `sandbox_check(pid, "file-read-data", PATH | NO_REPORT, path) == 0`.
    fn may_read(&self, pid: i32, path: &Path) -> Option<bool>;
}

/// The paths of one membership test: the instance mark (or the zone mark, generic sweep), the decoy, the canary.
pub struct Marks<'a> {
    pub mark: &'a Path,
    pub decoy: &'a Path,
    pub canary: &'a Path,
}

/// D1: (a) sandboxed, (b) may read the mark, (c) denied both the sibling decoy and the Root canary.
pub fn member(t: &dyn ProcTable, pid: i32, m: &Marks) -> bool {
    t.sandboxed(pid) == Some(true)
        && t.may_read(pid, m.mark) == Some(true)
        && t.may_read(pid, m.decoy) == Some(false)
        && t.may_read(pid, m.canary) == Some(false)
}

/// The dry-run's verdict on one process against a set of marks (the instance marks and the zone mark).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// D1 holds for these marks (indices into the caller's list): a sweep would take this process.
    Member(Vec<usize>),
    /// Fails (a): not sandboxed, the owner's ordinary processes.
    NotSandboxed,
    /// Fails (b) for every mark: sandboxed, but reads none of them.
    MarkUnreadable,
    /// Fails (c): reads a mark but also the decoy or the canary (a permissive foreign sandbox).
    ReadsDecoyOrCanary,
    /// `sandbox_check` gave no answer for some condition; "cannot tell" never matches.
    Unknown,
}

/// Classifies `pid` with exactly `member` (the D1 rule the live sweep applies), and names the first
/// condition that fails when it is not a member.
pub fn classify(t: &dyn ProcTable, pid: i32, marks: &[&Path], decoy: &Path, canary: &Path) -> Verdict {
    let hits: Vec<usize> = (0..marks.len()).filter(|&i| member(t, pid, &Marks { mark: marks[i], decoy, canary })).collect();
    if !hits.is_empty() {
        return Verdict::Member(hits);
    }
    match t.sandboxed(pid) {
        None => return Verdict::Unknown,
        Some(false) => return Verdict::NotSandboxed,
        Some(true) => {}
    }
    let reads: Vec<Option<bool>> = marks.iter().map(|m| t.may_read(pid, m)).collect();
    if !reads.contains(&Some(true)) {
        return if reads.contains(&None) { Verdict::Unknown } else { Verdict::MarkUnreadable };
    }
    match (t.may_read(pid, decoy), t.may_read(pid, canary)) {
        (Some(true), _) | (_, Some(true)) => Verdict::ReadsDecoyOrCanary,
        _ => Verdict::Unknown,
    }
}

/// Up to 50 passes, 20 ms apart: every member except `skip` is handed to `kill`. Stops after a pass that
/// finds none. With `sandbox_check` missing (degraded, RF13) it never scans. Returns the pids killed.
/// Built only for the unit tests (fake tables) and `live-sweep` builds (ADR-008).
#[cfg(any(test, feature = "live-sweep"))]
pub fn sweep(t: &dyn ProcTable, m: &Marks, skip: &[i32], kill: &mut dyn FnMut(i32) -> bool) -> Vec<i32> {
    let mut killed = Vec::new();
    if t.sandboxed(std::process::id() as i32).is_none() {
        return killed;
    }
    for pass in 0..50 {
        let found: Vec<i32> = t.pids().into_iter().filter(|p| !skip.contains(p) && member(t, *p, m)).collect();
        if found.is_empty() {
            break;
        }
        killed.extend(found.into_iter().filter(|p| kill(*p)));
        if pass < 49 {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    killed
}

/// The disposable-environment sentinel of testing.md §3.1a (init-testkit `guard`): created by whoever
/// provisions a VM or a separate macOS user, never by a test, and impossible to create without root.
#[cfg(any(test, feature = "live-sweep"))]
pub const SENTINEL: &str = if cfg!(target_os = "macos") { "/var/db/init-test-disposable" } else { "/etc/init-test-disposable" };

/// The pure sentinel decision: a regular file owned by uid 0, not writable by group or others, whose
/// content is our uid. `meta` is (is a regular file, owner uid, mode, content).
#[cfg(any(test, feature = "live-sweep"))]
pub fn sentinel_ok(meta: Option<(bool, u32, u32, &str)>, uid: u32) -> Result<(), String> {
    let Some((regular, owner, mode, content)) = meta else { return Err(format!("no sentinel at {SENTINEL}")) };
    if !regular || owner != 0 || mode & 0o022 != 0 {
        return Err(format!("{SENTINEL} must be a regular file owned by root, not group- or world-writable"));
    }
    if content.trim().parse::<u32>().ok() != Some(uid) {
        return Err(format!("{SENTINEL} does not name uid {uid}"));
    }
    Ok(())
}

/// ADR-008's second lock on the live scan: even a `live-sweep` build scans only where the sentinel says
/// this account is disposable. The owner's account has none, so there it signals only held pids.
#[cfg(feature = "live-sweep")]
pub fn disposable() -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::symlink_metadata(SENTINEL).ok();
    let content = m.as_ref().filter(|m| m.is_file() && m.len() < 64).and_then(|_| std::fs::read_to_string(SENTINEL).ok()).unwrap_or_default();
    sentinel_ok(m.as_ref().map(|m| (m.file_type().is_file(), m.uid(), m.mode(), content.as_str())), crate::sys::euid())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    /// A fake process: sandboxed or not, and the set of paths its policy lets it read.
    #[derive(Clone)]
    struct P {
        sandboxed: Option<bool>,
        reads: Vec<PathBuf>,
        respawns: u32,
    }

    struct Fake {
        procs: RefCell<BTreeMap<i32, P>>,
        degraded: bool,
    }

    impl ProcTable for Fake {
        fn pids(&self) -> Vec<i32> {
            self.procs.borrow().keys().copied().collect()
        }
        fn sandboxed(&self, pid: i32) -> Option<bool> {
            if self.degraded {
                return None;
            }
            if pid == std::process::id() as i32 {
                return Some(false);
            }
            self.procs.borrow().get(&pid).and_then(|p| p.sandboxed)
        }
        fn may_read(&self, pid: i32, path: &Path) -> Option<bool> {
            self.procs.borrow().get(&pid).map(|p| p.reads.iter().any(|r| r == path))
        }
    }

    const MARK: &str = "/i/run/mark/a3f09c1e5b7d";
    const OTHER: &str = "/i/run/mark/0123456789ab";
    const DECOY: &str = "/i/run/mark/decoy";
    const CANARY: &str = "/r/run/canary";
    const ZONE: &str = "/i/run/zone-mark";

    fn p(sandboxed: Option<bool>, reads: &[&str]) -> P {
        P { sandboxed, reads: reads.iter().map(PathBuf::from).collect(), respawns: 0 }
    }

    fn table(procs: Vec<(i32, P)>) -> Fake {
        Fake { procs: RefCell::new(procs.into_iter().collect()), degraded: false }
    }

    fn marks(mark: &str) -> (PathBuf, PathBuf, PathBuf) {
        (mark.into(), DECOY.into(), CANARY.into())
    }

    /// Kills by removing from the fake table (a respawning process comes back once per respawn).
    fn run(t: &Fake, mark: &str, skip: &[i32]) -> Vec<i32> {
        let (m, d, c) = marks(mark);
        let m = Marks { mark: &m, decoy: &d, canary: &c };
        sweep(t, &m, skip, &mut |pid| {
            let mut procs = t.procs.borrow_mut();
            let Some(x) = procs.get_mut(&pid) else { return false };
            if x.respawns > 0 {
                x.respawns -= 1;
                let again = x.clone();
                procs.remove(&pid);
                procs.insert(pid + 10_000, again); // the escapee forked a successor before dying
            } else {
                procs.remove(&pid);
            }
            true
        })
    }

    #[test]
    fn d1_needs_all_three_conditions() {
        let zone = p(Some(true), &[MARK, ZONE]); // a zone instance: reads its mark and the zone mark only
        let t = table(vec![
            (101, zone.clone()),
            (102, p(Some(true), &[MARK, DECOY, CANARY])), // a permissive foreign sandbox: never
            (103, p(Some(true), &[MARK, DECOY])),         // reads the decoy: never
            (104, p(Some(true), &[MARK, CANARY])),        // reads the canary: never
            (105, p(Some(false), &[MARK])),               // the owner's shell, unsandboxed: never
            (106, p(Some(true), &[OTHER, ZONE])),         // another instance: not this one
            (107, p(None, &[MARK])),                      // cannot tell: never
            (108, p(Some(true), &[MARK])),                // reads the mark only, denied decoy and canary (RR-7): yes
        ]);
        let (m, d, c) = marks(MARK);
        let m = Marks { mark: &m, decoy: &d, canary: &c };
        let hits: Vec<i32> = t.pids().into_iter().filter(|pid| member(&t, *pid, &m)).collect();
        assert_eq!(hits, [101, 108]);
        assert_eq!(run(&t, MARK, &[]), [101, 108]);
        assert_eq!(t.pids(), [102, 103, 104, 105, 106, 107], "everything else is untouched");
    }

    /// The 2026-10-20 incident shape: 31 foreign sandboxed processes that can read the mark (the v0.2
    /// rule, (a) and (b) only, killed them all). D1 matches none of them.
    #[test]
    fn the_31_process_incident_matches_nothing() {
        let mut procs: Vec<(i32, P)> = (0..31).map(|i| (200 + i, p(Some(true), &[MARK, ZONE, DECOY, CANARY, "/Applications"]))).collect();
        procs.push((300, p(Some(true), &[ZONE])));
        let t = table(procs);
        let v02 = t.pids().into_iter().filter(|pid| t.sandboxed(*pid) == Some(true) && t.may_read(*pid, Path::new(MARK)) == Some(true)).count();
        assert_eq!(v02, 31, "the v0.2 rule would have matched them");
        assert!(run(&t, MARK, &[]).is_empty());
        assert_eq!(run(&t, ZONE, &[]), [300], "the generic sweep (zone mark) takes only the zone instance");
        assert_eq!(t.pids().len(), 31);
    }

    #[test]
    fn passes_repeat_until_clean_skip_and_degraded() {
        let mut esc = p(Some(true), &[MARK]);
        esc.respawns = 3;
        let t = table(vec![(101, esc), (102, p(Some(true), &[MARK]))]);
        let killed = run(&t, MARK, &[102]);
        assert_eq!(killed, [101, 10_101, 20_101, 30_101], "a re-forking escapee is caught on later passes");
        assert_eq!(t.pids(), [102], "skipped pids are never signalled");
        let mut forever = p(Some(true), &[MARK]);
        forever.respawns = 1_000;
        let t = table(vec![(1, forever)]);
        assert_eq!(run(&t, MARK, &[]).len(), 50, "bounded at 50 passes");
        let t = Fake { procs: RefCell::new([(101, p(Some(true), &[MARK]))].into_iter().collect()), degraded: true };
        assert!(run(&t, MARK, &[]).is_empty(), "no sandbox_check: never scan (RF13)");
        assert_eq!(t.pids(), [101]);
    }

    /// The dry-run view: the same rule as `member`, with the first failing condition named.
    #[test]
    fn classify_names_the_failing_condition() {
        let t = table(vec![
            (101, p(Some(true), &[MARK, ZONE])),
            (102, p(Some(true), &[MARK, DECOY, CANARY])),
            (103, p(Some(true), &[MARK, CANARY])),
            (104, p(Some(false), &[MARK])),
            (105, p(Some(true), &[OTHER])),
            (106, p(None, &[MARK])),
            (107, p(Some(true), &[ZONE])),
        ]);
        let (m, z, d, c) = (Path::new(MARK), Path::new(ZONE), Path::new(DECOY), Path::new(CANARY));
        let v = |pid| classify(&t, pid, &[m, z], d, c);
        assert_eq!(v(101), Verdict::Member(vec![0, 1]));
        assert_eq!(v(107), Verdict::Member(vec![1]), "the zone mark alone (generic sweep)");
        assert_eq!((v(102), v(103)), (Verdict::ReadsDecoyOrCanary, Verdict::ReadsDecoyOrCanary));
        assert_eq!((v(104), v(105), v(106)), (Verdict::NotSandboxed, Verdict::MarkUnreadable, Verdict::Unknown));
        assert_eq!(v(999), Verdict::Unknown, "a pid that is gone");
        for pid in t.pids() {
            let any = [m, z].iter().any(|mk| member(&t, pid, &Marks { mark: mk, decoy: d, canary: c }));
            assert_eq!(matches!(v(pid), Verdict::Member(_)), any, "classify agrees with member for {pid}");
        }
    }

    #[test]
    fn sentinel_needs_a_root_owned_file_naming_our_uid() {
        assert!(sentinel_ok(None, 501).is_err());
        assert!(sentinel_ok(Some((true, 501, 0o100644, "501")), 501).is_err(), "owned by the user: a test could write it");
        assert!(sentinel_ok(Some((true, 0, 0o100666, "501")), 501).is_err(), "world-writable");
        assert!(sentinel_ok(Some((false, 0, 0o120644, "501")), 501).is_err(), "a symlink");
        assert!(sentinel_ok(Some((true, 0, 0o100644, "502")), 501).is_err(), "another uid");
        assert!(sentinel_ok(Some((true, 0, 0o100644, "501\n")), 501).is_ok());
    }
}
