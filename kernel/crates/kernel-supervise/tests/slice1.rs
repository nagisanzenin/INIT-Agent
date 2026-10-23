//! Slice-1: the kernel under the Root (supervise.md §3.4, §3.5, §3.7, §3.10, §3.13). The fake Root
//! is in-test (tests/common); the body is `sv-testbody`.

mod common;

use common::*;
use init_proto::root_link::*;
use serde_json::json;
use std::time::Duration;

const T: Duration = Duration::from_secs(20);
/// The watchdog bound of every test here (K-FLAKE: 3-10x its normal runtime; init_testkit::watchdog).
const WD: Duration = Duration::from_secs(30);

fn running(fr: &FakeRoot) {
    fr.wait_for("ready + first display", T, |s| (s.ready && s.events.iter().any(|e| e.ev == "owner.out" && e.data["frame"]["text"].as_str().is_some_and(|t| t.starts_with("ready")))).then_some(()));
}

#[test]
fn slice1_owner_message_reaches_the_body_and_the_reply_comes_back() {
    let _wd = init_testkit::watchdog::arm("slice1_owner_message_reaches_the_body_and_the_reply_comes_back", WD);
    let home = make_home("s1-relay", "");
    let fr = FakeRoot::new(&home, Opts::default());
    let (h, code) = fr.start_in_process();
    running(&fr);

    // Handshake and spawn shape (§3.13.1, §3.4).
    let (hello, spawn) = fr.wait_for("hello", T, |s| Some((s.hello.clone()?, s.spawns.first().cloned()?)));
    assert_eq!(hello.proto, 1);
    assert_eq!((spawn.kind.as_str(), spawn.profile.as_str(), spawn.lane.as_deref()), ("body", "body", Some("b0")));
    assert_eq!(spawn.program, home.join("bin/init-body").to_string_lossy());
    assert_eq!(spawn.env["INIT_UI"], "stdio-jsonl/1");
    assert_eq!(spawn.env["INIT_BOOT_REASON"], "cold");
    assert_eq!(spawn.env["INIT_SHA"], X);
    assert_eq!(spawn.env["INIT_TEST"], "1", "test_mode passes INIT_TEST");
    assert!(spawn.env.keys().all(|k| !k.starts_with("INIT_ROOT") && !k.contains("API_KEY")), "SV-24");
    assert_eq!(spawn.params["BODY"], home.join("body").to_string_lossy());
    assert!(!spawn.params.contains_key("MARK"));
    let boot: serde_json::Value = serde_json::from_slice(&std::fs::read(home.join("state/kernel/boot.json")).unwrap()).unwrap();
    assert_eq!(boot["boot_id"], spawn.env["INIT_BOOT_ID"].as_str());
    assert_eq!(boot["session"], spawn.env["INIT_SESSION"].as_str());

    // owner.in → ok only once durable → echo → the body acks and replies.
    let r = fr.owner_line("hello body");
    assert!(r.is_ok());
    fr.wait_for("echo display", T, |s| s.events.iter().any(|e| e.data["frame"]["text"] == "echo: hello body").then_some(()));
    let out = fr.owner_out();
    assert!(out.iter().any(|f| f["t"] == "echo" && f["text"] == "hello body"));
    let log = session_log(&home);
    assert_eq!(log.iter().filter(|e| e["text"] == "hello body").count(), 1);
    // The ack drained the inbox.
    let inbox = jsonl(&home.join("state/kernel/inbox.jsonl"));
    assert!(inbox.iter().any(|l| l["t"] == "ack"));
    // A duplicate qid is answered ok and not delivered again.
    fr.owner_line_qid("same", "once");
    fr.owner_line_qid("same", "once");
    fr.wait_for("once", T, |s| s.events.iter().any(|e| e.data["frame"]["text"] == "echo: once").then_some(()));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(session_log(&home).iter().filter(|e| e["text"] == "once").count(), 1);

    // hb events, the kernel_start record with an idem key (§3.13.3), supervisor.json.
    fr.wait_for("hb", T, |s| s.events.iter().any(|e| e.ev == "hb" && e.data["slot"] == "running").then_some(()));
    let ks = fr.wait_for("kernel_start", T, |s| s.lineage.iter().find(|l| l.kind == "kernel_start").cloned());
    let (kboot, n) = ks.idem.split_once(':').unwrap();
    assert_eq!((kboot.len(), n.parse::<u64>().is_ok()), (16, true));
    let sup: serde_json::Value = serde_json::from_slice(&std::fs::read(home.join("state/kernel/supervisor.json")).unwrap()).unwrap();
    assert_eq!(sup["slot"], "running");
    assert!(h.snapshot().unwrap().primary.is_some());

    // /status and /kernel answer as notices; /later is a follow-up input (SV-31).
    fr.owner_line("/status");
    fr.owner_line("/later after this");
    fr.wait_for("later", T, |s| s.events.iter().any(|e| e.data["frame"]["text"] == "echo: after this").then_some(()));
    let inbox = jsonl(&home.join("state/kernel/inbox.jsonl"));
    assert!(inbox.iter().any(|l| l["t"] == "input" && l["text"] == "after this" && l["mode"] == "follow_up"));

    // SV-17: `/panic` reaching the kernel is an ordinary input (the Root must catch it).
    fr.owner_line("/panic");
    fr.wait_for("/panic relayed", T, |s| s.events.iter().any(|e| e.data["frame"]["text"] == "echo: /panic").then_some(()));
    // SV-30/SV-38: optional signals are recorded and gate nothing.
    fr.owner_line("/accept task-1");
    fr.owner_line("/bless j-000009");
    fr.wait_for("owner_signal records", T, |s| (s.lineage.iter().filter(|l| l.kind == "owner_signal").count() == 2).then_some(()));
    fr.wait_for("accepted notice acked", T, |_| jsonl(&home.join("state/kernel/inbox.jsonl")).iter().any(|l| l["t"] == "notice_ack").then_some(()));
    assert!(session_log(&home).iter().any(|e| e["type"] == "notice" && e["kind"] == "accepted"));
    assert_eq!(h.snapshot().unwrap().slot, kernel_supervise::machine::Slot::Running);

    // Root stop: the body gets `stop`, exits 0, the kernel answers ok and exits 0.
    let r = fr.request(&RootRequest::Stop(StopArgs { reason: "test".into(), grace_ms: 3000 }), T).unwrap();
    assert!(r.is_ok());
    assert_eq!(code.recv_timeout(T).unwrap(), 0);
    assert_eq!(fr.live_instances(), 0);
    remove_home(&home);
}

