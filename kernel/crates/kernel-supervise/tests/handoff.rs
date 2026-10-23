//! The overlapped handoff and body rollback end to end (supervise.md §3.6.4, §3.6.5, §3.6.7;
//! SV-5, SV-6, SV-8, SV-9, SV-10, SV-14), with a real `lineage/body.git`.

mod common;

use common::*;
use init_proto::root_link::*;
use kernel_supervise::machine::HandoffKind;
use kernel_supervise::supervisor::{Cmd, Handle};
use serde_json::json;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

const T: Duration = Duration::from_secs(30);
/// The watchdog bound of every test here (K-FLAKE: 3-10x its normal runtime; init_testkit::watchdog).
const WD: Duration = Duration::from_secs(80);

fn ino(p: &Path) -> u64 {
    std::fs::metadata(p).unwrap().ino()
}

struct Rig {
    home: std::path::PathBuf,
    fr: std::sync::Arc<FakeRoot>,
    h: Handle,
    code: mpsc::Receiver<i32>,
    x: String,
    y: String,
}

fn rig(name: &str, cfg: &str, plan: impl Fn(&str, &str) -> serde_json::Value) -> Rig {
    let home = make_home(name, cfg);
    let (x, y) = lineage(&home);
    set_plan(&home, plan(&x, &y));
    std::fs::copy(testbody(), home.join("bin/init-body.staged")).unwrap();
    let fr = FakeRoot::new(&home, Opts { tuple_sha: x.clone(), ..Opts::default() });
    let (h, code) = fr.start_in_process();
    fr.wait_for("body ready", T, |s| s.events.iter().any(|e| e.data["frame"]["text"].as_str().is_some_and(|t| t.starts_with("ready"))).then_some(()));
    Rig { home, fr, h, code, x, y }
}

impl Rig {
    fn promote(&self) -> Result<(), String> {
        let (tx, rx) = mpsc::channel();
        let staged_sha256 = sha256_file(&self.home.join("bin/init-body.staged"));
        assert!(self.h.send(Cmd::Handoff {
            kind: HandoffKind::Promote { job: "j-000001".into(), sha: self.y.clone(), staged_sha256, branch: Some("evo/t".into()) },
            reply: Some(tx),
        }));
        rx.recv_timeout(T).expect("handoff finished")
    }

    fn stop(self) {
        self.fr.request(&RootRequest::Stop(StopArgs { reason: "t".into(), grace_ms: 2000 }), T).unwrap();
        assert_eq!(self.code.recv_timeout(T).unwrap(), 0);
        remove_home(&self.home);
    }
}

