//! X2 (ADR-011 §2.3, lane K2-ADMIT) against the **real** `init-root`, at $0: the free path end to end
//! through the whole stack. The real Root (charter `[extend] mode = "free"`, test mode, a fake clock)
//! runs the real `init-kernel` under the Seatbelt `kernel` profile, which runs the real seed
//! `init-body` under the `body` profile; `init-mockllm` on loopback is the model. One owner line makes
//! the body (mock-scripted bash) write a T0 change — a sub-agent spec `agents/reviewer/AGENT.md` and a
//! `SELF.md` rule — commit it with the free trailer block (`Why`, `Prefix-change`, `Cap:`) and run
//! `initctl promote evo/rev --wait` from inside its sandbox. The kernel's free path stores the binary
//! and the `t0` blob, sends `change.open`, and the Root runs A0, X5's caps (computed from the kernel's
//! t0 tar) and X4's snapshot, appends `change_open` and `tuple`; the change goes live data-only. Then
//! the Root's clock moves past the 72 h window: the group closes `quiet`, the Root appends `lkg`, and
//! the kernel's `extend.window` handler moves its own LKG (`lkg.json.reason = "change_close"`).
//!
//! Needs (built beforehand, like A6): `root/target/debug/init-root` with `--features test-fixtures`
//! (`init-root test clock`), `init-kernel` and `initctl` in this target dir, `seed/target/debug/init-body`.
//! Run: `cargo test --offline -p init-testkit --test x2_real_root -- --ignored` (ignored by default:
//! it needs those three builds; ~1–2 minutes).
//!
//! Safety: the Root, the mock and every helper are this test's own children, stopped by held handles
//! (the Root by `init-root stop`, then SIGTERM to its held pid). A `test-` key only. The run dir
//! `target/x2r/` is removed by exact path on success.

use init_testkit::mockllm::http::{Listen, MockServer};
use init_testkit::mockllm::{Mock, MockConfig, Mode, Script};
use init_testkit::util;
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const WD: Duration = Duration::from_secs(420);

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap()
}

fn base() -> PathBuf {
    repo().join("target/x2r")
}

fn free_gib() -> f64 {
    let o = init_testkit::watchdog::output(Command::new("/bin/df").arg("-k").arg(repo())).unwrap();
    let t = String::from_utf8_lossy(&o.stdout);
    t.lines().nth(1).and_then(|l| l.split_whitespace().nth(3)).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0) / (1024.0 * 1024.0)
}

fn wait_for<T>(what: &str, t: Duration, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + t;
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let o = init_testkit::watchdog::output(Command::new("/usr/bin/git")
        .args(args)
        .current_dir(dir)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", base())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "INIT Seed")
        .env("GIT_AUTHOR_EMAIL", "seed@init.invalid")
        .env("GIT_COMMITTER_NAME", "INIT Seed")
        .env("GIT_COMMITTER_EMAIL", "seed@init.invalid")
        .env("GIT_AUTHOR_DATE", "2026-10-20T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-10-20T00:00:00Z"))
        .unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap().trim().to_string()
}

fn pipe_tar(mut producer: Command, dst: &Path, strip: u32) {
    fs::create_dir_all(dst).unwrap();
    let mut p = init_testkit::watchdog::spawn(producer.stdout(Stdio::piped())).unwrap();
    let st = init_testkit::watchdog::status(Command::new("/usr/bin/tar").args(["-x", "--strip-components", &strip.to_string(), "-C"]).arg(dst).stdin(p.stdout.take().unwrap())).unwrap();
    assert!(st.success() && p.wait().unwrap().success(), "tar into {}", dst.display());
}

fn root_cli(rh: &Path, args: &[&str]) -> std::process::Output {
    init_testkit::watchdog::output(Command::new(rh.join("bin/init-root"))
        .args(args)
        .env_clear()
        .env("HOME", std::env::var_os("HOME").unwrap())
        .env("INIT_ROOT_HOME", rh)
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null()))
        .unwrap()
}

fn status(rh: &Path) -> Value {
    serde_json::from_slice(&root_cli(rh, &["status", "--json"]).stdout).unwrap_or(Value::Null)
}

