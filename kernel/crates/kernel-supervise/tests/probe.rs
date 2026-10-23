//! The staging probe primitive (supervise.md §3.6.3): G2-style probe on a copy of the live log,
//! a G4-style run with `keep_on_stop`, and a candidate that crashes before ready
//! (F-crash-boot (a), SV-32).

mod common;

use common::*;
use init_proto::files::{BootReason, InitMode};
use kernel_supervise::instance::LocalSpawner;
use kernel_supervise::probe::{probe, Candidate, ProbeFail, ProbeOpts, Purpose};
use kernel_supervise::proberoot::SessionSrc;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// The watchdog bound of every test here (K-FLAKE: 3-10x its normal runtime; init_testkit::watchdog).
const WD: Duration = Duration::from_secs(30);

fn cfg() -> kernel_supervise::config::KernelCfg {
    let mut c = kernel_supervise::config::parse("[proxy]\ntest_mode = true\n[supervise]\ngit = \"/usr/bin/git\"\nexit_grace_s = 3\n").unwrap();
    c.sandbox.toolchain = "/usr".into();
    c
}

fn opts(purpose: Purpose, session: SessionSrc) -> ProbeOpts {
    ProbeOpts {
        session,
        mode: if purpose == Purpose::Probe { InitMode::Probe } else { InitMode::Normal },
        boot_reason: if purpose == Purpose::Probe { BootReason::Promote } else { BootReason::Cold },
        purpose,
        job: "j-000007".into(),
        extra_files: vec![],
        cwd: None,
        keep_on_stop: vec![],
    }
}

#[test]
fn g2_probe_on_a_copy_of_the_live_log_reports_and_cleans_up() {
    let _wd = init_testkit::watchdog::arm("g2_probe_on_a_copy_of_the_live_log_reports_and_cleans_up", WD);
    let home = make_home("pr-g2", "");
    let (x, y) = lineage(&home);
    set_plan(&home, json!({ "*": {"hb_ms": 200} }));
    // A live log to copy (the probe must not touch it).
    std::fs::write(home.join("state/sessions/live-1.jsonl"), "{\"id\":\"a\",\"type\":\"boot\"}\n").unwrap();
    let live_before = std::fs::read(home.join("state/sessions/live-1.jsonl")).unwrap();
    let sp: Arc<LocalSpawner> = Arc::new(LocalSpawner::new());
    let c = Candidate { sha: y.clone(), binary: home.join("bin/init-body"), ext_set_dir: None };
    let mut o = opts(Purpose::Probe, SessionSrc::Copy { live_log: home.join("state/sessions/live-1.jsonl"), sid: "live-1".into() });
    o.extra_files = vec![(home.join("work/testbody-plan.json"), PathBuf::from("work/testbody-plan.json"))];
    let h = probe(sp.clone(), &home, &cfg(), &c, o).unwrap();
    let root = h.root().to_path_buf();
    assert!(root.starts_with(home.join("probe")));
    assert!(std::fs::symlink_metadata(root.join("kernel")).unwrap().is_dir(), "D8: a clone, not a symlink");
    let r = h.wait_ready(Duration::from_secs(10)).unwrap();
    assert_eq!(r.sha, y);
    assert!(h.window(Duration::from_millis(1200)), "≥ 2 heartbeats with the probe's boot_id");
    let out = h.stop();
    assert!(out.window_ok && out.ready.is_some() && out.ready_at.is_some());
    assert_eq!(out.last_boot_event.unwrap()["sha"], y.as_str());
    let dir = out.outputs.expect("copied for the agent");
    assert_eq!(dir, home.join(format!("state/kernel/probe/{}", &y[..12])));
    assert!(dir.join("stdout.jsonl").exists() && dir.join("exit.json").exists() && dir.join("ready").exists());
    assert!(!root.exists(), "probe root deleted");
    assert_eq!(std::fs::read(home.join("state/sessions/live-1.jsonl")).unwrap(), live_before, "live log untouched");
    let _ = x;
    remove_home(&home);
}

#[test]
fn g4_run_keeps_the_task_workspace_and_copies_nothing_for_the_agent() {
    let _wd = init_testkit::watchdog::arm("g4_run_keeps_the_task_workspace_and_copies_nothing_for_the_agent", WD);
    let home = make_home("pr-g4", "");
    let (_x, y) = lineage(&home);
    let task = home.join("cache/task-src");
    std::fs::create_dir_all(&task).unwrap();
    std::fs::write(task.join("prompt.md"), "do it\n").unwrap();
    let sp: Arc<LocalSpawner> = Arc::new(LocalSpawner::new());
    let c = Candidate { sha: y.clone(), binary: home.join("bin/init-body"), ext_set_dir: None };
    let mut o = opts(Purpose::Run, SessionSrc::Fresh);
    o.extra_files = vec![(task.clone(), PathBuf::from("work/task"))];
    o.cwd = Some(PathBuf::from("work/task"));
    o.keep_on_stop = vec![(PathBuf::from("work/task"), PathBuf::from("task-1"))];
    let h = probe(sp, &home, &cfg(), &c, o).unwrap();
    h.wait_ready(Duration::from_secs(10)).unwrap();
    h.send_input("hello task");
    std::thread::sleep(Duration::from_millis(300));
    let out = h.stop();
    assert_eq!(out.kept, vec![home.join("cache/gate/j-000007/task-1")]);
    assert_eq!(std::fs::read_to_string(home.join("cache/gate/j-000007/task-1/prompt.md")).unwrap(), "do it\n");
    assert!(out.outputs.is_none(), "SV-32: nothing agent-visible for G4 runs");
    assert!(!home.join("state/kernel/probe").join(&y[..12]).exists());
    assert!(std::fs::read_dir(home.join("probe")).unwrap().next().is_none(), "probe roots deleted");
    remove_home(&home);
}

#[test]
fn f_crash_boot_a_candidate_crashes_in_the_probe() {
    let _wd = init_testkit::watchdog::arm("f_crash_boot_a_candidate_crashes_in_the_probe", WD);
    let home = make_home("pr-crash", "");
    let (_x, y) = lineage(&home);
    set_plan(&home, json!({ &y[..12]: {"crash_before_ready": true} }));
    let sp: Arc<LocalSpawner> = Arc::new(LocalSpawner::new());
    let c = Candidate { sha: y.clone(), binary: home.join("bin/init-body"), ext_set_dir: None };
    let mut o = opts(Purpose::Probe, SessionSrc::Fresh);
    o.extra_files = vec![(home.join("work/testbody-plan.json"), PathBuf::from("work/testbody-plan.json"))];
    let h = probe(sp, &home, &cfg(), &c, o).unwrap();
    match h.wait_ready(Duration::from_secs(10)) {
        Err(ProbeFail::Exit(e)) => assert_eq!(e.code, Some(101)),
        other => panic!("{other:?}"),
    }
    let out = h.stop();
    let err = std::fs::read_to_string(out.outputs.unwrap().join("stderr.log")).unwrap();
    assert!(err.contains("crash_before_ready"));
    remove_home(&home);
}
