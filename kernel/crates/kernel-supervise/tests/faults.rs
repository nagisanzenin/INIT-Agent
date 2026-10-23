//! `INIT_FAULT` points of supervise.md §4.10 (testing.md §3.8.1). Run with
//! `cargo test -p kernel-supervise --features fault-injection --test faults`.
//! Process-level faults kill only the `sv-kernel` child this test spawned (by its handle); the
//! fake Root then stops its own children by held handle. No sweep runs.
#![cfg(feature = "fault-injection")]

mod common;

use common::*;
use init_proto::root_link::*;
use std::time::Duration;

const T: Duration = Duration::from_secs(30);
/// The watchdog bound of every test here (K-FLAKE: 3-10x its normal runtime; init_testkit::watchdog).
const WD: Duration = Duration::from_secs(30);

fn body_ready(fr: &FakeRoot) {
    fr.wait_for("body ready", T, |s| s.events.iter().any(|e| e.data["frame"]["text"].as_str().is_some_and(|t| t.starts_with("ready"))).then_some(()));
}

#[test]
fn sv36_abort_after_outbox_before_response_resends_once_with_the_same_key() {
    let _wd = init_testkit::watchdog::arm("sv36_abort_after_outbox_before_response_resends_once_with_the_same_key", WD);
    let home = make_home("f-sv36", "");
    let fr = FakeRoot::new(&home, Opts::default());
    let mut k = fr.start_process(&[("INIT_FAULT", "supervise.root.after_outbox_before_response:abort")]);
    let st = wait_exit(&mut k, T);
    assert!(!st.success(), "aborted at the fault point");
    fr.kill_all();
    let outbox = jsonl(&home.join("state/kernel/root-outbox.jsonl"));
    let first = outbox.iter().find(|l| l["kind"] == "kernel_start").expect("recorded before the abort").clone();
    assert!(fr.seen.0.lock().unwrap().lineage.iter().all(|l| l.idem != first["idem"].as_str().unwrap()), "never sent");
    // Restart without the fault: resent exactly once, with its original idem.
    let fr2 = FakeRoot::new(&home, Opts::default());
    let mut k2 = fr2.start_process(&[]);
    body_ready(&fr2);
    std::thread::sleep(Duration::from_millis(500));
    let idem = first["idem"].as_str().unwrap();
    let n = fr2.seen.0.lock().unwrap().lineage.iter().filter(|l| l.idem == idem).count();
    assert_eq!(n, 1);
    fr2.request(&RootRequest::Stop(StopArgs { reason: "t".into(), grace_ms: 2000 }), T).unwrap();
    assert_eq!(wait_exit(&mut k2, T).code(), Some(0));
    remove_home(&home);
}

#[test]
fn abort_after_cold_sweep_then_clean_restart_runs_exactly_one_body() {
    let _wd = init_testkit::watchdog::arm("abort_after_cold_sweep_then_clean_restart_runs_exactly_one_body", WD);
    let home = make_home("f-cold", "");
    let fr = FakeRoot::new(&home, Opts::default());
    let mut k = fr.start_process(&[("INIT_FAULT", "supervise.cold.after_sweep:abort")]);
    assert!(!wait_exit(&mut k, T).success());
    assert_eq!(fr.spawn_count(), 0, "aborted before any spawn");
    let fr2 = FakeRoot::new(&home, Opts::default());
    let mut k2 = fr2.start_process(&[]);
    body_ready(&fr2);
    assert_eq!(fr2.live_instances(), 1);
    assert!(home.join("bin/init-body").exists());
    fr2.request(&RootRequest::Stop(StopArgs { reason: "t".into(), grace_ms: 2000 }), T).unwrap();
    assert_eq!(wait_exit(&mut k2, T).code(), Some(0));
    remove_home(&home);
}
