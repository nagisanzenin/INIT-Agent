//! The Root's isolated conformance run. No live proxy, lineage restore or evolution jobs.
//! The Root supplies the LKG binary and owns the check child, including cleanup when
//! this kernel exits during a spawn or check. The probe has no `kill` command.
use crate::config::{self, KernelCfg};
use crate::instance::{SpawnError, SpawnRequest, Spawned, Spawner};
use crate::link::{Inbound, RootLink};
use init_proto::envelope::{ErrorBody, Response};
use init_proto::errors::ErrorCode;
use init_proto::root_link::{
    BootBlock, Ceilings, HbData, InstanceKind, KernelEvent, KernelRequest, RootRequest,
};
use serde_json::{Value, json};
use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

const RPC: Duration = Duration::from_secs(10);
const CHECK: Duration = Duration::from_secs(90);
const WALL: Duration = Duration::from_secs(150);

enum Stage {
    Ready(Receiver<Response>, Instant),
    Ceilings(Receiver<Response>, Instant),
    Spawn(Receiver<Result<Spawned, SpawnError>>, Instant),
    Body(String, Instant),
    AwaitStop,
}

fn boot_record(home: &Path, cfg: &KernelCfg, sha: &str) -> Value {
    json!({"v": 1, "body_id": "kprobe", "boot_id": crate::sys::random_hex(8),
        "boot_seq": 0, "session": format!("kp-{}", crate::sys::random_hex(6)),
        "sha": sha, "reason": "cold", "mode": "check", "branch": null,
        "cause": null, "crash": null, "from_sha": null, "provisional": null,
        "safe": null, "update_failed": null, "spawned_at": crate::clock::now_rfc3339(),
        "model": {"provider": cfg.model.provider, "model": cfg.model.id, "effort": cfg.model.effort},
        "env": {"init_home": home, "kernel_version": crate::supervisor::KERNEL_VERSION}})
}

fn check_request(home: &Path, cfg: &KernelCfg, boot: &Value) -> SpawnRequest {
    let run_dir = home.join("run");
    let test_vars = crate::spawnspec::own_test_vars();
    let env = crate::spawnspec::body_env(&crate::spawnspec::BodyEnv {
        root: home,
        run_dir: &run_dir,
        session: boot["session"].as_str().unwrap(),
        sha: boot["sha"].as_str().unwrap(),
        boot_id: boot["boot_id"].as_str().unwrap(),
        boot_reason: "cold",
        body_id: "kprobe",
        mode: "check",
        kernel_version: crate::supervisor::KERNEL_VERSION,
        sandbox: &cfg.sandbox,
        test_mode: cfg.test_mode,
        test_vars: &test_vars,
        access: None,
    });
    SpawnRequest {
        kind: InstanceKind::Body,
        profile: "body".into(),
        params: crate::spawnspec::body_params(
            home,
            &crate::spawnspec::BodyLane::Live,
            &cfg.sandbox,
        ),
        program: home.join("bin/init-body"),
        args: vec![],
        env,
        cwd: home.join("work"),
        lane: Some("b0".into()),
        replaces: None,
    }
}

