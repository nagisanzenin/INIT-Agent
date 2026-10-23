//! The job model (kernel/initctl.md §3.6, §4.3, §4.4).
//!
//! - Ids are `j-` + 6 digits, monotonic per `INIT_HOME` (`state/kernel/jobs/seq` plus the
//!   records on disk).
//! - Records live at `state/kernel/jobs/<id>.json`, rewritten atomically on every state or stage
//!   change (not on progress ticks). The newest 200 are kept; older terminal records are deleted
//!   by exact path.
//! - A job does not depend on any connection; `wait` reattaches.
//! - `wait` returns at a terminal state, `awaiting_boundary` or `awaiting_swap`.
//! - Ordering guarantee (§4.1 `promote`): [`JobCtl::awaiting_boundary`] and
//!   [`JobCtl::awaiting_swap`] return only after every connection waiting on the job has written
//!   its final response (bounded by `flush_timeout`), so the backend sends `pause` after them.
//! - At cold start every record that is not terminal is marked `failed` with
//!   `reason:"kernel_restart"` (§3.7, §5).

use crate::backend::LaneCtx;
use crate::clock;
use crate::fsx;
use init_proto::errors::ErrorCode;
use init_proto::initctl::{JOB_RECORDS_KEPT, JobRecord, JobState, JobSummary, Lane, Progress, RecordProgress, Stage, StageResult, job_id};
use init_proto::{ErrorBody, V1};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Job store tuning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobConfig {
    /// How long a milestone waits for pending final responses to be written.
    pub flush_timeout: Duration,
    /// Progress frames: at most one per interval unless the stage or state changes.
    pub progress_interval: Duration,
    /// Records kept (200).
    pub keep: usize,
}

impl Default for JobConfig {
    fn default() -> JobConfig {
        JobConfig {
            flush_timeout: Duration::from_secs(5),
            progress_interval: Duration::from_millis(init_proto::envelope::PROGRESS_MIN_INTERVAL_MS),
            keep: JOB_RECORDS_KEPT,
        }
    }
}

/// A progress frame without its timestamp (repeated ticks are skipped).
type FrameKey = (Option<u64>, String, u8, Option<Stage>, JobState);

/// Significant progress changes (stage or state) kept per job for waiters.
const SIG_KEPT: usize = 64;

struct Job {
    rec: JobRecord,
    /// The owning lane instance's key; `None` for records loaded at cold start.
    owner: Option<String>,
    /// Hidden while the synchronous part runs; never listed, never written.
    visible: bool,
    version: u64,
    /// Significant progress changes: (version, progress).
    sig: VecDeque<(u64, RecordProgress)>,
    /// Connections that will write a final response for this job at its milestone.
    pending_finals: u32,
    /// (state, stage count, progress stage) at the last write.
    persisted: Option<(JobState, usize, Option<Stage>)>,
}

impl Job {
    fn bump(&mut self) {
        self.version += 1;
    }
}

struct State {
    last_seq: u64,
    jobs: BTreeMap<u64, Job>,
    shutdown: bool,
}

struct Shared {
    dir: PathBuf,
    cfg: JobConfig,
    st: Mutex<State>,
    cv: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn path_of(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// Write the record if its state, stage list or progress stage changed (or `force`).
    fn persist(&self, job: &mut Job, force: bool) {
        if !job.visible {
            return;
        }
        let key = (job.rec.state.clone(), job.rec.stages.len(), job.rec.progress.as_ref().and_then(|p| p.stage.clone()));
        if !force && job.persisted.as_ref() == Some(&key) {
            return;
        }
        if fsx::write_json(&self.path_of(&job.rec.id), &job.rec).is_ok() {
            job.persisted = Some(key);
        }
    }

    /// Keep the newest `keep` records: delete older terminal ones by exact path.
    fn prune(&self, st: &mut State) {
        let visible = st.jobs.values().filter(|j| j.visible).count();
        if visible <= self.cfg.keep {
            return;
        }
        let mut excess = visible - self.cfg.keep;
        let old: Vec<u64> = st
            .jobs
            .iter()
            .filter(|(_, j)| j.visible && j.rec.state.is_terminal())
            .map(|(s, _)| *s)
            .collect();
        for seq in old {
            if excess == 0 {
                break;
            }
            if let Some(j) = st.jobs.remove(&seq) {
                let _ = std::fs::remove_file(self.path_of(&j.rec.id)); // exact path
                excess -= 1;
            }
        }
    }
}

/// The kernel's jobs. Cheap to clone.
#[derive(Clone)]
pub struct JobStore {
    sh: Arc<Shared>,
}

/// Why a [`JobCtl`] milestone did not apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobGone {
    /// The job was cancelled (`job.cancel`): stop working on it.
    Cancelled,
    /// The job is already terminal, or no longer exists.
    Finished,
}

