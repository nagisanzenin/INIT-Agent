//! The lane matrix (kernel/initctl.md §3.4) and kernel state gating (§3.5), as pure functions
//! of a request, its lane and a [`KernelView`] snapshot.
//!
//! Order of checks:
//! 1. §3.4 lane column (`E_LANE`), including `promote --sha` on the live lane only;
//! 2. `reload` with `wait:true` on the live lane (`E_WOULD_DEADLOCK`);
//! 3. the §3.5 rows, in table order: control link / stale ledger (`E_ROOT`), outbox backlog
//!    (`E_ROOT`), `Halted` (`E_HALTED`), `Stopping` (`E_SHUTTING_DOWN`), safe mode
//!    (`E_SAFE_MODE`), provisional (`E_PROVISIONAL`), provisional depth (`E_PROVISIONAL_DEPTH`),
//!    disk (`E_DISK`), handoff/rollback/pipeline and the quick-gate queue (`E_BUSY`), kernel
//!    candidate (`E_BUSY{kernel_candidate}`), quota (`E_QUOTA`), level (`E_LEVEL`),
//!    `parallel_bodies` (`E_CEILING`).

use crate::backend::{KernelView, LaneCtx};
use init_proto::ErrorBody;
use init_proto::errors::{BusyReason, ErrorCode, QuotaData, RootReason};
use init_proto::initctl::{Command, CommandName, Lane};

/// The cached level and ceilings are stale after 120 s (§3.5 row 1).
pub const LEDGER_STALE_S: u64 = 120;
/// `level` answers from the cache while it is younger than 60 s.
pub const LEDGER_FRESH_S: u64 = 60;
/// More than 2 quick gates queued → `E_BUSY{queue_full}` (§4.1 `gate`).
pub const QUICK_GATES_QUEUED_MAX: usize = 2;

/// The effective minimum level: the §3.4 column, and L2 for `promote --sha` (adoption of a
/// qualified SHA, IC-29).
pub fn min_level(cmd: &Command) -> u8 {
    match cmd {
        Command::Promote(a) if a.sha.is_some() => 2,
        c => c.name().min_level(),
    }
}

/// True when the command needs the level (refused with `E_ROOT` on a stale ledger).
pub fn level_gated(cmd: &Command) -> bool {
    min_level(cmd) > 0
}

/// Rows 1–2 of §3.4/§3.5's preamble: lane availability and the deadlock guard.
pub fn check_lane(cmd: &Command, lane: &LaneCtx) -> Result<(), ErrorBody> {
    check_lane_name(cmd.name(), lane)?;
    if let Command::Promote(a) = cmd
        && a.sha.is_some()
        && lane.lane != Lane::Live
    {
        return Err(ErrorBody::new(ErrorCode::Lane, "`promote --sha` adopts a SHA on the primary's lane only"));
    }
    if let Command::Reload(a) = cmd
        && a.wait == Some(true)
        && lane.lane == Lane::Live
    {
        return Err(ErrorBody::new(
            ErrorCode::WouldDeadlock,
            "reload happens at your next step boundary, after this tool call returns; run `initctl reload` without --wait",
        ));
    }
    Ok(())
}

/// The §3.4 lane column alone (`E_LANE`): it depends only on the command's name, so serve checks
/// it before decoding the arguments.
pub fn check_lane_name(name: CommandName, lane: &LaneCtx) -> Result<(), ErrorBody> {
    if !name.allowed_on(lane.lane, lane.purpose) {
        return Err(ErrorBody::new(ErrorCode::Lane, format!("`{name}` is not available on the {} lane", lane.lane.as_str())));
    }
    Ok(())
}

/// Facts serve knows from its own job store.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoreFacts {
    /// An active `promote` or `rollback` job (one pipeline runs at a time, system-wide).
    pub pipeline_job: Option<String>,
    /// An active `kernel.promote` job.
    pub kernel_job: Option<String>,
    /// `gate` jobs still `queued`.
    pub queued_gates: usize,
    /// The command's daily quota, when it is used up.
    pub quota_full: Option<QuotaData>,
}