fn ready_matches(ready: &Value, boot: &Value) -> bool {
    ready["mode"] == "check"
        && ready["boot_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty() && boot["boot_id"] == id)
}

/// Ready and ceilings requests never block handling of a Root stop/swap. Body spawn
/// runs on a separate thread because fd handover can take seconds. Root's own probe
/// independently verifies the check body's ready marker, exit and cleanup.
pub(crate) fn run(
    home: &Path,
    boot: &BootBlock,
    link: Arc<RootLink>,
    mut poll: impl FnMut(Duration) -> Option<Inbound>,
) -> i32 {
    let cfg = match config::load(&home.join("kernel/config.toml")) {
        Ok(cfg) => cfg,
        Err(errors) => {
            for error in errors {
                crate::klog::log("kupgrade-smoke", &error.to_string());
            }
            return 78;
        }
    };
    let body_boot = boot_record(home, &cfg, &boot.boot.tuple_body.sha);
    if let Err(error) = crate::fsx::write_json(&home.join("state/kernel/boot.json"), &body_boot) {
        crate::klog::log("kupgrade-smoke", &error.to_string());
        return 78;
    }
    let (_, ready) = match link.send(&KernelRequest::Ready) {
        Ok(v) => v,
        Err(_) => return 69,
    };
    let mut stage = Stage::Ready(ready, Instant::now());
    let started = Instant::now();
    let hb_interval = Duration::from_secs(boot.limits.hb_interval_s.clamp(1, 5));
    let mut heartbeat = Instant::now();
    link.event(&KernelEvent::Hb(HbData {
        busy: true,
        mode: "normal".into(),
        sha12: String::new(),
        slot: "kupgrade_smoke".into(),
    }));
    loop {
        if !link.is_alive() {
            return 69;
        }
        if started.elapsed() >= WALL {
            return 70;
        }
        if heartbeat.elapsed() >= hb_interval {
            link.event(&KernelEvent::Hb(HbData {
                busy: true,
                mode: "normal".into(),
                sha12: String::new(),
                slot: "kupgrade_smoke".into(),
            }));
            heartbeat = Instant::now();
        }
        if let Some(inbound) = poll(Duration::from_millis(20)) {
            match inbound {
                Inbound::Lost => return 69,
                Inbound::Request { id, req } => match req {
                    RootRequest::Stop(_) => {
                        link.respond(&id, Ok(json!({})));
                        return 0;
                    }
                    RootRequest::PrepareSwap(_) => {
                        // This isolated probe has no persistent state to transfer. Production
                        // kstate export/restore is a separate, still required K8 step.
                        link.respond(&id, Ok(json!({})));
                        return 75;
                    }
                    RootRequest::OwnerIn(_) => link.respond(
                        &id,
                        Err(ErrorBody::new(
                            ErrorCode::SafeMode,
                            "kernel conformance probe accepts no owner work",
                        )),
                    ),
                },
                Inbound::Event(_) => {} // exits are also retained on RootLink's board
            }
        }
        stage = match stage {
            Stage::Ready(rx, at) => match rx.try_recv() {
                Ok(response) if response.result.is_ok() => {
                    let (_, reply) = match link.send(&KernelRequest::CeilingsGet) {
                        Ok(v) => v,
                        Err(_) => return 69,
                    };
                    Stage::Ceilings(reply, Instant::now())
                }
                Ok(_) | Err(TryRecvError::Disconnected) => return 69,
                Err(TryRecvError::Empty) if at.elapsed() >= RPC => return 70,
                Err(TryRecvError::Empty) => Stage::Ready(rx, at),
            },
            Stage::Ceilings(rx, at) => match rx.try_recv() {
                Ok(response) => {
                    if response
                        .result
                        .ok()
                        .and_then(|v| serde_json::from_value::<Ceilings>(v).ok())
                        .is_none()
                    {
                        return 69;
                    }
                    let request = check_request(home, &cfg, &body_boot);
                    let (tx, rx) = mpsc::channel();
                    let spawner = link.clone();
                    if std::thread::Builder::new()
                        .name("kprobe-spawn".into())
                        .spawn(move || {
                            let _ = tx.send(spawner.spawn(&request));
                        })
                        .is_err()
                    {
                        return 70;
                    }
                    Stage::Spawn(rx, Instant::now())
                }
                Err(TryRecvError::Disconnected) => return 69,
                Err(TryRecvError::Empty) if at.elapsed() >= RPC => return 70,
                Err(TryRecvError::Empty) => Stage::Ceilings(rx, at),
            },
            Stage::Spawn(rx, at) => match rx.try_recv() {
                Ok(Ok(child)) => {
                    let instance = child.instance;
                    drop(child.stdin); // check mode consumes no owner input
                    for pipe in [child.stdout, child.stderr] {
                        if std::thread::Builder::new()
                            .name("kprobe-drain".into())
                            .spawn(move || {
                                let _ = std::io::copy(&mut File::from(pipe), &mut std::io::sink());
                            })
                            .is_err()
                        {
                            return 70;
                        }
                    }
                    Stage::Body(instance, at)
                }
                Ok(Err(error)) => {
                    crate::klog::log("kupgrade-smoke", &error.to_string());
                    return 70;
                }
                Err(TryRecvError::Disconnected) => return 69,
                Err(TryRecvError::Empty) if at.elapsed() >= RPC + Duration::from_secs(5) => {
                    return 70;
                }
                Err(TryRecvError::Empty) => Stage::Spawn(rx, at),
            },
            Stage::Body(instance, at) => {
                if let Some(exit) = link.board().get(&instance) {
                    let read = |p: &str| {
                        std::fs::read(home.join(p))
                            .ok()
                            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                            .unwrap_or(Value::Null)
                    };
                    // Root may replace the boot identity before spawning its sealed child.
                    if !exit.success()
                        || !ready_matches(&read("state/run/ready"), &read("state/kernel/boot.json"))
                    {
                        return 70;
                    }
                    Stage::AwaitStop
                } else if at.elapsed() >= CHECK {
                    return 70;
                } else {
                    Stage::Body(instance, at)
                }
            }
            Stage::AwaitStop => Stage::AwaitStop,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe_block() -> Value {
        json!({"boot":{"cause":null,"kstate":null,"level":1,"mode":"normal","panic":null,
                "reason":super::super::SMOKE_REASON,
                "tuple":{"current":"probe","lkg":"probe","settled":null,"audited":null,"twin":null},
                "tuple_body":{"binary":"b".repeat(64),"sha":"a".repeat(40),"ext":{"components":[],"set":null}},
                "tuple_kernel":{"built_from":"","init_kernel":"c".repeat(64),"source":"agent","version":"0.1.0"}},
                "charter_sha256":"d".repeat(64),"ladder":{"cap":1,"level":1,"evolution_share_max_permille":null},
                "limits":{"hb_interval_s":1,"max_instances":4,"parallel_bodies":1},
                "paths":{"egress_sock":"/absent/egress.sock","kernel_git":null},"root":{"proto":1,"version":"0.1.0"}})
    }

    #[test]
    fn check_spawn_has_model_and_no_owner_access() {
        let mut cfg = KernelCfg::default();
        cfg.model.provider = "custom".into();
        cfg.model.id = "configured-model".into();
        cfg.model.effort = Some("high".into());
        let home = Path::new("/probe");
        let boot = boot_record(home, &cfg, "lkg");
        let request = check_request(home, &cfg, &boot);
        assert_eq!(
            boot["model"],
            json!({"provider":"custom", "model":"configured-model", "effort":"high"})
        );
        assert_eq!(request.program, home.join("bin/init-body"));
        assert_eq!(request.env["INIT_MODE"], "check");
        assert_eq!(
            request.env["INIT_BOOT_ID"].as_str(),
            boot["boot_id"].as_str().unwrap()
        );
        assert_eq!(request.env["INIT_SHA"], "lkg");
        assert!(!request.env.contains_key("INIT_WORKSPACE"));
        assert!(!request.env.contains_key("INIT_ACCESS"));
        assert_eq!(request.kind, InstanceKind::Body);
    }

    #[test]
    fn readiness_requires_check_mode_and_current_boot_identity() {
        let boot = json!({"boot_id":"new"});
        assert!(ready_matches(
            &json!({"boot_id":"new","mode":"check"}),
            &boot
        ));
        for ready in [
            json!({"boot_id":"old","mode":"check"}),
            json!({"boot_id":"new","mode":"normal"}),
            Value::Null,
        ] {
            assert!(!ready_matches(&ready, &boot));
        }
        assert!(!ready_matches(&json!({"mode":"check"}), &Value::Null));
    }

    #[test]
    fn real_link_smoke_handles_stop_and_swap_without_waiting_for_ready_ack() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::{UnixDatagram, UnixStream};
        for (command, args, expected) in [
            ("stop", json!({"reason":"probe", "grace_ms":1000}), 0),
            ("prepare_swap", json!({"job":"probe", "deadline_s":10}), 75),
        ] {
            let home = crate::testutil::test_dir("kprobe-control");
            std::fs::create_dir_all(home.join("state/kernel")).unwrap();
            // Deliberately no lineage, body binary, proxy/egress or git installation.
            let (kernel, mut root) = UnixStream::pair().unwrap();
            let (pass, _root_pass) = UnixDatagram::pair().unwrap();
            root.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let (done, code) = mpsc::channel();
            let run_home = home.clone();
            let runner = std::thread::spawn(move || {
                let result = crate::supervisor::run(&run_home, kernel.into(), pass.into(), |_| {});
                let _ = done.send(result);
            });
            let mut reader = BufReader::new(root.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let hello: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(hello["cmd"], "hello");
            let block = probe_block();
            writeln!(
                root,
                "{}",
                json!({"v":1,"id":hello["id"],"ok":true,"result":block})
            )
            .unwrap();
            loop {
                line.clear();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                let frame: Value = serde_json::from_str(&line).unwrap();
                if frame["cmd"] == "ready" {
                    break;
                }
                assert_eq!(
                    frame["ev"], "hb",
                    "normal cold start must not request store/git/proxy work"
                );
            }
            // No ready response: lifecycle requests must interrupt a pending RPC.
            writeln!(
                root,
                "{}",
                json!({"v":1,"id":"r-1","cmd":command,"args":args})
            )
            .unwrap();
            loop {
                line.clear();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                let frame: Value = serde_json::from_str(&line).unwrap();
                if frame["id"] == "r-1" {
                    assert_eq!(frame["ok"], true);
                    break;
                }
                assert_eq!(frame["ev"], "hb");
            }
            assert_eq!(code.recv_timeout(Duration::from_secs(3)).unwrap(), expected);
            drop(reader);
            drop(root);
            runner.join().unwrap();
            crate::testutil::remove_test_dir(&home);
        }
    }

    #[test]
    fn check_body_rejects_stale_ready_and_nonzero_early_exit() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::fd::AsFd;
        use std::os::unix::net::{UnixDatagram, UnixStream};
        for (body_code, stale) in [(0, true), (78, false)] {
            let home = crate::testutil::test_dir("kprobe-body");
            std::fs::create_dir_all(home.join("state/kernel")).unwrap();
            std::fs::create_dir_all(home.join("state/run")).unwrap();
            let (kernel, mut root) = UnixStream::pair().unwrap();
            let (pass, root_pass) = UnixDatagram::pair().unwrap();
            root.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let (tx, rx) = mpsc::channel();
            let link =
                RootLink::start(kernel.into(), pass.into(), tx, Duration::from_secs(2)).unwrap();
            let (done, code) = mpsc::channel();
            let run_home = home.clone();
            let runner = std::thread::spawn(move || {
                let boot = serde_json::from_value(probe_block()).unwrap();
                let result = run(&run_home, &boot, link, |wait| rx.recv_timeout(wait).ok());
                let _ = done.send(result);
            });
            let mut reader = BufReader::new(root.try_clone().unwrap());
            let mut order = vec![];
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                let frame: Value = serde_json::from_str(&line).unwrap();
                let Some(command) = frame["cmd"].as_str() else {
                    continue;
                };
                order.push(command.to_string());
                let result = match command {
                    "ready" => json!({}),
                    "ceilings.get" => json!({"v":1,"level":1}),
                    "spawn" => {
                        assert_eq!(frame["args"]["env"]["INIT_MODE"], "check");
                        let boot: Value = serde_json::from_slice(
                            &std::fs::read(home.join("state/kernel/boot.json")).unwrap(),
                        )
                        .unwrap();
                        let id = if stale {
                            json!("previous")
                        } else {
                            boot["boot_id"].clone()
                        };
                        crate::fsx::write_json(
                            &home.join("state/run/ready"),
                            &json!({"mode":"check","boot_id":id}),
                        )
                        .unwrap();
                        let mut descriptors = vec![];
                        let mut peers = vec![];
                        for _ in 0..3 {
                            let (a, b) = UnixStream::pair().unwrap();
                            descriptors.push(a);
                            peers.push(b);
                        }
                        let tag = serde_json::to_vec(&json!({"id":frame["id"],"n":3})).unwrap();
                        crate::sys::send_fds(
                            root_pass.as_fd(),
                            &tag,
                            &descriptors.iter().map(AsFd::as_fd).collect::<Vec<_>>(),
                        )
                        .unwrap();
                        // The event can precede spawn's response; ExitBoard must retain it.
                        writeln!(root, "{}", json!({"v":1,"ev":"exited","data":{"instance":"check-1","code":body_code,"signal":null,"at":"now","rusage_ms":0}})).unwrap();
                        writeln!(root, "{}", json!({"v":1,"id":frame["id"],"ok":true,"result":{"instance":"check-1","pid":1,"started_at":"now"}})).unwrap();
                        drop(peers);
                        break;
                    }
                    other => panic!("unexpected live operation {other}"),
                };
                writeln!(
                    root,
                    "{}",
                    json!({"v":1,"id":frame["id"],"ok":true,"result":result})
                )
                .unwrap();
            }
            assert_eq!(order, ["ready", "ceilings.get", "spawn"]);
            assert_eq!(code.recv_timeout(Duration::from_secs(3)).unwrap(), 70);
            drop(reader);
            drop(root);
            runner.join().unwrap();
            crate::testutil::remove_test_dir(&home);
        }
    }
}
