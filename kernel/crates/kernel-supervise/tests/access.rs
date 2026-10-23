//! WIRE-009 (ADR-009, SBX-009 §D): the kernel relays the Root's workspace and access to the live
//! body's spawn as `INIT_WORKSPACE` / `INIT_ACCESS` (from the `hello` result's `access`, then from
//! `access` events), and on `access{restart:"idle"}` restarts the body gracefully at its next idle
//! point: `stop`, exit 0, a new spawn on the same version and session with `INIT_BOOT_REASON=handoff`
//! and `boot.json.cause.kind = "workspace_change"`, never counted as a crash. The fake Root is
//! in-test (tests/common); the body is `sv-testbody`.

mod common;

use common::*;
use init_proto::root_link::*;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const T: Duration = Duration::from_secs(30);
/// The watchdog bound of every test here (K-FLAKE: 3-10x its normal runtime; init_testkit::watchdog).
const WD: Duration = Duration::from_secs(60);

fn homes_root() -> PathBuf {
    let r = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/wire009");
    std::fs::create_dir_all(&r).unwrap();
    std::fs::canonicalize(r).unwrap()
}

fn home(name: &str) -> PathBuf {
    let h = homes_root().join(format!("{name}-{}", kernel_supervise::sys::random_hex(3)));
    for d in ["bin", "kernel", "state/kernel", "state/run", "state/sessions", "work", "vault", "cache", "run", "root", "lineage"] {
        std::fs::create_dir_all(h.join(d)).unwrap();
    }
    std::fs::copy(testbody(), h.join("bin/init-body")).unwrap();
    std::fs::hard_link(h.join("bin/init-body"), h.join("bin/init-body.lkg")).unwrap();
    let cfg = "[proxy]\ntest_mode = true\n\n[supervise]\ngit = \"/usr/bin/git\"\nready_timeout_s = 10\nsettle_s = 1\nbackoff_initial_ms = 50\nbackoff_max_ms = 400\nexit_grace_s = 3\n";
    std::fs::write(h.join("kernel/config.toml"), cfg).unwrap();
    h
}

fn access_event(access: &str, full: bool, workspace: Option<&str>, restart: Option<&str>) -> RootEvent {
    let data = json!({"access": access, "full": full, "restart": restart, "workspace": workspace});
    RootEvent::Access(serde_json::from_value(data).unwrap())
}

fn texts(s: &Seen) -> Vec<String> {
    s.events.iter().filter(|e| e.ev == "owner.out").filter_map(|e| e.data["frame"]["text"].as_str().map(str::to_string)).collect()
}

