//! The agent loop (body/loop.md): take owner input, ask the model for the next step, run its tool
//! calls in parallel with results in call order, feed the results back; stop when the model stops
//! calling tools and nothing is queued. Everything durable goes through log-context's boundary.
//!
//! Threads (§3.1): the main thread runs `Session::serve`; the input and output threads are in
//! `frames.rs`; the heartbeat is in `status.rs`; tool workers live for one batch
//! (`std::thread::scope`, at most 16), each call wrapped in `catch_unwind`.

pub mod delegate;
pub mod frames;
pub mod hooks;
pub mod status;

#[cfg(test)]
mod tests;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

pub use frames::{Control, InputFrame, InputMode, Notice, Out, OutTx, PauseReq, ReloadJob};
#[allow(unused_imports)]
pub use hooks::{BoundaryInput, ContextHooks, LogContext, OwnerInput, Timing};
pub use status::{Phase, Status};

use crate::context::head::Head;
use crate::context::{BoundaryOut, CallStart, ContextError, ReloadIn, RequestOut};
use crate::log::event::ErrorEv;
use crate::model::{
    Abort, AssistantMessage, Effort, ModelClient, ModelInfo, ProviderError, Purpose, Request,
    StopReason, StreamEvent, ToolCall, ToolResult, Usage,
};
use crate::tools::{
    ExtHostApi, Job, Mode, PathCx, Prepared, Ran, ReloadReport, Snapshot, ToolCx, Tools,
    error_result,
};

/// Workers per batch (§3.7).
pub const MAX_PARALLEL: usize = 16;
/// Reload waits: at a requested boundary, and before a batch with an `ext` call (host.md §4.1).
pub const RELOAD_WAIT: Duration = Duration::from_secs(10);
pub const RELOAD_WAIT_EXT: Duration = Duration::from_secs(60);
/// While idle with a reload job waiting for its T1 report, the report is polled this often.
pub const IDLE_RELOAD_POLL: Duration = Duration::from_millis(100);
/// Consecutive length stops before the guard ends the run (§3.6).
pub const LENGTH_STRIKES: u32 = 3;

pub const ABORTED_TEXT: &str = "error: aborted: not executed (owner abort)";
/// Interrupt-and-steer continues the run (§3.5).
pub const CONTINUE_TEXT: &str = "interrupted · continuing with your note";
/// An owner interrupt pauses the task (§3.5.1); a stop, EOF or fault abort still says "aborted".
pub const PAUSED_TEXT: &str = "paused · type to continue, or say what to do instead";
/// Appended after an owner interrupt so the next run knows the task is paused, not done.
pub const PAUSED_NOTE: &str = "[init:paused] The owner interrupted this turn (Esc / Ctrl-C). It is a pause, not a cancel: finished work stands, and calls marked \"aborted: not executed\" did not run. On the owner's next message, resume the task where it stopped (re-issue only the calls that did not run, redo nothing that finished) unless that message changes or drops the task; when resuming, say so in one line.";

/// How a run ended. `Exit(code)`: the process must exit now (handoff 75, stop 0, fatal 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunEnd {
    Idle,
    Aborted,
    Refused,
    Failed,
    Stopping,
    Exit(u8),
}

/// No-progress guards (§3.10); not turn caps.
#[derive(Default)]
struct Guards {
    length_strikes: u32,
    overflow_retried: bool,
    next_max: Option<u32>,
    /// The last boundary appended a steer (interrupt-and-steer may answer it, §3.5).
    last_steer: bool,
}

impl Guards {
    fn take_max(&mut self, info: &ModelInfo) -> u32 {
        crate::provider::models::request_tokens(info.provider, info.id, self.next_max.take().unwrap_or(info.default_max_tokens))
    }
}

/// Inputs queued while a run is in progress (§3.4).
#[derive(Default)]
struct Queue {
    steers: Vec<InputFrame>,
    follow_ups: VecDeque<InputFrame>,
    commands: Vec<InputFrame>,
    notices: Vec<Notice>,
}

enum Class {
    Duplicate,
    /// The owner retracted it before it was consumed (§3.4): dropped silently.
    Retracted,
    Command,
    Plain,
    FollowUp,
}

enum PauseEnd {
    Resumed,
    Exit(u8),
}

enum TurnOutcome {
    Done(AssistantMessage, Timing),
    Failed(ProviderError, u32),
}

/// A T0 reload committed while its T1 part is still loading: `reload_done` waits for it.
struct WaitingReload {
    job: String,
    t0_ok: bool,
    t0_error: Option<String>,
    prefix_changed: bool,
}

/// Fault hook (testing.md `INIT_FAULT`): returns true to crash at the named point. Boot wires it
/// to `process::abort()` under `INIT_TEST=1`; tests make the loop return `Exit(FAULT_EXIT)`.
pub type FaultHook = Box<dyn Fn(&str) -> bool>;
pub const FAULT_EXIT: u8 = 134;

/// Rebuilds the T0 head from `body/` (log-context §3.8; awareness.md). Supplied by boot (WP13).
pub type HeadSource = Box<dyn FnMut() -> Result<Head, String>>;

/// Called with `(session, task)` whenever the loop learns the current task: after a boundary that
/// appended owner input, and with every rendered request. Boot wires it to the provider's
/// summarizer context, so compaction requests carry the run's `x-init-task` (loop.md §3.3).
pub type TaskHook = Box<dyn FnMut(&str, Option<&str>)>;

pub struct SessionConfig {
    pub ctx: Box<dyn ContextHooks>,
    pub model: Arc<dyn ModelClient>,
    pub tools: Tools,
    pub abort: Arc<Abort>,
    pub status: Arc<Status>,
    pub ctl: Receiver<Control>,
    pub out: OutTx,
    pub mode: Mode,
    /// `boot.json` effort, or the model's default.
    pub effort: Effort,
    /// The body cwd (`$INIT_HOME/work`), captured once at boot.
    pub cwd: PathBuf,
    /// `$INIT_HOME`.
    pub home: PathBuf,
    /// `$HOME`, for `~/` in tool paths.
    pub user_home: Option<PathBuf>,
    pub head_source: Option<HeadSource>,
    pub fault: Option<FaultHook>,
    pub on_task: Option<TaskHook>,
}

