//! SV-1 (the §3.3 table), SV-2, SV-3, SV-4, SV-8, SV-10, SV-11, SV-12, SV-28 and the handoff
//! phases against the pure machine.

use super::*;

const X: &str = "1111111111111111111111111111111111111111";
const Y: &str = "2222222222222222222222222222222222222222";

fn cfg() -> SuperviseCfg {
    SuperviseCfg { ready_timeout_s: 90, settle_s: 3, pause_timeout_s: 2, probation_s: 120, ..SuperviseCfg::default() }
}

fn m() -> Machine {
    Machine::new(cfg(), Ver { sha: X.into(), provisional: false }, X.into(), Mode::Normal)
}

fn spawns(a: &[Act]) -> Vec<SpawnPlan> {
    a.iter().filter_map(|x| if let Act::Spawn(p) = x { Some(p.clone()) } else { None }).collect()
}

fn has(a: &[Act], f: impl Fn(&Act) -> bool) -> bool {
    a.iter().any(f)
}

/// Start → spawned → ready; returns the instance id.
fn boot(m: &mut Machine, now: Now, id: &str) -> Vec<Act> {
    let tag = match &m.slot {
        Slot::Starting { tag, .. } => *tag,
        s => panic!("not starting: {s:?}"),
    };
    let mut a = m.on(Ev::Spawned { tag, result: Ok(id.into()) }, now);
    a.extend(m.on(Ev::Ready { inst: id.into() }, now));
    a
}

fn crash(m: &mut Machine, id: &str, now: Now) -> Vec<Act> {
    m.on(Ev::Exited { inst: id.into(), code: Some(101), signal: None, disk_low: false, stderr_tail: "panicked".into() }, now)
}

#[test]
fn cold_start_ready_and_redelivery() {
    let mut m = m();
    let t = Now::real();
    let a = m.on(Ev::Start { reason: BootReason::Cold }, t);
    let p = &spawns(&a)[0];
    assert_eq!((p.bin, p.reason, p.mode, p.sha.as_str()), (BinSel::Current, BootReason::Cold, InitMode::Normal, X));
    let a = boot(&mut m, t, "i1");
    assert_eq!(m.slot, Slot::Running);
    assert!(has(&a, |x| matches!(x, Act::Redeliver { inst } if inst == "i1")));
    assert_eq!(m.relay_target(), Some("i1"));
}

#[test]
fn sv2_backoff_doubles_and_resets_after_300s_ready() {
    let mut m = Machine::new(SuperviseCfg { crash_limit: 100, ..cfg() }, Ver { sha: X.into(), provisional: false }, X.into(), Mode::Normal);
    let mut t = Now::real();
    m.on(Ev::Start { reason: BootReason::Cold }, t);
    let mut delays = Vec::new();
    for n in 0..8 {
        let id = format!("i{n}");
        boot(&mut m, t, &id);
        crash(&mut m, &id, t);
        let Slot::Backoff { until, .. } = m.slot.clone() else { panic!("{:?}", m.slot) };
        delays.push((until - t.mono).as_secs());
        t = Now { mono: until, wall_ms: t.wall_ms + 1 };
        let a = m.on(Ev::Tick, t);
        let p = &spawns(&a)[0];
        assert_eq!(p.reason, BootReason::Crash);
        assert!(p.crash.as_ref().is_some_and(|c| c.exit_code == Some(101) && c.log_tail == "panicked"));
    }
    assert_eq!(delays, [1, 2, 4, 8, 16, 32, 60, 60]);
    boot(&mut m, t, "ok");
    m.on(Ev::Tick, t.plus(Duration::from_secs(301)));
    assert_eq!(m.crash.consecutive, 0, "reset after 300 s ready");
}

