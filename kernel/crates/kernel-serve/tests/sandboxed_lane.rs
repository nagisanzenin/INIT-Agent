//! FIX1 (build/status/S1.md gap 3): the live lane binds and serves `run/kernel.sock` when `run/`
//! itself is write-protected the way the Root's zone tail protects it. Z4 (root/root.md §3.3.2)
//! includes `(deny file-write* (literal (param "Z_RUN")))`, which also refuses a chmod of `run/`.
//!
//! The outer test re-runs this test binary under `/usr/bin/sandbox-exec` with `(allow default)`
//! plus exactly that deny. The inner test starts the real server, adds the live lane and pings it
//! over the socket. Only the child this test spawns is waited on; the home is deleted by exact path.
#![cfg(target_os = "macos")]

mod common;

use common::*;
use kernel_serve::*;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

/// The watchdog bound of every test here (K-FLAKE: 3-10x its normal runtime; init_testkit::watchdog).
const WD: Duration = Duration::from_secs(30);

const INNER_HOME: &str = "KSERVE_FIX1_INNER_HOME";
const Z4_RUN: &str = "(version 1)\n(allow default)\n(deny file-write* (literal (param \"RUN\")))\n";

#[test]
fn live_lane_serves_when_run_dir_is_z4_protected() {
    let _wd = init_testkit::watchdog::arm("live_lane_serves_when_run_dir_is_z4_protected", WD);
    let home = test_home("fix1-z4");
    let run = home.join("run");
    // The Root's install creates INIT_HOME/run with mode 0700 (init-root make_dirs).
    std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700)).unwrap();
    let exe = std::env::current_exe().unwrap();
    let out = init_testkit::watchdog::output(Command::new("/usr/bin/sandbox-exec")
        .arg("-p")
        .arg(Z4_RUN)
        .arg(format!("-DRUN={}", run.display()))
        .arg("--")
        .arg(&exe)
        .args(["--exact", "inner_live_lane_under_z4", "--ignored", "--nocapture", "--test-threads=1"])
        .env(INNER_HOME, &home))
        .unwrap();
    let (so, se) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "inner test failed ({:?})\nstdout:\n{so}\nstderr:\n{se}", out.status);
    assert!(so.contains("1 passed"), "the inner test did not run\nstdout:\n{so}\nstderr:\n{se}");
    std::fs::remove_dir_all(&home).unwrap(); // exact path
}

#[test]
#[ignore = "runs inside sandbox-exec, started by live_lane_serves_when_run_dir_is_z4_protected"]
fn inner_live_lane_under_z4() {
    let _wd = init_testkit::watchdog::arm("inner_live_lane_under_z4", WD);
    let Some(home) = std::env::var_os(INNER_HOME).map(PathBuf::from) else {
        return;
    };
    let run = home.join("run");
    // The condition is real: run/ cannot be chmod-ed in here, not even to its current mode.
    let e = std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700)).unwrap_err();
    assert_eq!(e.raw_os_error(), Some(libc::EPERM), "{e}");
    let fx = Fx::start_in(home, Fake::new(), |_| {}, Arc::new(SameUidPolicy));
    let mut r = Raw::connect(&fx.sock());
    r.send_line(br#"{"args":{},"cmd":"ping","id":"z4.1","v":1}"#);
    let f = r.frame().expect("a ping response");
    assert_eq!(f["ok"], true, "{f}");
    assert_eq!(f["result"]["lane"], "live", "{f}");
}