pub struct Session {
    ctx: Box<dyn ContextHooks>,
    model: Arc<dyn ModelClient>,
    tools: Tools,
    ext: Option<Arc<dyn ExtHostApi>>,
    abort: Arc<Abort>,
    status: Arc<Status>,
    ctl: Receiver<Control>,
    out: OutTx,
    mode: Mode,
    effort: Effort,
    cwd: PathBuf,
    home: PathBuf,
    user_home: Option<PathBuf>,
    head_source: Option<HeadSource>,
    fault: Option<FaultHook>,
    on_task: Option<TaskHook>,
    queue: Queue,
    pending_pause: Option<PauseReq>,
    pending_reload: Vec<ReloadJob>,
    waiting_reload: Vec<WaitingReload>,
    late_report: Option<ReloadReport>,
    pending_errors: Vec<ErrorEv>,
    stopping: bool,
}

fn loop_error(code: &str, message: impl Into<String>, data: Option<Value>) -> ErrorEv {
    ErrorEv {
        where_: "loop".into(),
        code: code.into(),
        message: message.into(),
        data,
    }
}

fn provider_error(code: &str, message: impl Into<String>, data: Option<Value>) -> ErrorEv {
    ErrorEv {
        where_: "provider".into(),
        code: code.into(),
        message: message.into(),
        data,
    }
}

fn aborted_in_retry() -> ErrorEv {
    provider_error("aborted_in_retry", "aborted while waiting to retry", None)
}

fn owner_input(f: &InputFrame) -> OwnerInput {
    OwnerInput {
        id: f.id.clone(),
        seq: f.seq,
        text: f.text.clone(),
    }
}

/// `/compact`, `/new`, `/branch`, `/label` (log-context §4.5), as a whole word.
pub fn is_command(text: &str) -> bool {
    let t = text.trim_start();
    ["/compact", "/new", "/branch", "/label"].iter().any(|c| {
        t.strip_prefix(c)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
    })
}

pub fn cut_off_note(max_tokens: u32) -> String {
    format!(
        "[init:cutoff] Your previous reply was cut off at the output limit (max_tokens={max_tokens}). Continue from where it stopped."
    )
}

pub fn length_cutoff_text(max_tokens: u32) -> String {
    format!(
        "error: length_cutoff: not executed. Your reply reached the output limit (max_tokens={max_tokens}) before it ended, so this call may be incomplete. Re-issue it; split large content (write a file in parts, or use edit)."
    )
}

pub fn refused_text(category: Option<&str>) -> String {
    format!(
        "error: refused: not executed (the provider stopped the reply: {})",
        category.unwrap_or("unspecified")
    )
}

fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic payload".into())
}

impl Session {
    pub fn new(c: SessionConfig) -> Session {
        let ext = c.tools.ext().cloned();
        Session {
            ctx: c.ctx,
            model: c.model,
            tools: c.tools,
            ext,
            abort: c.abort,
            status: c.status,
            ctl: c.ctl,
            out: c.out,
            mode: c.mode,
            effort: c.effort,
            cwd: c.cwd,
            home: c.home,
            user_home: c.user_home,
            head_source: c.head_source,
            fault: c.fault,
            on_task: c.on_task,
            queue: Queue::default(),
            pending_pause: None,
            pending_reload: Vec::new(),
            waiting_reload: Vec::new(),
            late_report: None,
            pending_errors: Vec::new(),
            stopping: false,
        }
    }

    fn fault(&self, point: &str) -> bool {
        self.fault.as_ref().is_some_and(|f| f(point))
    }

    fn set_phase(&self, p: Phase) {
        let old = Phase::from_u8(self.status.phase.swap(p as u8, Ordering::SeqCst));
        if old.busy() != p.busy() {
            self.out.send(Out::State(p.busy()));
        }
    }

    fn info(&self) -> &ModelInfo {
        self.model.info()
    }

    fn exit(&mut self, code: u8) -> u8 {
        self.set_phase(Phase::Stopping);
        if let Some(x) = &self.ext {
            x.shutdown();
        }
        code
    }

    fn fatal(&self, what: &str, e: &std::io::Error) -> RunEnd {
        eprintln!(
            "{}",
            json!({"type": "fatal", "step": what, "message": e.to_string()})
        );
        RunEnd::Exit(1)
    }

    fn protocol_warning(&self, c: &Control) {
        eprintln!(
            "{}",
            json!({"type": "warning", "step": "protocol", "message": format!("unexpected frame while {}: {c:?}", self.status.phase().as_str())})
        );
    }

    // -- serve ----------------------------------------------------------------------------------

    /// Called by boot after `ready`. Returns the process exit code (0 stop/EOF, 75 handoff,
    /// 1 fatal log error); the caller flushes `out` and exits with it.
    pub fn serve(mut self) -> u8 {
        self.status.set_leaf(self.ctx.leaf());
        if self.mode == Mode::Normal && self.ctx.has_pending_work() {
            // Continue after a handoff, crash or promote (spec §6.2 step 4).
            if let RunEnd::Exit(c) = self.run(BoundaryInput::default()) {
                return self.exit(c);
            }
        }
        loop {
            if self.stopping {
                return self.exit(0);
            }
            if let Some(p) = self.pending_pause.take() {
                match self.pause(p) {
                    PauseEnd::Resumed => continue,
                    PauseEnd::Exit(c) => return self.exit(c),
                }
            }
            if let Some(code) = self.idle_leftovers() {
                return self.exit(code);
            }
            if let Some(f) = self.queue.follow_ups.pop_front() {
                let first = BoundaryInput {
                    owner: vec![owner_input(&f)],
                    ..Default::default()
                };
                if let RunEnd::Exit(c) = self.run(first) {
                    return self.exit(c);
                }
                continue;
            }
            self.set_phase(Phase::Idle);
            let c = if self.waiting_reload.is_empty() {
                match self.ctl.recv() {
                    Ok(c) => c,
                    Err(_) => return self.exit(0), // the input thread is gone
                }
            } else {
                // A T1 load is still compiling: poll for its report while idle (host.md §3.9).
                match self.ctl.recv_timeout(IDLE_RELOAD_POLL) {
                    Ok(c) => c,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return self.exit(0),
                }
            };
            let end = match c {
                Control::Input(i) => match self.classify(&i) {
                    Class::Duplicate => {
                        self.out.send(Out::Ack(i.seq));
                        None
                    }
                    Class::Retracted => None,
                    Class::Command => self.idle_boundary(BoundaryInput {
                        commands: vec![owner_input(&i)],
                        ..Default::default()
                    }),
                    Class::Plain | Class::FollowUp => {
                        match self.run(BoundaryInput {
                            owner: vec![owner_input(&i)],
                            ..Default::default()
                        }) {
                            RunEnd::Exit(c) => Some(c),
                            _ => None,
                        }
                    }
                },
                Control::Notice(n) => self.idle_boundary(BoundaryInput {
                    notices: vec![n],
                    ..Default::default()
                }),
                Control::Reload(job) => {
                    self.pending_reload.push(job);
                    None // committed by idle_leftovers at the top of the loop
                }
                Control::Pause(p) => {
                    self.pending_pause = Some(p);
                    None
                }
                Control::Interrupt { .. } => None, // nothing running; the next run resets it
                Control::Stop { .. } | Control::Eof => return self.exit(0),
                Control::Retract { .. } => None, // answered by the input thread
                other @ (Control::Resume { .. } | Control::Exit { .. }) => {
                    self.protocol_warning(&other);
                    None
                }
            };
            if let Some(code) = end {
                return self.exit(code);
            }
        }
    }