#[test]
fn sv5_sv14_happy_handoff_keeps_the_conversation_and_moves_main() {
    let _wd = init_testkit::watchdog::arm("sv5_sv14_happy_handoff_keeps_the_conversation_and_moves_main", WD);
    // B takes 1.5 s to become ready, so the owner types while A is paused (SV-14).
    let r = rig("h-happy", "", |_, y| json!({ &y[..12]: {"ready_delay_ms": 1500} }));
    let old_ino = ino(&r.home.join("bin/init-body"));
    let staged_ino = ino(&r.home.join("bin/init-body.staged"));
    r.fr.owner_line("before");
    r.fr.wait_for("before", T, |s| s.events.iter().any(|e| e.data["frame"]["text"] == "echo: before").then_some(()));
    let (tx, rx) = mpsc::channel();
    {
        let fr = r.fr.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(600));
            fr.owner_line("during the handoff");
            let _ = tx.send(());
        });
    }
    assert_eq!(r.promote(), Ok(()));
    rx.recv_timeout(T).unwrap();
    r.fr.wait_for("input delivered to B", T, |s| s.events.iter().any(|e| e.data["frame"]["text"] == "echo: during the handoff").then_some(()));
    // bin/ renames: B's inode is init-body, A's is init-body.prev, .staged is gone.
    assert_eq!(ino(&r.home.join("bin/init-body")), staged_ino);
    assert_eq!(ino(&r.home.join("bin/init-body.prev")), old_ino);
    assert!(!r.home.join("bin/init-body.staged").exists());
    assert_eq!(main_of(&r.home), r.y);
    let g = r.fr.seen.0.lock().unwrap();
    let b = &g.spawns[1];
    assert_eq!((b.env["INIT_BOOT_REASON"].as_str(), b.env["INIT_SHA"].as_str()), ("promote", r.y.as_str()));
    assert!(b.program.ends_with("bin/init-body.staged"), "B is spawned from .staged before the rename");
    assert!(b.replaces.is_some());
    assert!(g.exits.iter().any(|x| x.code == Some(75)), "A exits 75");
    let p = g.lineage.iter().find(|l| l.kind == "promotion").expect("promotion reported");
    assert_eq!((p.data["sha"].as_str(), p.data["lkg"].as_bool()), (Some(r.y.as_str()), Some(false)));
    drop(g);
    let log = session_log(&r.home);
    assert_eq!(log.iter().filter(|e| e["text"] == "during the handoff").count(), 1, "exactly once in the log");
    assert_eq!(log.iter().filter(|e| e["type"] == "boot").count(), 2);
    let snap = r.h.snapshot().unwrap();
    assert!(matches!(snap.mode, kernel_supervise::machine::Mode::Provisional(_)));
    assert_eq!((snap.counters.swaps_ok, snap.counters.crashes_total), (1, 0));
    let sup: serde_json::Value = serde_json::from_slice(&std::fs::read(r.home.join("state/kernel/supervisor.json")).unwrap()).unwrap();
    assert_eq!((sup["current"]["sha"].as_str(), sup["current"]["provisional"].as_bool()), (Some(r.y.as_str()), Some(true)));
    assert!(sup["handoff"].is_null());
    let local = jsonl(&r.home.join("vault/lineage.jsonl"));
    assert!(local.iter().any(|l| l["kind"] == "handoff"));
    r.stop();
}

#[test]
fn sv6_candidate_crash_before_ready_resumes_a_with_update_failed() {
    let _wd = init_testkit::watchdog::arm("sv6_candidate_crash_before_ready_resumes_a_with_update_failed", Duration::from_secs(40));
    let r = rig("h-sv6", "", |_, y| json!({ &y[..12]: {"crash_before_ready": true} }));
    let before = sha256_file(&r.home.join("bin/init-body"));
    let e = r.promote().unwrap_err();
    assert!(e.contains("candidate exited"), "{e}");
    assert_eq!(sha256_file(&r.home.join("bin/init-body")), before, "bin/init-body unchanged");
    assert!(r.home.join("bin/init-body.staged").exists(), "kept for forensics");
    assert_eq!(main_of(&r.home), r.x);
    let uf: serde_json::Value = serde_json::from_slice(&std::fs::read(r.home.join("state/kernel/update-failed.json")).unwrap()).unwrap();
    assert_eq!((uf["stage"].as_str(), uf["exit_code"].as_i64()), (Some("handoff"), Some(101)));
    assert_eq!(uf["sha"], r.y.as_str());
    let snap = r.h.snapshot().unwrap();
    assert_eq!((snap.counters.swaps_failed, snap.counters.crashes_total), (1, 0));
    // A resumed: it re-locked, appended update_failed (branch-back) and still answers.
    r.fr.owner_line("still A");
    r.fr.wait_for("A answers", T, |s| s.events.iter().any(|e| e.data["frame"]["text"] == "echo: still A").then_some(()));
    assert!(session_log(&r.home).iter().any(|e| e["type"] == "update_failed" && e["stage"] == "handoff"));
    assert!(jsonl(&r.home.join("vault/lineage.jsonl")).iter().any(|l| l["kind"] == "handoff_failed"));
    assert!(!r.fr.lineage_kinds().contains(&"handoff_failed".to_string()), "failed swaps are non-events at the Root");
    r.stop();
}

#[test]
fn sv8_a_never_pauses_aborts_at_pause_timeout() {
    let _wd = init_testkit::watchdog::arm("sv8_a_never_pauses_aborts_at_pause_timeout", Duration::from_secs(50));
    let r = rig("h-sv8", "pause_timeout_s = 2\n", |x, _| json!({ &x[..12]: {"ignore_pause": true} }));
    let e = r.promote().unwrap_err();
    assert_eq!(e, "pause_timeout");
    let uf: serde_json::Value = serde_json::from_slice(&std::fs::read(r.home.join("state/kernel/update-failed.json")).unwrap()).unwrap();
    assert_eq!(uf["stage"], "pause");
    assert_eq!(r.fr.spawn_count(), 1, "B never spawned");
    r.stop();
}