#[test]
fn f_crash_boot_c_lkg_crash_loop_enters_body_safe_mode_then_halts() {
    let _wd = init_testkit::watchdog::arm("f_crash_boot_c_lkg_crash_loop_enters_body_safe_mode_then_halts", Duration::from_secs(90));
    let home = make_home("s1-safe", "crash_window_s = 60\n");
    set_plan(&home, json!({"*": {"crash_before_ready": true}}));
    let fr = FakeRoot::new(&home, Opts::default());
    let (_h, code) = fr.start_in_process();
    // 5 crashes on LKG → the 6th spawn is LKG in safe mode, body_safe reported.
    let safe = fr.wait_for("safe spawn", T, |s| s.spawns.iter().position(|a| a.env["INIT_MODE"] == "safe"));
    assert_eq!(safe, 5);
    {
        let g = fr.seen.0.lock().unwrap();
        assert!(g.spawns[..5].iter().all(|a| a.env["INIT_MODE"] == "normal"));
        assert_eq!(g.spawns[1].env["INIT_BOOT_REASON"], "crash");
        assert!(g.spawns[5].program.ends_with("bin/init-body.lkg"));
        let bs = g.lineage.iter().find(|l| l.kind == "body_safe").expect("body_safe reported");
        assert_eq!(bs.data["on"], true);
        assert!(g.alerts.iter().any(|a| a.code == "safe_entered"));
    }
    let alerts = jsonl(&home.join("state/kernel/alerts.jsonl"));
    assert!(alerts.iter().any(|a| a["code"] == "safe_entered" && a["source"] == "supervise"), "SV-3");
    let boot: serde_json::Value = serde_json::from_slice(&std::fs::read(home.join("state/kernel/boot.json")).unwrap()).unwrap();
    assert_eq!(boot["mode"], "safe");
    assert_eq!(boot["crash"]["exit_code"], 101);
    assert!(boot["crash"]["log_tail"].as_str().unwrap().contains("crash_before_ready"));
    // SV-4: 3 crashes in safe → Halted{crash_loop_safe}; no more spawns.
    fr.wait_for("halted", T, |s| s.alerts.iter().any(|a| a.code == "halted").then_some(()));
    let n = fr.spawn_count();
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(fr.spawn_count(), n, "no body while halted");
    assert_eq!(n, 8);
    // Owner /resume → safe again with fresh counters (the plan now lets the body live).
    set_plan(&home, json!({}));
    fr.owner_line("/resume");
    running(&fr);
    assert_eq!(fr.seen.0.lock().unwrap().spawns.last().unwrap().env["INIT_MODE"], "safe");
    fr.request(&RootRequest::Stop(StopArgs { reason: "test".into(), grace_ms: 2000 }), T).unwrap();
    assert_eq!(code.recv_timeout(T).unwrap(), 0);
    remove_home(&home);
}