    /// Work left by a run or arriving while idle: reloads, queued commands, notices and steers.
    fn idle_leftovers(&mut self) -> Option<u8> {
        let mut input = BoundaryInput::default();
        if !self.pending_reload.is_empty() {
            let jobs = std::mem::take(&mut self.pending_reload);
            input.reload = self.commit_reload(jobs);
        } else {
            input.reload = self.late_reload();
        }
        input.commands = self
            .queue
            .commands
            .drain(..)
            .map(|f| owner_input(&f))
            .collect();
        input.notices = std::mem::take(&mut self.queue.notices);
        input.errors = std::mem::take(&mut self.pending_errors);
        let steers: Vec<InputFrame> = self.queue.steers.drain(..).collect();
        if !input.is_empty()
            && let Some(c) = self.idle_boundary(input)
        {
            return Some(c);
        }
        // A steer left over at idle (a run ended between its arrival and the boundary) starts a run.
        if !steers.is_empty() {
            let first = BoundaryInput {
                owner: steers.iter().map(owner_input).collect(),
                ..Default::default()
            };
            if let RunEnd::Exit(c) = self.run(first) {
                return Some(c);
            }
        }
        None
    }

    /// A boundary with steps 1–4 only, then sync and acks (idle notices, commands, reloads).
    fn idle_boundary(&mut self, input: BoundaryInput) -> Option<u8> {
        match self.append(input) {
            Ok(_) => None,
            Err(RunEnd::Exit(c)) => Some(c),
            Err(_) => None,
        }
    }

    fn classify(&self, i: &InputFrame) -> Class {
        if self.ctx.has_input(&i.id) {
            Class::Duplicate
        } else if self.status.gate.is_retracted(i.seq) {
            Class::Retracted
        } else if is_command(&i.text) {
            Class::Command
        } else if i.mode == InputMode::FollowUp {
            Class::FollowUp
        } else {
            Class::Plain
        }
    }

    /// A control frame that arrives while a run is in progress (or while paused).
    fn enqueue(&mut self, c: Control) {
        match c {
            Control::Input(i) => match self.classify(&i) {
                Class::Duplicate => self.out.send(Out::Ack(i.seq)),
                Class::Retracted => {}
                Class::Command => self.queue.commands.push(i),
                Class::FollowUp => self.queue.follow_ups.push_back(i),
                Class::Plain => self.queue.steers.push(i),
            },
            Control::Interrupt { .. } => {} // the input thread already triggered the abort
            Control::Retract { .. } => {}   // answered by the input thread
            Control::Pause(p) => self.pending_pause = Some(p),
            Control::Stop { .. } | Control::Eof => self.stopping = true,
            Control::Reload(job) => self.pending_reload.push(job),
            Control::Notice(n) => self.queue.notices.push(n),
            other @ (Control::Resume { .. } | Control::Exit { .. }) => {
                self.protocol_warning(&other)
            }
        }
    }

    fn drain_ctl(&mut self) {
        while let Ok(c) = self.ctl.try_recv() {
            self.enqueue(c);
        }
    }

    // -- the boundary ---------------------------------------------------------------------------

    /// The retract gate (§3.4), right before an append: drops retracted owner inputs, steers and
    /// commands, and marks the rest consumed, under the lock the input thread answers `retract`
    /// with. Returns true when it dropped something.
    fn admit(&self, input: &mut BoundaryInput) -> bool {
        let mut g = self.status.gate.lock();
        let n = input.owner.len() + input.steers.len() + input.commands.len();
        for v in [&mut input.owner, &mut input.steers, &mut input.commands] {
            v.retain(|o| !g.retracted.contains(&o.seq));
        }
        for o in input.owner.iter().chain(&input.steers).chain(&input.commands) {
            g.consume(o.seq);
        }
        n != input.owner.len() + input.steers.len() + input.commands.len()
    }

    /// The single choke point before `ctx.boundary_append`: the gate, then the append.
    fn append(&mut self, mut input: BoundaryInput) -> Result<BoundaryOut, RunEnd> {
        let dropped = self.admit(&mut input);
        self.append_admitted(input, dropped)
    }

