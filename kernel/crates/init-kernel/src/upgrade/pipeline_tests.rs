//! Wiring test with real Git, a fixture build tool and a fake Root. No confinement claim.
use super::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixDatagram, UnixStream};
use std::process::Command;
use std::sync::atomic::AtomicBool;

// Validate the relevant Root path boundary too: a permissive local spawner alone
// previously hid the invalid Build-SRC=work import request.
struct CheckedSpawner { inner: kernel_supervise::instance::LocalSpawner, home: PathBuf }
impl Spawner for CheckedSpawner {
    fn spawn(&self, req: &SpawnRequest) -> Result<Spawned, SpawnError> {
        match req.kind {
            InstanceKind::GitRead => {
                assert_eq!(req.profile, "git-read");
                assert_eq!(req.cwd, self.home.join("work/kernel"));
                assert_eq!(req.params.get("WORK"), Some(&self.home.join("work").to_string_lossy().into_owned()));
                assert!(req.args.windows(3).any(|a| a == ["bundle", "create", "-"]));
            }
            InstanceKind::Build => {
                assert_eq!(req.profile, "build");
                for key in ["SRC", "OUT"] { assert!(Path::new(&req.params[key]).starts_with(self.home.join("cache"))); }
            }
            _ => panic!("unexpected preparation instance kind"),
        }
        self.inner.spawn(req)
    }
    fn kill(&self, instance: &str, mode: KillMode) -> Result<(), SpawnError> { self.inner.kill(instance, mode) }
    fn wait_exit(&self, instance: &str, timeout: Duration) -> Option<ExitInfo> { self.inner.wait_exit(instance, timeout) }
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("/usr/bin/git").arg("-C").arg(repo).args(args)
        .env("GIT_AUTHOR_NAME", "INIT Agent").env("GIT_AUTHOR_EMAIL", "agent@init.local")
        .env("GIT_COMMITTER_NAME", "INIT Agent").env("GIT_COMMITTER_EMAIL", "agent@init.local")
        .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().into()
}
fn commit(repo: &Path, name: &str) -> String {
    git(repo, &["add", "-A"]); git(repo, &["commit", "-qm", name]); git(repo, &["rev-parse", "HEAD"])
}
fn scenario(refuse_admission: bool) {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/kernel-upgrade");
    std::fs::create_dir_all(&base).unwrap();
    let home = base.canonicalize().unwrap().join(format!("pipeline-{}",kernel_supervise::sys::random_hex(4)));
    for path in ["state/kernel", "work/kernel", "lineage", "toolchain/bin"] { std::fs::create_dir_all(home.join(path)).unwrap(); }
    let repo = home.join("work/kernel");
    git(&repo, &["init", "-q", "--initial-branch=main"]);
    for name in ["Cargo.toml", "Cargo.lock", "crates/init-kernel/Cargo.toml", "crates/initctl/Cargo.toml"] {
        let path = repo.join(name); std::fs::create_dir_all(path.parent().unwrap()).unwrap(); std::fs::write(path,b"# fixture\n").unwrap();
    }
    let parent = commit(&repo,"parent");
    let root_git = Git::with_dir(Path::new("/usr/bin/git"),&home,&home.join("lineage/kernel.git"));
    root_git.init_bare(&root_git.git_dir).unwrap();
    root_git.fetch(&repo,"refs/heads/main","refs/heads/main","git-upload-pack").unwrap();
    git(&repo,&["checkout","-qb","kevo/test"]);
    std::fs::write(repo.join("change"),b"kernel candidate").unwrap();
    let sha = commit(&repo,"candidate");
    // This owned fixture tool writes test artifacts only; it never compiles untrusted source.
    let cargo = home.join("toolchain/bin/cargo");
    std::fs::write(&cargo,b"#!/bin/sh\nset -eu\nmkdir -p \"$CARGO_TARGET_DIR/debug\"\nfor name in init-kernel initctl; do\nprintf '#!/bin/sh\\nexit 0\\n' > \"$CARGO_TARGET_DIR/debug/$name\"\nchmod 700 \"$CARGO_TARGET_DIR/debug/$name\"\ndone\n").unwrap();
    std::fs::set_permissions(&cargo,std::fs::Permissions::from_mode(0o700)).unwrap();
    let tuple = json!({"body":{"binary":"b".repeat(64),"sha":"c".repeat(40)},"ext":{"components":[],"set":null},
        "kernel":{"built_from":parent,"init_kernel":"d".repeat(64),"initctl":"e".repeat(64),"source":"maintainer","version":"test"},"v":1});
    let boot: init_proto::root_link::BootBlock = serde_json::from_value(json!({
        "boot":{"cause":null,"kstate":null,"level":1,"mode":"normal","panic":null,"reason":"cold",
            "tuple":{"current":"t0","lkg":"t0","audited":null,"settled":null,"twin":null},
            "tuple_body":{"binary":"b".repeat(64),"sha":"c".repeat(40),"ext":{"components":[],"set":null}},
            "tuple_kernel":{"built_from":parent,"init_kernel":"d".repeat(64),"source":"maintainer","version":"test"}},
        "charter_sha256":"f".repeat(64),"ladder":{"cap":1,"level":1,"evolution_share_max_permille":null},
        "limits":{"hb_interval_s":5,"max_instances":4,"parallel_bodies":1},
        "paths":{"egress_sock":"/test/egress.sock","kernel_git":"/test/kernel.git"},"root":{"proto":1,"version":"test"},"extend":{"mode":"free"}
    })).unwrap();
    let (ctl_kernel, mut ctl_root) = UnixStream::pair().unwrap();
    let (fds_kernel, fds_root) = UnixDatagram::pair().unwrap();
    let (sink, _events) = std::sync::mpsc::channel();
    let link = kernel_supervise::link::RootLink::start(ctl_kernel.into(),fds_kernel.into(),sink,Duration::from_secs(10)).unwrap();
    let commands = Arc::new(Mutex::new(Vec::new())); let seen = commands.clone();
    let expected_sha = sha.clone();
    let root = std::thread::spawn(move || {
        ctl_root.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        let mut reader = BufReader::new(ctl_root.try_clone().unwrap());
        loop {
            let mut line=String::new(); if reader.read_line(&mut line).unwrap() == 0 { break; }
            let req: Value=serde_json::from_str(&line).unwrap();
            let cmd=req["cmd"].as_str().unwrap(); seen.lock().unwrap().push(cmd.to_string());
            let args=&req["args"];
            let result = match cmd {
                "tuple.get" => json!({"current":{"id":"t0","tuple":tuple},"lkg":{"id":"t0","tuple":tuple},"audited":null}),
                "store.put" => {
                    let (_,fds)=kernel_supervise::sys::recv_fds(fds_root.as_fd(),Some(Duration::from_secs(5))).unwrap().unwrap();
                    assert_eq!(fds.len(),1);
                    let mut file=std::fs::File::from(fds.into_iter().next().unwrap()); let mut bytes=Vec::new(); file.read_to_end(&mut bytes).unwrap();
                    assert_eq!(args["sha256"],kernel_supervise::fsx::sha256_bytes(&bytes));
                    assert_eq!(args["bytes"],bytes.len());
                    json!({"sha256":args["sha256"]})
                }
                "change.open" => {
                    assert_eq!(args["scope"],"k"); assert_eq!(args["sha"],expected_sha);
                    assert_eq!(args["candidate"]["body"],tuple["body"]); assert_eq!(args["candidate"]["ext"],tuple["ext"]);
                    if refuse_admission {
                        let response=init_proto::envelope::Response::err(req["id"].as_str().map(str::to_string),error("fake Root conformance refusal"));
                        ctl_root.write_all(&init_proto::icj::to_line(&response).unwrap()).unwrap(); break;
                    }
                    json!({"group":7,"seq":8,"snapshot":"snapshot"})
                }
                "system.swap" => { assert_eq!(args["group"],7); assert_eq!(args["source_sha"],expected_sha); json!({"job":"s-000001"}) }
                other => panic!("unexpected Root command {other}"),
            };
            let response=init_proto::envelope::Response::ok_value(req["id"].as_str().unwrap(),result);
            ctl_root.write_all(&init_proto::icj::to_line(&response).unwrap()).unwrap();
            if cmd=="system.swap" { break; }
        }
    });
    let mut cfg=kernel_supervise::config::KernelCfg::default(); cfg.supervise.git="/usr/bin/git".into(); cfg.sandbox.toolchain=home.join("toolchain");
    let sv=Services { home:home.clone(),cfg,spawner:Arc::new(CheckedSpawner { inner: kernel_supervise::instance::LocalSpawner::new(), home: home.clone() }),outbox:kernel_supervise::outbox::Outbox::open(&home,"test"),level:1,link,boot,extend_free:Arc::new(AtomicBool::new(true)) };
    let (jobs,_)=JobStore::open(&home.join("state/kernel/jobs"),kernel_serve::JobConfig::default()).unwrap();
    let job=jobs.create(&kernel_serve::LaneCtx::live(),"kernel.promote",json!({"ref":"kevo/test"})).unwrap();
    let upgrade=Upgrade::new(&home);
    *upgrade.state.lock().unwrap()=State { receipt:Some(json!({"v":1,"job":job.id(),"phase":"import","root_job":null})),job:Some(job.clone()) };
    let result=upgrade.run(&sv,"kevo/test",&job);
    root.join().unwrap();
    let commands=commands.lock().unwrap();
    if refuse_admission { assert!(result.is_err()); assert!(!commands.iter().any(|c| c=="system.swap")); }
    else { result.unwrap(); assert_eq!(job.record().unwrap().state,JobState::AwaitingSwap); assert_eq!(job.record().unwrap().sha,Some(sha)); assert!(!job.is_finished()); }
    assert_eq!(commands.iter().filter(|c| *c=="store.put").count(),2);
    drop(sv); drop(jobs); drop(upgrade); std::fs::remove_dir_all(home).unwrap();
}
#[test] fn kernel_upgrade_pipeline_reaches_root_swap_only_after_admission() { scenario(false); }
#[test] fn kernel_upgrade_pipeline_root_refusal_never_sends_swap() { scenario(true); }