#[test]
fn sv3_fifth_lkg_crash_enters_body_safe_mode_and_reports() {
    let mut m = m();
    let mut t = Now::real();
    m.on(Ev::Start { reason: BootReason::Cold }, t);
    let mut entered = None;
    for n in 0..5 {
        let id = format!("i{n}");
        boot(&mut m, t, &id);
        let a = crash(&mut m, &id, t.plus(Duration::from_secs(1)));
        if has(&a, |x| matches!(x, Act::Report { kind: "body_safe", .. })) {
            entered = Some(n);
            assert!(has(&a, |x| matches!(x, Act::Alert { code: "safe_entered", .. })));
            assert!(has(&a, |x| matches!(x, Act::StopFleet)));
        }
        let Slot::Backoff { until, .. } = m.slot.clone() else { panic!() };
        t = Now { mono: until, wall_ms: t.wall_ms + 10_000 };
        let a = m.on(Ev::Tick, t);
        if n == 4 {
            let p = &spawns(&a)[0];
            assert_eq!((p.mode, p.bin), (InitMode::Safe, BinSel::Lkg), "the 5th crash → spawn LKG with INIT_MODE=safe");
        }
    }
    assert_eq!(entered, Some(4));
    assert!(m.mode.is_safe());
}

#[test]
fn crashes_outside_the_window_do_not_count() {
    let mut m = m();
    let mut t = Now::real();
    m.on(Ev::Start { reason: BootReason::Cold }, t);
    for n in 0..8 {
        let id = format!("i{n}");
        boot(&mut m, t, &id);
        crash(&mut m, &id, t);
        let Slot::Backoff { until, .. } = m.slot.clone() else { panic!() };
        t = Now { mono: until, wall_ms: t.wall_ms + 200_000 }; // 200 s apart: at most 3 in 600 s
        m.on(Ev::Tick, t);
    }
    assert!(!m.mode.is_safe());
}

#[test]
fn sv4_three_safe_crashes_halt_and_resume_reenters_safe() {
    let mut m = Machine::new(cfg(), Ver { sha: X.into(), provisional: false }, X.into(), Mode::Safe(SafeInfo { entered_ms: 0, reason: "t".into(), system: true, crashes: vec![] }));
    let mut t = Now::real();
    m.on(Ev::Start { reason: BootReason::Cold }, t);
    for n in 0..3 {
        let id = format!("s{n}");
        boot(&mut m, t, &id);
        let a = crash(&mut m, &id, t);
        if n == 2 {
            assert_eq!(m.slot, Slot::Halted { reason: HaltReason::CrashLoopSafe });
            assert!(has(&a, |x| matches!(x, Act::Alert { code: "halted", .. })));
            assert!(has(&a, |x| matches!(x, Act::Report { kind: "halted", .. })));
            break;
        }
        let Slot::Backoff { until, .. } = m.slot.clone() else { panic!() };
        t = Now { mono: until, wall_ms: t.wall_ms + 1000 };
        m.on(Ev::Tick, t);
    }
    let a = m.on(Ev::OwnerResume, t);
    let p = &spawns(&a)[0];
    assert_eq!((p.mode, p.bin), (InitMode::Safe, BinSel::Lkg));
    assert_eq!(m.crash, CrashTrack::default(), "counters reset");
}

#[test]
fn sv28_exit_78_twice_halts_config_without_counting_crashes() {
    let mut m = m();
    let t = Now::real();
    m.on(Ev::Start { reason: BootReason::Cold }, t);
    let Slot::Starting { tag, .. } = m.slot.clone() else { panic!() };
    m.on(Ev::Spawned { tag, result: Ok("a".into()) }, t);
    m.on(Ev::Exited { inst: "a".into(), code: Some(78), signal: None, disk_low: false, stderr_tail: String::new() }, t);
    let Slot::Backoff { until, .. } = m.slot.clone() else { panic!("{:?}", m.slot) };
    m.on(Ev::Tick, Now { mono: until, ..t });
    let Slot::Starting { tag, .. } = m.slot.clone() else { panic!() };
    m.on(Ev::Spawned { tag, result: Ok("b".into()) }, t);
    let a = m.on(Ev::Exited { inst: "b".into(), code: Some(78), signal: None, disk_low: false, stderr_tail: "bad key x".into() }, t);
    assert_eq!(m.slot, Slot::Halted { reason: HaltReason::Config });
    assert!(has(&a, |x| matches!(x, Act::Alert { code: "config_error", message } if message.contains("bad key x"))));
    assert_eq!(m.counters.crashes_total, 0);
    assert!(m.crash.lkg_window.is_empty());
}