fn records(rh: &Path, kind: &str) -> Vec<Value> {
    util::read_jsonl(&rh.join("lineage/lineage.jsonl")).into_iter().filter(|r| r["kind"] == kind).collect()
}

fn events(ih: &Path) -> Vec<Value> {
    let mut files: Vec<PathBuf> = fs::read_dir(ih.join("state/sessions")).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    files.retain(|p| p.extension().is_some_and(|x| x == "jsonl"));
    files.sort();
    files.iter().flat_map(|f| util::read_jsonl(f)).collect()
}

struct Stack {
    rh: PathBuf,
    root: Option<Child>,
    mock: Option<MockServer>,
}

impl Drop for Stack {
    fn drop(&mut self) {
        if let Some(mut c) = self.root.take() {
            if c.try_wait().ok().flatten().is_none() {
                let _ = root_cli(&self.rh, &["stop"]);
                let deadline = Instant::now() + Duration::from_secs(20);
                while Instant::now() < deadline && status(&self.rh)["instances"].as_array().is_some_and(|a| !a.is_empty()) {
                    std::thread::sleep(Duration::from_millis(250));
                }
                let _ = init_testkit::watchdog::status(Command::new("/bin/kill").args(["-TERM", &c.id().to_string()]));
                let deadline = Instant::now() + Duration::from_secs(15);
                while Instant::now() < deadline && c.try_wait().ok().flatten().is_none() {
                    std::thread::sleep(Duration::from_millis(100));
                }
                if c.try_wait().ok().flatten().is_none() {
                    let _ = c.kill();
                }
            }
            let _ = c.wait();
        }
        if let Some(m) = self.mock.take() {
            m.stop();
        }
        let _ = fs::remove_file(self.rh.join("keys/providers/deepseek"));
    }
}

const COMMIT: &str = "cd \"@IH@/body\" && /usr/bin/git checkout -q -b evo/rev && mkdir -p agents/reviewer && printf 'role: review diffs\\n' > agents/reviewer/AGENT.md && printf '\\nI review my own diffs before I promote them.\\n' >> SELF.md && /usr/bin/git add -A && /usr/bin/git -c user.name='INIT Agent' -c user.email=agent@init.local commit -q -F \"@IH@/work/msg.txt\" && \"@IH@/kernel/bin/initctl\" promote evo/rev --wait";

