//! The initctl server inside `init-kernel supervise --under-root` (WP7b): kernel-serve's
//! [`Backend`] over the running supervisor, the lane sockets, the promote pipeline's seams, and
//! the 60 s provisional watch.
//!
//! Lives in the binary, like [`crate::proxy`], so the lane crates stay independent: kernel-serve
//! (the wire, jobs, gating) and kernel-supervise (the supervisor and the T2 pipeline) meet here.
//!
//! Start order (supervise.md §3.2, §3.9; initctl.md §3.1, §3.3; FIX6):
//! 1. Right after the supervisor takes `run/kernel.lock` (§3.9 step 1) and before its cold start,
//!    [`spawn`] binds `run/kernel.sock` as a [`kernel_serve::EarlyLane`]: from then on a body's
//!    `initctl` always connects, and gets `E_BUSY{reason:"starting"}` (retry: yes, exit 75) until
//!    the server is ready. It also starts hashing the kernel's own binary in the background (the
//!    gate record's `kernel.build_sha256`; 154 MB in a debug build).
//! 2. Once the supervisor's loop answers `Services` (the cold start is over), the server opens
//!    the job store (interrupted jobs become `kernel_restart`, their candidate refs are deleted),
//!    adopts the early socket (same listener, no gap), installs the probe-lane hook and serves.
//! 3. When the hash is known, the promote pipeline's [`Env`] is built and the watch thread starts.
//!    Until then `promote`, `gate`, `rollback` and `kernel status` wait for it (bounded by
//!    [`ENV_WAIT`], then `E_BUSY{starting}`); every other command is served at once.
//!
//! Nothing here can stop the supervisor: a server that fails to start is logged and alerted, its
//! early socket is removed, and the kernel runs on without initctl.

use crate::proxy::KernelProxy;
use init_proto::envelope::ErrorBody;
use init_proto::errors::{BusyReason, ErrorCode, RootReason};
use init_proto::files::{self as pf, LkgFile};
use init_proto::initctl::*;
use init_proto::root_link::{AlertArgs, AlertLevel, KernelRequest, ProposalFileArgs};
use init_proto::serde_json::{json, Value};
use kernel_serve::{Backend, Begun, BodyCount, BusyInfo, EarlyLane, JobCtl, KernelView, LaneCtx, LaneSpec, LedgerSnapshot, Limits, PeerPolicy, SameUidPolicy, SandboxPolicy, ServeConfig, ServerHandle};
use kernel_supervise::machine::{Mode as SMode, Slot as SSlot};
use kernel_supervise::probe::{LaneHook, Purpose};
use kernel_supervise::promote::{self, Env, GateCfg, Job, Jobs, KernelInfo, MemJob};
use kernel_supervise::supervisor::{Cmd, Handle, Services, Snapshot};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How often the provisional watch runs (WP8b: G-46 resubmission, F16 expiry).
pub const WATCH_EVERY: Duration = Duration::from_secs(60);
/// `reload` gives up after this long without `reload_done` (initctl.md §4.1).
const RELOAD_TIMEOUT: Duration = Duration::from_secs(1800);
/// How long `promote`, `gate`, `rollback` and `kernel status` wait for the gate environment at
/// start (the kernel's own SHA-256) before they answer `E_BUSY{reason:"starting"}`.
pub const ENV_WAIT: Duration = Duration::from_secs(300);

/// When [`spawn`] ran (≈ the kernel's start): the `ms` of the start-up log lines (FIX6 timing).
static T0: OnceLock<Instant> = OnceLock::new();

fn log(what: &str) {
    let ms = T0.get().map(|t| t.elapsed().as_millis() as u64);
    eprintln!("{}", json!({"c": "serve", "msg": what, "ms": ms, "ts": kernel_supervise::clock::now_rfc3339()}));
}

/// A value that arrives after start-up, with a bounded wait for it.
struct Late<T> {
    v: Mutex<Option<T>>,
    cv: Condvar,
}