#[test]
fn sv9_a_keeps_the_lock_so_the_fence_fails() {
    let _wd = init_testkit::watchdog::arm("sv9_a_keeps_the_lock_so_the_fence_fails", WD);
    let r = rig("h-sv9", "", |x, _| json!({ &x[..12]: {"keep_lock_on_pause": true} }));
    let e = r.promote().unwrap_err();
    assert!(e.starts_with("fence"), "{e}");
    let uf: serde_json::Value = serde_json::from_slice(&std::fs::read(r.home.join("state/kernel/update-failed.json")).unwrap()).unwrap();
    assert_eq!(uf["stage"], "fence");
    r.stop();
}

#[test]
fn sv10_crash_in_probation_rolls_back_to_the_previous_binary_and_main() {
    let _wd = init_testkit::watchdog::arm("sv10_crash_in_probation_rolls_back_to_the_previous_binary_and_main", Duration::from_secs(70));
    let r = rig("h-sv10", "", |_, y| json!({ &y[..12]: {"crash_after_ready_ms": 2500} }));
    let old_ino = ino(&r.home.join("bin/init-body"));
    assert_eq!(r.promote(), Ok(()));
    // B crashes inside probation → the previous binary is reinstalled and spawned at once.
    r.fr.wait_for("rollback spawn", T, |s| s.spawns.iter().any(|a| a.env["INIT_BOOT_REASON"] == "rollback").then_some(()));
    r.fr.wait_for("body_rollback", T, |s| s.lineage.iter().any(|l| l.kind == "body_rollback").then_some(()));
    assert_eq!(ino(&r.home.join("bin/init-body")), old_ino);
    assert_eq!(main_of(&r.home), r.x);
    assert_eq!(git(&r.home.join("lineage/body.git"), &r.home, &["rev-parse", &format!("refs/init/rolled-back/{}", r.y)]), r.y);
    let uf: serde_json::Value = serde_json::from_slice(&std::fs::read(r.home.join("state/kernel/update-failed.json")).unwrap()).unwrap();
    assert_eq!(uf["stage"], "probation");
    let boot: serde_json::Value = serde_json::from_slice(&std::fs::read(r.home.join("state/kernel/boot.json")).unwrap()).unwrap();
    assert_eq!((boot["reason"].as_str(), boot["sha"].as_str()), (Some("rollback"), Some(r.x.as_str())));
    assert_eq!(boot["update_failed"]["stage"], "probation");
    r.stop();
}

#[test]
fn initctl_rollback_hands_off_back_to_lkg() {
    let _wd = init_testkit::watchdog::arm("initctl_rollback_hands_off_back_to_lkg", Duration::from_secs(90));
    let r = rig("h-rollback", "", |_, _| json!({}));
    let lkg_ino = ino(&r.home.join("bin/init-body.lkg"));
    assert_eq!(r.promote(), Ok(()));
    let (tx, rx) = mpsc::channel();
    r.h.send(Cmd::Rollback { reason: "initctl".into(), immediate: true, reply: Some(tx) });
    assert_eq!(rx.recv_timeout(T).unwrap(), Ok(()));
    assert_eq!(ino(&r.home.join("bin/init-body")), lkg_ino);
    assert_eq!(main_of(&r.home), r.x);
    let snap = r.h.snapshot().unwrap();
    assert_eq!(snap.mode, kernel_supervise::machine::Mode::Normal);
    assert_eq!(snap.counters.rollbacks, 1);
    let g = r.fr.seen.0.lock().unwrap();
    assert_eq!(g.spawns.last().unwrap().env["INIT_BOOT_REASON"], "rollback");
    assert!(g.spawns.last().unwrap().program.ends_with("bin/init-body.lkg"));
    assert!(g.lineage.iter().any(|l| l.kind == "body_rollback" && l.data["to_sha"] == r.x.as_str()));
    drop(g);
    r.stop();
}