#[test]
fn exit_zero_stops_and_owner_line_restarts_cold() {
    let mut m = m();
    let t = Now::real();
    m.on(Ev::Start { reason: BootReason::Cold }, t);
    boot(&mut m, t, "a");
    let a = m.on(Ev::OwnerStop, t);
    assert!(has(&a, |x| matches!(x, Act::Send { frame: ToBody::Stop { .. }, .. })));
    m.on(Ev::Exited { inst: "a".into(), code: Some(0), signal: None, disk_low: false, stderr_tail: String::new() }, t);
    assert_eq!(m.slot, Slot::Stopped);
    assert_eq!(m.counters.crashes_total, 0);
    let a = m.on(Ev::OwnerLine, t);
    assert_eq!(spawns(&a)[0].reason, BootReason::Cold);
}

#[test]
fn ready_timeout_and_stale_heartbeat_kill_then_count_a_crash() {
    let mut m = m();
    let t = Now::real();
    m.on(Ev::Start { reason: BootReason::Cold }, t);
    let Slot::Starting { tag, .. } = m.slot.clone() else { panic!() };
    m.on(Ev::Spawned { tag, result: Ok("a".into()) }, t);
    let a = m.on(Ev::Tick, t.plus(Duration::from_secs(91)));
    assert!(has(&a, |x| matches!(x, Act::Kill { inst, mode: KillMode::Kill } if inst == "a")));
    m.on(Ev::Exited { inst: "a".into(), code: None, signal: Some(9), disk_low: false, stderr_tail: String::new() }, t);
    assert!(matches!(m.slot, Slot::Backoff { .. }));
    assert_eq!(m.counters.crashes_total, 1);
    // F-hang (b): a hung live body.
    let Slot::Backoff { until, .. } = m.slot.clone() else { panic!() };
    let t2 = Now { mono: until, wall_ms: t.wall_ms + 5000 };
    m.on(Ev::Tick, t2);
    boot(&mut m, t2, "b");
    let a = m.on(Ev::HeartbeatStale { inst: "b".into() }, t2);
    assert!(has(&a, |x| matches!(x, Act::Kill { inst, .. } if inst == "b")));
    m.on(Ev::Exited { inst: "b".into(), code: None, signal: Some(9), disk_low: false, stderr_tail: String::new() }, t2);
    assert_eq!(m.counters.crashes_total, 2);
}

#[test]
fn disk_low_crash_is_not_counted() {
    let mut m = m();
    let t = Now::real();
    m.on(Ev::Start { reason: BootReason::Cold }, t);
    boot(&mut m, t, "a");
    let a = m.on(Ev::Exited { inst: "a".into(), code: Some(1), signal: None, disk_low: true, stderr_tail: String::new() }, t);
    assert!(matches!(m.slot, Slot::DiskFull { .. }));
    assert!(has(&a, |x| matches!(x, Act::Alert { code: "disk_low", .. })));
    assert_eq!(m.counters.crashes_total, 0);
    let a = m.on(Ev::DiskRecovered, t);
    assert_eq!(spawns(&a).len(), 1);
}

fn running_provisional(probation_left: Duration) -> (Machine, Now) {
    let mut m = m();
    let t = Now::real();
    m.on(Ev::Start { reason: BootReason::Cold }, t);
    boot(&mut m, t, "a");
    m.prev = Some(Ver { sha: X.into(), provisional: false });
    m.current = Ver { sha: Y.into(), provisional: true };
    m.mode = Mode::Provisional(Prov { sha: Y.into(), depth: 1, since_ms: t.wall_ms, probation_until: Some(t.mono + probation_left) });
    (m, t)
}

#[test]
fn sv10_crash_in_probation_rolls_back_to_the_previous_binary_at_once() {
    let (mut m, t) = running_provisional(Duration::from_secs(120));
    let a = crash(&mut m, "a", t.plus(Duration::from_secs(30)));
    assert!(has(&a, |x| matches!(x, Act::InstallBin { from: BinSel::Prev })));
    assert!(has(&a, |x| matches!(x, Act::UpdateFailed { stage: UpdateFailedStage::Probation, .. })));
    assert!(has(&a, |x| matches!(x, Act::Report { kind: "body_rollback", .. })));
    let p = &spawns(&a)[0];
    assert_eq!((p.reason, p.bin), (BootReason::Rollback, BinSel::Current));
    assert_eq!(m.counters.swaps_failed, 1);
    assert_eq!(m.mode, Mode::Normal);
    assert_eq!(m.current.sha, X);
}