impl<T: Clone> Late<T> {
    fn new() -> Late<T> {
        Late { v: Mutex::new(None), cv: Condvar::new() }
    }
    fn set(&self, t: T) {
        *self.v.lock().unwrap_or_else(|e| e.into_inner()) = Some(t);
        self.cv.notify_all();
    }
    fn get(&self) -> Option<T> {
        self.v.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    fn wait(&self, d: Duration) -> Option<T> {
        let g = self.v.lock().unwrap_or_else(|e| e.into_inner());
        let (g, _) = self.cv.wait_timeout_while(g, d, |v| v.is_none()).unwrap_or_else(|e| e.into_inner());
        g.clone()
    }
}

fn internal(msg: impl AsRef<str>) -> ErrorBody {
    ErrorBody::new(ErrorCode::Internal, msg)
}

// ---- the pipeline's job seam ------------------------------------------------------------------

/// `promote::Job` over kernel-serve's `JobCtl`: the adapter of WP8b's `tests/serve_seam.rs`.
struct ServeJob(JobCtl);

impl Job for ServeJob {
    fn id(&self) -> String {
        self.0.id().to_string()
    }
    fn cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
    fn progress(&self, state: JobState, stage: Option<Stage>, pct: u8, msg: &str) {
        self.0.progress(state, stage, pct, None, msg);
    }
    fn stage_done(&self, r: StageResult) {
        self.0.stage_done(r);
    }
    fn awaiting_boundary(&self) -> bool {
        self.0.awaiting_boundary().is_ok()
    }
    fn succeed(&self, result: Value) {
        self.0.succeed_value(result);
    }
    fn fail(&self, err: ErrorBody) {
        self.0.fail(err);
    }
}

/// Kernel-internal jobs (an asynchronous G4 has its own `j-` id, gate.md §3.1) in serve's store,
/// owned by the live lane so `initctl job status`/`wait` see them.
#[derive(Default)]
struct ServeJobs {
    srv: OnceLock<ServerHandle>,
}

impl Jobs for ServeJobs {
    fn create(&self, cmd: &str, args: Value) -> Arc<dyn Job> {
        if let Some(srv) = self.srv.get() {
            match srv.jobs().create(&LaneCtx::live(), cmd, args) {
                Ok(j) => return Arc::new(ServeJob(j)),
                Err(e) => log(&format!("job store: {e}; `{cmd}` runs untracked")),
            }
        }
        MemJob::new(&format!("j-9{:05}", std::process::id() % 100_000))
    }
}

// ---- the backend ------------------------------------------------------------------------------

/// kernel-serve's [`Backend`] for this kernel.
pub struct KernelBackend {
    home: PathBuf,
    h: Handle,
    sv: Services,
    /// The promote pipeline's environment, set once the kernel's own SHA-256 is known.
    env: Late<Arc<Env>>,
    started: Instant,
    /// `refresh_ledger`'s copy (the supervisor keeps its own, refreshed every `ledger_refresh_s`).
    ledger: Mutex<Option<(Value, Value, Instant)>>,
    /// Probe lanes: probe id → its instance (for the macOS peer check's mark).
    probes: Mutex<BTreeMap<String, String>>,
    upgrade: Arc<crate::upgrade::Upgrade>,
}

impl KernelBackend {
    fn snap(&self) -> Result<Snapshot, ErrorBody> {
        self.h.latest().ok_or_else(|| internal("the supervisor has not published its state yet"))
    }

    fn mark(&self, instance: &str) -> PathBuf {
        self.home.join("run/mark").join(instance)
    }

    /// The gate environment, waiting for it at start (at most [`ENV_WAIT`]).
    fn env(&self) -> Result<Arc<Env>, ErrorBody> {
        self.env.wait(ENV_WAIT).ok_or_else(kernel_serve::starting_error)
    }
}

fn depth(m: &SMode) -> u32 {
    match m {
        SMode::Provisional(p) => p.depth,
        _ => 0,
    }
}

fn slot_name(s: &SSlot) -> init_proto::initctl::Slot {
    s.name()
}

fn short(s: &str) -> &str {
    &s[..s.len().min(12)]
}

impl Backend for KernelBackend {
    fn view(&self, _lane: &LaneCtx) -> KernelView {
        let Some(s) = self.h.latest() else {
            return KernelView { link_up: false, ledger_age_s: None, level: None, ..KernelView::default() };
        };
        let own_age = self.ledger.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|(_, _, t)| t.elapsed().as_secs());
        let ledger_age_s = match (s.ledger_age_s, own_age) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let pb = s.ceilings.as_ref().and_then(|c| c.get("parallel_bodies"));
        let bodies = pb.and_then(|p| p.get("limit").and_then(Value::as_u64)).map(|limit| BodyCount { limit: limit as u32, used: 1 });
        KernelView {
            link_up: s.link_up,
            ledger_age_s,
            level: Some(s.level),
            outbox_backlog: s.outbox_backlog,
            halted: match &s.slot {
                SSlot::Halted { reason } => Some(reason.as_str().to_string()),
                _ => None,
            },
            stopping: matches!(s.slot, SSlot::Stopping { .. }),
            safe_mode: s.mode.is_safe() || s.system_safe,
            primary_provisional: matches!(s.mode, SMode::Provisional(_)),
            provisional_depth: depth(&s.mode),
            // ADR-011: on the free path the Root's blast-radius budget replaces the depth limit.
            max_provisional_depth: if s.extend_free { u32::MAX } else { self.sv.cfg.supervise.max_provisional_depth },
            disk_low: s.free_bytes < s.min_free_build_bytes,
            busy: s.handoff.as_ref().map(|_| BusyInfo { job: None, reason: BusyReason::Handoff }),
            kernel_candidate: self.upgrade.active(),
            bodies,
            extend_free: s.extend_free,
        }
    }

