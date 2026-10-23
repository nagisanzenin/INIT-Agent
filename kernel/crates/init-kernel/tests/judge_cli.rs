//! WP8c: the `gate judge` dispatch of the `init-kernel` binary (gate.md §4.1a exit codes, `-c`
//! overrides). The judge itself is tested in kernel-gate (`tests/judge.rs`) and through WP24's
//! gate-of-gates (`INIT_GOG_GATE_BIN`). Homes under `kernel/target/test-homes/wp8c/`, removed by path.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// The watchdog bound of every test here (K-FLAKE: 3-10x its normal runtime; init_testkit::watchdog).
const WD: Duration = Duration::from_secs(30);

const BIN: &str = env!("CARGO_BIN_EXE_init-kernel");

fn home(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-homes/wp8c")
        .join(format!("{name}-{}", std::process::id()));
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap(); // exact path, an earlier run of this test
    }
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn code(args: &[&str]) -> Option<i32> {
    init_testkit::watchdog::status(Command::new(BIN)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null()))
        .unwrap()
        .code()
}

#[test]
fn gate_judge_usage_errors_and_out_record() {
    let _wd = init_testkit::watchdog::arm("gate_judge_usage_errors_and_out_record", WD);
    assert_eq!(code(&["gate", "judge"]), Some(64), "missing arguments");
    assert_eq!(
        code(&["-c", "gate.g4.min_pool=4", "selfmap"]),
        Some(64),
        "-c is for gate only"
    );
    assert_eq!(code(&["-c"]), Some(64));
    let h = home("cli");
    let (hs, out) = (h.to_str().unwrap().to_string(), h.join("out.json"));
    let base = [
        "gate",
        "judge",
        "--home",
        &hs,
        "--candidate",
        &hs,
        "--running",
        &hs,
        "--lkg",
        &hs,
        "--out",
        out.to_str().unwrap(),
    ];
    let mut bad = vec!["-c", "gate.g4.nope=1"];
    bad.extend(base);
    assert_eq!(code(&bad), Some(64), "an unknown -c key is a usage error");
    // a tree without init-body cannot be judged: error (3), with the verdict JSON in --out
    let mut ok = vec!["-c", "gate.g4.min_pool=4"];
    ok.extend(base);
    assert_eq!(code(&ok), Some(3));
    let v: init_proto::serde_json::Value =
        init_proto::serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(
        (v["verdict"].as_str(), v["reason"].as_str()),
        (Some("error"), Some("unsupported_tree")),
        "{v}"
    );
    assert_eq!(
        std::fs::read_dir(&h).unwrap().count(),
        1,
        "only --out is written"
    );
    std::fs::remove_dir_all(&h).unwrap();
}
