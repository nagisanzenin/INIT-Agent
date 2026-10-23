//! Cold start and recovery (supervise.md §3.9): bin/ repair (SV-22), restore from the Root's
//! tuple after a whole-system rollback (SV-35) and after a corrupt LKG (F15), interrupted
//! handoffs (§3.9 step 9), and the `panic_resume` boot record.

mod common;

use common::*;
use init_proto::root_link::*;
use kernel_supervise::machine::HandoffKind;
use kernel_supervise::supervisor::Cmd;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

const T: Duration = Duration::from_secs(30);
/// The watchdog bound of every test here (K-FLAKE: 3-10x its normal runtime; init_testkit::watchdog).
const WD: Duration = Duration::from_secs(100);

fn body_ready(fr: &FakeRoot) {
    fr.wait_for("body ready", T, |s| s.events.iter().any(|e| e.data["frame"]["text"].as_str().is_some_and(|t| t.starts_with("ready"))).then_some(()));
}

fn stop(fr: &FakeRoot, code: mpsc::Receiver<i32>) {
    fr.request(&RootRequest::Stop(StopArgs { reason: "t".into(), grace_ms: 2000 }), T).unwrap();
    assert_eq!(code.recv_timeout(T).unwrap(), 0);
}

fn read_json(p: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
}

/// Run once and promote to Y; leaves the home provisional on Y.
fn promoted_home(name: &str) -> (std::path::PathBuf, String, String) {
    let home = make_home(name, "");
    let (x, y) = lineage(&home);
    std::fs::copy(testbody(), home.join("bin/init-body.staged")).unwrap();
    let fr = FakeRoot::new(&home, Opts { tuple_sha: x.clone(), ..Opts::default() });
    let (h, code) = fr.start_in_process();
    body_ready(&fr);
    let (tx, rx) = mpsc::channel();
    let staged_sha256 = sha256_file(&home.join("bin/init-body.staged"));
    h.send(Cmd::Handoff { kind: HandoffKind::Promote { job: "j-1".into(), sha: y.clone(), staged_sha256, branch: None }, reply: Some(tx) });
    assert_eq!(rx.recv_timeout(T).unwrap(), Ok(()));
    stop(&fr, code);
    (home, x, y)
}

#[test]
fn sv22_missing_init_body_is_restored_from_prev_before_spawn() {
    let _wd = init_testkit::watchdog::arm("sv22_missing_init_body_is_restored_from_prev_before_spawn", Duration::from_secs(30));
    let home = make_home("r-sv22", "");
    std::fs::copy(testbody(), home.join("bin/init-body.prev")).unwrap();
    std::fs::remove_file(home.join("bin/init-body")).unwrap();
    let fr = FakeRoot::new(&home, Opts::default());
    let (_h, code) = fr.start_in_process();
    body_ready(&fr);
    assert!(home.join("bin/init-body").exists());
    assert!(!home.join("bin/init-body.prev").exists(), "renamed, not copied");
    stop(&fr, code);
    remove_home(&home);
}