    /// `ctx.boundary_append`, then acks, notice acks, command outcomes and reload replies.
    fn append_admitted(
        &mut self,
        input: BoundaryInput,
        dropped: bool,
    ) -> Result<BoundaryOut, RunEnd> {
        if dropped && input.is_empty() {
            return Ok(BoundaryOut::default()); // everything in it was retracted
        }
        let seqs: Vec<u64> = input
            .owner
            .iter()
            .chain(&input.steers)
            .chain(&input.commands)
            .map(|o| o.seq)
            .collect();
        let notice_ids: Vec<String> = input.notices.iter().map(|n| n.id.clone()).collect();
        let new_task = input.owner.last().map(|o| o.id.clone());
        let reload_tiers: Vec<(String, bool, Option<String>)> = input
            .reload
            .iter()
            .map(|r| (r.tier.clone(), r.ok, r.error.clone()))
            .collect();
        let reload_components: Vec<String> = input
            .reload
            .iter()
            .filter(|r| r.tier == "T1")
            .flat_map(|r| r.components.clone())
            .collect();
        let out = match self.ctx.boundary_append(input) {
            Ok(o) => o,
            Err(e) => return Err(self.fatal("log", &e)),
        };
        self.status.progress();
        self.status.set_leaf(self.ctx.leaf());
        if let (Some(t), Some(f)) = (new_task, self.on_task.as_mut()) {
            f(&self.ctx.session_id(), Some(&t));
        }
        for s in seqs {
            self.out.send(Out::Ack(s));
        }
        for id in notice_ids {
            self.out.send(Out::NoticeAck(id));
        }
        for (_, outcome) in &out.outcomes {
            self.out.send(Out::display("info", outcome.clone()));
        }
        if out.new_session.is_some() {
            if let Some(x) = &self.ext {
                x.reset_trials(); // host.md §3.9 item 5
            }
            if let Some(f) = self.on_task.as_mut() {
                f(&self.ctx.session_id(), None); // a fresh session has no owner message yet
            }
        }
        if !reload_tiers.is_empty() {
            self.reload_replies(&reload_tiers, &out.reload_prefix_changed, reload_components);
        }
        Ok(out)
    }

    /// `reload_done` for the jobs this boundary completed (T1 present, or no extension host).
    fn reload_replies(
        &mut self,
        tiers: &[(String, bool, Option<String>)],
        changed: &[bool],
        components: Vec<String>,
    ) {
        let t1 = tiers.iter().position(|t| t.0 == "T1");
        let t0 = tiers.iter().position(|t| t.0 == "T0");
        let t0_changed = t0
            .map(|i| changed.get(i).copied().unwrap_or(false))
            .unwrap_or(false);
        let mut done = Vec::new();
        for w in self.waiting_reload.iter_mut() {
            if t0.is_some() && w.t0_ok && w.t0_error.is_none() {
                w.prefix_changed |= t0_changed;
            }
        }
        if t1.is_some() || self.ext.is_none() {
            done.append(&mut self.waiting_reload);
        }
        let (t1_ok, t1_err) = t1
            .map(|i| (tiers[i].1, tiers[i].2.clone()))
            .unwrap_or((true, None));
        for w in done {
            self.out.send(Out::ReloadDone {
                job: w.job,
                ok: w.t0_ok && t1_ok,
                prefix_changed: w.prefix_changed,
                components: components.clone(),
                error: w.t0_error.or(t1_err.clone()),
            });
        }
    }

    /// loop.md §3.8.2: T0 head rebuild now; T1 from the host (waits up to 10 s, else later).
    fn commit_reload(&mut self, jobs: Vec<ReloadJob>) -> Vec<ReloadIn> {
        if self.mode == Mode::Safe {
            for j in jobs {
                self.out.send(Out::ReloadDone {
                    job: j.job,
                    ok: false,
                    prefix_changed: false,
                    components: Vec::new(),
                    error: Some("safe mode".into()),
                });
            }
            return Vec::new();
        }
        let mut tiers = Vec::new();
        let (t0_ok, t0_error) = match self.head_source.as_mut().map(|f| f()) {
            Some(Ok(head)) => {
                tiers.push(ReloadIn {
                    tier: "T0".into(),
                    ok: true,
                    components: Vec::new(),
                    head: Some(head),
                    error: None,
                    text: "[init:reload] T0: the head was rebuilt from body/ (GENESIS.md, SELF.md, skills).".into(),
                });
                (true, None)
            }
            Some(Err(e)) => {
                tiers.push(ReloadIn {
                    tier: "T0".into(),
                    ok: false,
                    components: Vec::new(),
                    head: None,
                    error: Some(e.clone()),
                    text: format!("[init:reload] T0 failed; the previous head stays: {e}"),
                });
                (false, Some(e))
            }
            None => (true, None),
        };
        for j in jobs {
            self.waiting_reload.push(WaitingReload {
                job: j.job,
                t0_ok,
                t0_error: t0_error.clone(),
                prefix_changed: false,
            });
        }
        let report = self
            .late_report
            .take()
            .or_else(|| self.ext.as_ref().and_then(|x| x.commit_reload(RELOAD_WAIT)));
        if let Some(r) = report {
            tiers.push(t1_tier(r));
        }
        tiers
    }

    /// A load that finished after its boundary is picked up for free (host.md §3.9).
    fn late_reload(&mut self) -> Vec<ReloadIn> {
        let report = self.late_report.take().or_else(|| {
            self.ext
                .as_ref()
                .and_then(|x| x.commit_reload(Duration::ZERO))
        });
        report.map(t1_tier).into_iter().collect()
    }

