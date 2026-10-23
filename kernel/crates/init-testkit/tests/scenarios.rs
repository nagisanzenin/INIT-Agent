//! WP3: the scenario runner executes a fake-body scenario, a kernel scenario under the fake Root
//! (with the fake kernel standing in until WP4's kernel exists), an `INIT_FAULT` scenario, and
//! refuses a fake-kernel Root scenario in the live account (SW-6).
//!
//! Scratch homes go under `kernel/target/test-homes/wp3/` (set below) and are deleted by exact
//! path on success.

use init_testkit::guard;
use init_testkit::scenario::{Outcome, RunOpts, run_scenario_with};
use init_testkit::util;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The watchdog bound of every test here (K-FLAKE: 3-10x its normal runtime; init_testkit::watchdog).
const WD: Duration = Duration::from_secs(30);

fn opts() -> RunOpts {
    RunOpts {
        scratch_root: Some(util::test_root()),
        ..RunOpts::default()
    }
}

fn file(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scenarios")
        .join(name)
}

fn passes(name: &str) {
    let r = run_scenario_with(&file(name), &opts());
    assert_eq!(r.outcome, Outcome::Passed, "{r}");
    assert!(
        r.scratch.is_none(),
        "the scratch home is deleted on success"
    );
}

#[test]
fn fake_body_scenario() {
    let _wd = init_testkit::watchdog::arm("fake_body_scenario", WD);
    passes("wp3-fakebody.json");
}

#[test]
fn kernel_scenario_under_the_fake_root() {
    let _wd = init_testkit::watchdog::arm("kernel_scenario_under_the_fake_root", WD);
    passes("wp3-kernel-under-fakeroot.json");
}

#[test]
fn fault_injection_scenario() {
    let _wd = init_testkit::watchdog::arm("fault_injection_scenario", WD);
    passes("wp3-fault.json");
}

/// SW-6: a disposable scenario (here, the real init-root supervising init-fakekernel) refuses to
/// start in the live account, even when INIT_TEST_ENV claims `disposable`; it reports skipped,
/// never passed, and creates nothing.
#[test]
fn sw6_disposable_scenario_refuses_the_live_account() {
    let _wd = init_testkit::watchdog::arm("sw6_disposable_scenario_refuses_the_live_account", WD);
    if guard::read_sentinel().is_some() {
        eprintln!(
            "note: {} exists; SW-6 is asserted only where no sentinel exists",
            guard::SENTINEL
        );
        return;
    }
    for env in [
        None,
        Some("disposable".to_string()),
        Some("live".to_string()),
    ] {
        let o = RunOpts {
            test_env: env.clone(),
            ..opts()
        };
        let r = run_scenario_with(&file("wp3-root-fakekernel.json"), &o);
        match &r.outcome {
            Outcome::Skipped(why) => assert!(
                why.starts_with("skipped: needs a disposable environment"),
                "{why}"
            ),
            other => panic!("INIT_TEST_ENV={env:?}: expected skipped, got {other:?}"),
        }
        assert!(r.scratch.is_none(), "nothing is created before the guard");
    }
}

/// A scenario that starts the real init-root must say `"env": "disposable"`; the runner rejects
/// the file before the guard, so it can never run in the live account.
#[test]
fn live_scenario_starting_the_real_root_is_rejected() {
    let _wd = init_testkit::watchdog::arm("live_scenario_starting_the_real_root_is_rejected", WD);
    let d = util::test_dir("rej");
    let p = d.join("bad.json");
    std::fs::write(
        &p,
        r#"{"v":1,"id":"BAD","env":"live","steps":[{"op":"start_root"}]}"#,
    )
    .unwrap();
    let r = run_scenario_with(&p, &opts());
    assert!(
        matches!(&r.outcome, Outcome::Failed(m) if m.contains("disposable")),
        "{r}"
    );
    std::fs::write(&p, r#"{"v":1,"id":"BAD2","env":"live","steps":[{"op":"start_kernel"},{"op":"root_cmd","argv":["clear-safe"]}]}"#).unwrap();
    let r = run_scenario_with(&p, &opts());
    assert!(
        matches!(&r.outcome, Outcome::Failed(m) if m.contains("approval")),
        "{r}"
    );
    std::fs::remove_dir_all(&d).unwrap();
}

/// A failing expectation fails the scenario, keeps the scratch home and reports diagnostics.
#[test]
fn a_failing_expectation_keeps_the_scratch_home() {
    let _wd = init_testkit::watchdog::arm("a_failing_expectation_keeps_the_scratch_home", WD);
    let d = util::test_dir("failx");
    let p = d.join("fail.json");
    std::fs::write(
        &p,
        r#"{"v":1,"id":"FAILX","env":"live","steps":[{"op":"start_body"},{"op":"wait_file","path":"state/run/ready","timeout_s":10},{"op":"stop_body","expect_exit":0}],
            "expect":{"events_in_order":[{"type":"boot","reason":"promote"}]}}"#,
    )
    .unwrap();
    let r = run_scenario_with(&p, &opts());
    let Outcome::Failed(m) = &r.outcome else {
        panic!("expected a failure: {r}")
    };
    assert!(m.contains("events_in_order") && m.contains("last"), "{m}");
    let kept = r.scratch.clone().expect("kept on failure");
    assert!(kept.join("h/state/run/ready").exists());
    std::fs::remove_dir_all(&kept).unwrap();
    std::fs::remove_dir_all(&d).unwrap();
}