#[test]
fn sv35_rollback_system_restores_bin_and_lineage_from_the_tuple() {
    let _wd = init_testkit::watchdog::arm("sv35_rollback_system_restores_bin_and_lineage_from_the_tuple", WD);
    let (home, x, y) = promoted_home("r-sv35");
    // The Root rolled the system back to tuple X; the local files are newer or damaged.
    let pristine = home.join("cache/pristine-body");
    std::fs::copy(testbody(), &pristine).unwrap();
    let tuple_bin = sha256_file(&pristine);
    std::fs::remove_file(home.join("bin/init-body")).unwrap();
    std::fs::write(home.join("bin/init-body"), b"#!/bin/sh\nexit 1\n").unwrap();
    std::fs::remove_file(home.join("bin/init-body.lkg")).unwrap();
    std::fs::write(home.join("bin/init-body.lkg"), b"garbage").unwrap();
    let before_ino = std::fs::metadata(home.join("bin/init-body")).unwrap().ino();
    let fr = FakeRoot::new(&home, Opts { reason: ZoneBootReason::RollbackSystem, tuple_sha: x.clone(), tuple_binary: tuple_bin.clone(), store: BTreeMap::from([(tuple_bin.clone(), pristine)]), ..Opts::default() });
    let (h, code) = fr.start_in_process();
    body_ready(&fr);
    assert_eq!(sha256_file(&home.join("bin/init-body")), tuple_bin);
    assert_eq!(sha256_file(&home.join("bin/init-body.lkg")), tuple_bin);
    assert_ne!(std::fs::metadata(home.join("bin/init-body")).unwrap().ino(), before_ino, "a new inode");
    assert_eq!(main_of(&home), x);
    assert_eq!(git(&home.join("lineage/body.git"), &home, &["rev-parse", &format!("refs/init/rolled-back/{y}")]), y);
    assert_eq!(read_json(&home.join("state/kernel/lkg.json"))["reason"], "system_rollback");
    assert_eq!(read_json(&home.join("state/kernel/update-failed.json"))["stage"], "system_rollback");
    let spawn = fr.seen.0.lock().unwrap().spawns[0].clone();
    assert_eq!((spawn.env["INIT_BOOT_REASON"].as_str(), spawn.env["INIT_SHA"].as_str()), ("rollback", x.as_str()));
    let boot = read_json(&home.join("state/kernel/boot.json"));
    assert_eq!(boot["zone"]["boot_reason"], "rollback_system");
    assert_eq!(boot["update_failed"]["stage"], "system_rollback");
    assert_eq!(h.snapshot().unwrap().mode, kernel_supervise::machine::Mode::Normal);
    stop(&fr, code);
    remove_home(&home);
}

#[test]
fn f15_corrupt_lkg_is_fetched_from_the_store_on_a_cold_start() {
    let _wd = init_testkit::watchdog::arm("f15_corrupt_lkg_is_fetched_from_the_store_on_a_cold_start", Duration::from_secs(30));
    let home = make_home("r-f15", "");
    let pristine = home.join("cache/pristine-body");
    std::fs::copy(testbody(), &pristine).unwrap();
    let tuple_bin = sha256_file(&pristine);
    std::fs::remove_file(home.join("bin/init-body.lkg")).unwrap();
    std::fs::write(home.join("bin/init-body.lkg"), b"corrupt").unwrap();
    let fr = FakeRoot::new(&home, Opts { tuple_binary: tuple_bin.clone(), store: BTreeMap::from([(tuple_bin.clone(), pristine)]), ..Opts::default() });
    let (_h, code) = fr.start_in_process();
    body_ready(&fr);
    assert_eq!(sha256_file(&home.join("bin/init-body.lkg")), tuple_bin);
    assert_eq!(fr.seen.0.lock().unwrap().spawns[0].env["INIT_BOOT_REASON"], "cold");
    stop(&fr, code);
    remove_home(&home);
}

/// Rewrite `supervisor.json.handoff`.
fn set_handoff(home: &Path, phase: &str, from: &str, to: &str, staged: &str) {
    let p = home.join("state/kernel/supervisor.json");
    let mut v = read_json(&p);
    v["handoff"] = json!({"bid": "b0", "from": from, "new_instance": null, "old_instance": "dead00000000", "phase": phase,
                          "staged_sha256": staged, "started_at": "2026-10-20T00:00:00.000Z", "to": to, "token": "t"});
    std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
}

