//! Retention of failing pool runs' session logs (evolution-v2.md §3.4.1, §3.12 EV-TRACE, §4.3):
//! `vault/gate/jobs/<job>/sessions/<run_id>.jsonl`, the body's session log of a **pool** run that
//! failed, copied verbatim by the kernel before the probe root is deleted. The miner (EVO-LITE) reads
//! them as trajectory evidence (`run:<gate_id>/<run_id>`); the agent sees them only through the
//! evidence directory. Holdout, canary, capability and S runs are never retained. The job directory
//! is kept 30 days (gate.md §3.2 "Cleanup"), and these logs go with it.

use std::io::Read;
use std::path::{Path, PathBuf};

use crate::g4::RunRecord;

/// The largest log kept per run; a longer one is cut at the last full line before this size.
pub const MAX_LOG_BYTES: u64 = 8 << 20;

/// `vault/gate/jobs/<job>/sessions/`.
pub fn sessions_dir(home: &Path, job: &str) -> PathBuf {
    home.join("vault/gate/jobs").join(job).join("sessions")
}

/// `vault/gate/jobs/<job>/sessions/<run_id>.jsonl`.
pub fn retained_path(home: &Path, job: &str, run_id: &str) -> PathBuf {
    sessions_dir(home, job).join(format!("{run_id}.jsonl"))
}

/// A failing pool run: set `pool` and a score below 1000 (whatever the outcome: `done`, `timeout`,
/// `cost_capped`, `crash`). Every other set is never retained, and a passing run carries no failure.
pub fn should_retain(rec: &RunRecord) -> bool {
    rec.set == "pool" && rec.score_permille < 1000
}

fn safe_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && !s.starts_with('.') && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}

/// Copy `log` (the run's session log, from the probe root) to [`retained_path`] when the run
/// [`should_retain`]. Returns the retained path, or `None` when the run is not retained or there is no
/// log. Never follows a symlink at `log`; never overwrites an existing file.
pub fn retain(home: &Path, rec: &RunRecord, log: &Path) -> std::io::Result<Option<PathBuf>> {
    if !should_retain(rec) {
        return Ok(None);
    }
    if !safe_name(&rec.gate_id) || !safe_name(&rec.run_id) {
        return Err(std::io::Error::other(format!("unsafe job or run id {:?}/{:?}", rec.gate_id, rec.run_id)));
    }
    let Ok(meta) = std::fs::symlink_metadata(log) else { return Ok(None) };
    if !meta.is_file() {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    std::fs::File::open(log)?.take(MAX_LOG_BYTES).read_to_end(&mut bytes)?;
    if meta.len() > MAX_LOG_BYTES {
        let keep = bytes.iter().rposition(|b| *b == b'\n').map(|i| i + 1).unwrap_or(0);
        bytes.truncate(keep);
    }
    let dest = retained_path(home, &rec.gate_id, &rec.run_id);
    std::fs::create_dir_all(sessions_dir(home, &rec.gate_id))?;
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&dest)?;
    std::io::Write::write_all(&mut f, &bytes)?;
    Ok(Some(dest))
}

/// The newest session log under a probe root's `state/sessions/` (one run, one session), if any.
pub fn session_log_in(root: &Path) -> Option<PathBuf> {
    session_log_in_dir(&root.join("state/sessions"))
}

/// The newest regular `*.jsonl` file directly in `dir`. `dir` itself must be a real directory, not a
/// symlink (the body wrote it; the kernel never follows its links), and so must each file.
pub fn session_log_in_dir(dir: &Path) -> Option<PathBuf> {
    if !std::fs::symlink_metadata(dir).ok()?.is_dir() {
        return None;
    }
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        let Ok(m) = std::fs::symlink_metadata(&p) else { continue };
        if !m.is_file() || p.extension().and_then(|x| x.to_str()) != Some("jsonl") {
            continue;
        }
        let t = m.modified().unwrap_or(std::time::UNIX_EPOCH);
        if best.as_ref().is_none_or(|(bt, bp)| (t, &p) > (*bt, bp)) {
            best = Some((t, p));
        }
    }
    best.map(|b| b.1)
}