#[test]
fn sv37_g4_pass_store_refused_stays_provisional_then_lkg_commit() {
    let _wd = init_testkit::watchdog::arm("sv37_g4_pass_store_refused_stays_provisional_then_lkg_commit", Duration::from_secs(70));
    let home = make_home("h-sv37", "");
    let (x, y) = lineage(&home);
    std::fs::copy(testbody(), home.join("bin/init-body.staged")).unwrap();
    let fr = FakeRoot::new(&home, Opts { tuple_sha: x.clone(), refuse_store_puts: 1, ..Opts::default() });
    let (h, code) = fr.start_in_process();
    fr.wait_for("body ready", T, |s| s.events.iter().any(|e| e.data["frame"]["text"].as_str().is_some_and(|t| t.starts_with("ready"))).then_some(()));
    let r = Rig { home: home.clone(), fr: fr.clone(), h: h.clone(), code, x: x.clone(), y: y.clone() };
    assert_eq!(r.promote(), Ok(()));
    // First G4 pass: the Root refuses the store → still provisional, `lkg_store_failed`.
    let (tx, rx) = mpsc::channel();
    h.send(Cmd::G4Pass { sha: y.clone(), job: "j-g4".into(), reply: Some(tx) });
    assert!(rx.recv_timeout(T).unwrap().unwrap_err().contains("store.put"));
    assert!(matches!(h.snapshot().unwrap().mode, kernel_supervise::machine::Mode::Provisional(_)));
    assert!(jsonl(&home.join("state/kernel/alerts.jsonl")).iter().any(|a| a["code"] == "lkg_store_failed"));
    // Then accepted: store.put, promotion{lkg:true}, LKG install, lkg.json.lineage_seq.
    let (tx, rx) = mpsc::channel();
    h.send(Cmd::G4Pass { sha: y.clone(), job: "j-g4".into(), reply: Some(tx) });
    assert_eq!(rx.recv_timeout(T).unwrap(), Ok(()));
    assert_eq!(ino(&home.join("bin/init-body.lkg")), ino(&home.join("bin/init-body")));
    let lkg: serde_json::Value = serde_json::from_slice(&std::fs::read(home.join("state/kernel/lkg.json")).unwrap()).unwrap();
    assert_eq!((lkg["sha"].as_str(), lkg["reason"].as_str(), lkg["prev_sha"].as_str()), (Some(y.as_str()), Some("g4_pass"), Some(x.as_str())));
    assert!(lkg["lineage_seq"].as_u64().is_some());
    assert_eq!(git(&home.join("lineage/body.git"), &home, &["rev-parse", "refs/heads/lkg"]), y);
    assert!(home.join(format!("lineage/trees/{}/SELF.md", &y[..12])).exists(), "LKG tree exported");
    let g = fr.seen.0.lock().unwrap();
    assert_eq!(g.store_puts.len(), 2);
    assert!(g.lineage.iter().any(|l| l.kind == "promotion" && l.data["lkg"] == true && l.data["sha"] == y.as_str()));
    drop(g);
    assert_eq!(h.snapshot().unwrap().mode, kernel_supervise::machine::Mode::Normal);
    r.stop();
}

#[test]
fn sv12_g4_fail_rolls_back_at_idle() {
    let _wd = init_testkit::watchdog::arm("sv12_g4_fail_rolls_back_at_idle", Duration::from_secs(100));
    let r = rig("h-g4fail", "", |_, _| json!({}));
    assert_eq!(r.promote(), Ok(()));
    r.h.send(Cmd::G4Fail { sha: r.y.clone(), job: "j-g4".into(), reason: "reject".into() });
    r.fr.wait_for("rollback spawn", T, |s| s.spawns.iter().any(|a| a.env["INIT_BOOT_REASON"] == "rollback").then_some(()));
    r.fr.wait_for("body_rollback", T, |s| s.lineage.iter().any(|l| l.kind == "body_rollback").then_some(()));
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(main_of(&r.home), r.x);
    assert!(r.fr.lineage_kinds().contains(&"gate".to_string()));
    r.stop();
}
