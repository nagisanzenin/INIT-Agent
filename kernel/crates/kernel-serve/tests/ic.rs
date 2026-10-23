//! kernel/initctl.md §8 against the real server and the real `initctl` client (WP1), with an
//! in-test fake backend. Frame-level cases (IC-2, IC-3, IC-15, IC-22) use a raw socket client.

mod common;

use common::*;
use init_proto::initctl::{CommandName, JobState, ProbePurpose};
use kernel_serve::*;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The watchdog bound of every test here (K-FLAKE: 3-10x its normal runtime; init_testkit::watchdog).
const WD: Duration = Duration::from_secs(30);

fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

fn fast(cfg: &mut ServeConfig) {
    cfg.limits.rate_per_sec = 100_000;
    cfg.limits.burst = 100_000;
}

fn integers_only(v: &Value, path: &str) {
    match v {
        Value::Number(n) => assert!(n.is_i64() || n.is_u64(), "float at {path}: {n}"),
        Value::Array(a) => a.iter().enumerate().for_each(|(i, x)| integers_only(x, &format!("{path}/{i}"))),
        Value::Object(m) => m.iter().for_each(|(k, x)| integers_only(x, &format!("{path}/{k}"))),
        _ => {}
    }
}

#[test]
fn ping_and_envelope() {
    let _wd = init_testkit::watchdog::arm("ping_and_envelope", WD);
    let fx = Fx::new("ping");
    let o = fx.ctl(&["ping"]);
    assert_eq!(o.code, 0, "{o:?}");
    let v = o.json();
    assert_eq!((v["lane"].as_str(), v["proto"].as_u64(), v["kernel_version"].as_str()), (Some("live"), Some(1), Some("0.1.0")));
    // Raw frames: sorted keys, v:1, id echoed; unknown cmd and bad args keep the connection.
    let mut r = fx.raw();
    r.send_line(br#"{"args":{},"cmd":"ping","id":"a.1","v":1,"zzz":"ignored"}"#);
    let line = r.frame().unwrap();
    assert_eq!(line["id"], "a.1");
    assert_eq!(line["ok"], true);
    let (_, f) = r.call("2", "approve", json!({}));
    assert_eq!(f["error"]["code"], "E_UNKNOWN_CMD");
    let (_, f) = r.call("3", "promote", json!({"ref": "main"}));
    assert_eq!(f["error"]["code"], "E_ARGS");
    assert_eq!(f["error"]["details"], json!([]));
    let (_, f) = r.call("4", "ping", json!({}));
    assert_eq!(f["ok"], true);
}

#[test]
fn ic2_line_over_64k_is_e_proto_and_closes() {
    let _wd = init_testkit::watchdog::arm("ic2_line_over_64k_is_e_proto_and_closes", WD);
    let fx = Fx::new("ic2");
    // Exactly 64 KiB is accepted.
    let mut r = fx.raw();
    let base = r#"{"args":{"pad":""},"cmd":"ping","id":"1","v":1}"#.len();
    let pad = "x".repeat(64 * 1024 - base);
    r.send_line(format!(r#"{{"args":{{"pad":"{pad}"}},"cmd":"ping","id":"1","v":1}}"#).as_bytes());
    assert_eq!(r.frame().unwrap()["ok"], true);
    // 64 KiB + 1 → E_PROTO with a null id, then close.
    let pad = "x".repeat(64 * 1024 - base + 1);
    r.send_line_lossy(format!(r#"{{"args":{{"pad":"{pad}"}},"cmd":"ping","id":"1","v":1}}"#).as_bytes());
    let f = r.frame().unwrap();
    assert_eq!((f["error"]["code"].as_str(), f["id"].is_null()), (Some("E_PROTO"), true));
    assert!(r.closed());
    // Garbage → E_PROTO, closed.
    let mut r = fx.raw();
    r.send_line(b"not json");
    assert_eq!(r.frame().unwrap()["error"]["code"], "E_PROTO");
    assert!(r.closed());
}

#[test]
fn ic3_version_two_is_e_version() {
    let _wd = init_testkit::watchdog::arm("ic3_version_two_is_e_version", WD);
    let fx = Fx::new("ic3");
    let mut r = fx.raw();
    r.send_line(br#"{"args":{},"cmd":"ping","id":"7","v":2}"#);
    let f = r.frame().unwrap();
    assert_eq!((f["error"]["code"].as_str(), f["id"].as_str(), f["v"].as_u64()), (Some("E_VERSION"), Some("7"), Some(1)));
    let (_, f) = r.call("8", "ping", json!({}));
    assert_eq!(f["ok"], true, "E_VERSION keeps the connection");
}

/// Valid args for every command (paths are relative to the lane's home).
fn args_for(c: CommandName) -> Value {
    use CommandName::*;
    match c {
        Reload => json!({}),
        Gate => json!({"quick": true, "repo": "work/wt", "rev": "HEAD", "wait": false}),
        Promote => json!({"ref": "evo/m"}),
        Rollback => json!({"reason": "r"}),
        Vendor => json!({"crate": "vt100", "version": "0.16.2", "wait": false}),
        TaskSubmit => json!({"dir": "work/fam4", "wait": false}),
        TaskShow => json!({"family": "fam4"}),
        BudgetSet => json!({"set": {"task_musd": 100}}),
        ModelSet => json!({"model": "deepseek/deepseek-flash"}),
        FleetSpawn => json!({"prompt": "go"}),
        FleetSend => json!({"bid": "b1", "text": "hi"}),
        FleetStop => json!({"bid": "all"}),
        KernelPropose => json!({"title": "t", "patch": "work/p.diff", "rationale": ""}),
        KernelPromote => json!({"ref": "kevo/x"}),
        JobStatus | JobCancel => json!({"job": "j-999999"}),
        JobWait => json!({"job": "j-999999", "timeout_s": 1}),
        _ => json!({}),
    }
}

#[test]
fn ic4_lane_and_level_matrix() {
    let _wd = init_testkit::watchdog::arm("ic4_lane_and_level_matrix", WD);
    let fx = Fx::with("ic4", fast, Arc::new(SameUidPolicy));
    let h = fx.home.clone();
    std::fs::create_dir_all(h.join("work/wt")).unwrap();
    std::fs::create_dir_all(h.join("work/fam4")).unwrap();
    std::fs::write(h.join("work/fam4/family.json"), r#"{"instances":["i1","i2","i3","i4"]}"#).unwrap();
    std::fs::write(h.join("work/p.diff"), "--- a\n+++ b\n").unwrap();
    std::fs::create_dir_all(h.join("fleet/b1")).unwrap();
    fx.srv.add_lane(LaneSpec::member(&h, "b1", &h.join("fleet/b1"))).unwrap();
    fx.srv.add_lane(LaneSpec::probe(&h, "p1", ProbePurpose::Probe)).unwrap();
    fx.srv.add_lane(LaneSpec::probe(&h, "p2", ProbePurpose::Run)).unwrap();
    fx.fake.st().view.bodies = Some(BodyCount { limit: 100, used: 1 });
    let lanes = [
        (LaneCtx::live(), h.join("run/kernel.sock")),
        (LaneCtx::member("b1"), h.join("run/fleet/b1/kernel.sock")),
        (LaneCtx::probe("p1", ProbePurpose::Probe), h.join("probe/p1/run/kernel.sock")),
        (LaneCtx::probe("p2", ProbePurpose::Run), h.join("probe/p2/run/kernel.sock")),
    ];
    let mut checked = 0;
    for level in 0..=3u8 {
        fx.fake.set_level(level);
        for (ctx, sock) in &lanes {
            let mut r = Raw::connect(sock);
            for c in CommandName::ALL {
                let before = fx.fake.calls().len();
                let roots_before = fx.fake.root_requests().iter().filter(|r| r.starts_with("spawn") || r.starts_with("system.swap")).count();
                let (_, f) = r.call("m", c.as_str(), args_for(*c));
                let code = f["error"]["code"].as_str().unwrap_or("ok").to_string();
                let need = c.min_level();
                if !c.allowed_on(ctx.lane, ctx.purpose) {
                    assert_eq!(code, "E_LANE", "{c} on {} at L{level}", ctx.key());
                } else if need > level {
                    assert_eq!(code, "E_LEVEL", "{c} on {} at L{level}", ctx.key());
                    assert_eq!(f["error"]["data"], json!({"have": level, "need": need}));
                    assert_eq!(fx.fake.calls().len(), before, "{c}: the backend must not run below the level");
                    let roots = fx.fake.root_requests().iter().filter(|r| r.starts_with("spawn") || r.starts_with("system.swap")).count();
                    assert_eq!(roots, roots_before, "{c}: nothing reaches the Root below the level (A6)");
                } else {
                    assert!(code != "E_LANE" && code != "E_LEVEL", "{c} on {} at L{level}: {f}", ctx.key());
                }
                checked += 1;
            }
        }
    }
    assert_eq!(checked, CommandName::ALL.len() * 4 * 4);
    // Spot checks through the CLI: exit 77 for both.
    fx.fake.set_level(0);
    let o = fx.ctl(&["task", "list"]);
    assert_eq!((o.code, o.err_code()), (77, "E_LEVEL".to_string()));
    let o = fx.ctl_on(&h.join("run/fleet/b1/kernel.sock"), &["rollback"]);
    assert_eq!((o.code, o.err_code()), (77, "E_LANE".to_string()));
}

#[cfg(target_os = "macos")]
#[test]
fn ic5_unattributed_peer_gets_e_peer_and_is_logged() {
    let _wd = init_testkit::watchdog::arm("ic5_unattributed_peer_gets_e_peer_and_is_logged", WD);
    let fx = Fx::with("ic5", |_| {}, Arc::new(SandboxPolicy { canary: "/nonexistent-root/run/canary".into() }));
    let o = fx.ctl(&["status"]);
    assert_eq!((o.code, o.err_code()), (77, "E_PEER".to_string()), "{o:?}");
    let log = std::fs::read_to_string(fx.home.join("state/kernel/logs/kernel.jsonl")).unwrap();
    let line: Value = serde_json::from_str(log.lines().last().unwrap()).unwrap();
    assert_eq!((line["ev"].as_str(), line["lane"].as_str(), line["reason"].as_str()), (Some("peer_refused"), Some("live:b0"), Some("peer is not sandboxed")));
    assert!(line["pid"].as_i64().unwrap() > 0);
    assert!(fx.fake.calls().is_empty(), "nothing ran");
}

#[test]
fn ic7_reload_wait_on_live_lane_would_deadlock() {
    let _wd = init_testkit::watchdog::arm("ic7_reload_wait_on_live_lane_would_deadlock", WD);
    let fx = Fx::new("ic7");
    let o = fx.ctl(&["reload", "--wait"]);
    assert_eq!((o.code, o.err_code()), (64, "E_WOULD_DEADLOCK".to_string()));
    assert!(fx.fake.calls().is_empty());
}

#[test]
fn ic8_promote_wait_returns_at_awaiting_boundary() {
    let _wd = init_testkit::watchdog::arm("ic8_promote_wait_returns_at_awaiting_boundary", WD);
    let fx = Fx::new("ic8");
    let o = fx.ctl(&["promote", "evo/pty-tool", "--wait"]);
    assert_eq!(o.code, 0, "{o:?}");
    let v = o.json();
    assert_eq!(v["state"], "awaiting_boundary");
    assert_eq!(v["sha"], sha_of("evo/pty-tool"));
    let stages: Vec<&str> = v["stages"].as_array().unwrap().iter().map(|s| s["stage"].as_str().unwrap()).collect();
    assert_eq!(stages, ["build", "G0", "G1", "G2", "G3"]);
    assert!(o.stderr.contains("[j-000001 G0 "), "progress on stderr: {}", o.stderr);
    // The handoff follows at the boundary.
    let job = v["job"].as_str().unwrap().to_string();
    fx.until(&job, "succeeded", |r| r.state == JobState::Succeeded);
    assert!(fx.fake.events().contains(&Ev::Body(format!("pause {job}"))));
    let o = fx.ctl(&["wait", &job]);
    assert_eq!((o.code, o.json()["state"].as_str()), (0, Some("succeeded")));
    // The record on disk (§4.4).
    let disk: Value = serde_json::from_slice(&std::fs::read(fx.home.join(format!("state/kernel/jobs/{job}.json"))).unwrap()).unwrap();
    assert_eq!((disk["state"].as_str(), disk["cmd"].as_str(), disk["lane"].as_str(), disk["v"].as_u64()), (Some("succeeded"), Some("promote"), Some("live"), Some(1)));
}

#[test]
fn ic9_kill_initctl_mid_gate_then_wait_reattaches() {
    let _wd = init_testkit::watchdog::arm("ic9_kill_initctl_mid_gate_then_wait_reattaches", WD);
    let fx = Fx::new("ic9");
    fx.fake.st().stage_ms = 250;
    let child = spawn_initctl(&fx.home, &fx.sock(), &["promote", "evo/slow", "--wait"]);
    let pid = child.id();
    let rec = fx.until("j-000001", "in G0", |r| r.progress.as_ref().is_some_and(|p| p.stage == Some(init_proto::initctl::Stage::G0)));
    assert_eq!(rec.state, JobState::Running);
    let mut child = child;
    child.kill().unwrap(); // our own child, by PID
    let _ = child.wait();
    assert!(pid > 0);
    sleep_ms(300);
    let r = fx.srv.jobs().record("j-000001").unwrap();
    assert!(!r.state.is_terminal(), "the job continues without its connection: {:?}", r.state);
    let o = fx.ctl(&["wait", "j-000001"]);
    assert_eq!(o.code, 0, "{o:?}");
    assert_eq!(o.json()["state"], "awaiting_boundary");
    assert!(o.stderr.contains("[j-000001"), "progress frames on reattach: {}", o.stderr);
    fx.until("j-000001", "final", |r| r.state == JobState::Succeeded);
    let o = fx.ctl(&["wait", "j-000001"]);
    assert_eq!(o.json()["state"], "succeeded");
}

#[test]
fn ic10_second_promote_is_busy() {
    let _wd = init_testkit::watchdog::arm("ic10_second_promote_is_busy", WD);
    let fx = Fx::new("ic10");
    fx.fake.st().stage_ms = 200;
    let o = fx.ctl(&["promote", "evo/a"]);
    assert_eq!(o.code, 0);
    let v = o.json();
    assert_eq!((v["job"].as_str(), v["state"].as_str()), (Some("j-000001"), Some("queued")));
    let o = fx.ctl(&["promote", "evo/b"]);
    assert_eq!((o.code, o.err_code()), (75, "E_BUSY".to_string()));
    assert_eq!(o.json()["error"]["data"], json!({"job": "j-000001", "reason": "pipeline"}));
    // Rollback is refused during the pipeline too.
    fx.fake.st().current = sha_of("x");
    assert_eq!(fx.ctl(&["rollback", "--reason", "x"]).err_code(), "E_BUSY");
}

#[test]
fn ic13_vendor_while_provisional() {
    let _wd = init_testkit::watchdog::arm("ic13_vendor_while_provisional", WD);
    let fx = Fx::new("ic13");
    fx.fake.st().view.primary_provisional = true;
    let o = fx.ctl(&["vendor", "vt100@0.16.2"]);
    assert_eq!((o.code, o.err_code()), (77, "E_PROVISIONAL".to_string()));
    assert!(fx.fake.calls().is_empty());
}

#[test]
fn ic15_connection_cap_and_rate_limit() {
    let _wd = init_testkit::watchdog::arm("ic15_connection_cap_and_rate_limit", WD);
    let fx = Fx::new("ic15");
    let held: Vec<Raw> = (0..8).map(|_| fx.raw()).collect();
    let t = Instant::now();
    while fx.srv.connections("live:b0") != Some(8) {
        assert!(t.elapsed() < Duration::from_secs(5));
        sleep_ms(2);
    }
    let mut ninth = fx.raw();
    let f = ninth.frame().unwrap();
    assert_eq!((f["error"]["code"].as_str(), f["id"].is_null()), (Some("E_RATE"), true));
    assert!(ninth.closed());
    // Other lanes are unaffected (per-lane limits).
    fx.srv.add_lane(LaneSpec::probe(&fx.home, "p1", ProbePurpose::Probe)).unwrap();
    assert_eq!(fx.ctl_on(&fx.home.join("probe/p1/run/kernel.sock"), &["ping"]).code, 0);
    drop(held);
    let t = Instant::now();
    while fx.srv.connections("live:b0") != Some(0) {
        assert!(t.elapsed() < Duration::from_secs(5));
        sleep_ms(2);
    }
    // 10 req/s, burst 20: a burst of 25 gets ~20 answers, then E_RATE with the id echoed.
    let mut r = fx.raw();
    let mut ok = 0;
    let mut rate = 0;
    for i in 0..25 {
        let (_, f) = r.call(&format!("r{i}"), "ping", json!({}));
        if f["ok"] == true {
            ok += 1;
        } else {
            assert_eq!((f["error"]["code"].as_str(), f["id"].as_str()), (Some("E_RATE"), Some(format!("r{i}").as_str())));
            rate += 1;
        }
    }
    assert!((20..=22).contains(&ok) && rate >= 3, "ok {ok} rate {rate}");
    let o = fx.ctl(&["ping"]);
    assert!(o.code == 75 || o.code == 0, "E_RATE exits 75: {o:?}");
    sleep_ms(250);
    let (_, f) = r.call("later", "ping", json!({}));
    assert_eq!(f["ok"], true, "tokens refill at 10/s");
}

#[test]
fn ic16_kernel_gone_then_back_and_restart_recovery() {
    let _wd = init_testkit::watchdog::arm("ic16_kernel_gone_then_back_and_restart_recovery", WD);
    let fx = Fx::new("ic16");
    fx.fake.st().stage_ms = 400;
    assert_eq!(fx.ctl(&["promote", "evo/long"]).code, 0);
    fx.until("j-000001", "running", |r| r.state == JobState::Running);
    fx.srv.shutdown();
    let o = fx.ctl(&["ping"]);
    assert_eq!(o.code, 69, "no kernel: {o:?}");
    assert!(o.stdout.is_empty());
    // The kernel comes back (after the Root's panic_resume or restart).
    let srv2 = kernel_serve::start(ServeConfig::new(&fx.home, "0.1.0"), Fake::new(), Arc::new(SameUidPolicy)).unwrap();
    srv2.add_lane(LaneSpec::live(&fx.home)).unwrap();
    assert_eq!(srv2.recovered(), ["j-000001".to_string()]);
    assert_eq!(fx.ctl(&["ping"]).code, 0);
    let o = fx.ctl(&["job", "status", "j-000001"]);
    let v = o.json();
    assert_eq!((v["state"].as_str(), v["reason"].as_str()), (Some("failed"), Some("kernel_restart")));
    let o = fx.ctl(&["wait", "j-000001"]);
    assert_eq!(o.code, 1, "a failed record exits 1: {o:?}");
    // Ids continue after the restart.
    assert_eq!(fx.ctl(&["promote", "evo/next"]).json()["job"], "j-000002");
    srv2.shutdown();
}

#[test]
fn stale_socket_is_replaced_and_non_socket_kept() {
    let _wd = init_testkit::watchdog::arm("stale_socket_is_replaced_and_non_socket_kept", WD);
    let home = test_home("sock");
    let p = home.join("run/kernel.sock");
    drop(std::os::unix::net::UnixListener::bind(&p).unwrap()); // leaves a stale socket file
    assert!(kernel_serve::server::is_socket(&p));
    let srv = kernel_serve::start(ServeConfig::new(&home, "0.1.0"), Fake::new(), Arc::new(SameUidPolicy)).unwrap();
    srv.add_lane(LaneSpec::live(&home)).unwrap();
    assert_eq!(run_initctl(&home, &p, &["ping"]).code, 0);
    // A regular file where a lane socket goes is never deleted.
    std::fs::create_dir_all(home.join("probe/p9/run")).unwrap();
    std::fs::write(home.join("probe/p9/run/kernel.sock"), "keep").unwrap();
    assert!(srv.add_lane(LaneSpec::probe(&home, "p9", ProbePurpose::Probe)).is_err());
    assert_eq!(std::fs::read_to_string(home.join("probe/p9/run/kernel.sock")).unwrap(), "keep");
    // Duplicate lane.
    assert!(srv.add_lane(LaneSpec::live(&home)).is_err());
    srv.remove_lane("live:b0").unwrap();
    assert!(!p.exists(), "removed by exact path");
    srv.shutdown();
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn ic18_kernel_propose_files_symlink_and_quota() {
    let _wd = init_testkit::watchdog::arm("ic18_kernel_propose_files_symlink_and_quota", WD);
    let fx = Fx::new("ic18");
    let w = fx.home.join("work");
    std::fs::write(w.join("p.diff"), "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n").unwrap();
    std::fs::write(w.join("why.md"), "the gate is slow").unwrap();
    std::os::unix::fs::symlink(w.join("p.diff"), w.join("link.diff")).unwrap();
    let o = fx.ctl(&["kernel", "propose", "link.diff", "--title", "Faster gate"]);
    assert_eq!((o.code, o.err_code()), (1, "E_NOT_FOUND".to_string()), "IC-18: {o:?}");
    assert!(fx.fake.root_requests().is_empty());
    let o = fx.ctl(&["kernel", "propose", "p.diff", "--title", "Faster gate", "--rationale-file", "why.md"]);
    assert_eq!(o.code, 0, "{o:?}");
    let v = o.json();
    assert_eq!(v, json!({"path": "state/kernel/proposals/kp-0001/", "proposal": "kp-0001", "root_id": "rp-0001", "status": "filed"}));
    let dir = fx.home.join("state/kernel/proposals/kp-0001");
    assert_eq!(std::fs::read_to_string(dir.join("patch.diff")).unwrap(), std::fs::read_to_string(w.join("p.diff")).unwrap());
    let pj: Value = serde_json::from_slice(&std::fs::read(dir.join("proposal.json")).unwrap()).unwrap();
    assert_eq!((pj["root_id"].as_str(), pj["title"].as_str(), pj["rationale"].as_str()), (Some("rp-0001"), Some("Faster gate"), Some("the gate is slow")));
    // 5 per day; the refused symlink attempt did not count. The alias works too.
    for i in 2..=5 {
        let o = fx.ctl(&["propose-kernel", "p.diff", "--title", &format!("p{i}")]);
        assert_eq!(o.json()["proposal"], format!("kp-{i:04}"));
    }
    let o = fx.ctl(&["kernel", "propose", "p.diff", "--title", "sixth"]);
    assert_eq!((o.code, o.err_code()), (75, "E_QUOTA".to_string()));
    assert_eq!(o.json()["error"]["data"]["quota"], "proposals_day");
    assert_eq!(fx.fake.root_requests().len(), 5);
}

#[test]
fn ic19_wait_last_and_last_cmd() {
    let _wd = init_testkit::watchdog::arm("ic19_wait_last_and_last_cmd", WD);
    let fx = Fx::new("ic19");
    let o = fx.ctl(&["wait", "--last"]);
    assert_eq!((o.code, o.err_code()), (1, "E_NOT_FOUND".to_string()));
    assert_eq!(fx.ctl(&["promote", "evo/a", "--wait"]).code, 0);
    fx.until("j-000001", "done", |r| r.state.is_terminal());
    assert_eq!(fx.ctl(&["reload"]).json()["job"], "j-000002");
    let o = fx.ctl(&["wait", "--last"]);
    assert_eq!((o.code, o.json()["id"].as_str(), o.json()["cmd"].as_str()), (0, Some("j-000002"), Some("reload")));
    let o = fx.ctl(&["wait", "--last", "--cmd", "promote"]);
    assert_eq!((o.code, o.json()["id"].as_str(), o.json()["state"].as_str()), (0, Some("j-000001"), Some("succeeded")));
    let o = fx.ctl(&["wait", "--last", "--cmd", "vendor"]);
    assert_eq!(o.err_code(), "E_NOT_FOUND");
}

#[test]
fn ic20_gate_from_a_run_probe_lane() {
    let _wd = init_testkit::watchdog::arm("ic20_gate_from_a_run_probe_lane", WD);
    let fx = Fx::new("ic20");
    fx.srv.add_lane(LaneSpec::probe(&fx.home, "p2", ProbePurpose::Run)).unwrap();
    let o = fx.ctl_on(&fx.home.join("probe/p2/run/kernel.sock"), &["gate", "--quick", "HEAD"]);
    assert_eq!((o.code, o.err_code()), (77, "E_LANE".to_string()), "{o:?}");
    // A run probe may still reload and read its status.
    assert_eq!(fx.ctl_on(&fx.home.join("probe/p2/run/kernel.sock"), &["ping"]).json()["lane"], "probe");
}

#[test]
fn ic21_status_fields() {
    let _wd = init_testkit::watchdog::arm("ic21_status_fields", WD);
    let fx = Fx::new("ic21");
    fx.fake.st().hold = true;
    assert_eq!(fx.ctl(&["promote", "evo/held"]).code, 0);
    fx.until("j-000001", "gating", |r| r.stages.len() >= 5);
    let o = fx.ctl(&["status"]);
    assert_eq!(o.code, 0, "{o:?}");
    let v = o.json();
    integers_only(&v, "");
    let bytes = std::fs::read(fx.home.join("kernel/CONSTITUTION.md")).unwrap();
    let d: [u8; 32] = <sha2::Sha256 as sha2::Digest>::digest(&bytes).into();
    let hex: String = d.iter().map(|x| format!("{x:02x}")).collect();
    assert_eq!(v["constitution_sha256"], format!("sha256:{hex}"));
    assert_eq!(
        init_proto::icj::to_string(&v["root"]).unwrap(),
        init_proto::icj::to_string(&fx.fake.st().root_status).unwrap(),
        "root relayed byte-equal"
    );
    assert_eq!(v["lane"], "live");
    assert!(v["link"]["connected"].as_bool().unwrap());
    assert!(v.get("fleet").is_some() && v["fleet"].is_null());
    assert_eq!(v["budget"]["soft"]["day_musd"], 5_000_000);
    assert_eq!(v["jobs"], json!([{"cmd": "promote", "id": "j-000001", "stage": "G3", "state": "running"}]));
    fx.fake.st().hold = false;
}

#[test]
fn ic22_final_response_written_before_pause() {
    let _wd = init_testkit::watchdog::arm("ic22_final_response_written_before_pause", WD);
    let fx = Fx::new("ic22");
    let mut r = fx.raw();
    fx.fake.st().probe = Some(r.s.try_clone().unwrap());
    r.send_line(br#"{"args":{"ref":"evo/order","wait":true},"cmd":"promote","id":"o1","v":1}"#);
    // Do not read until the fake has sent `pause`.
    let t = Instant::now();
    while fx.fake.st().pause_saw_final.is_none() {
        assert!(t.elapsed() < Duration::from_secs(10), "no pause");
        sleep_ms(2);
    }
    assert_eq!(fx.fake.st().pause_saw_final, Some(true), "the final response must be in the client's socket before pause");
    let mut last = Value::Null;
    while let Some(f) = r.frame() {
        let fin = f.get("ok").is_some();
        last = f;
        if fin {
            break;
        }
    }
    assert_eq!((last["id"].as_str(), last["result"]["state"].as_str()), (Some("o1"), Some("awaiting_boundary")));
}

#[test]
fn ic23_level_cache_refresh_and_link_down() {
    let _wd = init_testkit::watchdog::arm("ic23_level_cache_refresh_and_link_down", WD);
    let fx = Fx::new("ic23");
    fx.fake.st().ledger = Some(ledger_snapshot(0, 12));
    let o = fx.ctl(&["level"]);
    assert_eq!(o.code, 0);
    let v = o.json();
    assert_eq!((v["age_s"].as_u64(), v["ledger"]["level"].as_u64()), (Some(12), Some(0)));
    assert_eq!(v["ceilings"]["parallel_bodies"], 1);
    assert!(fx.fake.root_requests().is_empty(), "answered from cache");
    fx.fake.st().ledger = Some(ledger_snapshot(0, 61));
    let o = fx.ctl(&["level"]);
    assert_eq!(o.json()["age_s"], 0);
    assert_eq!(fx.fake.root_requests(), ["ledger.get", "ceilings.get"]);
    fx.fake.st().view.link_up = false;
    let o = fx.ctl(&["level"]);
    assert_eq!((o.code, o.err_code()), (75, "E_ROOT".to_string()));
    assert_eq!(o.json()["error"]["data"]["reason"], "link_down");
    fx.fake.st().view.link_up = true;
    fx.fake.st().ledger = Some(ledger_snapshot(0, 600));
    fx.fake.st().refresh_ok = false;
    assert_eq!(fx.ctl(&["level"]).err_code(), "E_ROOT");
}

#[test]
fn ic24_task_submit_admission() {
    let _wd = init_testkit::watchdog::arm("ic24_task_submit_admission", WD);
    let fx = Fx::new("ic24");
    let w = fx.home.join("work");
    for (name, fam) in [
        ("one", r#"{"instances":["a"]}"#),
        ("nondet", r#"{"instances":["a","b"],"deterministic":false}"#),
        ("four", r#"{"instances":["i1","i2","i3","i4"]}"#),
    ] {
        std::fs::create_dir_all(w.join(name)).unwrap();
        std::fs::write(w.join(name).join("family.json"), fam).unwrap();
    }
    let o = fx.ctl(&["task", "submit", "four"]);
    assert_eq!((o.code, o.err_code()), (77, "E_LEVEL".to_string()));
    assert!(fx.fake.calls().is_empty());
    fx.fake.set_level(1);
    let o = fx.ctl(&["task", "submit", "one"]);
    assert_eq!(o.code, 1, "admitted:false exits 1: {o:?}");
    let v = o.json();
    assert_eq!((v["admitted"].as_bool(), v["checks"][0]["rule"].as_str(), v["checks"][0]["ok"].as_bool()), (Some(false), Some("family_size"), Some(false)));
    let o = fx.ctl(&["task", "submit", "nondet"]);
    assert_eq!(o.code, 1);
    assert!(o.json()["checks"].as_array().unwrap().iter().any(|c| c["rule"] == "determinism" && c["ok"] == false));
    let o = fx.ctl(&["task", "submit", "four"]);
    assert_eq!(o.code, 0, "{o:?}");
    let v = o.json();
    assert_eq!((v["admitted"].as_bool(), v["sealed"].as_u64(), v["seq"].as_u64()), (Some(true), Some(2), Some(1000)));
    assert_eq!(v["visible"], json!(["i3", "i4"]));
    assert!(fx.fake.root_requests().contains(&"lineage.append task_admit".to_string()));
    // Paths: outside work/ and body/ → E_ARGS; missing → E_NOT_FOUND.
    std::fs::create_dir_all(fx.home.join("state/t")).unwrap();
    let o = fx.ctl(&["task", "submit", fx.home.join("state/t").to_str().unwrap()]);
    assert_eq!((o.code, o.err_code()), (64, "E_ARGS".to_string()));
    assert_eq!(fx.ctl(&["task", "submit", "nope"]).err_code(), "E_NOT_FOUND");
    assert_eq!(fx.ctl(&["task", "list"]).code, 0);
}

#[test]
fn ic25_fleet_ceiling_quota_and_lanes() {
    let _wd = init_testkit::watchdog::arm("ic25_fleet_ceiling_quota_and_lanes", WD);
    let fx = Fx::with("ic25", fast, Arc::new(SameUidPolicy));
    std::fs::write(fx.home.join("work/prompt.txt"), "improve the gate").unwrap();
    fx.fake.set_level(2);
    fx.fake.st().view.bodies = Some(BodyCount { limit: 2, used: 1 });
    let o = fx.ctl(&["fleet", "spawn", "--prompt-file", "prompt.txt"]);
    assert_eq!(o.code, 0, "{o:?}");
    assert_eq!(o.json()["bid"], "b1");
    let spawns = |f: &Fx| f.fake.root_requests().iter().filter(|r| r.as_str() == "spawn body").count();
    assert_eq!(spawns(&fx), 1);
    let o = fx.ctl(&["fleet", "spawn", "--prompt-file", "prompt.txt"]);
    assert_eq!((o.code, o.err_code()), (77, "E_CEILING".to_string()));
    assert_eq!(o.json()["error"]["data"], json!({"ceiling": "parallel_bodies", "limit": 2, "used": 2}));
    assert_eq!(spawns(&fx), 1, "no spawn reaches the Root past the ceiling");
    // fleet_spawns_day = 50.
    fx.fake.st().view.bodies = Some(BodyCount { limit: 1000, used: 1 });
    let mut r = fx.raw();
    for i in 2..=50 {
        let (_, f) = r.call(&format!("s{i}"), "fleet.spawn", json!({"prompt": "p"}));
        assert_eq!(f["ok"], true, "{i}: {f}");
    }
    let (_, f) = r.call("s51", "fleet.spawn", json!({"prompt": "p"}));
    assert_eq!((f["error"]["code"].as_str(), f["error"]["data"]["quota"].as_str(), f["error"]["data"]["limit"].as_u64()), (Some("E_QUOTA"), Some("fleet_spawns_day"), Some(50)));
    // Members cannot create or message members.
    std::fs::create_dir_all(fx.home.join("fleet/b1")).unwrap();
    fx.srv.add_lane(LaneSpec::member(&fx.home, "b1", &fx.home.join("fleet/b1"))).unwrap();
    let ms = fx.home.join("run/fleet/b1/kernel.sock");
    assert_eq!(fx.ctl_on(&ms, &["fleet", "spawn", "--prompt-file", "prompt.txt"]).code, 77);
    assert_eq!(fx.ctl_on(&ms, &["fleet", "send", "b1", "--text", "hi"]).err_code(), "E_LANE");
    // fleet stop at L0.
    fx.fake.set_level(0);
    let o = fx.ctl(&["fleet", "stop", "--all"]);
    assert_eq!(o.code, 0, "{o:?}");
    assert_eq!(o.json()["stopping"].as_array().unwrap().len(), 50);
}

#[test]
fn ic26_kernel_promote_level_and_gate_of_gates() {
    let _wd = init_testkit::watchdog::arm("ic26_kernel_promote_level_and_gate_of_gates", WD);
    let fx = Fx::new("ic26");
    fx.fake.st().stage_ms = 150;
    fx.fake.set_level(2);
    let o = fx.ctl(&["kernel", "promote", "kevo/gate-speedup"]);
    assert_eq!((o.code, o.err_code()), (77, "E_LEVEL".to_string()));
    assert!(!fx.fake.root_requests().iter().any(|r| r == "system.swap"), "no system.swap below L3");
    fx.fake.set_level(3);
    fx.fake.st().hold = true; // the swap waits after awaiting_swap
    let o = fx.ctl(&["kernel", "promote", "kevo/gate-speedup", "--wait"]);
    assert_eq!(o.code, 0, "{o:?}");
    let v = o.json();
    assert_eq!((v["state"].as_str(), v["root_job"].as_str()), (Some("awaiting_swap"), Some("s-000001")));
    let stages: Vec<&str> = v["stages"].as_array().unwrap().iter().map(|s| s["stage"].as_str().unwrap()).collect();
    assert_eq!(stages, ["K0", "K1", "K2", "K3"]);
    let roots = fx.fake.root_requests();
    assert_eq!(roots.iter().filter(|r| r.starts_with("store.put")).count(), 3);
    assert!(roots.contains(&"system.swap".to_string()));
    assert!(o.stderr.contains("gate_of_gates"), "{}", o.stderr);
    // While the candidate is pending, promote (live) and kernel promote are busy.
    assert_eq!(fx.ctl(&["promote", "evo/x"]).json()["error"]["data"]["reason"], "kernel_candidate");
    assert_eq!(fx.ctl(&["kernel", "promote", "kevo/other"]).err_code(), "E_BUSY");
    fx.fake.st().hold = false;
    let job = v["job"].as_str().unwrap().to_string();
    fx.until(&job, "swapped", |r| r.state == JobState::Succeeded);
    // gog_fail.
    fx.fake.st().gog_pass = false;
    let o = fx.ctl(&["kernel", "promote", "kevo/bad", "--wait"]);
    assert_eq!((o.code, o.err_code()), (1, "E_GATE_FAILED".to_string()), "{o:?}");
    assert_eq!(o.json()["error"]["data"]["stage"], "gate_of_gates");
    assert_eq!(fx.ctl(&["kernel", "status"]).code, 0);
}

#[test]
fn ic27_vendor_without_approval() {
    let _wd = init_testkit::watchdog::arm("ic27_vendor_without_approval", WD);
    let fx = Fx::new("ic27");
    let o = fx.ctl(&["vendor", "vt100@0.16.2"]);
    assert_eq!(o.code, 0, "{o:?}");
    let v = o.json();
    assert_eq!((v["crate"].as_str(), v["version"].as_str()), (Some("vt100"), Some("0.16.2")));
    assert!(v["lock"].as_str().unwrap().ends_with("/Cargo.lock"));
    assert!(!o.stderr.contains("approval"), "{}", o.stderr);
    let roots = fx.fake.root_requests();
    assert!(!roots.is_empty() && roots.iter().all(|r| r.starts_with("egress crates GET")), "{roots:?}");
    // --no-wait answers at once with the job.
    let o = fx.ctl(&["vendor", "serde@1.0.229", "--no-wait"]);
    assert_eq!(o.json()["job"], "j-000002");
}

#[test]
fn ic28_budget_and_model_set() {
    let _wd = init_testkit::watchdog::arm("ic28_budget_and_model_set", WD);
    let fx = Fx::new("ic28");
    assert_eq!(fx.ctl(&["budget", "set", "task_musd=100"]).err_code(), "E_LEVEL");
    assert_eq!(fx.ctl(&["model", "set", "deepseek/deepseek-flash"]).err_code(), "E_LEVEL");
    assert!(fx.fake.calls().is_empty());
    fx.fake.set_level(1);
    let o = fx.ctl(&["budget", "set", "a_musd=3000000", "b_musd=3000000"]);
    assert_eq!((o.code, o.err_code()), (1, "E_BUDGET".to_string()));
    assert_eq!(o.json()["error"]["data"], json!({"key": "day_musd", "limit": 5000000}));
    let o = fx.ctl(&["model", "set", "openai/not-in-charter"]);
    assert_eq!((o.code, o.err_code()), (64, "E_ARGS".to_string()));
    assert_eq!(fx.ctl(&["model", "set", "deepseek/deepseek-flash"]).code, 0);
    assert_eq!(fx.ctl(&["budget", "set", "task_musd=500000"]).code, 0);
}

#[test]
fn ic29_member_promote_qualifies_then_primary_adopts() {
    let _wd = init_testkit::watchdog::arm("ic29_member_promote_qualifies_then_primary_adopts", WD);
    let fx = Fx::new("ic29");
    std::fs::create_dir_all(fx.home.join("fleet/b1")).unwrap();
    fx.srv.add_lane(LaneSpec::member(&fx.home, "b1", &fx.home.join("fleet/b1"))).unwrap();
    let ms = fx.home.join("run/fleet/b1/kernel.sock");
    let o = fx.ctl_on(&ms, &["promote", "evo/m1", "--wait"]);
    assert_eq!(o.code, 0, "{o:?}");
    let v = o.json();
    let stages: Vec<&str> = v["stages"].as_array().unwrap().iter().map(|s| s["stage"].as_str().unwrap()).collect();
    assert!(stages.contains(&"G4"), "G4 against the system LKG: {stages:?}");
    let sha = v["sha"].as_str().unwrap().to_string();
    let job = v["job"].as_str().unwrap().to_string();
    fx.until(&job, "qualified", |r| r.state == JobState::Succeeded);
    assert!(fx.fake.st().qualified.contains(&sha));
    assert_eq!(fx.fake.st().current, LKG, "no LKG change");
    // The member's job is not visible from the live lane.
    assert_eq!(fx.ctl(&["job", "status", &job]).err_code(), "E_LANE");
    fx.fake.set_level(1);
    let o = fx.ctl(&["promote", "--sha", &sha]);
    assert_eq!((o.code, o.err_code()), (77, "E_LEVEL".to_string()));
    assert_eq!(o.json()["error"]["data"], json!({"have": 1, "need": 2}));
    fx.fake.set_level(2);
    fx.fake.st().hold = true;
    let o = fx.ctl(&["promote", "--sha", &sha]);
    assert_eq!(o.code, 0, "{o:?}");
    let adopt = o.json()["job"].as_str().unwrap().to_string();
    // Idempotent while active.
    assert_eq!(fx.ctl(&["promote", "--sha", &sha]).json()["job"], adopt.as_str());
    // A member lane cannot adopt.
    assert_eq!(fx.ctl_on(&ms, &["promote", "--sha", &sha]).err_code(), "E_LANE");
    fx.fake.st().hold = false;
}

#[test]
fn jobs_cancel_list_timeout_and_lanes() {
    let _wd = init_testkit::watchdog::arm("jobs_cancel_list_timeout_and_lanes", WD);
    let fx = Fx::new("jobs");
    fx.fake.st().stage_ms = 500;
    let o = fx.ctl(&["gate", "--quick", "HEAD", "--no-wait"]);
    assert_eq!(o.code, 0, "{o:?}");
    let v = o.json();
    assert_eq!((v["job"].as_str(), v["state"].as_str()), (Some("j-000001"), Some("queued")));
    assert_eq!(v["sha"], sha_of("HEAD"));
    fx.until("j-000001", "running", |r| r.state == JobState::Running);
    // A client timeout: exit 2, the job keeps running.
    let o = fx.ctl(&["--timeout", "1", "wait", "j-000001"]);
    assert_eq!(o.code, 2, "{o:?}");
    assert_eq!(o.json()["timed_out"], true);
    let o = fx.ctl(&["job", "list"]);
    assert_eq!(o.json()["jobs"].as_array().unwrap().len(), 1);
    let o = fx.ctl(&["job", "cancel", "j-000001"]);
    assert_eq!((o.code, o.json()["state"].as_str()), (0, Some("cancelled")));
    assert!(fx.fake.root_requests().contains(&"kill instances of j-000001".to_string()));
    let o = fx.ctl(&["wait", "j-000001"]);
    assert_eq!((o.code, o.json()["state"].as_str()), (1, Some("cancelled")));
    sleep_ms(400);
    assert_eq!(fx.srv.jobs().record("j-000001").unwrap().state, JobState::Cancelled, "late updates are ignored");
    assert!(fx.ctl(&["job", "list"]).json()["jobs"].as_array().unwrap().is_empty());
    assert_eq!(fx.ctl(&["job", "list", "--all"]).json()["jobs"].as_array().unwrap().len(), 1);
    // Past the milestone: not cancellable.
    fx.fake.st().stage_ms = 5;
    fx.fake.st().hold = false;
    assert_eq!(fx.ctl(&["promote", "evo/c", "--wait"]).code, 0);
    let o = fx.ctl(&["job", "cancel", "j-000002"]);
    assert!(o.code == 1, "{o:?}");
    assert!(["E_NOT_CANCELLABLE"].contains(&o.err_code().as_str()));
    // Another lane's job.
    fx.srv.add_lane(LaneSpec::probe(&fx.home, "p1", ProbePurpose::Probe)).unwrap();
    let o = fx.ctl_on(&fx.home.join("probe/p1/run/kernel.sock"), &["job", "status", "j-000001"]);
    assert_eq!((o.code, o.err_code()), (77, "E_LANE".to_string()));
    assert_eq!(fx.ctl(&["job", "status", "j-424242"]).err_code(), "E_NOT_FOUND");
}

#[test]
fn state_rows_end_to_end() {
    let _wd = init_testkit::watchdog::arm("state_rows_end_to_end", WD);
    let fx = Fx::new("rows");
    let set = |f: &dyn Fn(&mut KernelView)| {
        let mut s = fx.fake.st();
        s.view = KernelView::default();
        f(&mut s.view);
    };
    set(&|v| v.halted = Some("crash_loop_safe".into()));
    assert_eq!(fx.ctl(&["promote", "evo/x"]).err_code(), "E_HALTED");
    assert_eq!(fx.ctl(&["status"]).code, 0);
    set(&|v| v.stopping = true);
    let o = fx.ctl(&["ping"]);
    assert_eq!((o.code, o.err_code()), (75, "E_SHUTTING_DOWN".to_string()));
    set(&|v| v.safe_mode = true);
    let o = fx.ctl(&["gate", "--quick", "HEAD"]);
    assert_eq!((o.code, o.err_code()), (77, "E_SAFE_MODE".to_string()));
    set(&|v| v.disk_low = true);
    assert_eq!(fx.ctl(&["promote", "evo/x"]).code, 75);
    set(&|v| v.outbox_backlog = true);
    let o = fx.ctl(&["vendor", "vt100@0.16.2"]);
    assert_eq!((o.err_code(), o.json()["error"]["data"]["reason"].as_str()), ("E_ROOT".to_string(), Some("outbox_backlog")));
    set(&|v| {
        v.level = Some(1);
        v.ledger_age_s = Some(200);
    });
    let o = fx.ctl(&["task", "list"]);
    assert_eq!((o.code, o.json()["error"]["data"]["reason"].as_str()), (75, Some("stale")));
    set(&|v| v.provisional_depth = 3);
    assert_eq!(fx.ctl(&["promote", "evo/x"]).err_code(), "E_PROVISIONAL_DEPTH");
    assert!(fx.fake.calls().iter().all(|c| c == "status"), "{:?}", fx.fake.calls());
}

#[test]
fn gate_results_and_dotted_errors() {
    let _wd = init_testkit::watchdog::arm("gate_results_and_dotted_errors", WD);
    let fx = Fx::new("gate");
    let o = fx.ctl(&["gate", "--quick", "HEAD"]);
    assert_eq!(o.code, 0, "{o:?}");
    let v = o.json();
    assert_eq!((v["pass"].as_bool(), v["report"].as_str()), (Some(true), Some("state/kernel/gate/j-000001/report.json")));
    assert!(fx.fake.calls()[0].starts_with(&format!("gate {}", fx.home.join("body").display())), "{:?}", fx.fake.calls());
    fx.fake.st().gate_pass = false;
    let o = fx.ctl(&["gate", "--quick", "HEAD"]);
    assert_eq!(o.code, 1, "pass:false exits 1");
    let v = o.json();
    assert_eq!(v["pass"], false);
    assert!(v["stages"].as_array().unwrap().iter().any(|s| s["code"] == "gate.g1.self_json_drift"));
    // A gate failure inside promote --wait: the stage's dotted code with {stage, reason, report}.
    let o = fx.ctl(&["promote", "evo/drift", "--wait"]);
    assert_eq!((o.code, o.err_code()), (1, "gate.g1.self_json_drift".to_string()));
    assert_eq!(o.json()["error"]["data"]["stage"], "G1");
    // Validator codes with details[].
    let o = fx.ctl(&["promote", "evo/no-trailers"]);
    assert_eq!((o.code, o.err_code()), (1, "trailer.missing:Predicts".to_string()));
    assert_eq!(o.json()["error"]["details"].as_array().unwrap().len(), 2);
    assert_eq!(fx.ctl(&["promote", "evo/missing"]).err_code(), "E_NOT_FOUND");
    // The failed synchronous parts left no job behind.
    assert_eq!(fx.ctl(&["job", "list", "--all"]).json()["jobs"].as_array().unwrap().len(), 3);
}

#[test]
fn reload_rollback_and_idle() {
    let _wd = init_testkit::watchdog::arm("reload_rollback_and_idle", WD);
    let fx = Fx::with("rel", |c| c.limits.idle = Duration::from_millis(300), Arc::new(SameUidPolicy));
    fx.fake.st().stage_ms = 200;
    let a = fx.ctl(&["reload"]).json();
    assert_eq!((a["job"].as_str(), a["state"].as_str()), (Some("j-000001"), Some("awaiting_boundary")));
    assert_eq!(fx.ctl(&["reload"]).json()["job"], "j-000001", "a pending reload is returned (idempotent)");
    assert!(fx.fake.events().contains(&Ev::Body("reload j-000001 request".into())));
    fx.until("j-000001", "done", |r| r.state == JobState::Succeeded);
    assert_eq!(fx.srv.jobs().record("j-000001").unwrap().result.unwrap()["prefix_changed"], true);
    fx.fake.st().no_body = true;
    assert_eq!(fx.ctl(&["reload"]).err_code(), "E_NO_BODY");
    // Rollback.
    assert_eq!(fx.ctl(&["rollback"]).err_code(), "E_ALREADY_LKG");
    fx.fake.st().current = sha_of("evo/x");
    let o = fx.ctl(&["rollback", "--reason", "G4 will fail", "--wait"]);
    assert_eq!(o.code, 0, "{o:?}");
    let v = o.json();
    assert_eq!((v["state"].as_str(), v["to"].as_str(), v["from"].as_str()), (Some("awaiting_boundary"), Some(LKG), Some(sha_of("evo/x").as_str())));
    // Idle connections close after the idle timeout.
    let mut r = fx.raw();
    sleep_ms(600);
    assert!(r.closed());
}

/// One `ping` on a fresh connection: the final answer, or the connect/read error.
fn ping_raw(sock: &std::path::Path) -> Result<Value, String> {
    use std::io::{BufRead, Write};
    let mut s = std::os::unix::net::UnixStream::connect(sock).map_err(|e| format!("connect: {e}"))?;
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(b"{\"args\":{},\"cmd\":\"ping\",\"id\":\"p\",\"v\":1}\n").map_err(|e| format!("send: {e}"))?;
    let mut l = String::new();
    std::io::BufReader::new(s).read_line(&mut l).map_err(|e| format!("read: {e}"))?;
    serde_json::from_str(&l).map_err(|e| format!("{e}: {l:?}"))
}

fn is_starting(v: &Value) -> bool {
    v["ok"] == false && v["error"]["code"] == "E_BUSY" && v["error"]["data"]["reason"] == STARTING
}

/// FIX6: `run/kernel.sock` bound before the server exists. Every request gets
/// `E_BUSY{reason:"starting"}` with its own `id` (retry: yes, CLI exit 75) and the connection is
/// closed; a malformed line still gets `E_PROTO`. The server then adopts the same socket (same
/// inode): a client looping over connect + ping during the takeover never fails to connect and
/// sees `starting`, then only real answers. An early lane dropped unadopted unlinks its socket;
/// a non-socket file at the path is refused and kept.
#[test]
fn early_lane_answers_starting_then_the_server_adopts_it() {
    let _wd = init_testkit::watchdog::arm("early_lane_answers_starting_then_the_server_adopts_it", WD);
    use std::os::unix::fs::MetadataExt;
    let home = test_home("fix6-early");
    std::fs::write(home.join("kernel/CONSTITUTION.md"), "# CONSTITUTION v0.3 (test copy)\n").unwrap();
    let sock = home.join("run/kernel.sock");
    drop(std::os::unix::net::UnixListener::bind(&sock).unwrap()); // a stale socket of an earlier kernel
    let limits = Limits { rate_per_sec: 100_000, burst: 100_000, ..Limits::default() };
    let early = EarlyLane::bind(LaneSpec::live(&home), limits).unwrap();
    assert_eq!(early.socket(), sock.as_path());
    let ino = std::fs::metadata(&sock).unwrap().ino();

    let mut r = Raw::connect(&sock);
    let (progress, f) = r.call("e-1", "promote", json!({"ref": "evo/x"}));
    assert!(progress.is_empty());
    assert_eq!((f["id"].as_str(), f["ok"].as_bool()), (Some("e-1"), Some(false)), "{f}");
    assert!(is_starting(&f), "{f}");
    assert_eq!(f["error"]["data"]["retry_after_ms"], 1000, "{f}");
    assert!(r.closed(), "one answer, then the early lane closes");
    let o = run_initctl(&home, &sock, &["status"]);
    assert_eq!(o.code, 75, "E_BUSY is EX_TEMPFAIL: {o:?}");
    assert!(is_starting(&json!({"ok": false, "error": o.json()["error"]})), "{o:?}");
    let mut r = Raw::connect(&sock);
    r.send_line(b"not json");
    assert_eq!(r.frame().unwrap()["error"]["code"], "E_PROTO");

    // The takeover, under a client that keeps connecting.
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let client = {
        let (sock, stop) = (sock.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut seen = Vec::new();
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                seen.push(ping_raw(&sock));
                sleep_ms(2);
            }
            seen
        })
    };
    sleep_ms(150);
    let fake = Fake::new();
    let mut cfg = ServeConfig::new(&home, "0.1.0");
    fast(&mut cfg);
    let srv = kernel_serve::start(cfg, fake, Arc::new(SameUidPolicy)).unwrap();
    srv.adopt_lane(early).unwrap();
    sleep_ms(150);
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let seen = client.join().unwrap();
    assert_eq!(std::fs::metadata(&sock).unwrap().ino(), ino, "the same socket: never unbound in between");
    let errs: Vec<&String> = seen.iter().filter_map(|r| r.as_ref().err()).collect();
    assert!(errs.is_empty(), "every connect and answer succeeded across the takeover: {errs:?}");
    let answers: Vec<&Value> = seen.iter().filter_map(|r| r.as_ref().ok()).collect();
    let first_real = answers.iter().position(|v| !is_starting(v)).expect("the server answered");
    assert!(first_real > 0, "the early lane answered first");
    assert!(answers[first_real..].iter().all(|v| v["ok"] == true && v["result"]["lane"] == "live"), "after the takeover only the server answers: {:?}", &answers[first_real..]);
    let o = run_initctl(&home, &sock, &["ping"]);
    assert_eq!(o.code, 0, "{o:?}");
    srv.shutdown();
    assert!(!sock.exists(), "the server unlinks the adopted socket at shutdown");

    // Dropped unadopted: the socket goes with it.
    let early = EarlyLane::bind(LaneSpec::live(&home), Limits::default()).unwrap();
    assert!(server::is_socket(&sock));
    drop(early);
    assert!(!sock.exists());
    // A non-socket file at the path is refused, not removed.
    std::fs::write(&sock, b"x").unwrap();
    assert!(EarlyLane::bind(LaneSpec::live(&home), Limits::default()).is_err());
    assert!(sock.is_file());
    std::fs::remove_dir_all(&home).unwrap(); // exact path
}