/// Every retained log: `(job, run_id, path)`, sorted.
pub fn list_retained(home: &Path) -> Vec<(String, String, PathBuf)> {
    let mut out = Vec::new();
    let Ok(jobs) = std::fs::read_dir(home.join("vault/gate/jobs")) else { return out };
    for j in jobs.flatten() {
        let job = j.file_name().to_string_lossy().into_owned();
        let Ok(rd) = std::fs::read_dir(sessions_dir(home, &job)) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(run) = name.strip_suffix(".jsonl") {
                out.push((job.clone(), run.to_string(), e.path()));
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{remove_home, test_home};

    fn rec(set: &str, run: &str, score: u32) -> RunRecord {
        RunRecord { v: 3, gate_id: "j-000031".into(), run_id: run.into(), arm: "c".into(), set: set.into(), score_permille: score, requests: 3, outcome: "done".into(), ..Default::default() }
    }

    /// EV-07: a failing pool run's log is retained; a passing pool run's, a holdout's and a canary's
    /// are not.
    #[test]
    fn ev07_failing_pool_logs_retained_holdout_canary_not() {
        let home = test_home("ev07");
        let root = home.join("probe/0000000000a1");
        std::fs::create_dir_all(root.join("state/sessions")).unwrap();
        let log = root.join("state/sessions/20261020-230000-abcdef.jsonl");
        std::fs::write(&log, b"{\"id\":\"1\",\"type\":\"session\"}\n{\"id\":\"2\",\"type\":\"error\"}\n").unwrap();
        assert_eq!(session_log_in(&root).as_deref(), Some(log.as_path()));
        let failing = rec("pool", "c-repo-03-r1", 0);
        let kept = retain(&home, &failing, &log).unwrap().unwrap();
        assert_eq!(kept, home.join("vault/gate/jobs/j-000031/sessions/c-repo-03-r1.jsonl"));
        assert_eq!(std::fs::read(&kept).unwrap(), std::fs::read(&log).unwrap());
        let mut capped = rec("pool", "p-repo-03-r1", 400);
        capped.outcome = "cost_capped".into();
        capped.arm = "p".into();
        assert!(retain(&home, &capped, &log).unwrap().is_some());
        for (set, run, score) in [("pool", "c-ok-r1", 1000), ("holdout", "c-h1-r1", 0), ("canary", "c-k1-r1", 0), ("capability", "c-a1-r1", 0)] {
            assert_eq!(retain(&home, &rec(set, run, score), &log).unwrap(), None, "{set}");
            assert!(!retained_path(&home, "j-000031", run).exists(), "{set}");
        }
        let listed: Vec<String> = list_retained(&home).into_iter().map(|(j, r, _)| format!("{j}/{r}")).collect();
        assert_eq!(listed, ["j-000031/c-repo-03-r1", "j-000031/p-repo-03-r1"]);
        // No log, or a symlink: nothing retained; an unsafe run id is refused.
        assert_eq!(retain(&home, &rec("pool", "c-x-r1", 0), &root.join("missing.jsonl")).unwrap(), None);
        std::os::unix::fs::symlink(&log, root.join("link.jsonl")).unwrap();
        assert_eq!(retain(&home, &rec("pool", "c-y-r1", 0), &root.join("link.jsonl")).unwrap(), None);
        assert!(retain(&home, &rec("pool", "../x", 0), &log).is_err());
        // A kept sessions dir that is a symlink (the body replaced it) is never followed.
        std::os::unix::fs::symlink(root.join("state/sessions"), home.join("kept-link")).unwrap();
        assert_eq!(session_log_in_dir(&home.join("kept-link")), None);
        assert_eq!(session_log_in_dir(&root.join("state/sessions")).as_deref(), Some(log.as_path()));
        remove_home(&home);
    }

    /// An oversized log is cut at the last full line before [`MAX_LOG_BYTES`].
    #[test]
    fn oversized_log_cut_at_a_line() {
        let home = test_home("retain-big");
        let log = home.join("big.jsonl");
        let line = format!("{{\"pad\":\"{}\"}}\n", "x".repeat(1000));
        let n = (MAX_LOG_BYTES as usize / line.len()) + 3;
        std::fs::write(&log, line.repeat(n)).unwrap();
        let kept = retain(&home, &rec("pool", "c-big-r1", 0), &log).unwrap().unwrap();
        let b = std::fs::read(kept).unwrap();
        assert!(b.len() as u64 <= MAX_LOG_BYTES && b.ends_with(b"\n") && b.len().is_multiple_of(line.len()));
        remove_home(&home);
    }
}