#[test]
fn sv11_provisional_crash_after_probation_spawns_lkg_without_backoff() {
    let (mut m, t) = running_provisional(Duration::from_secs(0));
    m.on(Ev::Tick, t.plus(Duration::from_secs(1)));
    let a = crash(&mut m, "a", t.plus(Duration::from_secs(200)));
    assert!(has(&a, |x| matches!(x, Act::InstallBin { from: BinSel::Lkg })));
    assert!(has(&a, |x| matches!(x, Act::UpdateFailed { stage: UpdateFailedStage::CrashProvisional, .. })));
    assert!(matches!(m.slot, Slot::Starting { .. }), "no backoff");
    assert_eq!(spawns(&a)[0].reason, BootReason::Rollback);
}

fn promote() -> HandoffKind {
    HandoffKind::Promote { job: "j-1".into(), sha: Y.into(), staged_sha256: "ab".into(), branch: Some("evo/x".into()) }
}

/// Drive a handoff to the point where B is spawned; returns B's tag.
fn to_spawned(m: &mut Machine, t: Now) -> Tag {
    let a = m.on(Ev::StartHandoff(promote()), t);
    let token = a.iter().find_map(|x| if let Act::Send { frame: ToBody::Pause { token, reason: PauseReason::Handoff }, .. } = x { Some(token.clone()) } else { None }).unwrap();
    assert_eq!(m.slot, Slot::Handoff);
    assert_eq!(m.relay_target(), None, "inputs queue during the pause");
    let a = m.on(Ev::Paused { inst: "a".into(), token }, t);
    assert!(has(&a, |x| matches!(x, Act::FenceTry)));
    let a = m.on(Ev::FenceOk, t);
    let p = spawns(&a).pop().unwrap();
    assert_eq!((p.bin, p.reason, p.replaces.as_deref()), (BinSel::Staged, BootReason::Promote, Some("a")));
    p.tag
}

#[test]
fn sv5_happy_handoff_commits_then_retires_a() {
    let mut m = m();
    let t = Now::real();
    m.on(Ev::Start { reason: BootReason::Cold }, t);
    boot(&mut m, t, "a");
    let tag = to_spawned(&mut m, t);
    let a = m.on(Ev::Spawned { tag, result: Ok("b".into()) }, t);
    assert!(has(&a, |x| matches!(x, Act::FenceRelease)));
    m.on(Ev::Ready { inst: "b".into() }, t);
    assert!(m.on(Ev::Tick, t.plus(Duration::from_secs(1))).iter().all(|x| !matches!(x, Act::Commit { .. })), "settle 3 s");
    let a = m.on(Ev::Tick, t.plus(Duration::from_secs(3)));
    assert!(has(&a, |x| matches!(x, Act::Commit { to_sha, .. } if to_sha == Y)));
    assert!(has(&a, |x| matches!(x, Act::Send { inst, frame: ToBody::Exit { code: 75, .. } } if inst == "a")));
    assert!(has(&a, |x| matches!(x, Act::Redeliver { inst } if inst == "b")));
    assert_eq!(m.relay_target(), None, "still retiring");
    let a = m.on(Ev::Exited { inst: "a".into(), code: Some(75), signal: None, disk_low: false, stderr_tail: String::new() }, t);
    assert!(has(&a, |x| matches!(x, Act::HandoffDone { ok: true, .. })));
    assert_eq!(m.relay_target(), Some("b"));
    assert!(matches!(m.mode, Mode::Provisional(Prov { depth: 1, .. })));
    assert_eq!((m.counters.swaps_ok, m.counters.crashes_total), (1, 0));
}