/// The end of a wait.
#[derive(Debug)]
pub(crate) enum WaitEnd {
    /// The record reached a state `wait` returns at.
    Reached(JobRecord),
    /// The deadline passed; the current record.
    TimedOut(JobRecord),
    /// The client went away or the server is shutting down.
    Gone,
}

/// Where a waiter starts reading progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WaitFrom {
    /// Every significant change since the job was reserved (the command's own wait).
    Start,
    /// The current progress once, then changes (a reattaching `job.wait`).
    Now,
}

/// Decrements the job's pending final responses on drop (after the final response is written).
pub(crate) struct WaiterGuard {
    sh: Arc<Shared>,
    seq: u64,
    active: bool,
}

impl WaiterGuard {
    /// Move the registration to another job (`Begun::Existing`).
    pub(crate) fn retarget(&mut self, seq: u64) {
        if self.active && self.seq == seq {
            return;
        }
        self.release();
        let mut st = self.sh.lock();
        if let Some(j) = st.jobs.get_mut(&seq) {
            j.pending_finals += 1;
            self.active = true;
        }
        self.seq = seq;
    }

    fn release(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        let mut st = self.sh.lock();
        if let Some(j) = st.jobs.get_mut(&self.seq) {
            j.pending_finals = j.pending_finals.saturating_sub(1);
        }
        drop(st);
        self.sh.cv.notify_all();
    }
}

impl Drop for WaiterGuard {
    fn drop(&mut self) {
        self.release();
    }
}

fn parse_seq(id: &str) -> Option<u64> {
    id.strip_prefix("j-").filter(|d| d.len() >= 6 && d.bytes().all(|b| b.is_ascii_digit()))?.parse().ok()
}

impl JobStore {
    /// Open `dir` (`INIT_HOME/state/kernel/jobs`): load the records, mark every non-terminal one
    /// `failed` with `reason:"kernel_restart"`, and return the ids so marked.
    pub fn open(dir: &Path, cfg: JobConfig) -> io::Result<(JobStore, Vec<String>)> {
        std::fs::create_dir_all(dir)?;
        let mut jobs = BTreeMap::new();
        let mut last_seq = std::fs::read_to_string(dir.join("seq")).ok().and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(0);
        let mut recovered = Vec::new();
        for e in std::fs::read_dir(dir)? {
            let e = e?;
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_suffix(".json") else { continue };
            let Some(seq) = parse_seq(id) else { continue };
            let Ok(bytes) = std::fs::read(e.path()) else { continue };
            let Ok(mut rec) = init_proto::files::decode::<JobRecord>(&bytes) else { continue };
            if rec.id != id {
                continue;
            }
            last_seq = last_seq.max(seq);
            let mut dirty = false;
            if !rec.state.is_terminal() {
                rec.state = JobState::Failed;
                rec.reason = Some("kernel_restart".into());
                rec.finished_at = Some(clock::now_rfc3339());
                recovered.push(rec.id.clone());
                dirty = true;
            }
            let mut job = Job {
                rec,
                owner: None,
                visible: true,
                version: 1,
                sig: VecDeque::new(),
                pending_finals: 0,
                persisted: None,
            };
            let key = (job.rec.state.clone(), job.rec.stages.len(), job.rec.progress.as_ref().and_then(|p| p.stage.clone()));
            job.persisted = Some(key);
            if dirty {
                fsx::write_json(&dir.join(format!("{id}.json")), &job.rec)?;
            }
            // Live-lane records keep their owner: there is exactly one live lane.
            if job.rec.lane == Lane::Live {
                job.owner = Some(LaneCtx::live().key());
            }
            jobs.insert(seq, job);
        }
        recovered.sort();
        let sh = Arc::new(Shared {
            dir: dir.to_path_buf(),
            cfg,
            st: Mutex::new(State { last_seq, jobs, shutdown: false }),
            cv: Condvar::new(),
        });
        {
            let mut st = sh.lock();
            sh.prune(&mut st);
        }
        Ok((JobStore { sh }, recovered))
    }

    /// The store's directory.
    pub fn dir(&self) -> &Path {
        &self.sh.dir
    }

    fn alloc(&self, owner: &LaneCtx, cmd: &str, args: Value, visible: bool) -> io::Result<JobCtl> {
        let mut st = self.sh.lock();
        let seq = st.last_seq + 1;
        fsx::write_atomic(&self.sh.dir.join("seq"), format!("{seq}\n").as_bytes())?;
        st.last_seq = seq;
        let id = job_id(seq);
        let rec = JobRecord {
            v: V1,
            id: id.clone(),
            cmd: cmd.to_string(),
            lane: owner.lane,
            state: JobState::Queued,
            args,
            created_at: clock::now_rfc3339(),
            finished_at: None,
            progress: None,
            result: None,
            error: None,
            sha: None,
            stages: Vec::new(),
            root_job: None,
            reason: None,
            timed_out: None,
        };
        let mut job = Job {
            rec,
            owner: Some(owner.key()),
            visible,
            version: 1,
            sig: VecDeque::new(),
            pending_finals: 0,
            persisted: None,
        };
        if visible {
            self.sh.persist(&mut job, true);
        }
        st.jobs.insert(seq, job);
        if visible {
            self.sh.prune(&mut st);
        }
        drop(st);
        self.sh.cv.notify_all();
        Ok(JobCtl { sh: self.sh.clone(), seq, id })
    }