/// K-RACE (final CI run 2): a body that dies at once can be reported (`exited`) before its spawn
/// answer reaches the supervisor loop, whether the Root sent the event first or the answer was
/// still on its way through the spawn thread. That exit is the crash. It used to be dropped as an
/// unknown instance's: the slot sat in `starting` until the ready timeout, killed an instance the
/// Root no longer had, and waited for an `exited` that had already come (no 3rd spawn for 20 s).
#[test]
fn k_race_exit_reported_before_the_spawn_answer_is_still_the_crash() {
    let _wd = init_testkit::watchdog::arm("k_race_exit_reported_before_the_spawn_answer_is_still_the_crash", WD);
    let home = make_home("s1-race", "crash_window_s = 60\n");
    set_plan(&home, json!({"*": {"crash_before_ready": true}}));
    let fr = FakeRoot::new(&home, Opts { exited_first: true, ..Opts::default() });
    let (_h, code) = fr.start_in_process();
    // Every exit precedes its spawn answer; the run is F-crash-boot (c) all the same.
    let safe = fr.wait_for("safe spawn", T, |s| s.spawns.iter().position(|a| a.env["INIT_MODE"] == "safe"));
    assert_eq!(safe, 5);
    fr.wait_for("halted", T, |s| s.alerts.iter().any(|a| a.code == "halted").then_some(()));
    {
        let g = fr.seen.0.lock().unwrap();
        assert_eq!(g.spawns.len(), 8);
        assert!(g.spawns[1..].iter().all(|a| a.env["INIT_BOOT_REASON"] == "crash"));
        // No ready-timeout kill: each early exit was taken as the crash.
        assert!(g.kills.is_empty(), "{:?}", g.kills);
    }
    // The crash record still carries the dead body's stderr.
    let boot: serde_json::Value = serde_json::from_slice(&std::fs::read(home.join("state/kernel/boot.json")).unwrap()).unwrap();
    assert_eq!(boot["crash"]["exit_code"], 101);
    assert!(boot["crash"]["log_tail"].as_str().unwrap().contains("crash_before_ready"), "{boot}");
    fr.request(&RootRequest::Stop(StopArgs { reason: "test".into(), grace_ms: 2000 }), T).unwrap();
    assert_eq!(code.recv_timeout(T).unwrap(), 0);
    remove_home(&home);
}

#[test]
fn f_crash_boot_d_exit_78_twice_halts_config() {
    let _wd = init_testkit::watchdog::arm("f_crash_boot_d_exit_78_twice_halts_config", WD);
    let home = make_home("s1-cfg", "");
    set_plan(&home, json!({"*": {"exit_before_ready": 78}}));
    let fr = FakeRoot::new(&home, Opts::default());
    let (h, code) = fr.start_in_process();
    fr.wait_for("config_error", T, |s| s.alerts.iter().any(|a| a.code == "config_error").then_some(()));
    let snap = h.snapshot().unwrap();
    assert_eq!(snap.counters.crashes_total, 0, "SV-28: not counted as crashes");
    assert_eq!(fr.spawn_count(), 2);
    assert!(fr.lineage_kinds().contains(&"halted".to_string()));
    fr.request(&RootRequest::Stop(StopArgs { reason: "t".into(), grace_ms: 1000 }), T).unwrap();
    assert_eq!(code.recv_timeout(T).unwrap(), 0);
    remove_home(&home);
}

