//! Runs the whole on-machine half of kernel/sandbox.md §8 against real `sandbox-exec` instances.
//! Temporary roots live under root/target/test-homes/wp6a/<run id>/ and are removed on success.
//! Only processes this test spawns are ever signalled, by the PIDs it holds.
#![cfg(target_os = "macos")]

use std::path::PathBuf;

use init_sandbox_profiles::selftest::{self, SelftestConfig, Status};

#[test]
fn sandbox_selftest_on_machine_half() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/test-homes/wp6a");
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_init-sbx-selftest"));
    let mut cfg = SelftestConfig::detect(base, exe, vec!["inner".into()]).expect("detect toolchain");
    cfg.builds = std::env::var_os("WP6A_NO_BUILDS").is_none();
    let report = selftest::run(&cfg).expect("selftest setup");
    for r in &report.results {
        if let Status::Fail(d) = &r.status {
            eprintln!("FAIL {} [{}] {}: {d}", r.id, r.profile, r.what);
        }
    }
    eprintln!("{}", report.summary());
    assert_eq!(report.failed(), 0, "{}", report.summary());
    // Every skipped row is either disposable-env only, S-M38 (no DNS traffic) or --no-builds.
    for r in &report.results {
        if let Status::Skipped(why) = &r.status {
            assert!(why.contains("disposable-env") || r.id == "S-M38" || why == "--no-builds", "{} {why}", r.id);
        }
    }
}