    /// Create a visible job owned by `owner` (kernel-initiated jobs, such as an asynchronous G4,
    /// whose id then appears in `status` and `job.*`).
    pub fn create(&self, owner: &LaneCtx, cmd: &str, args: Value) -> io::Result<JobCtl> {
        self.alloc(owner, cmd, args, true)
    }

    /// Reserve a hidden job for a command's synchronous part.
    pub(crate) fn reserve(&self, owner: &LaneCtx, cmd: &str, args: Value) -> io::Result<JobCtl> {
        self.alloc(owner, cmd, args, false)
    }

    /// Make a reserved job visible and write its record.
    pub(crate) fn publish(&self, seq: u64) {
        let mut st = self.sh.lock();
        if let Some(j) = st.jobs.get_mut(&seq) {
            j.visible = true;
            self.sh.persist(j, true);
            j.bump();
        }
        self.sh.prune(&mut st);
        drop(st);
        self.sh.cv.notify_all();
    }

    /// Drop a reserved job whose synchronous part failed (its id stays consumed).
    pub(crate) fn discard(&self, seq: u64) {
        let mut st = self.sh.lock();
        if st.jobs.get(&seq).is_some_and(|j| !j.visible) {
            st.jobs.remove(&seq);
        }
        drop(st);
        self.sh.cv.notify_all();
    }

    /// Reattach only a self-upgrade job interrupted by this kernel restart. The caller must
    /// first match its durable receipt's kernel binary against Root's current tuple; this
    /// resumes observation, never declares success without Root's probation event.
    pub fn resume_kernel_swap(&self, id: &str, sha: &str, root_job: Option<&str>) -> Option<JobCtl> {
        let seq = parse_seq(id)?;
        let mut st = self.sh.lock();
        let j = st.jobs.get_mut(&seq)?;
        if j.rec.cmd != "kernel.promote" || j.rec.reason.as_deref() != Some("kernel_restart")
            || j.rec.sha.as_deref() != Some(sha) { return None; }
        j.rec.state = JobState::AwaitingSwap;
        j.rec.reason = None;
        j.rec.error = None;
        j.rec.finished_at = None;
        j.rec.root_job = root_job.map(str::to_string).or_else(|| j.rec.root_job.clone());
        j.bump();
        self.sh.persist(j, true);
        drop(st);
        self.sh.cv.notify_all();
        Some(JobCtl { sh: self.sh.clone(), seq, id: id.into() })
    }

    /// A visible job's record.
    pub fn record(&self, id: &str) -> Option<JobRecord> {
        let seq = parse_seq(id)?;
        let st = self.sh.lock();
        st.jobs.get(&seq).filter(|j| j.visible).map(|j| j.rec.clone())
    }

    /// `(seq, record, owner key)` of a visible job.
    pub(crate) fn lookup(&self, id: &str) -> Option<(u64, JobRecord, Option<String>)> {
        let seq = parse_seq(id)?;
        let st = self.sh.lock();
        st.jobs.get(&seq).filter(|j| j.visible).map(|j| (seq, j.rec.clone(), j.owner.clone()))
    }

    /// Visible jobs of `owner`, newest first.
    pub(crate) fn list(&self, owner: &str, active_only: bool, limit: usize) -> Vec<JobRecord> {
        let st = self.sh.lock();
        st.jobs
            .values()
            .rev()
            .filter(|j| j.visible && j.owner.as_deref() == Some(owner) && (!active_only || !j.rec.state.is_terminal()))
            .take(limit)
            .map(|j| j.rec.clone())
            .collect()
    }

    /// `status.jobs`: the active jobs of `owner`, oldest first.
    pub(crate) fn summaries(&self, owner: &str) -> Vec<JobSummary> {
        let st = self.sh.lock();
        st.jobs
            .values()
            .filter(|j| j.visible && j.owner.as_deref() == Some(owner) && !j.rec.state.is_terminal())
            .map(|j| JobSummary {
                cmd: j.rec.cmd.clone(),
                id: j.rec.id.clone(),
                stage: j.rec.progress.as_ref().and_then(|p| p.stage.clone()),
                state: j.rec.state.clone(),
            })
            .collect()
    }

    /// The newest visible job of `owner` (of command `cmd`).
    pub(crate) fn newest(&self, owner: &str, cmd: Option<&str>) -> Option<String> {
        let st = self.sh.lock();
        st.jobs
            .values()
            .rev()
            .find(|j| j.visible && j.owner.as_deref() == Some(owner) && cmd.is_none_or(|c| j.rec.cmd == c))
            .map(|j| j.rec.id.clone())
    }