    /// The turn boundary (§3.3): steps 1–4, the pause check, then steps 5–8.
    fn boundary(
        &mut self,
        input: &mut BoundaryInput,
        g: &mut Guards,
    ) -> Result<RequestWithMax, RunEnd> {
        self.drain_ctl();
        input
            .steers
            .extend(self.queue.steers.drain(..).map(|f| owner_input(&f)));
        input
            .commands
            .extend(self.queue.commands.drain(..).map(|f| owner_input(&f)));
        input.notices.append(&mut self.queue.notices);
        input.errors.append(&mut self.pending_errors);
        if !self.pending_reload.is_empty() {
            let jobs = std::mem::take(&mut self.pending_reload);
            input.reload = self.commit_reload(jobs);
        } else {
            input.reload = self.late_reload();
        }
        if self.pending_pause.is_some() {
            // At a pause, queued follow-ups are appended before the ack too (§3.4).
            input
                .owner
                .extend(self.queue.follow_ups.drain(..).map(|f| owner_input(&f)));
        }
        let had_owner_text = !input.owner.is_empty() || !input.steers.is_empty();
        let only_owner_text = input.results.is_empty() && input.harness_text.is_none();
        let dropped = self.admit(input);
        let has_owner_text = !input.owner.is_empty() || !input.steers.is_empty();
        let steer_now = !input.steers.is_empty();
        let answerable = steer_now || g.last_steer;
        g.last_steer = steer_now;
        self.append_admitted(std::mem::take(input), dropped)?;
        if self.abort.is_set() {
            if self.abort.wants_continue() && answerable {
                // Interrupt-and-steer (§3.5): the aborted results and the note are in the log.
                self.abort.reset();
                self.out.send(Out::display("info", CONTINUE_TEXT));
            } else {
                return Err(self.end_aborted_at_boundary());
            }
        }
        if let Some(p) = self.pending_pause.take()
            && let PauseEnd::Exit(c) = self.pause(p)
        {
            return Err(RunEnd::Exit(c));
        }
        if self.stopping {
            return Err(RunEnd::Stopping);
        }
        if had_owner_text && !has_owner_text && only_owner_text {
            // Everything new was retracted (§3.4): no request with nothing to answer.
            return Err(self.end_idle());
        }
        let max = g.take_max(self.info());
        let info = self.model.info().clone();
        match self.ctx.next_request(&info, max) {
            Ok(r) => {
                self.status.set_leaf(self.ctx.leaf());
                if let Some(f) = self.on_task.as_mut() {
                    f(&self.ctx.session_id(), r.task_id.as_deref());
                }
                Ok(RequestWithMax {
                    out: r,
                    max_tokens_used: max,
                })
            }
            Err(ContextError::Io(e)) => Err(self.fatal("log", &e)),
            Err(ContextError::Overflow(m)) => {
                // log-context already appended error{context_overflow} and told the owner.
                self.out
                    .send(Out::display("error", format!("context overflow: {m}")));
                Err(RunEnd::Failed)
            }
            Err(ContextError::Render(m)) => {
                self.out
                    .send(Out::display("error", format!("request render failed: {m}")));
                let e = ErrorEv {
                    where_: "context".into(),
                    code: "render".into(),
                    message: m,
                    data: None,
                };
                Err(self.end_with(
                    BoundaryInput {
                        errors: vec![e],
                        ..Default::default()
                    },
                    RunEnd::Failed,
                ))
            }
        }
    }

    // -- the run ----------------------------------------------------------------------------------