/// SV-10 race (K-FLAKE): B crashes in probation before A has exited after the commit. The
/// handoff must still be answered (it committed), and the crash rules then roll back at once.
#[test]
fn sv10_b_crashes_while_a_retires_answers_the_handoff_then_rolls_back() {
    let mut m = m();
    let t = Now::real();
    m.on(Ev::Start { reason: BootReason::Cold }, t);
    boot(&mut m, t, "a");
    let tag = to_spawned(&mut m, t);
    m.on(Ev::Spawned { tag, result: Ok("b".into()) }, t);
    m.on(Ev::Ready { inst: "b".into() }, t);
    let a = m.on(Ev::Tick, t.plus(Duration::from_secs(3)));
    assert!(has(&a, |x| matches!(x, Act::Commit { .. })), "committed; A is retiring");
    let a = crash(&mut m, "b", t.plus(Duration::from_secs(4)));
    assert!(has(&a, |x| matches!(x, Act::HandoffDone { ok: true, .. })), "the promote is answered: {a:?}");
    assert!(has(&a, |x| matches!(x, Act::InstallBin { from: BinSel::Prev })));
    assert!(has(&a, |x| matches!(x, Act::UpdateFailed { stage: UpdateFailedStage::Probation, .. })));
    assert!(spawns(&a).is_empty(), "the rollback spawn waits for A, which still holds the log lock");
    assert_eq!(m.current.sha, X);
    // A's exit then releases the deferred rollback spawn; the handoff is not answered twice.
    let a = m.on(Ev::Exited { inst: "a".into(), code: Some(75), signal: None, disk_low: false, stderr_tail: String::new() }, t.plus(Duration::from_secs(4)));
    assert!(!has(&a, |x| matches!(x, Act::HandoffDone { .. })), "{a:?}");
    assert_eq!(spawns(&a)[0].reason, BootReason::Rollback);
}

#[test]
fn sv6_candidate_exits_before_ready_resumes_a() {
    let mut m = m();
    let t = Now::real();
    m.on(Ev::Start { reason: BootReason::Cold }, t);
    boot(&mut m, t, "a");
    let tag = to_spawned(&mut m, t);
    m.on(Ev::Spawned { tag, result: Ok("b".into()) }, t);
    let a = m.on(Ev::Exited { inst: "b".into(), code: Some(1), signal: None, disk_low: false, stderr_tail: String::new() }, t);
    assert!(has(&a, |x| matches!(x, Act::UpdateFailed { stage: UpdateFailedStage::Handoff, exit: Some((Some(1), None)), .. })));
    assert!(has(&a, |x| matches!(x, Act::LocalLineage { kind: "handoff_failed", .. })));
    assert!(has(&a, |x| matches!(x, Act::ResumeA { inst, .. } if inst == "a")));
    assert!(has(&a, |x| matches!(x, Act::HandoffDone { ok: false, .. })));
    assert_eq!((m.counters.swaps_failed, m.counters.crashes_total), (1, 0));
    assert_eq!(m.relay_target(), Some("a"));
    assert_eq!(m.current.sha, X);
}

#[test]
fn sv7_sv8_sv9_timeouts_abort() {
    // SV-8: A never pauses.
    let mut m = m();
    let t = Now::real();
    m.on(Ev::Start { reason: BootReason::Cold }, t);
    boot(&mut m, t, "a");
    m.on(Ev::StartHandoff(promote()), t);
    let a = m.on(Ev::Tick, t.plus(Duration::from_secs(3)));
    assert!(has(&a, |x| matches!(x, Act::UpdateFailed { stage: UpdateFailedStage::Pause, .. })));
    assert_eq!(m.slot, Slot::Running);
    // SV-9: paused but keeps the lock.
    let a = m.on(Ev::StartHandoff(promote()), t);
    let token = a.iter().find_map(|x| if let Act::Send { frame: ToBody::Pause { token, .. }, .. } = x { Some(token.clone()) } else { None }).unwrap();
    m.on(Ev::Paused { inst: "a".into(), token }, t);
    m.on(Ev::FenceBusy, t.plus(Duration::from_secs(1)));
    let a = m.on(Ev::Tick, t.plus(Duration::from_secs(6)));
    assert!(has(&a, |x| matches!(x, Act::UpdateFailed { stage: UpdateFailedStage::Fence, .. })));
    // SV-7: B never ready → kill B, wait for its exit, then resume A.
    let tag = to_spawned(&mut m, t);
    m.on(Ev::Spawned { tag, result: Ok("b".into()) }, t);
    let a = m.on(Ev::Tick, t.plus(Duration::from_secs(91)));
    assert!(has(&a, |x| matches!(x, Act::Kill { inst, .. } if inst == "b")));
    assert!(!has(&a, |x| matches!(x, Act::ResumeA { .. })), "A resumes only after B's exit");
    let a = m.on(Ev::Exited { inst: "b".into(), code: None, signal: Some(9), disk_low: false, stderr_tail: String::new() }, t);
    assert!(has(&a, |x| matches!(x, Act::ResumeA { .. })));
    assert_eq!(m.counters.swaps_failed, 3);
    assert_eq!(m.counters.crashes_total, 0);
}