#[test]
#[ignore = "needs init-root (test-fixtures), init-kernel, initctl and the seed init-body built; run with --ignored"]
fn x2_t0_self_admits_through_the_real_root_and_closes_to_lkg() {
    let _wd = init_testkit::watchdog::arm("x2_t0_self_admits_through_the_real_root_and_closes_to_lkg", WD);
    assert!(free_gib() >= 5.5, "below 5.5 GiB free: stop");
    let b = base();
    if b.exists() {
        fs::remove_dir_all(&b).unwrap(); // exact path, an earlier run of this test
    }
    let (rh, ih, rel) = (b.join("rh"), b.join("ih"), b.join("rel"));
    for d in [&rh, &rel, &b.join("home"), &b.join("mock")] {
        fs::create_dir_all(d).unwrap();
    }
    assert!(rh.as_os_str().len() <= 64 && ih.as_os_str().len() <= 64, "homes within 64 bytes");
    let debug = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().to_path_buf();
    for bin in ["init-kernel", "initctl", "init-mockllm"] {
        assert!(debug.join(bin).is_file(), "build {bin} into this target dir first");
        let st = init_testkit::watchdog::status(Command::new("/bin/cp").arg("-c").arg(debug.join(bin)).arg(rel.join(bin))).unwrap();
        assert!(st.success());
    }
    fs::copy(repo().join("root/target/debug/init-root"), b.join("init-root")).unwrap();
    fs::copy(repo().join("seed/target/debug/init-body"), b.join("init-body")).unwrap();
    let tc = {
        let o = init_testkit::watchdog::output(Command::new("rustc").args(["--print", "sysroot"])).unwrap();
        PathBuf::from(String::from_utf8(o.stdout).unwrap().trim()).canonicalize().unwrap()
    };

    // The model: one turn of bash (the body commits and promotes its own change), then a reply.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let script = format!(
        "{}\n{}\n{}\n",
        json!({"v": 1, "kind": "meta", "provider": "deepseek", "model": "deepseek-flash", "name": "x2", "chunk": 16}),
        json!({"v": 1, "kind": "turn", "reply": {"think": "Keep a reviewer sub-agent.", "calls": [{"name": "bash", "input": {"command": COMMIT.replace("@IH@", ih.to_str().unwrap())}}], "stop": "tool_use", "usage": {"input": 9000, "cache_read": 0, "output": 200}}}),
        json!({"v": 1, "kind": "turn", "reply": {"text": "kept the reviewer sub-agent", "stop": "end_turn", "usage": {"input": 9500, "cache_read": 8000, "output": 20}}})
    );
    let mut mc = MockConfig::new(Mode::Upstream, Script::parse(&script, &b).unwrap());
    mc.strict_deepseek = true;
    mc.record_dir = Some(b.join("mock"));
    let mock = MockServer::start(Mock::new(mc).unwrap(), Listen::Tcp(format!("127.0.0.1:{port}"))).unwrap();
    let mut stack = Stack { rh: rh.clone(), root: None, mock: Some(mock) };

    // Charter: A6's, plus `[extend] mode = "free"`.
    let charter = format!(
        "schema_version = 1\n[purpose]\ntext = \"X2: the free path through the real Root.\"\n[autonomy]\nmax_level = 4\n\
         [money]\nday_usd = \"1.00\"\ntotal_usd = \"1.00\"\nday_starts_at_utc_offset = \"+00:00\"\n[quotas]\ndisk_min_free_gib = 1\n\
         [sealed]\nreserve_usd = \"0.00\"\naudit_usd = \"0.00\"\n[mirror]\nurl = \"\"\n[extend]\nmode = \"free\"\n[root]\ntest_mode = true\n\
         [actions.providers]\ndestinations = [\"127.0.0.1:{port}\"]\npaths = [\"chat/completions\", \"models\", \"user/balance\"]\n\
         [actions.providers.providers.deepseek]\nhost = \"127.0.0.1:{port}\"\n\
         [actions.providers.models.\"deepseek-flash\"]\nmin_level = 0\ninput_musd_per_mtok = 300000\noutput_musd_per_mtok = 1200000\nreserve_output_tokens = 65536\n"
    );
    fs::write(b.join("charter.toml"), charter).unwrap();
    let out = init_testkit::watchdog::output(Command::new(b.join("init-root"))
        .arg("install").arg("--release").arg(&rel).arg("--charter").arg(b.join("charter.toml")).arg("--toolchain").arg(&tc)
        .env_clear().env("HOME", b.join("home")).env("INIT_ROOT_HOME", &rh).env("INIT_HOME", &ih).env("PATH", "/usr/bin:/bin"))
        .unwrap();
    assert!(out.status.success(), "init-root install: {}", String::from_utf8_lossy(&out.stderr));

    fs::write(
        ih.join("kernel/config.toml"),
        format!(
            "schema_version = 2\n[model]\nprovider = \"deepseek\"\nid = \"deepseek-flash\"\n\n[proxy]\ntest_mode = false\n\n\
             [budget]\nday_usd = \"0.50\"\ntask_usd = \"0.50\"\nrequest_max_usd = \"0.20\"\n\n\
             [sandbox]\ntoolchain = {tc:?}\ndeveloper_dir = \"/Library/Developer/CommandLineTools\"\n\n\
             [providers.deepseek.models.\"deepseek-flash\"]\nmax_output = 393216\nreserve_output_tokens = 65536\n\
             prices.standard = {{ input = \"0.30\", cache_read = \"0.006\", output = \"1.20\" }}\n",
            tc = tc.to_string_lossy()
        ),
    )
    .unwrap();
    fs::set_permissions(ih.join("kernel/config.toml"), fs::Permissions::from_mode(0o644)).unwrap();

    // What `init-kernel install` would leave: the seed as lineage/body.git and body/, the LKG binary.
    let src = b.join("src");
    let mut ar = Command::new("/usr/bin/git");
    ar.arg("-C").arg(repo()).args(["archive", "--format=tar", "HEAD", "seed"]);
    pipe_tar(ar, &src, 1);
    git(&src, &["init", "-q", "-b", "main"]);
    git(&src, &["add", "-A"]);
    git(&src, &["commit", "-q", "-m", "feat: seed\n\nWhy: the X2 real-Root seed.\nPrefix-change: no\n"]);
    let x = git(&src, &["rev-parse", "HEAD"]);
    fs::create_dir_all(ih.join("lineage")).unwrap();
    git(&ih.join("lineage"), &["clone", "-q", "--bare", src.to_str().unwrap(), "body.git"]);
    let bare = ih.join("lineage/body.git");
    git(&bare, &["remote", "remove", "origin"]);
    git(&bare, &["update-ref", "refs/heads/lkg", &x]);
    if ih.join("body").exists() {
        fs::remove_dir_all(ih.join("body")).unwrap();
    }
    git(&ih, &["clone", "-q", bare.to_str().unwrap(), "body"]);
    git(&ih.join("body"), &["remote", "set-url", "origin", "../lineage/body.git"]);
    let mut ar = Command::new("/usr/bin/git");
    ar.arg(format!("--git-dir={}", bare.display())).args(["archive", "--format=tar", &x]);
    pipe_tar(ar, &ih.join("lineage/trees").join(&x[..12]), 0);
    fs::remove_dir_all(&src).unwrap();
    for n in ["bin/init-body", "bin/init-body.lkg"] {
        fs::copy(b.join("init-body"), ih.join(n)).unwrap();
        fs::set_permissions(ih.join(n), fs::Permissions::from_mode(0o755)).unwrap();
    }
    let lkg = json!({"v": 1, "sha": x, "binary": "bin/init-body.lkg", "binary_sha256": util::sha256_hex(&fs::read(b.join("init-body")).unwrap()),
        "tree": format!("lineage/trees/{}", &x[..12]), "reason": "seed", "lineage_seq": null, "seq": 1, "prev_sha": null,
        "gate_job": null, "committed_at": "2026-10-20T00:00:00.000Z", "model": null, "ext_set": null});
    fs::write(ih.join("state/kernel/lkg.json"), serde_json::to_vec(&lkg).unwrap()).unwrap();
    // The commit message the body uses (the free trailer block: Why, Prefix-change, and a Cap).
    fs::create_dir_all(ih.join("work")).unwrap();
    fs::write(ih.join("work/msg.txt"), "skill: keep a reviewer sub-agent\n\nWhy: the owner wants every diff reviewed before it is promoted.\nPrefix-change: no\nCap: agent/reviewer\n").unwrap();
    let kdest = rh.join("keys/providers/deepseek");
    fs::write(&kdest, "test-x2-mock-key").unwrap();
    fs::set_permissions(&kdest, fs::Permissions::from_mode(0o600)).unwrap();

    // Start the Root (held child) on a fake clock, then wait for the kernel and the body.
    let log = fs::File::create(b.join("root.out")).unwrap();
    stack.root = Some(
        init_testkit::watchdog::spawn(Command::new(rh.join("bin/init-root"))
            .args(["start", "--foreground"])
            .env_clear()
            .env("HOME", std::env::var_os("HOME").unwrap())
            .env("INIT_ROOT_HOME", &rh)
            .env("INIT_ROOT_TEST_NOW", "2030-01-01T00:00:00Z")
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log))
            .unwrap(),
    );
    wait_for("owner.sock", Duration::from_secs(90), || rh.join("run/owner.sock").exists().then_some(()));
    wait_for("kernel running and body ready", Duration::from_secs(120), || (status(&rh)["slot"] == "running" && ih.join("state/run/ready").exists()).then_some(()));

    // One owner line: the body commits its change and runs `initctl promote evo/rev --wait`.
    let mut s = UnixStream::connect(rh.join("run/owner.sock")).unwrap();
    s.write_all(b"{\"t\":\"attach\",\"client\":\"x2-real-root\",\"replay\":false}\n").unwrap();
    s.write_all(b"{\"t\":\"line\",\"text\":\"Keep a reviewer sub-agent for your diffs.\"}\n").unwrap();
    wait_for("the final reply", Duration::from_secs(180), || {
        events(&ih).into_iter().find(|e| e["type"] == "message" && e["role"] == "assistant" && e["stop_reason"] == "end_turn")
    });
    drop(s);
    let evs = events(&ih);
    let res = evs.iter().find(|e| e["type"] == "tool_result").cloned().expect("the bash result");
    let out = res["output"].to_string();
    assert_eq!(res["exit_code"].as_i64(), Some(0), "commit + initctl promote --wait failed: {res}\nroot.out: {}", fs::read_to_string(b.join("root.out")).unwrap_or_default());
    assert!(out.contains("change.open: A0, A4 and A5 at the Root") && out.contains("\\\"state\\\": \\\"succeeded\\\""), "the promote job: {out}");

    // The Root admitted it: change_open (A0 pass, root-evidence caps from the kernel's t0 tar), a tuple
    // with the t0 blob, and the kernel's v5 gate record.
    let y = git(&bare, &["rev-parse", "refs/heads/main"]);
    assert_ne!(y, x, "main moved (data-only, no handoff)");
    let opens = records(&rh, "change_open");
    assert_eq!(opens.len(), 1, "{opens:?}");
    let d = &opens[0]["data"];
    assert_eq!((d["scope"].as_str(), d["sha"].as_str(), d["from_sha"].as_str(), d["checks"]["A0"].as_str(), d["group"].as_u64()), (Some("t0"), Some(y.as_str()), Some(x.as_str()), Some("pass"), Some(1)), "{d}");
    let caps = d["caps"].as_array().cloned().unwrap_or_default();
    let reviewer = caps.iter().find(|c| c["id"] == "agent/reviewer").unwrap_or_else(|| panic!("agent/reviewer in {caps:?}"));
    assert_eq!((reviewer["evidence"].as_str(), reviewer["op"].as_str()), (Some("root"), Some("add")), "the Root computed the cap from the kernel's t0 blob: {reviewer}");
    assert!(caps.iter().any(|c| c["id"] == "prompt/SELF.md" && c["evidence"] == "root"), "{caps:?}");
    let tuple_id = d["tuple"].as_str().unwrap().to_string();
    let tuple = records(&rh, "tuple").into_iter().rev().find(|t| t["data"]["id"] == tuple_id.as_str()).expect("the candidate tuple");
    assert!(tuple["data"]["tuple"]["body"]["t0"].as_str().is_some_and(|t| t.len() == 64), "{tuple}");
    let gates: Vec<Value> = records(&rh, "gate").into_iter().map(|r| r["data"].clone()).collect();
    let g = gates.iter().find(|g| g["accept"] == "free").unwrap_or_else(|| panic!("the v5 gate record: {gates:?}"));
    assert_eq!((g["v"].as_u64(), g["verdict"].as_str(), g["group"].as_u64(), g["scope"].as_str()), (Some(5), Some("accept"), Some(1), Some("t0")), "{g}");
    assert!(!records(&rh, "promotion").iter().any(|p| p["data"]["lkg"] == true && p["data"]["sha"] == y.as_str()), "no promotion{{lkg:true}} on the free path");

    // The window: 73 h later the group closes `quiet` (no owner turns), the Root moves `lkg`, and the
    // kernel's own LKG follows on `extend.window`.
    let clock = root_cli(&rh, &["test", "clock", "--advance", &(73 * 3600).to_string()]);
    assert!(clock.status.success(), "test clock: {}", String::from_utf8_lossy(&clock.stderr));
    let close = wait_for("change_close", Duration::from_secs(30), || records(&rh, "change_close").into_iter().next());
    assert_eq!((close["data"]["group"].as_u64(), close["data"]["outcome"].as_str()), (Some(1), Some("quiet")), "{close}");
    wait_for("the Root's lkg", Duration::from_secs(30), || records(&rh, "lkg").into_iter().find(|l| l["data"]["tuple"] == tuple_id.as_str()));
    wait_for("the kernel's LKG", Duration::from_secs(60), || (git(&bare, &["rev-parse", "refs/heads/lkg"]) == y).then_some(()));
    let lkg: Value = serde_json::from_slice(&fs::read(ih.join("state/kernel/lkg.json")).unwrap()).unwrap();
    assert_eq!((lkg["sha"].as_str(), lkg["reason"].as_str()), (Some(y.as_str()), Some("change_close")), "{lkg}");

    drop(stack);
    assert!(b.starts_with(repo().join("target")) && b.ends_with("x2r"));
    let _ = fs::remove_dir_all(&b);
}
