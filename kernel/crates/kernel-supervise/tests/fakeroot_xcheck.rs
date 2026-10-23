//! Contract cross-check against lane T's independent `init-fakeroot` (WP3): it spawns
//! `sv-kernel supervise --under-root` with fds 3/4, serves `ROOT_HOME/run/owner.sock`, and its
//! broker spawns the body. The owner's line must come back as the body's reply.
//!
//! Needs `init-fakeroot` in the same target dir:
//! `CARGO_TARGET_DIR=target/k1 cargo build --offline -p init-testkit --bin init-fakeroot`.
//! Skipped (with a note) when it is absent.

mod common;

use common::*;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The watchdog bound of every test here (K-FLAKE: 3-10x its normal runtime; init_testkit::watchdog).
const WD: Duration = Duration::from_secs(30);

#[test]
fn owner_line_round_trip_under_init_fakeroot() {
    let _wd = init_testkit::watchdog::arm("owner_line_round_trip_under_init_fakeroot", WD);
    let exe = std::env::current_exe().unwrap();
    let fakeroot = exe.parent().unwrap().parent().unwrap().join("init-fakeroot");
    if !fakeroot.exists() {
        eprintln!("SKIP: {} not built", fakeroot.display());
        return;
    }
    let home = make_home("xc", "");
    let rh = home.join("rh");
    std::fs::create_dir_all(&rh).unwrap();
    let mut fr = init_testkit::watchdog::spawn(Command::new(&fakeroot)
        .args(["--root-home", rh.to_str().unwrap(), "--home", home.to_str().unwrap(), "--kernel", env!("CARGO_BIN_EXE_sv-kernel")])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null()))
        .unwrap();
    let sock = rh.join("run/owner.sock");
    let deadline = Instant::now() + Duration::from_secs(20);
    let s = loop {
        if let Ok(s) = UnixStream::connect(&sock) {
            break s;
        }
        assert!(Instant::now() < deadline, "owner.sock never appeared");
        std::thread::sleep(Duration::from_millis(100));
    };
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut w = s.try_clone().unwrap();
    w.write_all(b"{\"t\":\"attach\",\"client\":\"wp4\",\"replay\":true}\n").unwrap();
    let mut r = BufReader::new(s);
    let mut line = String::new();
    let mut sent = false;
    let mut got = false;
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        line.clear();
        if r.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let v: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
        let text = v["text"].as_str().or_else(|| v["frame"]["text"].as_str()).unwrap_or("");
        if !sent && text.starts_with("ready ") {
            w.write_all(b"{\"t\":\"line\",\"text\":\"hello from init-fakeroot\"}\n").unwrap();
            sent = true;
        }
        if text == "echo: hello from init-fakeroot" {
            got = true;
            break;
        }
    }
    let mut c = UnixStream::connect(&sock).unwrap();
    c.write_all(b"{\"v\":1,\"id\":\"x1\",\"cmd\":\"shutdown\",\"args\":{}}\n").unwrap();
    let status = wait_exit(&mut fr, Duration::from_secs(30));
    assert!(sent, "the body never reported ready through init-fakeroot");
    assert!(got, "the owner's line did not come back");
    assert!(status.success() || status.code().is_some());
    let lineage = std::fs::read_to_string(rh.join("lineage/lineage.jsonl")).unwrap_or_default();
    assert!(lineage.contains("kernel_start"), "kernel lineage records reach the fake Root's chain");
    remove_home(&home);
}
