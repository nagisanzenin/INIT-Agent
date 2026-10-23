//! The kernel as a process (`sv-kernel supervise --under-root`, fds 3 and 4): SV-33, exit codes
//! (§3.13.5), and a kill -9 of the kernel by the PID this test holds (the fake Root then stops
//! its own children by their held handles; no sweep runs).

mod common;

use common::*;
use init_proto::root_link::*;
use std::time::Duration;

const T: Duration = Duration::from_secs(20);
/// The watchdog bound of every test here (K-FLAKE: 3-10x its normal runtime; init_testkit::watchdog).
const WD: Duration = Duration::from_secs(30);

#[test]
fn sv33_refuses_without_fds_and_exits_78_on_a_bad_config_before_ready() {
    let _wd = init_testkit::watchdog::arm("sv33_refuses_without_fds_and_exits_78_on_a_bad_config_before_ready", WD);
    let home = make_home("p-sv33", "");
    let mut c = spawn_kernel(&home, None, &[], &["supervise", "--under-root"]);
    assert_eq!(wait_exit(&mut c, T).code(), Some(64), "no fds 3/4 → refuse");
    let mut c = spawn_kernel(&home, None, &[], &["supervise"]);
    assert_eq!(wait_exit(&mut c, T).code(), Some(64), "no --under-root → refuse");

    std::fs::write(home.join("kernel/config.toml"), "[supervise]\nready_timeout_s = \"ninety\"\n").unwrap();
    let fr = FakeRoot::new(&home, Opts::default());
    let mut k = fr.start_process(&[]);
    assert_eq!(wait_exit(&mut k, T).code(), Some(78));
    let g = fr.seen.0.lock().unwrap();
    assert!(g.hello.is_some() && !g.ready, "exit 78 comes after hello, before ready");
    assert!(g.spawns.is_empty());
    drop(g);
    remove_home(&home);
}

#[test]
fn process_kernel_relays_and_exits_0_on_stop_and_75_on_prepare_swap() {
    let _wd = init_testkit::watchdog::arm("process_kernel_relays_and_exits_0_on_stop_and_75_on_prepare_swap", WD);
    let home = make_home("p-exit", "");
    let fr = FakeRoot::new(&home, Opts::default());
    let mut k = fr.start_process(&[]);
    fr.wait_for("ready", T, |s| s.ready.then_some(()));
    fr.owner_line("hi from the owner");
    fr.wait_for("echo", T, |s| s.events.iter().any(|e| e.data["frame"]["text"] == "echo: hi from the owner").then_some(()));
    let r = fr.request(&RootRequest::Stop(StopArgs { reason: "t".into(), grace_ms: 2000 }), T).unwrap();
    assert!(r.is_ok());
    assert_eq!(wait_exit(&mut k, T).code(), Some(0));

    let fr = FakeRoot::new(&home, Opts::default());
    let mut k = fr.start_process(&[]);
    fr.wait_for("ready", T, |s| s.ready.then_some(()));
    fr.wait_for("body ready", T, |s| s.events.iter().any(|e| e.data["frame"]["text"].as_str().is_some_and(|t| t.starts_with("ready"))).then_some(()));
    let r = fr.request(&RootRequest::PrepareSwap(PrepareSwapArgs { job: "s-1".into(), deadline_s: 5 }), T).unwrap();
    assert!(r.is_ok(), "{r:?}");
    assert_eq!(wait_exit(&mut k, Duration::from_secs(10)).code(), Some(75), "exit 75 within 10 s of answering");
    remove_home(&home);
}

#[test]
fn kernel_killed_9_restarts_with_exactly_one_body_and_bin_intact() {
    let _wd = init_testkit::watchdog::arm("kernel_killed_9_restarts_with_exactly_one_body_and_bin_intact", WD);
    let home = make_home("p-kill9", "");
    let fr = FakeRoot::new(&home, Opts::default());
    let mut k = fr.start_process(&[]);
    fr.wait_for("body ready", T, |s| s.events.iter().any(|e| e.data["frame"]["text"].as_str().is_some_and(|t| t.starts_with("ready"))).then_some(()));
    fr.owner_line("before the kill");
    fr.wait_for("echo", T, |s| s.events.iter().any(|e| e.data["frame"]["text"] == "echo: before the kill").then_some(()));
    // kill -9 the kernel by the PID this test holds; the "Root" then stops its own children.
    k.kill().unwrap();
    let _ = k.wait();
    fr.kill_all();
    let fr2 = FakeRoot::new(&home, Opts::default());
    let mut k2 = fr2.start_process(&[]);
    fr2.wait_for("body ready again", T, |s| s.events.iter().any(|e| e.data["frame"]["text"].as_str().is_some_and(|t| t.starts_with("ready"))).then_some(()));
    assert_eq!(fr2.live_instances(), 1, "exactly one body");
    assert_eq!(fr2.seen.0.lock().unwrap().spawns[0].env["INIT_BOOT_REASON"], "cold");
    let sup: serde_json::Value = serde_json::from_slice(&std::fs::read(home.join("state/kernel/supervisor.json")).unwrap()).unwrap();
    assert_eq!(sup["binaries"]["current"]["sha256"], sha256_file(&home.join("bin/init-body")));
    assert_eq!(session_log(&home).iter().filter(|e| e["text"] == "before the kill").count(), 1);
    fr2.request(&RootRequest::Stop(StopArgs { reason: "t".into(), grace_ms: 2000 }), T).unwrap();
    assert_eq!(wait_exit(&mut k2, T).code(), Some(0));
    remove_home(&home);
}