#[test]
fn f_hang_b_stale_heartbeat_is_killed_and_restarted_as_a_crash() {
    let _wd = init_testkit::watchdog::arm("f_hang_b_stale_heartbeat_is_killed_and_restarted_as_a_crash", Duration::from_secs(70));
    let home = make_home("s1-hang", "heartbeat_stale_s = 2\n");
    set_plan(&home, json!({"*": {"hang_after_ready": true, "hb_ms": 200}}));
    let fr = FakeRoot::new(&home, Opts::default());
    let (h, code) = fr.start_in_process();
    let k = fr.wait_for("kill of the hung body", T, |s| s.kills.first().cloned());
    assert_eq!(k.mode, KillMode::Kill);
    fr.wait_for("respawn", T, |s| (s.spawns.len() >= 2).then_some(()));
    assert_eq!(fr.seen.0.lock().unwrap().spawns[1].env["INIT_BOOT_REASON"], "crash");
    assert!(h.snapshot().unwrap().counters.crashes_total >= 1);
    fr.request(&RootRequest::Stop(StopArgs { reason: "t".into(), grace_ms: 1000 }), T).unwrap();
    assert_eq!(code.recv_timeout(T).unwrap(), 0);
    remove_home(&home);
}

#[test]
fn sv15_input_before_ack_is_redelivered_once() {
    let _wd = init_testkit::watchdog::arm("sv15_input_before_ack_is_redelivered_once", WD);
    let home = make_home("s1-redeliver", "");
    // Boot #1 crashes on its first input, before appending or acking it.
    set_plan(&home, json!({"*": {"crash_on_input_boots": 1}}));
    let fr = FakeRoot::new(&home, Opts::default());
    let (h, code) = fr.start_in_process();
    running(&fr);
    fr.owner_line("survive me");
    fr.wait_for("redelivered to the next body", T, |s| s.events.iter().any(|e| e.data["frame"]["text"] == "echo: survive me").then_some(()));
    assert_eq!(fr.spawn_count(), 2);
    assert_eq!(session_log(&home).iter().filter(|e| e["text"] == "survive me").count(), 1, "exactly once in the log");
    assert_eq!(h.snapshot().unwrap().counters.crashes_total, 1);
    fr.request(&RootRequest::Stop(StopArgs { reason: "t".into(), grace_ms: 1000 }), T).unwrap();
    assert_eq!(code.recv_timeout(T).unwrap(), 0);
    remove_home(&home);
}

#[test]
fn owner_stop_then_line_restarts_cold_and_queued_input_arrives() {
    let _wd = init_testkit::watchdog::arm("owner_stop_then_line_restarts_cold_and_queued_input_arrives", WD);
    let home = make_home("s1-stop", "");
    let fr = FakeRoot::new(&home, Opts::default());
    let (h, code) = fr.start_in_process();
    running(&fr);
    fr.owner_line("/stop");
    fr.wait_for("body exit 0", T, |s| s.exits.iter().any(|x| x.code == Some(0)).then_some(()));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(h.snapshot().unwrap().slot, kernel_supervise::machine::Slot::Stopped);
    // SV-14 shape: a line while stopped is queued, starts a cold body, and is delivered once.
    fr.owner_line("wake up");
    fr.wait_for("woken", T, |s| s.events.iter().any(|e| e.data["frame"]["text"] == "echo: wake up").then_some(()));
    assert_eq!(fr.seen.0.lock().unwrap().spawns.last().unwrap().env["INIT_BOOT_REASON"], "cold");
    assert_eq!(h.snapshot().unwrap().counters.crashes_total, 0);
    fr.request(&RootRequest::Stop(StopArgs { reason: "t".into(), grace_ms: 1000 }), T).unwrap();
    assert_eq!(code.recv_timeout(T).unwrap(), 0);
    remove_home(&home);
}