#[test]
fn sv12_g4_rollback_waits_for_idle_then_forces() {
    let (mut m, t) = running_provisional(Duration::from_secs(0));
    m.cfg.rollback_max_defer_s = 3;
    m.on(Ev::Busy { inst: "a".into(), busy: true }, t);
    let a = m.request_rollback("g4_fail", false, t);
    assert!(a.is_empty(), "busy: wait");
    let a = m.on(Ev::Tick, t.plus(Duration::from_secs(1)));
    assert!(!has(&a, |x| matches!(x, Act::Send { frame: ToBody::Pause { .. }, .. })));
    let a = m.on(Ev::Tick, t.plus(Duration::from_secs(3)));
    assert!(has(&a, |x| matches!(x, Act::Send { frame: ToBody::Pause { reason: PauseReason::Rollback, .. }, .. })), "forced after rollback_max_defer_s");
    assert_eq!(m.slot, Slot::RollingBack);
    // The idle path.
    let (mut m, t) = running_provisional(Duration::from_secs(0));
    m.on(Ev::Busy { inst: "a".into(), busy: true }, t);
    m.request_rollback("g4_fail", false, t);
    let a = m.on(Ev::Busy { inst: "a".into(), busy: false }, t);
    assert!(has(&a, |x| matches!(x, Act::Send { frame: ToBody::Pause { reason: PauseReason::Rollback, .. }, .. })));
}

#[test]
fn rollback_handoff_commits_to_lkg() {
    let (mut m, t) = running_provisional(Duration::from_secs(0));
    let a = m.request_rollback("initctl", true, t);
    let token = a.iter().find_map(|x| if let Act::Send { frame: ToBody::Pause { token, .. }, .. } = x { Some(token.clone()) } else { None }).unwrap();
    m.on(Ev::Paused { inst: "a".into(), token }, t);
    let a = m.on(Ev::FenceOk, t);
    let p = spawns(&a).pop().unwrap();
    assert_eq!((p.bin, p.reason, p.sha.as_str()), (BinSel::Lkg, BootReason::Rollback, X));
    m.on(Ev::Spawned { tag: p.tag, result: Ok("b".into()) }, t);
    m.on(Ev::Ready { inst: "b".into() }, t);
    let a = m.on(Ev::Tick, t.plus(Duration::from_secs(4)));
    assert!(has(&a, |x| matches!(x, Act::CommitRollback { from_sha, to_sha, .. } if from_sha == Y && to_sha == X)));
    assert_eq!(m.mode, Mode::Normal);
}

#[test]
fn a_dies_while_paused_after_b_spawned_commits_anyway() {
    let mut m = m();
    let t = Now::real();
    m.on(Ev::Start { reason: BootReason::Cold }, t);
    boot(&mut m, t, "a");
    let tag = to_spawned(&mut m, t);
    m.on(Ev::Spawned { tag, result: Ok("b".into()) }, t);
    m.on(Ev::Exited { inst: "a".into(), code: None, signal: Some(9), disk_low: false, stderr_tail: String::new() }, t);
    m.on(Ev::Ready { inst: "b".into() }, t);
    let a = m.on(Ev::Tick, t.plus(Duration::from_secs(3)));
    assert!(has(&a, |x| matches!(x, Act::Commit { .. })));
    assert!(has(&a, |x| matches!(x, Act::HandoffDone { ok: true, .. })));
    assert_eq!(m.relay_target(), Some("b"));
}