    fn marks(&self, lane: &LaneCtx) -> Vec<PathBuf> {
        match lane.lane {
            Lane::Probe => self.probes.lock().unwrap_or_else(|e| e.into_inner()).get(&lane.id).map(|i| vec![self.mark(i)]).unwrap_or_default(),
            _ => self.h.latest().map(|s| s.instances.iter().map(|i| self.mark(i)).collect()).unwrap_or_default(),
        }
    }

    fn status(&self, lane: &LaneCtx) -> Result<StatusResult, ErrorBody> {
        let s = self.snap()?;
        let lkg: Option<LkgFile> = std::fs::read(self.home.join(pf::paths::LKG)).ok().and_then(|b| pf::decode(&b).ok());
        let money = s.ceilings.as_ref().and_then(|c| c.get("money")).cloned().unwrap_or(Value::Null);
        let since = match &s.mode {
            SMode::Provisional(p) => kernel_supervise::clock::format_ms(p.since_ms),
            _ => s.kernel_started.clone(),
        };
        // No pipeline can run before the gate environment exists.
        let env = self.env.get();
        let g4 = env.as_ref().and_then(|e| e.g4_running.lock().unwrap_or_else(|e| e.into_inner()).clone());
        let pipeline = env.as_ref().is_some_and(|e| e.pipeline_busy.load(std::sync::atomic::Ordering::SeqCst));
        let safe = match &s.mode {
            SMode::Safe(i) => Some(json!({"reason": i.reason, "system": i.system, "entered_at": kernel_supervise::clock::format_ms(i.entered_ms)})),
            _ => None,
        };
        Ok(StatusResult {
            body: StatusBody { id: lane.id.clone(), lane: lane.lane },
            budget: StatusBudget {
                day_cap_musd: money.get("day_limit_musd").and_then(Value::as_u64).unwrap_or(0),
                day_spent_musd: money.get("day_spent_musd").and_then(Value::as_u64).unwrap_or(0),
                gate_cap_musd: None,
                gate_spent_musd: None,
                soft: BTreeMap::new(),
            },
            cache_hit_permille: None,
            constitution_sha256: String::new(), // serve fills it
            counters: s.counters.clone(),
            current: StatusCurrent { depth: depth(&s.mode), provisional: matches!(s.mode, SMode::Provisional(_)), sha: s.current.sha.clone(), since },
            disk: StatusDisk { free_mib: if s.free_bytes == u64::MAX { 0 } else { s.free_bytes / (1024 * 1024) } },
            // M7: the last G4's agent-visible aggregate (tier, verdict, accept class, the slice's
            // rounded effect; never ids or per-task values; m7-design.md §3.2.11).
            evolution: json!({"pipeline": pipeline, "g4_job": g4,
                              "g4_last": std::fs::read(self.home.join("state/kernel/gate/g4_last.json")).ok().and_then(|b| init_proto::serde_json::from_slice::<Value>(&b).ok())}),
            fleet: None,
            handoff: s.handoff.clone(),
            jobs: vec![], // serve fills it
            kernel: StatusKernel { proto: init_proto::PROTO, uptime_s: self.started.elapsed().as_secs(), version: kernel_supervise::supervisor::KERNEL_VERSION.into() },
            lane: lane.lane,
            link: StatusLink { boot_reason: s.boot_reason.clone(), ceilings_age_s: s.ledger_age_s, connected: s.link_up, outbox_unanswered: s.outbox_unanswered as u64 },
            lkg: StatusLkg {
                seq: lkg.as_ref().map(|l| l.seq).unwrap_or(0),
                sha: s.lkg_sha.clone(),
                since: lkg.as_ref().map(|l| l.committed_at.clone()).unwrap_or_else(|| s.kernel_started.clone()),
            },
            mode: s.mode.name(),
            root: s.ledger.clone().unwrap_or(Value::Null),
            safe,
            sandbox: Some(kernel_supervise::supervisor::sandbox_status()),
            slot: slot_name(&s.slot),
            sweep: Sweep::Full,
        })
    }