/// The §3.5 rows, in table order.
pub fn check_state(cmd: &Command, lane: &LaneCtx, v: &KernelView, f: &StoreFacts) -> Result<(), ErrorBody> {
    let name = cmd.name();
    // 1. Control link down, or the cached level/ceilings older than 120 s.
    if level_gated(cmd) || name.needs_root() {
        if !v.link_up {
            return Err(ErrorBody::root(RootReason::LinkDown, "the control link to the Root is down; retry after it reconnects"));
        }
        match v.ledger_age_s {
            Some(a) if a <= LEDGER_STALE_S => {}
            Some(a) => {
                return Err(ErrorBody::root(RootReason::Stale, format!("the cached level and ceilings are {a} s old (stale after {LEDGER_STALE_S} s)")));
            }
            None => return Err(ErrorBody::root(RootReason::Stale, "no level or ceilings received from the Root yet")),
        }
    }
    // 2. Outbox backlog.
    if v.outbox_backlog && name.refused_on_outbox_backlog() {
        return Err(ErrorBody::root(RootReason::OutboxBacklog, "the Root outbox is backed up; retry later"));
    }
    // 3. Halted.
    if let Some(reason) = &v.halted
        && !name.allowed_when_halted()
    {
        return Err(ErrorBody::new(ErrorCode::Halted, format!("the kernel is halted ({reason}); the owner's /resume clears it"))
            .with_data(&serde_json::json!({"reason": reason})));
    }
    // 4. Stopping.
    if v.stopping {
        return Err(ErrorBody::new(ErrorCode::ShuttingDown, "the kernel is stopping"));
    }
    // 5. Safe mode.
    if v.safe_mode && name.refused_in_safe_mode() {
        return Err(ErrorBody::new(ErrorCode::SafeMode, format!("`{name}` is refused in safe mode")));
    }
    // 6. The primary is provisional.
    if v.primary_provisional && name.refused_when_provisional() {
        return Err(ErrorBody::new(ErrorCode::Provisional, format!("`{name}` is refused while the primary is provisional (until the LKG commit)")));
    }
    // 7. Provisional depth.
    if name == CommandName::Promote && v.provisional_depth >= v.max_provisional_depth {
        return Err(ErrorBody::new(
            ErrorCode::ProvisionalDepth,
            format!("provisional chain is {} deep (max {}); wait for G4", v.provisional_depth, v.max_provisional_depth),
        ));
    }
    // 8. Disk.
    if v.disk_low && name.refused_on_low_disk() {
        return Err(ErrorBody::new(ErrorCode::Disk, "free space is below disk.min_free_build_gib"));
    }
    // 9. Handoff, rollback or pipeline; the quick-gate queue.
    if matches!(name, CommandName::Promote | CommandName::Rollback) {
        if let Some(job) = &f.pipeline_job {
            return Err(ErrorBody::busy(Some(job), Some(BusyReason::Pipeline), format!("job {job} is running; wait for it: initctl wait {job}")));
        }
        if let Some(b) = &v.busy {
            return Err(ErrorBody::busy(b.job.as_deref(), Some(b.reason), "a handoff, rollback or pipeline is in progress"));
        }
    }
    if name == CommandName::Gate && f.queued_gates >= QUICK_GATES_QUEUED_MAX {
        return Err(ErrorBody::busy(None, Some(BusyReason::QueueFull), format!("{} quick gates are already queued", f.queued_gates)));
    }
    // 10. Kernel candidate.
    if (name == CommandName::Promote && lane.lane == Lane::Live) || name == CommandName::KernelPromote {
        let job = f.kernel_job.as_deref().or(v.kernel_candidate.as_deref());
        if f.kernel_job.is_some() || v.kernel_candidate.is_some() {
            return Err(ErrorBody::busy(job.filter(|j| !j.is_empty()), Some(BusyReason::KernelCandidate), "a kernel candidate or its sealed audit is pending"));
        }
    }
    // 11. Quota.
    if let Some(q) = &f.quota_full {
        let quota = serde_json::to_value(q.quota).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        return Err(ErrorBody::new(ErrorCode::Quota, format!("quota {quota} ({}) is used up until {}", q.limit, q.resets_at)).with_data(q));
    }
    // 12. Level (ADR-011: on the free path `fleet spawn` and `fleet send` need no level).
    let need = if v.extend_free && matches!(cmd, Command::FleetSpawn(_) | Command::FleetSend(_)) { 0 } else { min_level(cmd) };
    if need > 0 {
        let have = v.level.unwrap_or(0);
        if have < need {
            return Err(ErrorBody::level(need, have));
        }
    }
    // 13. parallel_bodies (fleet spawn; the primary counts).
    if name == CommandName::FleetSpawn {
        match v.bodies {
            None => return Err(ErrorBody::root(RootReason::Stale, "parallel_bodies is not known yet")),
            Some(b) if b.used >= b.limit => return Err(ErrorBody::ceiling("parallel_bodies", u64::from(b.limit), u64::from(b.used))),
            Some(_) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{BodyCount, BusyInfo};
    use init_proto::errors::{BusyData, LevelData, QuotaName, RootData};
    use init_proto::initctl::ProbePurpose;
    use serde_json::json;

    fn c(cmd: &str, a: serde_json::Value) -> Command {
        Command::parse(cmd, &a).unwrap()
    }

    fn code(r: Result<(), ErrorBody>) -> Option<ErrorCode> {
        r.err().map(|e| e.code)
    }

    #[test]
    fn lane_rules() {
        let live = LaneCtx::live();
        let member = LaneCtx::member("b1");
        let run = LaneCtx::probe("p1", ProbePurpose::Run);
        let probe = LaneCtx::probe("p1", ProbePurpose::Probe);
        let gate = c("gate", json!({"quick": true, "repo": "work/x", "rev": "HEAD"}));
        assert_eq!(code(check_lane(&gate, &run)), Some(ErrorCode::Lane), "IC-20");
        assert_eq!(code(check_lane(&gate, &probe)), None);
        let sha = c("promote", json!({"sha": "4b1e0c2d9a7f3e5d1c0b9a8f7e6d5c4b3a291807"}));
        assert_eq!(code(check_lane(&sha, &member)), Some(ErrorCode::Lane));
        assert_eq!(code(check_lane(&sha, &live)), None);
        assert_eq!(code(check_lane(&c("reload", json!({"wait": true})), &live)), Some(ErrorCode::WouldDeadlock), "IC-7");
        assert_eq!(code(check_lane(&c("reload", json!({"wait": true})), &member)), None);
        assert_eq!(code(check_lane(&c("fleet.spawn", json!({"prompt": "p"})), &member)), Some(ErrorCode::Lane));
        assert_eq!(code(check_lane(&c("rollback", json!({"reason": "r"})), &probe)), Some(ErrorCode::Lane));
    }

    #[test]
    fn rows_in_order() {
        let live = LaneCtx::live();
        let f = StoreFacts::default();
        let promote = c("promote", json!({"ref": "evo/x"}));
        let vendor = c("vendor", json!({"crate": "vt100", "version": "0.16.2"}));
        let submit = c("task.submit", json!({"dir": "work/t"}));
        let ok = KernelView::default();
        assert_eq!(code(check_state(&promote, &live, &ok, &f)), None);

        // Row 1: link down → E_ROOT for level-gated and Root-needing commands only.
        let down = KernelView { link_up: false, ..ok.clone() };
        let e = check_state(&vendor, &live, &down, &f).unwrap_err();
        assert_eq!((e.code.clone(), e.data_as::<RootData>().unwrap().reason), (ErrorCode::Root, Some(RootReason::LinkDown)));
        assert_eq!(code(check_state(&promote, &live, &down, &f)), None, "promote needs no Root request");
        let stale = KernelView { ledger_age_s: Some(121), level: Some(1), ..ok.clone() };
        assert_eq!(check_state(&submit, &live, &stale, &f).unwrap_err().data_as::<RootData>().unwrap().reason, Some(RootReason::Stale));
        let never = KernelView { ledger_age_s: None, ..ok.clone() };
        assert_eq!(code(check_state(&submit, &live, &never, &f)), Some(ErrorCode::Root));
        // Row 1 precedes row 3 (halted).
        let both = KernelView { link_up: false, halted: Some("config".into()), ..ok.clone() };
        assert_eq!(code(check_state(&vendor, &live, &both, &f)), Some(ErrorCode::Root));

        // Row 2.
        let backlog = KernelView { outbox_backlog: true, ..ok.clone() };
        assert_eq!(check_state(&promote, &live, &backlog, &f).unwrap_err().data_as::<RootData>().unwrap().reason, Some(RootReason::OutboxBacklog));
        assert_eq!(code(check_state(&c("gate", json!({"quick": true, "repo": "w", "rev": "HEAD"})), &live, &backlog, &f)), None);

        // Row 3: halted allows ping/status/level/job.*.
        let halted = KernelView { halted: Some("crash_loop_safe".into()), ..ok.clone() };
        assert_eq!(code(check_state(&promote, &live, &halted, &f)), Some(ErrorCode::Halted));
        assert_eq!(code(check_state(&Command::Status, &live, &halted, &f)), None);
        assert_eq!(code(check_state(&c("job.list", json!({})), &live, &halted, &f)), None);

        // Row 4: stopping refuses everything.
        let stopping = KernelView { stopping: true, ..ok.clone() };
        assert_eq!(code(check_state(&Command::Ping, &live, &stopping, &f)), Some(ErrorCode::ShuttingDown));

        // Row 5: safe mode.
        let safe = KernelView { safe_mode: true, ..ok.clone() };
        assert_eq!(code(check_state(&promote, &live, &safe, &f)), Some(ErrorCode::SafeMode));
        assert_eq!(code(check_state(&c("kernel.propose", json!({"title": "t", "patch": "work/p", "rationale": ""})), &live, &safe, &f)), None);

        // Row 6: provisional (IC-13).
        let prov = KernelView { primary_provisional: true, provisional_depth: 1, ..ok.clone() };
        assert_eq!(code(check_state(&vendor, &live, &prov, &f)), Some(ErrorCode::Provisional));
        assert_eq!(code(check_state(&promote, &live, &prov, &f)), None);

        // Row 7: depth.
        let deep = KernelView { primary_provisional: true, provisional_depth: 3, ..ok.clone() };
        assert_eq!(code(check_state(&promote, &live, &deep, &f)), Some(ErrorCode::ProvisionalDepth));

        // Row 8: disk.
        let disk = KernelView { disk_low: true, ..ok.clone() };
        assert_eq!(code(check_state(&promote, &live, &disk, &f)), Some(ErrorCode::Disk));
        assert_eq!(code(check_state(&c("rollback", json!({"reason": "r"})), &live, &disk, &f)), None);

        // Row 9: pipeline (IC-10), backend busy, quick-gate queue.
        let fp = StoreFacts { pipeline_job: Some("j-000041".into()), ..f.clone() };
        let e = check_state(&promote, &live, &ok, &fp).unwrap_err();
        assert_eq!(e.code, ErrorCode::Busy);
        assert_eq!(e.data_as::<BusyData>().unwrap().job.as_deref(), Some("j-000041"));
        let busy = KernelView { busy: Some(BusyInfo { job: None, reason: BusyReason::Handoff }), ..ok.clone() };
        assert_eq!(code(check_state(&c("rollback", json!({"reason": "r"})), &live, &busy, &f)), Some(ErrorCode::Busy));
        let fq = StoreFacts { queued_gates: 2, ..f.clone() };
        assert_eq!(code(check_state(&c("gate", json!({"quick": true, "repo": "w", "rev": "HEAD"})), &live, &ok, &fq)), Some(ErrorCode::Busy));

        // Row 10: kernel candidate (promote on the live lane only, and kernel promote).
        let fk = StoreFacts { kernel_job: Some("j-000050".into()), ..f.clone() };
        let e = check_state(&promote, &live, &ok, &fk).unwrap_err();
        assert_eq!(e.data_as::<BusyData>().unwrap().reason, Some(BusyReason::KernelCandidate));
        assert_eq!(code(check_state(&promote, &LaneCtx::member("b1"), &ok, &fk)), None);

        // Row 11: quota precedes level.
        let q = QuotaData { quota: QuotaName::FleetSpawnsDay, limit: 50, resets_at: "2026-10-21T00:00:00.000Z".into() };
        let fq = StoreFacts { quota_full: Some(q), ..f.clone() };
        let spawn = c("fleet.spawn", json!({"prompt": "go"}));
        assert_eq!(code(check_state(&spawn, &live, &ok, &fq)), Some(ErrorCode::Quota));

        // Row 12: level (IC-4, IC-24, IC-26, IC-28, IC-29).
        let e = check_state(&submit, &live, &ok, &f).unwrap_err();
        assert_eq!(e.data_as::<LevelData>(), Some(LevelData { need: 1, have: 0 }));
        let l1 = KernelView { level: Some(1), ..ok.clone() };
        let adopt = c("promote", json!({"sha": "4b1e0c2d9a7f3e5d1c0b9a8f7e6d5c4b3a291807"}));
        assert_eq!(check_state(&adopt, &live, &l1, &f).unwrap_err().data_as::<LevelData>(), Some(LevelData { need: 2, have: 1 }));
        let l2 = KernelView { level: Some(2), ..ok.clone() };
        assert_eq!(code(check_state(&adopt, &live, &l2, &f)), None);
        assert_eq!(code(check_state(&c("kernel.promote", json!({"ref": "kevo/x"})), &live, &l2, &f)), Some(ErrorCode::Level));
        assert_eq!(code(check_state(&c("fleet.stop", json!({"bid": "all"})), &live, &ok, &f)), None, "fleet stop: any level");

        // Row 13: parallel_bodies (IC-25).
        let full = KernelView { level: Some(2), bodies: Some(BodyCount { limit: 2, used: 2 }), ..ok.clone() };
        let e = check_state(&spawn, &live, &full, &f).unwrap_err();
        assert_eq!((e.code.clone(), e.exit_code()), (ErrorCode::Ceiling, 77));
        let room = KernelView { level: Some(2), bodies: Some(BodyCount { limit: 2, used: 1 }), ..ok.clone() };
        assert_eq!(code(check_state(&spawn, &live, &room, &f)), None);
    }
}