#[test]
fn prepare_swap_pauses_stops_and_exits_75() {
    let _wd = init_testkit::watchdog::arm("prepare_swap_pauses_stops_and_exits_75", WD);
    let home = make_home("s1-swap", "");
    let fr = FakeRoot::new(&home, Opts::default());
    let (_h, code) = fr.start_in_process();
    running(&fr);
    let r = fr.request(&RootRequest::PrepareSwap(PrepareSwapArgs { job: "s-000001".into(), deadline_s: 5 }), T).unwrap();
    assert!(r.is_ok(), "{r:?}");
    assert_eq!(code.recv_timeout(T).unwrap(), 75);
    assert!(fr.seen.0.lock().unwrap().exits.iter().any(|x| x.code == Some(0)), "the paused body exits 0 on stop (D11)");
    remove_home(&home);
}

#[test]
fn prepare_swap_with_a_body_that_never_pauses_is_refused_and_the_kernel_keeps_running() {
    let _wd = init_testkit::watchdog::arm("prepare_swap_with_a_body_that_never_pauses_is_refused_and_the_kernel_keeps_running", Duration::from_secs(40));
    let home = make_home("s1-swapbusy", "");
    set_plan(&home, json!({"*": {"ignore_pause": true}}));
    let fr = FakeRoot::new(&home, Opts::default());
    let (h, code) = fr.start_in_process();
    running(&fr);
    let r = fr.request(&RootRequest::PrepareSwap(PrepareSwapArgs { job: "s-1".into(), deadline_s: 1 }), T).unwrap();
    assert_eq!(r.result.unwrap_err().code.as_str(), "root.busy", "F31");
    assert_eq!(h.snapshot().unwrap().slot, kernel_supervise::machine::Slot::Running);
    fr.request(&RootRequest::Stop(StopArgs { reason: "t".into(), grace_ms: 1000 }), T).unwrap();
    assert_eq!(code.recv_timeout(T).unwrap(), 0);
    remove_home(&home);
}

#[test]
fn sv34_control_link_loss_stops_bodies_and_exits_69() {
    let _wd = init_testkit::watchdog::arm("sv34_control_link_loss_stops_bodies_and_exits_69", WD);
    let home = make_home("s1-lost", "");
    let fr = FakeRoot::new(&home, Opts::default());
    let (_h, code) = fr.start_in_process();
    running(&fr);
    let t0 = std::time::Instant::now();
    fr.cut();
    assert_eq!(code.recv_timeout(T).unwrap(), 69);
    assert!(t0.elapsed() < Duration::from_secs(10));
    fr.wait_for("body exit", T, |s| s.exits.iter().any(|x| x.code == Some(0)).then_some(()));
    remove_home(&home);
}

#[test]
fn outbox_resends_unanswered_records_with_their_original_key_after_restart() {
    let _wd = init_testkit::watchdog::arm("outbox_resends_unanswered_records_with_their_original_key_after_restart", WD);
    let home = make_home("s1-outbox", "");
    let fr = FakeRoot::new(&home, Opts { answer_lineage: false, ..Opts::default() });
    let (_h, code) = fr.start_in_process();
    running(&fr);
    let first = fr.wait_for("kernel_start", T, |s| s.lineage.iter().find(|l| l.kind == "kernel_start").cloned());
    fr.request(&RootRequest::Stop(StopArgs { reason: "t".into(), grace_ms: 1000 }), T).unwrap();
    assert_eq!(code.recv_timeout(T).unwrap(), 0);
    // A new kernel on the same home: the unanswered record goes out again with the same idem.
    let fr2 = FakeRoot::new(&home, Opts::default());
    let (_h2, code2) = fr2.start_in_process();
    let again = fr2.wait_for("resend", T, |s| s.lineage.iter().find(|l| l.idem == first.idem).cloned());
    assert_eq!(again.kind, "kernel_start");
    let second = fr2.wait_for("new kernel_start", T, |s| s.lineage.iter().find(|l| l.kind == "kernel_start" && l.idem != first.idem).cloned());
    assert_ne!(second.idem.split(':').next(), first.idem.split(':').next(), "a new kboot per start");
    let n1: u64 = first.idem.split(':').nth(1).unwrap().parse().unwrap();
    let n2: u64 = second.idem.split(':').nth(1).unwrap().parse().unwrap();
    assert!(n2 > n1, "the outbox seq continues across restarts");
    fr2.request(&RootRequest::Stop(StopArgs { reason: "t".into(), grace_ms: 1000 }), T).unwrap();
    assert_eq!(code2.recv_timeout(T).unwrap(), 0);
    remove_home(&home);
}