    fn ledger(&self) -> Option<LedgerSnapshot> {
        let own = self.ledger.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let s = self.h.latest();
        let sup = s.as_ref().and_then(|s| Some((s.ledger.clone()?, s.ceilings.clone().unwrap_or(Value::Null), s.ledger_age_s.unwrap_or(u64::MAX))));
        match (own, sup) {
            (Some((l, c, t)), Some((sl, sc, sa))) => {
                let a = t.elapsed().as_secs();
                Some(if a <= sa { LedgerSnapshot { ledger: l, ceilings: c, age_s: a } } else { LedgerSnapshot { ledger: sl, ceilings: sc, age_s: sa } })
            }
            (Some((l, c, t)), None) => Some(LedgerSnapshot { ledger: l, ceilings: c, age_s: t.elapsed().as_secs() }),
            (None, Some((l, c, a))) => Some(LedgerSnapshot { ledger: l, ceilings: c, age_s: a }),
            (None, None) => None,
        }
    }

    fn refresh_ledger(&self) -> Result<LedgerSnapshot, ErrorBody> {
        let t = self.sv.link.request_timeout;
        let ledger = self.sv.link.call(&KernelRequest::LedgerGet, t).map_err(|e| ErrorBody::root(RootReason::Refused, format!("ledger.get: {e}")))?;
        let ceilings = self.sv.link.call(&KernelRequest::CeilingsGet, t).map_err(|e| ErrorBody::root(RootReason::Refused, format!("ceilings.get: {e}")))?;
        *self.ledger.lock().unwrap_or_else(|e| e.into_inner()) = Some((ledger.clone(), ceilings.clone(), Instant::now()));
        Ok(LedgerSnapshot { ledger, ceilings, age_s: 0 })
    }