    /// Active (non-terminal) visible jobs of command `cmd`, any lane, oldest first:
    /// `(id, record)`.
    pub fn active_of(&self, cmd: &str) -> Vec<JobRecord> {
        let st = self.sh.lock();
        st.jobs.values().filter(|j| j.visible && j.rec.cmd == cmd && !j.rec.state.is_terminal()).map(|j| j.rec.clone()).collect()
    }

    /// The active job of `owner` running command `cmd`, if any (idempotent `reload`).
    pub(crate) fn active_owned(&self, owner: &str, cmd: &str) -> Option<(u64, JobRecord)> {
        let st = self.sh.lock();
        st.jobs
            .iter()
            .rev()
            .find(|(_, j)| j.visible && j.owner.as_deref() == Some(owner) && j.rec.cmd == cmd && !j.rec.state.is_terminal())
            .map(|(s, j)| (*s, j.rec.clone()))
    }

    /// `job.cancel`: mark a cancellable job `cancelled` (atomically with the check).
    pub(crate) fn cancel(&self, seq: u64) -> Result<JobRecord, ErrorBody> {
        let mut st = self.sh.lock();
        let Some(j) = st.jobs.get_mut(&seq).filter(|j| j.visible) else {
            return Err(ErrorBody::new(ErrorCode::NotFound, "no such job"));
        };
        if !j.rec.state.cancellable() {
            return Err(ErrorBody::new(
                ErrorCode::NotCancellable,
                format!("job {} is {}: past its cancellable stages", j.rec.id, j.rec.state),
            ));
        }
        j.rec.state = JobState::Cancelled;
        j.rec.finished_at = Some(clock::now_rfc3339());
        j.rec.error = Some(ErrorBody::new(ErrorCode::Cancelled, "job was cancelled"));
        j.bump();
        self.sh.persist(j, true);
        let rec = j.rec.clone();
        self.sh.prune(&mut st);
        drop(st);
        self.sh.cv.notify_all();
        Ok(rec)
    }

    /// Register a connection that will write a final response for job `seq`.
    pub(crate) fn add_waiter(&self, seq: u64) -> WaiterGuard {
        let mut st = self.sh.lock();
        let active = match st.jobs.get_mut(&seq) {
            Some(j) => {
                j.pending_finals += 1;
                true
            }
            None => false,
        };
        WaiterGuard { sh: self.sh.clone(), seq, active }
    }

    /// Wait until job `seq` reaches a state `wait` returns at, the deadline passes, the client
    /// goes away (`alive` false) or the server shuts down. Progress frames go to `sink`: every
    /// stage or state change, and otherwise at most one per `progress_interval`.
    pub(crate) fn wait(
        &self,
        seq: u64,
        from: WaitFrom,
        deadline: Option<Instant>,
        sink: &mut dyn FnMut(&Progress) -> io::Result<()>,
        alive: &mut dyn FnMut() -> bool,
    ) -> WaitEnd {
        let interval = self.sh.cfg.progress_interval;
        let liveness = Duration::from_millis(200);
        let mut seen: u64 = 0;
        let mut last_sent: Option<Instant> = None;
        // The last frame sent, without its timestamp: a tick that repeats it is skipped.
        let mut last_frame: Option<FrameKey> = None;
        let key = |p: &Progress| (p.eta_s, p.msg.clone(), p.pct, p.stage.clone(), p.state.clone());
        let mut first = true;
        let mut next_alive = Instant::now() + liveness;
        let mut st = self.sh.lock();
        loop {
            if st.shutdown {
                return WaitEnd::Gone;
            }
            let Some(job) = st.jobs.get(&seq) else { return WaitEnd::Gone };
            let reached = job.rec.state.wait_returns();
            let now = Instant::now();
            // Which frames to send now.
            let mut out: Vec<Progress> = Vec::new();
            if first {
                first = false;
                match from {
                    WaitFrom::Now => {
                        seen = job.version;
                        if let Some(p) = job.rec.progress.as_ref().filter(|_| !reached) {
                            out.push(p.clone().with_job(&job.rec.id));
                        }
                    }
                    WaitFrom::Start => seen = 0,
                }
            }
            let mut wake = deadline.unwrap_or(now + Duration::from_secs(3600)).min(next_alive);
            if out.is_empty() && job.version > seen {
                let sig: Vec<Progress> =
                    job.sig.iter().filter(|(v, _)| *v > seen).map(|(_, p)| p.clone().with_job(&job.rec.id)).collect();
                if !sig.is_empty() {
                    out = sig;
                    seen = job.version;
                } else if reached {
                    seen = job.version;
                } else if let Some(p) = &job.rec.progress {
                    let frame = p.clone().with_job(&job.rec.id);
                    match last_sent {
                        _ if last_frame.as_ref() == Some(&key(&frame)) => seen = job.version,
                        Some(t) if now < t + interval => wake = wake.min(t + interval),
                        _ => {
                            out.push(frame);
                            seen = job.version;
                        }
                    }
                } else {
                    seen = job.version;
                }
            }
            if reached {
                // Every stage and state change goes out before the final answer.
                let rec = job.rec.clone();
                drop(st);
                for p in &out {
                    if sink(p).is_err() {
                        return WaitEnd::Gone;
                    }
                }
                return WaitEnd::Reached(rec);
            }
            if deadline.is_some_and(|d| now >= d) {
                return WaitEnd::TimedOut(job.rec.clone());
            }
            if !out.is_empty() {
                drop(st);
                for p in &out {
                    if sink(p).is_err() {
                        return WaitEnd::Gone;
                    }
                }
                last_frame = out.last().map(key);
                last_sent = Some(Instant::now());
                st = self.sh.lock();
                continue;
            }
            if now >= next_alive {
                drop(st);
                if !alive() {
                    return WaitEnd::Gone;
                }
                next_alive = Instant::now() + liveness;
                st = self.sh.lock();
                continue;
            }
            let timeout = wake.saturating_duration_since(now).max(Duration::from_millis(1));
            st = self.sh.cv.wait_timeout(st, timeout).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    /// Wake every waiter and make them return `Gone` (server shutdown).
    pub(crate) fn shutdown(&self) {
        self.sh.lock().shutdown = true;
        self.sh.cv.notify_all();
    }
}

/// The backend's handle on one job. Every update after the job is terminal (or cancelled) is
/// ignored and returns false or `Err(JobGone)`.
#[derive(Clone)]
pub struct JobCtl {
    sh: Arc<Shared>,
    seq: u64,
    id: String,
}

impl std::fmt::Debug for JobCtl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobCtl").field("id", &self.id).finish()
    }
}