#[test]
fn relay_and_graceful_restart_at_idle() {
    let _wd = init_testkit::watchdog::arm("relay_and_graceful_restart_at_idle", WD);
    let home = home("relay");
    let hello = json!({"access": "workspace", "full": false, "workspace": "/Users/o/proj"});
    let fr = FakeRoot::new(&home, Opts { access: Some(hello), ..Opts::default() });
    let (h, code) = fr.start_in_process();
    fr.wait_for("ready", T, |s| s.ready.then_some(()));
    fr.wait_for("body ready", T, |s| s.events.iter().any(|e| e.data["frame"]["text"].as_str().is_some_and(|t| t.starts_with("ready"))).then_some(()));
    {
        let g = fr.seen.0.lock().unwrap();
        let b = &g.spawns[0];
        assert_eq!((b.env["INIT_WORKSPACE"].as_str(), b.env["INIT_ACCESS"].as_str()), ("/Users/o/proj", "workspace"), "relayed from hello");
        assert!(!b.params.keys().any(|k| k.starts_with("Z_")), "no Z_* params from the kernel (SBX-009 §D)");
    }
    // An event without restart changes nothing now.
    fr.event(&access_event("workspace", false, Some("/Users/o/proj"), None));
    // The body is busy for 1.5 s when the Root asks for the restart: it waits for the idle point.
    assert!(fr.owner_line("!busy 1500").is_ok());
    fr.wait_for("busy", T, |s| s.events.iter().any(|e| e.data["frame"]["t"] == "state" && e.data["frame"]["busy"] == true).then_some(()));
    let asked = Instant::now();
    fr.event(&access_event("full", true, Some("/Users/o/other"), Some("idle")));
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(fr.seen.0.lock().unwrap().spawns.len(), 1, "no restart while busy");
    let b = fr.wait_for("the restart spawn", T, |s| s.spawns.get(1).cloned());
    assert!(asked.elapsed() >= Duration::from_millis(1000), "it waited for the turn to end");
    let first_session = fr.seen.0.lock().unwrap().spawns[0].env["INIT_SESSION"].clone();
    assert_eq!(b.env["INIT_BOOT_REASON"], "handoff");
    assert_eq!(b.env["INIT_SESSION"], first_session, "the same session continues");
    assert_eq!((b.env["INIT_WORKSPACE"].as_str(), b.env["INIT_ACCESS"].as_str()), ("/Users/o/other", "full"));
    assert!(b.program.ends_with("bin/init-body") && b.replaces.is_none());
    let boot: Value = serde_json::from_slice(&std::fs::read(home.join("state/kernel/boot.json")).unwrap()).unwrap();
    assert_eq!((boot["reason"].as_str(), boot["cause"]["kind"].as_str()), (Some("handoff"), Some("workspace_change")));
    fr.wait_for("the old body exited 0", T, |s| s.exits.iter().any(|x| x.code == Some(0)).then_some(()));
    fr.wait_for("the new body is ready", T, |s| (s.events.iter().filter(|e| e.data["frame"]["text"].as_str().is_some_and(|t| t.starts_with("ready"))).count() >= 2).then_some(()));
    // The busy turn finished before the stop; a line after the restart reaches the new body.
    assert!(texts(&fr.seen.0.lock().unwrap()).iter().any(|t| t == "echo: !busy 1500"));
    assert!(fr.owner_line("after").is_ok());
    fr.wait_for("echo after", T, |s| texts(s).iter().any(|t| t == "echo: after").then_some(()));
    let snap = h.snapshot().unwrap();
    assert_eq!((snap.counters.crashes_total, snap.counters.restarts_since_lkg), (0, 0), "a restart we asked for is no crash");
    assert_eq!(fr.seen.0.lock().unwrap().spawns.len(), 2, "exactly one restart");
    // A second change while idle restarts at once; an event with the same values and no restart does not.
    fr.event(&access_event("read_home", false, None, Some("idle")));
    let c = fr.wait_for("the second restart", T, |s| s.spawns.get(2).cloned());
    assert_eq!((c.env["INIT_WORKSPACE"].as_str(), c.env["INIT_ACCESS"].as_str()), ("", "read_home"), "no workspace is the empty string");
    fr.wait_for("ready 3", T, |s| (s.events.iter().filter(|e| e.data["frame"]["text"].as_str().is_some_and(|t| t.starts_with("ready"))).count() >= 3).then_some(()));
    fr.event(&access_event("read_home", false, None, None));
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(fr.seen.0.lock().unwrap().spawns.len(), 3);
    fr.request(&RootRequest::Stop(StopArgs { reason: "t".into(), grace_ms: 2000 }), T).unwrap();
    assert_eq!(code.recv_timeout(T).unwrap(), 0);
    let _ = std::fs::remove_dir_all(&home);
}

/// An older Root sends no `access`: the kernel sets neither variable.
#[test]
fn no_access_from_the_root_sets_nothing() {
    let _wd = init_testkit::watchdog::arm("no_access_from_the_root_sets_nothing", WD);
    let home = home("none");
    let fr = FakeRoot::new(&home, Opts::default());
    let (_h, code) = fr.start_in_process();
    let b = fr.wait_for("spawn", T, |s| s.spawns.first().cloned());
    assert!(!b.env.contains_key("INIT_WORKSPACE") && !b.env.contains_key("INIT_ACCESS"));
    fr.wait_for("ready", T, |s| s.ready.then_some(()));
    fr.request(&RootRequest::Stop(StopArgs { reason: "t".into(), grace_ms: 2000 }), T).unwrap();
    assert_eq!(code.recv_timeout(T).unwrap(), 0);
    let _ = std::fs::remove_dir_all(&home);
}