    fn reload(&self, _lane: &LaneCtx, _args: &ReloadArgs, job: JobCtl) -> Result<Begun, ErrorBody> {
        let s = self.snap()?;
        if s.primary.is_none() || !matches!(s.slot, SSlot::Running) {
            return Err(ErrorBody::new(ErrorCode::NoBody, format!("no body takes a reload: the body slot is {:?}", slot_name(&s.slot))));
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = job.set_awaiting_boundary();
        if !self.h.send(Cmd::Reload { job: job.id().to_string(), kind: init_proto::stdio::ReloadKind::Request, components: None, reply: tx }) {
            return Err(internal("the supervisor is gone"));
        }
        std::thread::spawn(move || match rx.recv_timeout(RELOAD_TIMEOUT) {
            Ok(Ok(outcome)) => {
                job.succeed(&outcome);
            }
            Ok(Err(e)) => {
                job.fail(ErrorBody::new(ErrorCode::NoBody, e));
            }
            Err(_) => {
                job.fail(ErrorBody::new(ErrorCode::Timeout, "no reload_done from the body within 1800 s"));
            }
        });
        Ok(Begun::started())
    }

    fn gate(&self, lane: &LaneCtx, args: &GateArgs, repo: &Path, job: JobCtl) -> Result<Begun, ErrorBody> {
        let r = promote::gate_quick(self.env()?, repo, &args.rev, lane.lane == Lane::Probe, Arc::new(ServeJob(job)))?;
        Ok(Begun::started().with("sha", r["sha"].clone()))
    }

    fn promote(&self, _lane: &LaneCtx, args: &PromoteArgs, job: JobCtl) -> Result<Begun, ErrorBody> {
        let jid = job.id().to_string();
        let job = crate::evo::EvoJob { inner: Arc::new(ServeJob(job)), home: self.home.clone(), lineage: self.sv.outbox.clone() };
        let r = promote::promote(self.env()?, args, Arc::new(job))?;
        // EVO-LITE: a screened proposal whose chosen variant this is becomes `promoted`.
        if let Some(sha) = r["sha"].as_str() {
            crate::evo::promoted(&self.home, &*self.sv.outbox, &jid, sha);
        }
        Ok(Begun::started().with("sha", r["sha"].clone()))
    }

    fn promote_check(&self, _lane: &LaneCtx, args: &PromoteArgs) -> Result<Value, ErrorBody> {
        promote::check(self.env()?, args)
    }

    fn rollback(&self, _lane: &LaneCtx, args: &RollbackArgs, job: JobCtl) -> Result<Begun, ErrorBody> {
        let r = promote::rollback(self.env()?, args, Arc::new(ServeJob(job)))?;
        Ok(Begun::started().with("from", r["from"].clone()).with("to", r["to"].clone()))
    }

    fn fleet_spawn(&self, _lane: &LaneCtx, args: &FleetSpawnArgs, _job: JobCtl) -> Result<Begun, ErrorBody> {
        let s = self.snap()?;
        let limit = s.ceilings.as_ref().and_then(|c| c["parallel_bodies"]["limit"].as_u64()).unwrap_or(1) as u32;
        kernel_supervise::fleet::check_spawn(s.level, 1, limit, s.extend_free)?;
        if let Some(agent) = &args.agent {
            let git = kernel_supervise::git::Git::body(&self.sv.cfg.supervise.git, &self.sv.home);
            let parent = args.parent.clone().unwrap_or_else(|| s.current.sha.clone());
            kernel_supervise::fleet::agent_spec(&git, &parent, agent)?;
        }
        Err(internal("fleet members (supervise.md §3.14) are not built in this kernel; ADR-011 leaves the member runtime to a later phase"))
    }

    fn kernel_promote(&self, _lane: &LaneCtx, args: &KernelPromoteArgs, job: JobCtl) -> Result<Begun, ErrorBody> {
        let s = self.snap()?;
        let env = self.env()?;
        let g4 = env.g4_running.lock().unwrap_or_else(|e| e.into_inner()).is_some();
        kernel_supervise::kupgrade::check_request_in(s.extend_free, s.level, s.mode.is_safe() || s.system_safe,
            matches!(s.mode, SMode::Provisional(_)) || g4 || s.busy, self.upgrade.active().is_some())?;
        self.upgrade.start(env, self.sv.clone(), args.git_ref.clone(), job)?;
        Ok(Begun::started())
    }

    fn task_template(&self, _lane: &LaneCtx) -> Result<Value, ErrorBody> {
        let inst = |n: u32| {
            json!({
                "v": 2, "id": format!("my-family-{n}"), "title": "one line", "prompt": "What the agent must do, in full.",
                "labels": ["basic"], "workspace": {"file": "workspace.tar", "sha256": "<64 hex of workspace.tar>", "bytes": 1},
                "check": {"argv": ["check/run"], "timeout_s": 60}, "limits": {"wall_s": 600, "max_cost_musd": 500000},
                "source": {"kind": "agent"}, "created": "<RFC 3339>"
            })
        };
        Ok(json!({
            "family": "my-family",
            "notes": "A family has at least 2 instances; half is sealed at admission (initctl.md `task submit`, kernel/gate.md §3.7). Each instance is a directory with task.json, workspace.tar, check/run and solution.tar.",
            "instances": [inst(1), inst(2)],
        }))
    }

    fn task_list(&self, _lane: &LaneCtx) -> Result<TaskListResult, ErrorBody> {
        // Agent-authored families are admitted by `task submit` (L1), which this kernel does not
        // build yet: the list is empty.
        Ok(TaskListResult(vec![]))
    }

    fn task_show(&self, _lane: &LaneCtx, args: &TaskShowArgs) -> Result<Value, ErrorBody> {
        Err(ErrorBody::new(ErrorCode::NotFound, format!("no task family `{}`", args.family)))
    }

    fn fleet_list(&self, _lane: &LaneCtx) -> Result<FleetListResult, ErrorBody> {
        Ok(FleetListResult { candidates: vec![], members: vec![], suggested_parent: None })
    }

    fn fleet_send(&self, _lane: &LaneCtx, args: &FleetSendArgs) -> Result<FleetSendResult, ErrorBody> {
        Err(ErrorBody::new(ErrorCode::NotFound, format!("no fleet member `{}`", args.bid)))
    }

    fn fleet_stop(&self, _lane: &LaneCtx, args: &FleetStopArgs) -> Result<FleetStopResult, ErrorBody> {
        if args.bid == "all" {
            return Ok(FleetStopResult { stopping: vec![] });
        }
        Err(ErrorBody::new(ErrorCode::NotFound, format!("no fleet member `{}`", args.bid)))
    }

    fn kernel_status(&self, _lane: &LaneCtx) -> Result<KernelStatusResult, ErrorBody> {
        Ok(KernelStatusResult {
            candidate: self.upgrade.candidate(),
            last_audit: None,
            running: KernelRunning { built_from: self.sv.boot.boot.tuple_kernel.built_from.clone(), source: self.sv.boot.boot.tuple_kernel.source.clone(), version: kernel_supervise::supervisor::KERNEL_VERSION.into() },
        })
    }

    fn file_proposal(&self, _lane: &LaneCtx, title: &str, text: &str) -> Result<String, ErrorBody> {
        let req = KernelRequest::ProposalFile(ProposalFileArgs { title: title.to_string(), text: text.to_string() });
        let v = self.sv.link.call(&req, self.sv.link.request_timeout).map_err(|e| ErrorBody::root(RootReason::Refused, format!("proposal.file: {e}")))?;
        v.get("id").and_then(Value::as_str).map(str::to_string).ok_or_else(|| ErrorBody::root(RootReason::Refused, "proposal.file: no id in the Root's answer"))
    }

    /// ADR-013 (GBAL): `initctl goal <op>` → the Root's `goal.*` / `card.*`.
    fn goal(&self, _lane: &LaneCtx, args: &init_proto::initctl::GoalArgs) -> Result<Value, ErrorBody> {
        let cmd = init_proto::initctl::goal_cmd(&args.op).ok_or_else(|| ErrorBody::new(ErrorCode::Args, format!("goal: unknown op {:?}", args.op)))?;
        let a = if args.args.is_null() { init_proto::serde_json::json!({}) } else { args.args.clone() };
        self.sv.link.call(&init_proto::root_link::KernelRequest::Goal(cmd, a), self.sv.link.request_timeout).map_err(|e| ErrorBody::root(RootReason::Refused, format!("{cmd}: {e}")))
    }

    fn grant_request(&self, _lane: &LaneCtx, args: &GrantArgs) -> Result<GrantResult, ErrorBody> {
        let a = init_proto::serde_json::to_value(args).map_err(|e| ErrorBody::new(ErrorCode::Internal, format!("grant args: {e}")))?;
        let v = kernel_supervise::promote::admit::grant_request(&self.sv.link, a)?;
        init_proto::serde_json::from_value(v).map_err(|e| ErrorBody::root(RootReason::Refused, format!("grant.request: the Root's answer: {e}")))
    }

    fn evo(&self, lane: &LaneCtx, cmd: &Command, text: Option<&str>) -> Result<Value, ErrorBody> {
        crate::evo::command(&self.sv, lane, cmd, text)
    }

    fn evo_screen(&self, _lane: &LaneCtx, args: &EvoScreenArgs, job: JobCtl) -> Result<Begun, ErrorBody> {
        crate::evo::screen(self.env()?, args, Arc::new(ServeJob(job)))
    }
}

// ---- probe lanes ------------------------------------------------------------------------------

/// Binds and removes `probe/<id>/run/kernel.sock` as probes come and go.
struct ProbeLanes {
    srv: ServerHandle,
    home: PathBuf,
    backend: Arc<KernelBackend>,
}

impl LaneHook for ProbeLanes {
    fn open(&self, probe_id: &str, purpose: Purpose) {
        let p = if purpose == Purpose::Run { ProbePurpose::Run } else { ProbePurpose::Probe };
        if let Err(e) = self.srv.add_lane(LaneSpec::probe(&self.home, probe_id, p)) {
            log(&format!("probe lane {probe_id}: {e}"));
        }
    }
    fn started(&self, probe_id: &str, instance: &str) {
        self.backend.probes.lock().unwrap_or_else(|e| e.into_inner()).insert(probe_id.to_string(), instance.to_string());
    }
    fn close(&self, probe_id: &str) {
        self.backend.probes.lock().unwrap_or_else(|e| e.into_inner()).remove(probe_id);
        let _ = self.srv.remove_lane(&format!("probe:{probe_id}"));
    }
}

// ---- wiring -----------------------------------------------------------------------------------

fn sha256_file(p: &Path) -> String {
    kernel_supervise::fsx::sha256_file(p).unwrap_or_default()
}

/// The peer policy (initctl.md §3.3): the real three-condition check, with the Root's canary at
/// `ROOT_HOME/run/canary` (the directory of the boot block's `paths.egress_sock`). Same-uid peers
/// are accepted only in a test kernel (`proxy.test_mode = true`) whose Root also passed
/// `INIT_TEST_SERVE_PEER=same-uid` (a test variable, root/root.md §4.4), so tests can drive
/// initctl from outside a sandbox.
fn policy(sv: &Services) -> Arc<dyn PeerPolicy> {
    if sv.cfg.test_mode && std::env::var("INIT_TEST_SERVE_PEER").as_deref() == Ok("same-uid") {
        return Arc::new(SameUidPolicy);
    }
    let canary = Path::new(&sv.boot.paths.egress_sock).parent().map(|d| d.join("canary")).unwrap_or_else(|| PathBuf::from("/nonexistent/canary"));
    Arc::new(SandboxPolicy { canary })
}

/// The promote pipeline's environment for this kernel (`build_sha256`: its own binary's SHA-256).
fn env(sv: &Services, h: &Handle, proxy: &Arc<KernelProxy>, jobs: Arc<ServeJobs>, build_sha256: String) -> Arc<Env> {
    let cfg_path = sv.home.join("kernel/config.toml");
    let text = std::fs::read_to_string(&cfg_path).unwrap_or_default();
    let gate = GateCfg::parse(&text).unwrap_or_else(|errs| {
        errs.iter().for_each(|e| log(&format!("[gate] config: {e}")));
        GateCfg::default()
    });
    Arc::new(Env {
        home: sv.home.clone(),
        cfg: sv.cfg.clone(),
        gate,
        spawner: sv.spawner.clone(),
        // G0's commands run as children of the kernel, inside the kernel's own sandbox profile
        // (WP8b deviation 1: no broker-side G0 runner yet).
        g0: Arc::new(kernel_gate::g0::DirectSpawner),
        proxy: Arc::new(crate::proxy::KernelGate::new(proxy.clone(), &sv.home, &sv.cfg.model.provider, &sv.cfg.model.id, h.clone())),
        lineage: sv.outbox.clone(),
        sup: Arc::new(h.clone()),
        jobs,
        // The sandboxed upload-pack relay (supervise.md §3.6.1) is not built: plain upload-pack,
        // still inside the kernel's sandbox (WP8b deviation 2).
        upload_pack: "git-upload-pack".into(),
        level: sv.level,
        kernel: KernelInfo {
            version: kernel_supervise::supervisor::KERNEL_VERSION.into(),
            build_sha256,
            config_sha256: sha256_file(&cfg_path),
            charter_sha256: sv.boot.charter_sha256.clone(),
        },
        model: sv.cfg.model.id.clone(),
        pipeline_busy: AtomicBool::new(false),
        g4_running: Mutex::new(None),
        gates: AtomicU64::new(0),
        g4_attempts: Mutex::new(Default::default()),
        fault: None,
        slot_lock: Mutex::new(()),
        // G4 v2 (M7): the sealed slice over the control link; `root.sealed_none` takes the v0.3 path.
        sealed: Some(sv.outbox.clone()),
        // ADR-011 (X2): the free path over the control link; the Root's `extend.mode` decides.
        extend: Some(Arc::new(kernel_supervise::promote::admit::RootExtend { link: sv.link.clone(), free: sv.extend_free.clone() })),
    })
}

/// The early socket until the server adopts it, then the server (for shutdown at exit).
#[derive(Default)]
pub struct SlotInner {
    early: Option<EarlyLane>,
    srv: Option<ServerHandle>,
    closed: bool,
}

/// Shared between [`spawn`]'s threads and [`shutdown`].
pub type Slot = Arc<Mutex<SlotInner>>;

/// The `on_start` hook of `main_under_root_started`: runs once this process holds
/// `run/kernel.lock`, before the cold start. Binds `run/kernel.sock` at once (answering
/// `E_BUSY{starting}`), starts hashing the kernel binary, and starts the server in the background
/// when the supervisor's loop answers. Returns at once.
pub fn spawn(h: Handle, proxy: Arc<KernelProxy>, slot: Slot) {
    let _ = T0.set(Instant::now());
    let home = h.home().to_path_buf();
    match EarlyLane::bind(LaneSpec::live(&home), Limits::default()) {
        Ok(e) => {
            log(&format!("{} bound; E_BUSY{{reason:\"starting\"}} until the server is ready", e.socket().display()));
            slot.lock().unwrap_or_else(|e| e.into_inner()).early = Some(e);
        }
        Err(e) => log(&format!("run/kernel.sock not bound early ({e}); binding it after the cold start")),
    }
    // The gate record's `kernel.build_sha256`, off the start-up path (WP15: hashing the 154 MB
    // debug binary with the unoptimized sha2 took 10 s and more, before the socket was bound).
    let exe_sha: Arc<Late<String>> = Arc::new(Late::new());
    {
        let exe_sha = exe_sha.clone();
        let _ = std::thread::Builder::new().name("kernel-sha256".into()).spawn(move || {
            let t = Instant::now();
            let sha = std::env::current_exe().map(|p| sha256_file(&p)).unwrap_or_default();
            log(&format!("kernel binary sha256 in {} ms", t.elapsed().as_millis()));
            exe_sha.set(sha);
        });
    }
    let _ = std::thread::Builder::new().name("serve-init".into()).spawn(move || {
        let sv = loop {
            if let Some(sv) = h.services() {
                break sv;
            }
            // `services()` waits 10 s; the cold start may take longer (hello, restores).
            if !h.alive() {
                return;
            }
        };
        if let Err(e) = start(&h, &sv, &proxy, &slot, &exe_sha) {
            // The early socket must not answer "starting" forever: drop it (unlinks the socket).
            slot.lock().unwrap_or_else(|e| e.into_inner()).early = None;
            log(&format!("initctl server not started: {e}"));
            let _ = sv.link.send(&KernelRequest::Alert(AlertArgs { code: "serve_failed".into(), level: AlertLevel::Warn, message: kernel_supervise::fsx::tail_str(&e, 500) }));
        }
    });
}

fn start(h: &Handle, sv: &Services, proxy: &Arc<KernelProxy>, slot: &Slot, exe_sha: &Late<String>) -> Result<(), String> {
    let jobs = Arc::new(ServeJobs::default());
    let backend = Arc::new(KernelBackend {
        home: sv.home.clone(),
        h: h.clone(),
        sv: sv.clone(),
        env: Late::new(),
        started: Instant::now(),
        ledger: Mutex::new(None),
        probes: Mutex::new(BTreeMap::new()),
        upgrade: crate::upgrade::Upgrade::new(&sv.home),
    });
    let srv = kernel_serve::start(ServeConfig::new(&sv.home, kernel_supervise::supervisor::KERNEL_VERSION), backend.clone(), policy(sv)).map_err(|e| format!("job store: {e}"))?;
    let _ = jobs.srv.set(srv.clone());
    let upgrades = backend.upgrade.clone();
    kernel_supervise::kupgrade::set_swap_hook(Arc::new(move |event| upgrades.on_swap(event)));
    backend.upgrade.recover(sv, srv.jobs());
    // §3.7: a job the restart interrupted leaves its candidate ref behind.
    let git = kernel_supervise::git::Git::body(&sv.cfg.supervise.git, &sv.home);
    for j in srv.recovered() {
        if j.starts_with("j-") && j.len() <= 16 && j[2..].bytes().all(|b| b.is_ascii_digit()) {
            let _ = git.run(&["update-ref", "-d", &format!("refs/init/candidates/{j}")], None, Duration::from_secs(30));
        }
    }
    // ADR-011 X0b: proposals past `screened` resume from the Root's records (nothing the restart
    // interrupted stays stranded at `promoted` or `opening`).
    let resumed = kernel_supervise::recovery::resume_proposals(&sv.home, &*sv.outbox);
    if resumed > 0 {
        log(&format!("{resumed} proposal transition(s) resumed from the Root's records"));
    }
    {
        let mut g = slot.lock().unwrap_or_else(|e| e.into_inner());
        if g.closed {
            srv.shutdown(); // the kernel is exiting
            return Ok(());
        }
        let bound = match g.early.take() {
            Some(early) => srv.adopt_lane(early).or_else(|e| {
                log(&format!("adopting the early run/kernel.sock: {e}; binding it again"));
                srv.add_lane(LaneSpec::live(&sv.home))
            }),
            None => srv.add_lane(LaneSpec::live(&sv.home)),
        };
        if let Err(e) = bound {
            srv.shutdown();
            return Err(format!("run/kernel.sock: {e}"));
        }
        g.srv = Some(srv.clone());
    }
    kernel_supervise::probe::set_lane_hook(Arc::new(ProbeLanes { srv: srv.clone(), home: sv.home.clone(), backend: backend.clone() }));
    crate::evo::spawn_miner(sv.home.clone(), sv.outbox.clone());
    log(&format!("initctl server on {} (live lane b0)", sv.home.join("run/kernel.sock").display()));
    // The gate environment once the kernel's own hash is known (it started with the cold start).
    let build_sha = loop {
        if let Some(sha) = exe_sha.wait(Duration::from_secs(60)) {
            break sha;
        }
        if !h.alive() {
            return Ok(());
        }
    };
    let env = env(sv, h, proxy, jobs, build_sha);
    backend.env.set(env.clone());
    log("gate environment ready (promote, gate, rollback)");
    let h2 = h.clone();
    let _ = std::thread::Builder::new().name("provisional-watch".into()).spawn(move || watch(env, h2));
    Ok(())
}

/// The 60 s provisional watch (WP8b `provisional_watch`): resubmit unacknowledged accept records
/// and commit LKG after the ack (G-46); roll back a provisional version older than
/// `evolution.provisional_max_age_s` (F16). Ends with the supervisor.
fn watch(env: Arc<Env>, h: Handle) {
    loop {
        std::thread::sleep(WATCH_EVERY);
        if !h.alive() {
            return;
        }
        let Some(s) = h.latest() else { continue };
        if matches!(s.slot, SSlot::Stopping { .. }) {
            return;
        }
        let age_s = match &s.mode {
            SMode::Provisional(p) => ((kernel_supervise::clock::now_ms() - p.since_ms).max(0) / 1000) as u64,
            _ => 0,
        };
        let r = promote::provisional_watch(&env, age_s);
        if r["rolled_back"] == json!(true) || r["resubmitted"].as_array().is_some_and(|a| !a.is_empty()) {
            log(&format!("provisional watch ({}): {r}", short(&s.current.sha)));
        }
    }
}

/// Stop the server (Stopping or exit): close the lane sockets and every connection; an early
/// socket that was never adopted is removed.
pub fn shutdown(slot: &Slot) {
    let mut g = slot.lock().unwrap_or_else(|e| e.into_inner());
    g.closed = true;
    g.early = None;
    if let Some(srv) = g.srv.take() {
        srv.shutdown();
    }
}