#[test]
fn interrupted_handoff_committing_is_finished_and_pausing_is_discarded() {
    let _wd = init_testkit::watchdog::arm("interrupted_handoff_committing_is_finished_and_pausing_is_discarded", WD);
    // Committing: finish the commit (each step idempotent).
    let home = make_home("r-commit", "");
    let (x, y) = lineage(&home);
    let fr = FakeRoot::new(&home, Opts { tuple_sha: x.clone(), ..Opts::default() });
    let (_h, code) = fr.start_in_process();
    body_ready(&fr);
    stop(&fr, code);
    std::fs::copy(testbody(), home.join("bin/init-body.staged")).unwrap();
    let staged_ino = std::fs::metadata(home.join("bin/init-body.staged")).unwrap().ino();
    set_handoff(&home, "committing", &x, &y, &sha256_file(&home.join("bin/init-body.staged")));
    let fr = FakeRoot::new(&home, Opts { tuple_sha: x.clone(), ..Opts::default() });
    let (h, code) = fr.start_in_process();
    body_ready(&fr);
    assert_eq!(std::fs::metadata(home.join("bin/init-body")).unwrap().ino(), staged_ino);
    assert_eq!(main_of(&home), y);
    assert_eq!(h.snapshot().unwrap().current.sha, y);
    assert_eq!(fr.seen.0.lock().unwrap().spawns[0].env["INIT_SHA"], y.as_str());
    assert!(fr.lineage_kinds().contains(&"promotion".to_string()));
    stop(&fr, code);
    remove_home(&home);

    // Pausing: not committed; discard, update-failed{kernel_restart}, current stays X.
    let home = make_home("r-pausing", "");
    let (x, y) = lineage(&home);
    let fr = FakeRoot::new(&home, Opts { tuple_sha: x.clone(), ..Opts::default() });
    let (_h, code) = fr.start_in_process();
    body_ready(&fr);
    stop(&fr, code);
    set_handoff(&home, "pausing", &x, &y, "00");
    let fr = FakeRoot::new(&home, Opts { tuple_sha: x.clone(), ..Opts::default() });
    let (h, code) = fr.start_in_process();
    body_ready(&fr);
    assert_eq!(main_of(&home), x);
    assert_eq!(h.snapshot().unwrap().current.sha, x);
    assert_eq!(read_json(&home.join("state/kernel/update-failed.json"))["stage"], "kernel_restart");
    assert!(read_json(&home.join("state/kernel/supervisor.json"))["handoff"].is_null());
    stop(&fr, code);
    remove_home(&home);
}

#[test]
fn system_safe_mode_boot_runs_the_body_in_safe_mode_and_a_normal_restart_reports_body_safe_off() {
    let _wd = init_testkit::watchdog::arm("system_safe_mode_boot_runs_the_body_in_safe_mode_and_a_normal_restart_reports_body_safe_off", Duration::from_secs(40));
    let home = make_home("r-safe", "");
    let fr = FakeRoot::new(&home, Opts { mode: ZoneMode::Safe, reason: ZoneBootReason::Demotion, ..Opts::default() });
    let (_h, code) = fr.start_in_process();
    body_ready(&fr);
    let s = fr.seen.0.lock().unwrap().spawns[0].clone();
    assert_eq!(s.env["INIT_MODE"], "safe");
    assert!(s.program.ends_with("bin/init-body.lkg"));
    let boot = read_json(&home.join("state/kernel/boot.json"));
    assert_eq!(boot["safe"]["system"], true);
    assert_eq!(boot["zone"]["boot_reason"], "demotion");
    stop(&fr, code);
    // safe_clear: normal mode again, `body_safe{on:false}` reported.
    let fr = FakeRoot::new(&home, Opts { reason: ZoneBootReason::SafeClear, ..Opts::default() });
    let (_h, code) = fr.start_in_process();
    body_ready(&fr);
    assert_eq!(fr.seen.0.lock().unwrap().spawns[0].env["INIT_MODE"], "normal");
    let off = fr.wait_for("body_safe off", T, |s| s.lineage.iter().find(|l| l.kind == "body_safe").cloned());
    assert_eq!(off.data["on"], false);
    stop(&fr, code);
    remove_home(&home);
}