#[test]
fn root_stop_and_prepare_swap() {
    // stop: stop frames, then `ok` and exit 0 once the body has exited.
    let mut m = m();
    let t = Now::real();
    m.on(Ev::Start { reason: BootReason::Cold }, t);
    boot(&mut m, t, "a");
    let a = m.on(Ev::RootStop { req: "r-1".into(), grace_ms: 1000 }, t);
    assert!(has(&a, |x| matches!(x, Act::Send { frame: ToBody::Stop { .. }, .. })));
    assert!(!has(&a, |x| matches!(x, Act::Respond { .. })));
    m.on(Ev::Exited { inst: "a".into(), code: Some(0), signal: None, disk_low: false, stderr_tail: String::new() }, t);
    let a = m.on(Ev::Tick, t);
    assert!(has(&a, |x| matches!(x, Act::Respond { req, result: Ok(_) } if req == "r-1")));
    assert!(has(&a, |x| matches!(x, Act::Exit(0))));
    // A body ignoring stop is killed after the grace period.
    let mut m = self::m();
    m.on(Ev::Start { reason: BootReason::Cold }, t);
    boot(&mut m, t, "a");
    m.on(Ev::RootStop { req: "r-2".into(), grace_ms: 1000 }, t);
    let a = m.on(Ev::Tick, t.plus(Duration::from_secs(2)));
    assert!(has(&a, |x| matches!(x, Act::Kill { mode: KillMode::Kill, .. })));
    // prepare_swap: pause, stop while paused, exit 75.
    let mut m = self::m();
    m.on(Ev::Start { reason: BootReason::Cold }, t);
    boot(&mut m, t, "a");
    let a = m.on(Ev::RootPrepareSwap { req: "r-3".into(), deadline_s: 5 }, t);
    let token = a.iter().find_map(|x| if let Act::Send { frame: ToBody::Pause { token, reason: PauseReason::Swap }, .. } = x { Some(token.clone()) } else { None }).unwrap();
    let a = m.on(Ev::Paused { inst: "a".into(), token }, t);
    assert!(has(&a, |x| matches!(x, Act::Send { frame: ToBody::Stop { reason, .. }, .. } if reason == "swap")));
    m.on(Ev::Exited { inst: "a".into(), code: Some(0), signal: None, disk_low: false, stderr_tail: String::new() }, t);
    let a = m.on(Ev::Tick, t);
    assert!(has(&a, |x| matches!(x, Act::Exit(75))));
    // F31: a body that never pauses → root.busy, resumed, kernel keeps running.
    let mut m = self::m();
    m.on(Ev::Start { reason: BootReason::Cold }, t);
    boot(&mut m, t, "a");
    m.on(Ev::RootPrepareSwap { req: "r-4".into(), deadline_s: 1 }, t);
    let a = m.on(Ev::Tick, t.plus(Duration::from_secs(2)));
    assert!(has(&a, |x| matches!(x, Act::Respond { result: Err(e), .. } if e.code.as_str() == "root.busy")));
    assert!(has(&a, |x| matches!(x, Act::Send { frame: ToBody::Resume { .. }, .. })));
    assert_eq!(m.slot, Slot::Running);
    // No body at all: answer at once.
    let mut m = self::m();
    m.on(Ev::Halt(HaltReason::NoLkg), t);
    let a = m.on(Ev::RootPrepareSwap { req: "r-5".into(), deadline_s: 1 }, t);
    assert!(has(&a, |x| matches!(x, Act::Exit(75))));
}

#[test]
fn spawn_refusal_backs_off_without_counting() {
    let mut m = m();
    let t = Now::real();
    m.on(Ev::Start { reason: BootReason::Cold }, t);
    let Slot::Starting { tag, .. } = m.slot.clone() else { panic!() };
    let a = m.on(Ev::Spawned { tag, result: Err("root.quota: instances".into()) }, t);
    assert!(has(&a, |x| matches!(x, Act::Alert { code: "spawn_failed", .. })));
    assert!(matches!(m.slot, Slot::Backoff { .. }));
    assert_eq!(m.counters.crashes_total, 0);
}