impl JobCtl {
    /// `j-NNNNNN` (evolution.md's `gate_id`).
    pub fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn seq(&self) -> u64 {
        self.seq
    }

    /// The current record.
    pub fn record(&self) -> Option<JobRecord> {
        self.sh.lock().jobs.get(&self.seq).map(|j| j.rec.clone())
    }

    /// True once `job.cancel` marked the job cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.sh.lock().jobs.get(&self.seq).is_none_or(|j| j.rec.state == JobState::Cancelled)
    }

    /// True once the job is terminal (or gone).
    pub fn is_finished(&self) -> bool {
        self.sh.lock().jobs.get(&self.seq).is_none_or(|j| j.rec.state.is_terminal())
    }

    fn update<R>(&self, f: impl FnOnce(&mut Job) -> R) -> Result<R, JobGone> {
        let mut st = self.sh.lock();
        if st.shutdown {
            // The server is gone: a stale backend thread must not rewrite records.
            return Err(JobGone::Finished);
        }
        let Some(j) = st.jobs.get_mut(&self.seq) else { return Err(JobGone::Finished) };
        match j.rec.state {
            JobState::Cancelled => return Err(JobGone::Cancelled),
            ref s if s.is_terminal() => return Err(JobGone::Finished),
            _ => {}
        }
        let r = f(j);
        j.bump();
        self.sh.persist(j, false);
        let terminal = j.rec.state.is_terminal();
        if terminal {
            self.sh.prune(&mut st);
        }
        drop(st);
        self.sh.cv.notify_all();
        Ok(r)
    }

    fn set_progress(j: &mut Job, state: JobState, stage: Option<Stage>, pct: u8, eta_s: Option<u64>, msg: &str) {
        let prev = j.rec.progress.as_ref();
        let pct = pct.min(100).max(prev.map_or(0, |p| p.pct));
        let significant = prev.is_none_or(|p| p.stage != stage || p.state != state);
        let p: RecordProgress = Progress::new(&j.rec.id, state, stage, pct, eta_s, msg, &clock::now_rfc3339()).into();
        j.rec.progress = Some(p.clone());
        if significant {
            j.sig.push_back((j.version + 1, p));
            while j.sig.len() > SIG_KEPT {
                j.sig.pop_front();
            }
        }
    }

    /// Report progress (§3.2, §4.3). `state` is the progress state (`running`, `gating`,
    /// `handing_off`, …); the record leaves `queued` for `running` on the first report and
    /// otherwise keeps its milestone state. `pct` never decreases within a job; `msg` is cut to
    /// 200 bytes.
    pub fn progress(&self, state: JobState, stage: Option<Stage>, pct: u8, eta_s: Option<u64>, msg: &str) -> bool {
        self.update(|j| {
            if j.rec.state == JobState::Queued && matches!(state, JobState::Running | JobState::Gating | JobState::HandingOff) {
                j.rec.state = JobState::Running;
            }
            Self::set_progress(j, state, stage, pct, eta_s, msg);
        })
        .is_ok()
    }

    /// Append a stage result (`stages[]`).
    pub fn stage_done(&self, r: StageResult) -> bool {
        self.update(|j| j.rec.stages.push(r)).is_ok()
    }

    /// Record the candidate SHA.
    pub fn set_sha(&self, sha: &str) -> bool {
        self.update(|j| { j.rec.sha = Some(sha.to_string()); j.persisted = None; }).is_ok()
    }

    /// Record the Root's `s-NNNNNN` (`kernel promote`).
    pub fn set_root_job(&self, root_job: &str) -> bool {
        self.update(|j| { j.rec.root_job = Some(root_job.to_string()); j.persisted = None; }).is_ok()
    }

    fn milestone(&self, state: JobState, msg: &str) -> Result<(), JobGone> {
        self.update(|j| {
            j.rec.state = state.clone();
            let (stage, pct) = j.rec.progress.as_ref().map_or((None, 0), |p| (p.stage.clone(), p.pct));
            Self::set_progress(j, state, stage, pct, None, msg);
        })
    }

    /// Block until every connection that will answer for this job (the command's own response,
    /// `wait:true` or not, and every `job.wait` on it) has written its final response, bounded by
    /// `flush_timeout`. Call it from the backend's own thread, never inside the synchronous part
    /// (the command's response is written only after that returns).
    pub fn wait_flushed(&self) {
        let deadline = Instant::now() + self.sh.cfg.flush_timeout;
        let mut st = self.sh.lock();
        loop {
            match st.jobs.get(&self.seq) {
                Some(j) if j.pending_finals > 0 && !st.shutdown => {}
                _ => return,
            }
            let now = Instant::now();
            if now >= deadline {
                return;
            }
            st = self.sh.cv.wait_timeout(st, deadline - now).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    /// Milestone `awaiting_boundary` (§3.6): `wait` returns here. Then blocks in
    /// [`JobCtl::wait_flushed`], so the caller's tool call has its answer before the backend sends
    /// `pause` (§4.1 ordering guarantee). From the backend's own thread only.
    pub fn awaiting_boundary(&self) -> Result<(), JobGone> {
        self.set_awaiting_boundary()?;
        self.wait_flushed();
        Ok(())
    }

    /// Enter `awaiting_boundary` without waiting: for the synchronous part of `reload` and
    /// `rollback`, whose response already says `awaiting_boundary`. Send `pause` only after
    /// [`JobCtl::wait_flushed`] (from a thread).
    pub fn set_awaiting_boundary(&self) -> Result<(), JobGone> {
        self.milestone(JobState::AwaitingBoundary, "awaiting the step boundary")
    }

    /// `awaiting_root` (`kernel promote` after `system.swap`): no longer cancellable; `wait`
    /// keeps waiting for the gate-of-gates verdict.
    pub fn awaiting_root(&self, root_job: Option<&str>) -> Result<(), JobGone> {
        if let Some(r) = root_job {
            self.update(|j| j.rec.root_job = Some(r.to_string()))?;
        }
        self.milestone(JobState::AwaitingRoot, "gate-of-gates running at the Root")
    }

    /// Observe a Root-owned kernel swap without blocking its event reader on client flush.
    pub fn observe_kernel_swap(&self) -> Result<(), JobGone> {
        self.milestone(JobState::AwaitingSwap, "Root kernel swap pending; probation not confirmed")
    }

    /// Milestone `awaiting_swap` (`gog_pass`): `wait` returns here; then blocks in
    /// [`JobCtl::wait_flushed`] like [`JobCtl::awaiting_boundary`].
    pub fn awaiting_swap(&self) -> Result<(), JobGone> {
        self.milestone(JobState::AwaitingSwap, "gate-of-gates passed; swap pending")?;
        self.wait_flushed();
        Ok(())
    }

    /// Terminal success with the command's result object.
    pub fn succeed<T: Serialize>(&self, result: &T) -> bool {
        match init_proto::icj::to_value(result) {
            Ok(v) => self.succeed_value(v),
            Err(e) => self.fail(ErrorBody::new(ErrorCode::Internal, format!("job result is not ICJ: {e}"))),
        }
    }

    /// Terminal success with a `Value` result.
    pub fn succeed_value(&self, result: Value) -> bool {
        self.update(|j| {
            j.rec.state = JobState::Succeeded;
            j.rec.result = Some(result);
            j.rec.finished_at = Some(clock::now_rfc3339());
            let stage = j.rec.progress.as_ref().and_then(|p| p.stage.clone());
            Self::set_progress(j, JobState::Succeeded, stage, 100, Some(0), "done");
        })
        .is_ok()
    }

    /// Terminal failure (`error` has the §3.2 shape: an `E_*` or dotted code).
    pub fn fail(&self, error: ErrorBody) -> bool {
        self.finish_failed(Some(error), None)
    }

    /// Terminal failure with a `reason` (e.g. `kernel_restart`, `pause_timeout`).
    pub fn fail_with_reason(&self, error: Option<ErrorBody>, reason: &str) -> bool {
        self.finish_failed(error, Some(reason))
    }

    fn finish_failed(&self, error: Option<ErrorBody>, reason: Option<&str>) -> bool {
        self.update(|j| {
            j.rec.state = JobState::Failed;
            j.rec.error = error.map(crate::server::sanitize_error);
            if let Some(r) = reason {
                j.rec.reason = Some(r.to_string());
            }
            j.rec.finished_at = Some(clock::now_rfc3339());
            let (stage, pct) = j.rec.progress.as_ref().map_or((None, 0), |p| (p.stage.clone(), p.pct));
            let msg = j.rec.error.as_ref().map_or_else(|| "failed".to_string(), |e| e.to_string());
            Self::set_progress(j, JobState::Failed, stage, pct, None, &msg);
        })
        .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn home(name: &str) -> PathBuf {
        let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/wp7").join(format!("unit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn kernel_swap_restart_recovery_is_scoped_and_not_success() {
        let d = home("kernel-swap-resume");
        let (store, _) = JobStore::open(&d, cfg()).unwrap();
        let job = store.create(&LaneCtx::live(), "kernel.promote", serde_json::json!({})).unwrap();
        job.set_sha(&"a".repeat(40));
        let id = job.id().to_string();
        let other = store.create(&LaneCtx::live(), "promote", serde_json::json!({})).unwrap();
        other.set_sha(&"a".repeat(40));
        let other_id = other.id().to_string();
        drop(job); drop(other); drop(store);
        let (store, _) = JobStore::open(&d, cfg()).unwrap();
        assert!(store.resume_kernel_swap(&id, &"b".repeat(40), Some("s-000001")).is_none());
        assert!(store.resume_kernel_swap(&other_id, &"a".repeat(40), Some("s-000001")).is_none());
        let resumed = store.resume_kernel_swap(&id, &"a".repeat(40), Some("s-000001")).unwrap();
        assert_eq!(resumed.record().unwrap().state, JobState::AwaitingSwap);
        assert!(!resumed.is_finished());
        assert!(store.resume_kernel_swap(&id, &"a".repeat(40), Some("s-000001")).is_none());
        std::fs::remove_dir_all(d).unwrap();
    }

    fn cfg() -> JobConfig {
        JobConfig { flush_timeout: Duration::from_secs(2), progress_interval: Duration::from_millis(100), keep: 200 }
    }

    #[test]
    fn ids_records_and_restart() {
        let h = home("ids");
        let (s, rec) = JobStore::open(&h, cfg()).unwrap();
        assert!(rec.is_empty());
        let a = s.create(&LaneCtx::live(), "promote", serde_json::json!({"ref": "evo/a"})).unwrap();
        let b = s.create(&LaneCtx::live(), "gate", serde_json::json!({})).unwrap();
        assert_eq!((a.id(), b.id()), ("j-000001", "j-000002"));
        assert!(a.progress(JobState::Gating, Some(Stage::G0), 30, Some(10), "clippy"));
        let r = s.record("j-000001").unwrap();
        assert_eq!(r.state, JobState::Running, "record leaves queued on the first progress");
        assert_eq!(r.progress.as_ref().unwrap().state, JobState::Gating);
        // pct never decreases.
        a.progress(JobState::Gating, Some(Stage::G1), 10, None, "x");
        assert_eq!(s.record("j-000001").unwrap().progress.unwrap().pct, 30);
        assert!(b.succeed_value(serde_json::json!({"pass": true})));
        assert!(!b.fail(ErrorBody::new(ErrorCode::Internal, "late")), "terminal jobs ignore updates");
        // A reserved, discarded job consumes its id.
        let c = s.reserve(&LaneCtx::live(), "promote", serde_json::json!({})).unwrap();
        s.discard(c.seq());
        assert!(s.record(c.id()).is_none());
        drop(s);
        // Cold start: j-000001 was running → failed, kernel_restart; ids continue after j-000003.
        let (s2, rec) = JobStore::open(&h, cfg()).unwrap();
        assert_eq!(rec, vec!["j-000001".to_string()]);
        let r = s2.record("j-000001").unwrap();
        assert_eq!((r.state, r.reason.as_deref()), (JobState::Failed, Some("kernel_restart")));
        assert_eq!(s2.record("j-000002").unwrap().state, JobState::Succeeded);
        let d = s2.create(&LaneCtx::live(), "reload", serde_json::json!({})).unwrap();
        assert_eq!(d.id(), "j-000004");
        let disk: JobRecord = init_proto::files::decode(&std::fs::read(h.join("j-000001.json")).unwrap()).unwrap();
        assert_eq!(disk.reason.as_deref(), Some("kernel_restart"));
        std::fs::remove_dir_all(&h).unwrap();
    }

    #[test]
    fn retention_keeps_newest() {
        let h = home("keep");
        let (s, _) = JobStore::open(&h, JobConfig { keep: 5, ..cfg() }).unwrap();
        let active = s.create(&LaneCtx::live(), "promote", Value::Null).unwrap();
        for _ in 0..8 {
            let j = s.create(&LaneCtx::live(), "gate", Value::Null).unwrap();
            j.succeed_value(Value::Null);
        }
        let files: Vec<String> = std::fs::read_dir(&h)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".json"))
            .collect();
        assert_eq!(files.len(), 5, "{files:?}");
        assert!(s.record(active.id()).is_some(), "an active job is never pruned");
        assert!(s.record("j-000002").is_none() && s.record("j-000009").is_some());
        std::fs::remove_dir_all(&h).unwrap();
    }

    #[test]
    fn wait_progress_and_milestone_flush() {
        let h = home("wait");
        let (s, _) = JobStore::open(&h, cfg()).unwrap();
        let j = s.reserve(&LaneCtx::live(), "promote", Value::Null).unwrap();
        let seq = j.seq();
        let guard = s.add_waiter(seq);
        s.publish(seq);
        let flushed = Arc::new(AtomicBool::new(false));
        let (j2, f2) = (j.clone(), flushed.clone());
        let worker = std::thread::spawn(move || {
            j2.progress(JobState::Running, Some(Stage::Build), 10, None, "build");
            for i in 0..30 {
                j2.progress(JobState::Running, Some(Stage::Build), 10 + i, None, "tick"); // ticks: rate-limited
                std::thread::sleep(Duration::from_millis(5));
            }
            j2.progress(JobState::Gating, Some(Stage::G0), 50, None, "G0");
            j2.progress(JobState::Gating, Some(Stage::G1), 60, None, "G1");
            j2.awaiting_boundary().unwrap();
            // The final response must already be written when awaiting_boundary returns.
            assert!(f2.load(Ordering::SeqCst), "pause before the final response");
            j2.succeed_value(Value::Null);
        });
        let mut frames = Vec::new();
        let end = s.wait(seq, WaitFrom::Start, None, &mut |p| {
            frames.push(p.clone());
            Ok(())
        }, &mut || true);
        let rec = match end {
            WaitEnd::Reached(r) => r,
            other => panic!("{other:?}"),
        };
        assert_eq!(rec.state, JobState::AwaitingBoundary);
        // "write the final response", then release.
        flushed.store(true, Ordering::SeqCst);
        drop(guard);
        worker.join().unwrap();
        let stages: Vec<_> = frames.iter().map(|p| p.stage.clone()).collect();
        assert!(stages.contains(&Some(Stage::G0)) && stages.contains(&Some(Stage::G1)), "every stage change is sent: {stages:?}");
        assert!(frames.len() < 12, "ticks are rate-limited: {}", frames.len());
        let pcts: Vec<u8> = frames.iter().map(|p| p.pct).collect();
        assert!(pcts.windows(2).all(|w| w[0] <= w[1]), "{pcts:?}");
        assert_eq!(s.record(j.id()).unwrap().state, JobState::Succeeded);
        std::fs::remove_dir_all(&h).unwrap();
    }

    #[test]
    fn cancel_rules() {
        let h = home("cancel");
        let (s, _) = JobStore::open(&h, cfg()).unwrap();
        let j = s.create(&LaneCtx::live(), "gate", Value::Null).unwrap();
        j.progress(JobState::Running, Some(Stage::Build), 5, None, "b");
        let r = s.cancel(j.seq()).unwrap();
        assert_eq!(r.state, JobState::Cancelled);
        assert!(j.is_cancelled());
        assert_eq!(j.awaiting_boundary(), Err(JobGone::Cancelled));
        let k = s.create(&LaneCtx::live(), "promote", Value::Null).unwrap();
        k.awaiting_boundary().unwrap();
        assert_eq!(s.cancel(k.seq()).unwrap_err().code, ErrorCode::NotCancellable);
        std::fs::remove_dir_all(&h).unwrap();
    }

    #[test]
    fn wait_times_out_and_detects_hangup() {
        let h = home("timeout");
        let (s, _) = JobStore::open(&h, cfg()).unwrap();
        let j = s.create(&LaneCtx::live(), "gate", Value::Null).unwrap();
        let t = Instant::now();
        match s.wait(j.seq(), WaitFrom::Now, Some(Instant::now() + Duration::from_millis(150)), &mut |_| Ok(()), &mut || true) {
            WaitEnd::TimedOut(r) => assert_eq!(r.state, JobState::Queued),
            other => panic!("{other:?}"),
        }
        assert!(t.elapsed() >= Duration::from_millis(150));
        assert!(matches!(s.wait(j.seq(), WaitFrom::Now, None, &mut |_| Ok(()), &mut || false), WaitEnd::Gone));
        std::fs::remove_dir_all(&h).unwrap();
    }
}
