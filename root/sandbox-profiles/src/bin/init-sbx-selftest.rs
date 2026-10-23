//! Stand-alone runner for the sandbox self-test and the in-sandbox probe it launches.
//!
//! ```text
//! init-sbx-selftest inner <op> [operands…]      the probe (run inside a sandbox by the runner)
//! init-sbx-selftest run [--base DIR] [--no-builds] [--keep]
//! ```
//!
//! `init-root doctor --sandbox-selftest` (lane R) calls the same library entry points:
//! `init_sandbox_profiles::selftest::run` and, for the probe, `init_sandbox_profiles::probe::inner_main`.
#![deny(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        #[cfg(unix)]
        Some("inner") => ExitCode::from(init_sandbox_profiles::probe::inner_main(&args[1..]) as u8),
        #[cfg(target_os = "macos")]
        Some("run") => run(&args[1..]),
        _ => {
            eprintln!("usage: init-sbx-selftest inner <op> [operands…] | run [--base DIR] [--no-builds] [--keep]");
            ExitCode::from(64)
        }
    }
}

#[cfg(target_os = "macos")]
fn run(args: &[String]) -> ExitCode {
    use init_sandbox_profiles::selftest::{self, SelftestConfig};
    use std::path::PathBuf;

    let mut base: Option<PathBuf> = None;
    let mut builds = true;
    let mut keep = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--base" => base = it.next().map(PathBuf::from),
            "--no-builds" => builds = false,
            "--keep" => keep = true,
            _ => {
                eprintln!("unknown argument {a}");
                return ExitCode::from(64);
            }
        }
    }
    let Some(base) = base else {
        eprintln!("--base DIR is required (e.g. root/target/test-homes/wp6a)");
        return ExitCode::from(64);
    };
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("current_exe: {e}");
            return ExitCode::from(70);
        }
    };
    let mut cfg = match SelftestConfig::detect(base, exe, vec!["inner".into()]) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("selftest setup: {e}");
            return ExitCode::from(70);
        }
    };
    cfg.builds = builds;
    cfg.keep_home = keep;
    match selftest::run(&cfg) {
        Ok(report) => {
            for line in report.json_lines() {
                println!("{line}");
            }
            eprintln!("{}", report.summary());
            if report.failed() == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) }
        }
        Err(e) => {
            eprintln!("selftest: {e}");
            ExitCode::from(70)
        }
    }
}