    fn run(&mut self, first: BoundaryInput) -> RunEnd {
        self.abort.reset();
        self.status.run.fetch_add(1, Ordering::Relaxed);
        self.set_phase(Phase::Model);
        let mut g = Guards::default();
        let mut input = first;
        loop {
            let req = match self.boundary(&mut input, &mut g) {
                Ok(r) => r,
                Err(end) => return end,
            };
            self.set_phase(Phase::Model);
            let (msg, timing) = match self.model_turn(&req) {
                TurnOutcome::Done(m, t) => (m, t),
                TurnOutcome::Failed(e, _) if e.is_context_overflow() && !g.overflow_retried => {
                    g.overflow_retried = true;
                    self.ctx.request_emergency_compaction(); // log-context §3.12.6
                    continue;
                }
                TurnOutcome::Failed(ProviderError::Aborted, _) if self.steer_to_answer(&g) => {
                    input = BoundaryInput {
                        errors: vec![aborted_in_retry()],
                        ..Default::default()
                    };
                    continue;
                }
                TurnOutcome::Failed(e, attempts) => return self.end_provider_failure(e, attempts),
            };
            g.overflow_retried = false;
            let max_tokens = req.max_tokens_used;
            let calls: Vec<ToolCall> = msg.tool_calls().cloned().collect();
            let pcx = PathCx {
                cwd: &self.cwd,
                home: self.user_home.as_deref(),
                init_home: Some(&self.home),
            };
            let prepared: Vec<Prepared> = calls
                .iter()
                .enumerate()
                .map(|(i, c)| self.tools.prepare(i, c, &pcx))
                .collect();
            let normal = !matches!(
                msg.stop,
                StopReason::Aborted
                    | StopReason::Refusal { .. }
                    | StopReason::MaxTokens
                    | StopReason::ContextWindow
            );
            // Markers only for calls that will execute (validation failures never do).
            let started: Vec<CallStart> = if normal {
                prepared
                    .iter()
                    .filter(|p| p.is_ready())
                    .map(|p| CallStart {
                        call_id: p.call_id().to_string(),
                        name: p.name().to_string(),
                        idempotency_key: None,
                    })
                    .collect()
            } else {
                Vec::new()
            };
            if let Err(e) = self.ctx.on_response(&msg, &started, timing) {
                return self.fatal("log", &e); // D1 failed: fsync errors are fatal
            }
            self.status.progress();
            self.status.set_leaf(self.ctx.leaf());
            if self.fault("loop.after_on_response") {
                return RunEnd::Exit(FAULT_EXIT);
            }
            self.turn_end(&msg.usage);
            input = BoundaryInput::default();
            match &msg.stop {
                StopReason::Aborted => {
                    input.results = self.fixed_results(&prepared, |_| ABORTED_TEXT.to_string());
                    if self.steer_to_answer(&g) {
                        continue; // the next boundary appends the results and the note (§3.5)
                    }
                    return self.end_aborted(input);
                }
                StopReason::Refusal { category } => {
                    let cat = category.clone();
                    input.results = self.fixed_results(&prepared, |_| refused_text(cat.as_deref()));
                    let c = cat.as_deref().unwrap_or("unspecified");
                    input.errors.push(provider_error(
                        "refusal",
                        format!("the provider stopped the reply (category: {c})"),
                        cat.as_ref().map(|c| json!({"category": c})),
                    ));
                    self.out.send(Out::display(
                        "error",
                        format!("The provider refused to continue (category: {c})."),
                    ));
                    return self.end_with(input, RunEnd::Refused);
                }
                StopReason::MaxTokens | StopReason::ContextWindow => {
                    g.length_strikes += 1;
                    if calls.is_empty() {
                        input.harness_text = Some(cut_off_note(max_tokens));
                    } else {
                        input.results =
                            self.fixed_results(&prepared, |_| length_cutoff_text(max_tokens));
                    }
                    if g.length_strikes >= LENGTH_STRIKES {
                        input.errors.push(loop_error(
                            "length_guard",
                            format!("{LENGTH_STRIKES} consecutive replies hit the output limit; the run was stopped"),
                            Some(json!({"max_tokens": max_tokens})),
                        ));
                        self.out.send(Out::display(
                            "error",
                            "The model hit the output limit 3 times in a row, so the run was stopped. Ask for the work in smaller parts.",
                        ));
                        return self.end_with(input, RunEnd::Failed);
                    }
                    // The next request only: max_tokens × 2, capped at max_output.
                    g.next_max = Some(max_tokens.saturating_mul(2).min(self.info().max_output));
                    continue;
                }
                StopReason::PauseTurn => continue, // re-send (server tools only; unused at seed)
                _ => g.length_strikes = 0,
            }
            if prepared.is_empty() {
                self.drain_ctl();
                if !self.queue.steers.is_empty() {
                    continue; // the next boundary appends them
                }
                if let Some(f) = self.queue.follow_ups.pop_front() {
                    input.owner.push(owner_input(&f));
                    continue;
                }
                return self.end_idle();
            }
            self.set_phase(Phase::Tools);
            // ask_owner (tools/ask.rs): its question frame, sent once the call has run.
            let asks: Vec<(String, Value)> = prepared
                .iter()
                .filter_map(|p| match p {
                    Prepared::Ready { job: Job::Ask(a), call_id, .. } => {
                        Some((call_id.clone(), crate::tools::ask::frame(a)))
                    }
                    _ => None,
                })
                .collect();
            match self.execute_batch(prepared, req.out.task_id.as_deref()) {
                Ok(results) => input.results = results,
                Err(end) => return end,
            }
            if self.send_asks(&asks, &input.results) {
                // The turn ends: the owner's answer is their next message. No timer, no default.
                self.drain_ctl();
                if self.queue.steers.is_empty() && self.queue.follow_ups.is_empty() {
                    return self.end_with(input, RunEnd::Idle);
                }
            }
            self.status.turn.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Sends the question frame of each `ask_owner` call whose result is not an error. True when
    /// at least one was sent (the run then ends, unless the owner already said something).
    fn send_asks(&self, asks: &[(String, Value)], results: &[(String, ToolResult)]) -> bool {
        let mut sent = false;
        for (id, frame) in asks {
            if results.iter().any(|(_, r)| r.call_id == *id && !r.is_error) {
                self.out.send(Out::Ask(frame.clone()));
                sent = true;
            }
        }
        sent
    }

    /// Interrupt-and-steer (§3.5): the abort asked to continue, and there is a note to answer
    /// (queued now, or appended by the last boundary). The next boundary still re-checks it.
    fn steer_to_answer(&mut self, g: &Guards) -> bool {
        if !self.abort.wants_continue() {
            return false;
        }
        self.drain_ctl();
        g.last_steer || !self.queue.steers.is_empty()
    }

    /// The same text for every call of the reply (aborted, refused, length cut-off).
    fn fixed_results(
        &self,
        prepared: &[Prepared],
        text: impl Fn(&Prepared) -> String,
    ) -> Vec<(String, ToolResult)> {
        prepared
            .iter()
            .map(|p| (p.name().to_string(), error_result(p.call_id(), &text(p))))
            .collect()
    }

    fn turn_end(&self, u: &Usage) {
        let dropped = self.out.take_dropped();
        let mut text = format!(
            "tokens: in {} (cached {}), out {}",
            u.input, u.cache_read, u.output
        );
        if dropped > 0 {
            text.push_str(&format!("; {dropped} display deltas dropped"));
        }
        self.out.send(Out::display("turn_end", text));
    }

    /// The run ends normally: flush anything queued, go idle.
    fn end_idle(&mut self) -> RunEnd {
        let mut input = BoundaryInput {
            commands: self
                .queue
                .commands
                .drain(..)
                .map(|f| owner_input(&f))
                .collect(),
            notices: std::mem::take(&mut self.queue.notices),
            errors: std::mem::take(&mut self.pending_errors),
            ..Default::default()
        };
        if !self.pending_reload.is_empty() {
            let jobs = std::mem::take(&mut self.pending_reload);
            input.reload = self.commit_reload(jobs);
        }
        if !input.is_empty()
            && let Err(e) = self.append(input)
        {
            return e;
        }
        RunEnd::Idle
    }

    /// A final boundary (steps 1–4 and sync, no request): results plus everything queued.
    fn end_with(&mut self, mut input: BoundaryInput, end: RunEnd) -> RunEnd {
        self.drain_ctl();
        input
            .steers
            .extend(self.queue.steers.drain(..).map(|f| owner_input(&f)));
        input
            .commands
            .extend(self.queue.commands.drain(..).map(|f| owner_input(&f)));
        input
            .owner
            .extend(self.queue.follow_ups.drain(..).map(|f| owner_input(&f)));
        input.notices.append(&mut self.queue.notices);
        input.errors.append(&mut self.pending_errors);
        match self.append(input) {
            Ok(_) => end,
            Err(e) => e,
        }
    }

    fn end_aborted(&mut self, mut input: BoundaryInput) -> RunEnd {
        input.harness_text = self.paused_note();
        let end = self.end_with(input, RunEnd::Aborted);
        if end == RunEnd::Aborted {
            self.say_aborted();
        }
        end
    }

    fn end_aborted_at_boundary(&mut self) -> RunEnd {
        if let Some(t) = self.paused_note()
            && let Err(e) = self.append(BoundaryInput {
                harness_text: Some(t),
                ..Default::default()
            })
        {
            return e;
        }
        self.say_aborted();
        RunEnd::Aborted
    }

    /// §3.5.1: an owner interrupt leaves a note, so the owner's next words resume the task.
    fn paused_note(&self) -> Option<String> {
        self.abort.by_owner().then(|| PAUSED_NOTE.to_string())
    }

    fn say_aborted(&self) {
        let text = if self.abort.by_owner() { PAUSED_TEXT } else { "aborted" };
        self.out.send(Out::display("info", text));
    }

    fn end_provider_failure(&mut self, e: ProviderError, attempts: u32) -> RunEnd {
        let (err, shown) = match &e {
            ProviderError::Budget { message } => (
                provider_error("budget", message.clone(), None),
                message.clone(),
            ),
            ProviderError::Aborted => {
                let end = self.end_with(
                    BoundaryInput {
                        errors: vec![aborted_in_retry()],
                        harness_text: self.paused_note(),
                        ..Default::default()
                    },
                    RunEnd::Aborted,
                );
                if end == RunEnd::Aborted {
                    self.say_aborted();
                }
                return end;
            }
            e if e.is_context_overflow() => (
                provider_error(
                    "context_overflow",
                    format!("the provider rejected the context twice: {e}"),
                    None,
                ),
                format!("context overflow: {e}"),
            ),
            e => (
                provider_error(
                    "provider",
                    e.to_string(),
                    Some(
                        json!({"kind": e.kind_str(), "attempts": attempts, "request_id": e.request_id()}),
                    ),
                ),
                format!("model request failed: {e}"),
            ),
        };
        self.out.send(Out::display("error", shown));
        self.end_with(
            BoundaryInput {
                errors: vec![err],
                ..Default::default()
            },
            RunEnd::Failed,
        )
    }

    // -- model turn -------------------------------------------------------------------------------

    fn model_turn(&mut self, r: &RequestWithMax) -> TurnOutcome {
        let session = self.ctx.session_id();
        let req = Request {
            body: &r.out.body,
            head_hash: &r.out.head_hash,
            messages: &r.out.messages,
            max_tokens: r.max_tokens_used,
            effort: self.effort,
            purpose: Purpose::Turn,
            session: &session,
            task: r.out.task_id.as_deref(),
            subtask: None,
        };
        let t0 = Instant::now();
        let mut ttft: Option<u64> = None;
        let mut attempts = 1u32;
        let mut warnings = Vec::new();
        let model = self.model.clone();
        let mut outcome = None;
        for ev in model.stream(&req, &self.abort) {
            self.status.progress();
            match ev {
                StreamEvent::Started { head_hash, .. } => {
                    if let Some(h) = head_hash
                        && h != r.out.head_hash
                    {
                        warnings.push(provider_error(
                            "head_hash_mismatch",
                            format!(
                                "the proxy computed head hash {h}; the body sent {}",
                                r.out.head_hash
                            ),
                            None,
                        ));
                    }
                }
                StreamEvent::TextDelta(t) => {
                    ttft.get_or_insert(t0.elapsed().as_millis() as u64);
                    self.out.delta(Out::display("text", t));
                }
                StreamEvent::ThinkingDelta(t) => {
                    ttft.get_or_insert(t0.elapsed().as_millis() as u64);
                    self.out.delta(Out::display("thinking", t));
                }
                StreamEvent::ToolCallReady(_) | StreamEvent::Usage(_) => {}
                StreamEvent::Warning { code, detail } => {
                    warnings.push(provider_error(code, detail, None));
                }
                StreamEvent::Retry {
                    attempt,
                    delay,
                    error,
                } => {
                    attempts = attempt + 1;
                    ttft = None;
                    self.out.send(Out::display(
                        "info",
                        format!(
                            "retrying (attempt {}, {}) in {} s",
                            attempt + 1,
                            error,
                            delay.as_secs_f64().ceil() as u64
                        ),
                    ));
                }
                StreamEvent::Done(m) => {
                    outcome = Some(TurnOutcome::Done(
                        m,
                        Timing {
                            wall_ms: Some(t0.elapsed().as_millis() as u64),
                            ttft_ms: ttft,
                        },
                    ));
                    break;
                }
                StreamEvent::Failed(e) => {
                    outcome = Some(TurnOutcome::Failed(e, attempts));
                    break;
                }
            }
        }
        self.abort.clear_stream();
        self.pending_errors.append(&mut warnings);
        outcome.unwrap_or_else(|| {
            TurnOutcome::Failed(
                ProviderError::Protocol("the stream ended without a terminal event".into()),
                attempts,
            )
        })
    }

    // -- tools ----------------------------------------------------------------------------------

    /// §3.7: units (same-path writes/edits serialized), ≤ 16 workers, results in call order.
    fn execute_batch(
        &mut self,
        prepared: Vec<Prepared>,
        task: Option<&str>,
    ) -> Result<Vec<(String, ToolResult)>, RunEnd> {
        let n = prepared.len();
        let mut slots: Vec<Option<ToolResult>> = vec![None; n];
        let mut units: Vec<Vec<usize>> = Vec::new();
        let mut by_path: Vec<(PathBuf, usize)> = Vec::new();
        for (i, p) in prepared.iter().enumerate() {
            match p {
                Prepared::Done { result, .. } => slots[i] = Some(result.clone()),
                Prepared::Ready {
                    job: Job::Delegate(_),
                    ..
                } => {} // run below, before the rest of the batch
                Prepared::Ready { .. } => match self.tools.path_key(p) {
                    Some(k) => match by_path.iter().find(|(q, _)| *q == k) {
                        Some(&(_, u)) => units[u].push(i),
                        None => {
                            by_path.push((k, units.len()));
                            units.push(vec![i]);
                        }
                    },
                    None => units.push(vec![i]),
                },
            }
        }
        // ext: never see a half-loaded registry (host.md §3.9).
        let has_ext = prepared.iter().any(|p| {
            matches!(
                p,
                Prepared::Ready {
                    job: crate::tools::Job::Ext(_),
                    ..
                }
            )
        });
        let reg: Option<Snapshot> = match (&self.ext, self.mode) {
            (Some(x), m) if m != Mode::Safe => {
                if has_ext
                    && x.reload_pending()
                    && let Some(r) = x.commit_reload(RELOAD_WAIT_EXT)
                {
                    self.late_report = Some(r); // the reload event goes into the next boundary
                }
                Some(x.snapshot())
            }
            _ => None,
        };
        let spill_dir = self
            .home
            .join("state")
            .join("spill")
            .join(self.ctx.session_id());
        // delegate (loop/delegate.rs): the loop owns its model calls. A batch's delegates run
        // first, concurrently (writers one at a time); each sends its own `tool_start`/`tool_end`.
        let delegates: Vec<usize> = (0..n)
            .filter(|&i| {
                matches!(
                    prepared[i],
                    Prepared::Ready {
                        job: Job::Delegate(_),
                        ..
                    }
                )
            })
            .collect();
        if !delegates.is_empty() {
            let done = self.run_delegates(&prepared, &delegates, reg.as_ref(), task, &spill_dir);
            for (i, ran) in done {
                slots[i] = Some(ran.result);
            }
        }
        for (i, p) in prepared.iter().enumerate() {
            if p.is_ready() && slots[i].is_none() {
                self.out.send(Out::Display {
                    kind: "tool_start",
                    text: self.tools.start_text(p),
                    call_id: Some(p.call_id().to_string()),
                    name: Some(p.name().to_string()),
                    ok: None,
                    parent: None,
                    detail: None,
                });
            }
        }
        let info = self.model.info().clone();
        let crash_after_first = self.fault.is_some() && self.fault("loop.batch.after_first_result");
        let (tools, abort, out, status) = (&self.tools, &*self.abort, &self.out, &*self.status);
        let pcx = PathCx {
            cwd: &self.cwd,
            home: self.user_home.as_deref(),
            init_home: Some(&self.home),
        };
        let (prepared_ref, units_ref, reg_ref) = (&prepared, &units, reg.as_ref());
        let cursor = std::sync::atomic::AtomicUsize::new(0);
        let mode = self.mode;
        let run_one = |i: usize| -> Ran {
            let p = &prepared_ref[i];
            if abort.is_set() {
                return Ran {
                    result: error_result(p.call_id(), ABORTED_TEXT),
                    summary: "aborted".into(),
                };
            }
            let cx = ToolCx {
                paths: pcx,
                spill_dir: &spill_dir,
                call_id: p.call_id(),
                abort,
                images: info.images,
                model_id: info.id,
                mode,
                reg: reg_ref,
            };
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tools.run(p, &cx)))
                .unwrap_or_else(|payload| {
                    let msg = format!(
                        "error: panic: the {} tool crashed: {}",
                        p.name(),
                        panic_message(&*payload)
                    );
                    Ran {
                        result: error_result(p.call_id(), &msg),
                        summary: "error: panic".into(),
                    }
                })
        };
        let mut crashed = false;
        std::thread::scope(|sc| {
            let (tx, rx) = mpsc::channel::<(usize, Ran)>();
            let workers = units_ref.len().min(MAX_PARALLEL);
            let mut spawned = 0;
            for w in 0..workers {
                let (tx, cursor, run_one) = (tx.clone(), &cursor, &run_one);
                let r = std::thread::Builder::new()
                    .name(format!("tool-{w}"))
                    .spawn_scoped(sc, move || {
                        loop {
                            let u = cursor.fetch_add(1, Ordering::Relaxed);
                            let Some(unit) = units_ref.get(u) else { break };
                            for &i in unit {
                                let _ = tx.send((i, run_one(i)));
                            }
                        }
                    });
                if r.is_ok() {
                    spawned += 1;
                }
            }
            drop(tx);
            if spawned == 0 {
                // Degraded: run every unit inline on this thread, sequentially.
                for unit in units_ref {
                    for &i in unit {
                        let r = run_one(i);
                        self::send_end(out, &prepared_ref[i], &r);
                        slots[i] = Some(r.result);
                    }
                }
                return;
            }
            for (i, r) in rx {
                status.progress();
                send_end(out, &prepared_ref[i], &r);
                slots[i] = Some(r.result);
                if crash_after_first && !crashed {
                    // Fault point: a crash with one result finished and others running. Kill the
                    // running groups; nothing more reaches the log.
                    crashed = true;
                    abort.trigger();
                }
            }
        });
        if crashed {
            return Err(RunEnd::Exit(FAULT_EXIT));
        }
        Ok(prepared
            .iter()
            .zip(slots)
            .map(|(p, r)| {
                (
                    p.name().to_string(),
                    r.unwrap_or_else(|| error_result(p.call_id(), ABORTED_TEXT)),
                )
            })
            .collect())
    }