/// ADR-009: a workspace change restarts the primary at its next idle point, on the same version,
/// booting `handoff` with the cause; the exit is not a crash.
#[test]
fn restart_waits_for_idle_and_boots_handoff_with_the_cause() {
    let mut m = m();
    let t = Now::real();
    m.on(Ev::Start { reason: BootReason::Cold }, t);
    boot(&mut m, t, "i1");
    m.on(Ev::Busy { inst: "i1".into(), busy: true }, t);
    assert!(m.on(Ev::Restart { cause: "workspace_change".into() }, t).is_empty(), "busy: nothing yet");
    assert_eq!(m.restart_pending.as_deref(), Some("workspace_change"));
    assert!(spawns(&m.on(Ev::Tick, t)).is_empty());
    let a = m.on(Ev::Busy { inst: "i1".into(), busy: false }, t);
    assert!(has(&a, |x| matches!(x, Act::Send { inst, frame: ToBody::Stop { reason, .. } } if inst == "i1" && reason == "restart")), "{a:?}");
    assert!(m.on(Ev::Restart { cause: "workspace_change".into() }, t).is_empty(), "one stop only");
    let a = m.on(Ev::Exited { inst: "i1".into(), code: Some(0), signal: None, disk_low: false, stderr_tail: String::new() }, t);
    let p = &spawns(&a)[0];
    assert_eq!((p.bin, p.reason, p.sha.as_str(), p.cause.as_deref()), (BinSel::Current, BootReason::Handoff, X, Some("workspace_change")));
    assert_eq!(m.counters.crashes_total, 0);
    assert_eq!(m.counters.restarts_since_lkg, 0);
    boot(&mut m, t, "i2");
    assert_eq!(m.slot, Slot::Running);
    assert!(m.restart_pending.is_none());
    assert!(!has(&m.on(Ev::Tick, t.plus(Duration::from_secs(60))), |x| matches!(x, Act::Send { .. } | Act::Kill { .. })));
}

/// The stop is bounded: a body that ignores it is killed, and the restart still follows.
#[test]
fn restart_kills_a_body_that_ignores_the_stop() {
    let mut m = m();
    let t = Now::real();
    m.on(Ev::Start { reason: BootReason::Cold }, t);
    boot(&mut m, t, "i1");
    let a = m.on(Ev::Restart { cause: "workspace_change".into() }, t);
    assert!(has(&a, |x| matches!(x, Act::Send { frame: ToBody::Stop { .. }, .. })), "idle: at once");
    let late = t.plus(Duration::from_millis(init_proto::stdio::STOP_GRACE_MS) + m.cfg.exit_grace() + Duration::from_secs(1));
    let a = m.on(Ev::Tick, late);
    assert!(has(&a, |x| matches!(x, Act::Kill { inst, mode: KillMode::Kill } if inst == "i1")), "{a:?}");
    let a = m.on(Ev::Exited { inst: "i1".into(), code: None, signal: Some(9), disk_low: false, stderr_tail: String::new() }, late);
    let p = &spawns(&a)[0];
    assert_eq!((p.reason, p.cause.as_deref()), (BootReason::Handoff, Some("workspace_change")));
    assert_eq!(m.counters.crashes_total, 0);
}

/// A restart asked while no body runs is dropped by the next spawn, which reads the new
/// environment anyway; one asked while the body is starting waits until it is ready.
#[test]
fn restart_without_a_running_body() {
    let mut m = m();
    let t = Now::real();
    m.on(Ev::Start { reason: BootReason::Cold }, t);
    assert!(m.on(Ev::Restart { cause: "workspace_change".into() }, t).is_empty(), "starting");
    let a = boot(&mut m, t, "i1");
    assert!(has(&a, |x| matches!(x, Act::Send { frame: ToBody::Stop { .. }, .. })), "ready and idle: the restart begins");
    let mut m2 = self::m();
    m2.on(Ev::Start { reason: BootReason::Cold }, t);
    boot(&mut m2, t, "j1");
    m2.on(Ev::OwnerStop, t);
    m2.on(Ev::Exited { inst: "j1".into(), code: Some(0), signal: None, disk_low: false, stderr_tail: String::new() }, t);
    assert_eq!(m2.slot, Slot::Stopped);
    assert!(m2.on(Ev::Restart { cause: "workspace_change".into() }, t).is_empty());
    let a = m2.on(Ev::OwnerResume, t);
    assert_eq!(spawns(&a)[0].reason, BootReason::Cold);
    assert!(m2.restart_pending.is_none());
}