    // -- pause (§3.8.1) -------------------------------------------------------------------------

    fn pause(&mut self, p: PauseReq) -> PauseEnd {
        let (log_len, leaf) = match self.ctx.pause() {
            Ok(x) => x,
            Err(e) => {
                let _ = self.fatal("log", &e);
                return PauseEnd::Exit(1);
            }
        };
        let prev = self.status.phase();
        self.set_phase(Phase::Paused);
        self.out.send(Out::Paused {
            token: p.token.clone(),
            log_len,
            leaf: leaf.clone(),
        });
        loop {
            match self.ctl.recv() {
                Ok(Control::Resume { update_failed, .. }) => {
                    // An interrupt that arrived while paused is a no-op on resume.
                    self.abort.reset();
                    if let Err(e) = self.ctx.resume(log_len, &leaf, update_failed) {
                        let _ = self.fatal("log", &e);
                        return PauseEnd::Exit(1);
                    }
                    self.status.set_leaf(self.ctx.leaf());
                    self.set_phase(if prev == Phase::Paused {
                        Phase::Idle
                    } else {
                        prev
                    });
                    return PauseEnd::Resumed;
                }
                Ok(Control::Exit { code, .. }) => {
                    return PauseEnd::Exit(u8::try_from(code).unwrap_or(1));
                }
                // D11: stop while paused exits 0 at once; the log is synced and unlocked, and
                // nothing more is written. stdin EOF likewise.
                Ok(Control::Stop { .. }) | Ok(Control::Eof) | Err(_) => return PauseEnd::Exit(0),
                Ok(Control::Interrupt { .. }) => {}
                Ok(c @ Control::Pause(_)) => self.protocol_warning(&c),
                Ok(other) => self.enqueue(other), // held until after resume
            }
        }
    }
}

/// `display{kind:"tool_end"}` for one finished call.
fn send_end(out: &OutTx, p: &Prepared, r: &Ran) {
    out.send(Out::Display {
        kind: "tool_end",
        text: r.summary.clone(),
        call_id: Some(p.call_id().to_string()),
        name: Some(p.name().to_string()),
        ok: Some(!r.result.is_error),
        parent: None,
        detail: crate::tools::fail_detail(&r.result),
    });
}

fn t1_tier(r: ReloadReport) -> ReloadIn {
    let error = if r.ok {
        None
    } else {
        r.text
            .lines()
            .find(|l| l.starts_with('!'))
            .map(|l| l.trim_start_matches('!').trim().to_string())
            .or_else(|| Some("extension load failures".into()))
    };
    ReloadIn {
        tier: "T1".into(),
        ok: r.ok,
        components: r.components,
        head: None,
        error,
        text: r.text,
    }
}

/// `RequestOut` plus the `max_tokens` it was rendered with.
pub struct RequestWithMax {
    pub out: RequestOut,
    pub max_tokens_used: u32,
}

impl std::ops::Deref for RequestWithMax {
    type Target = RequestOut;
    fn deref(&self) -> &RequestOut {
        &self.out
    }
}
