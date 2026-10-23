//! `initctl` against an in-test fake kernel socket (kernel/initctl.md §4.5, §8 IC-7, IC-10,
//! IC-11, IC-13, IC-17, IC-20). Every test home lives under `kernel/target/test-homes/wp1/` and
//! is deleted by its exact path afterwards. The only processes spawned are the `initctl` binary
//! under test and `git` (both waited for; a hung `initctl` is killed by its own PID).

use init_proto::initctl::{Command, CommandName};
use init_proto::serde_json::{self, Value, json};
use init_proto::{Frame, Request, Response};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command as Proc, Output, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The watchdog bound of every test here (K-FLAKE: 3-10x its normal runtime; init_testkit::watchdog).
const WD: Duration = Duration::from_secs(30);

const SHA: &str = "4b1e0c2d9a7f3e5d1c0b9a8f7e6d5c4b3a291807";

/// A per-test home under `kernel/target/test-homes/wp1/`.
struct Home {
    dir: PathBuf,
}

impl Home {
    fn new(name: &str) -> Home {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().join("target/test-homes/wp1");
        let dir = root.join(format!("{name}-{}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        std::fs::create_dir_all(dir.join("tmp")).unwrap();
        Home { dir }
    }

    fn sock(&self) -> PathBuf {
        self.dir.join("k.sock")
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

enum Reply {
    /// A line; `{id}` is replaced by the request id.
    Line(String),
    Sleep(u64),
}

/// A fake kernel: accepts one connection, reads one request line, sends the scripted replies,
/// closes. Joining returns the request line it received (None if nobody connected).
fn serve(path: &Path, replies: Vec<Reply>) -> JoinHandle<Option<String>> {
    if path.exists() {
        std::fs::remove_file(path).unwrap(); // exact path, our own fake's socket
    }
    let listener = UnixListener::bind(path).unwrap();
    listener.set_nonblocking(true).unwrap();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        let stream = loop {
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => return None,
            }
        };
        stream.set_nonblocking(false).unwrap();
        let mut r = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        let id = serde_json::from_str::<Value>(&line).ok().and_then(|v| v["id"].as_str().map(str::to_string)).unwrap_or_default();
        let mut w = stream;
        for rep in replies {
            match rep {
                Reply::Line(l) => {
                    let l = l.replace("{id}", &id);
                    if w.write_all(l.as_bytes()).and_then(|_| w.write_all(b"\n")).is_err() {
                        break;
                    }
                }
                Reply::Sleep(ms) => std::thread::sleep(Duration::from_millis(ms)),
            }
        }
        Some(line.trim_end().to_string())
    })
}

/// Run `initctl` with a clean environment, killing it by PID if it hangs.
fn initctl(home: &Home, args: &[&str], extra_env: &[(&str, &str)], cwd: Option<&Path>) -> Output {
    let mut c = Proc::new(env!("CARGO_BIN_EXE_initctl"));
    c.args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", &home.dir)
        .env("TMPDIR", home.dir.join("tmp"))
        .env("INIT_KERNEL_SOCK", home.sock())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in extra_env {
        c.env(k, v);
    }
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    let mut child = init_testkit::watchdog::spawn(&mut c).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() > deadline {
            let _ = child.kill(); // our own child, by PID
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut out = Vec::new();
    let mut err = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    child.stderr.take().unwrap().read_to_end(&mut err).unwrap();
    Output { status: child.wait().unwrap(), stdout: out, stderr: err }
}

fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

fn stdout_json(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("stdout not JSON ({e}): {}", String::from_utf8_lossy(&o.stdout)))
}

fn ok_line(result: &str) -> Reply {
    Reply::Line(format!(r#"{{"id":"{{id}}","ok":true,"result":{result},"v":1}}"#))
}

fn err_line(code: &str, data: &str) -> Reply {
    Reply::Line(format!(r#"{{"error":{{"code":"{code}","data":{data},"details":[],"message":"refused"}},"id":"{{id}}","ok":false,"v":1}}"#))
}

/// One request/response exchange; returns (output, request line).
fn exchange(name: &str, args: &[&str], replies: Vec<Reply>) -> (Output, String) {
    let home = Home::new(name);
    let srv = serve(&home.sock(), replies);
    let o = initctl(&home, args, &[], None);
    let req = srv.join().unwrap().unwrap_or_default();
    (o, req)
}

#[test]
fn ok_prints_sorted_pretty_result_and_exits_0() {
    let _wd = init_testkit::watchdog::arm("ok_prints_sorted_pretty_result_and_exits_0", WD);
    let (o, req) = exchange("ping", &["ping"], vec![ok_line(r#"{"kernel_version":"0.1.0","lane":"live","proto":1,"time":"2026-10-19T12:30:00.002Z"}"#)]);
    assert_eq!(code(&o), 0, "{}", String::from_utf8_lossy(&o.stderr));
    // The exact request line (id = <pid>-1).
    let r = Request::parse_line(req.as_bytes()).unwrap();
    assert!(r.id.ends_with("-1"));
    assert_eq!(req, format!(r#"{{"args":{{}},"cmd":"ping","id":"{}","v":1}}"#, r.id));
    let s = String::from_utf8(o.stdout.clone()).unwrap();
    assert_eq!(
        s,
        "{\n  \"kernel_version\": \"0.1.0\",\n  \"lane\": \"live\",\n  \"proto\": 1,\n  \"time\": \"2026-10-19T12:30:00.002Z\"\n}\n"
    );
}

#[test]
fn promote_wait_relays_progress_and_returns_at_awaiting_boundary() {
    let _wd = init_testkit::watchdog::arm("promote_wait_relays_progress_and_returns_at_awaiting_boundary", WD);
    // IC-8 shape: wait returns at awaiting_boundary, exit 0; progress on stderr.
    let (o, req) = exchange(
        "promote",
        &["promote", "evo/pty-tool", "--wait"],
        vec![
            Reply::Line(r#"{"id":"{id}","progress":{"eta_s":25,"job":"j-000047","msg":"cargo build (deps cache hit)","pct":20,"stage":"build","state":"running","ts":"t"},"v":1}"#.into()),
            Reply::Line(r#"{"id":"{id}","progress":{"eta_s":null,"job":"j-000047","msg":"probe: waiting for ready","pct":60,"stage":"G2","state":"gating","ts":"t"},"v":1}"#.into()),
            ok_line(&format!(r#"{{"job":"j-000047","sha":"{SHA}","stages":[],"state":"awaiting_boundary"}}"#)),
        ],
    );
    assert_eq!(code(&o), 0);
    assert!(req.contains(r#""args":{"ref":"evo/pty-tool","wait":true},"cmd":"promote""#), "{req}");
    let err = String::from_utf8(o.stderr.clone()).unwrap();
    assert_eq!(err, "[j-000047 build 20% eta 25s] cargo build (deps cache hit)\n[j-000047 G2 60%] probe: waiting for ready\n");
    assert_eq!(stdout_json(&o)["state"], "awaiting_boundary");
}

#[test]
fn quiet_drops_progress_and_json_prints_raw_frames() {
    let _wd = init_testkit::watchdog::arm("quiet_drops_progress_and_json_prints_raw_frames", WD);
    let prog = r#"{"id":"{id}","progress":{"eta_s":null,"job":"j-000001","msg":"m","pct":1,"stage":"fetch","state":"running","ts":"t"},"v":1}"#;
    let fin = r#"{"id":"{id}","ok":true,"result":{"job":"j-000001","state":"succeeded"},"v":1}"#;
    let (o, _) = exchange("quiet", &["--quiet", "wait", "j-000001"], vec![Reply::Line(prog.into()), Reply::Line(fin.into())]);
    assert_eq!(code(&o), 0, "{}", String::from_utf8_lossy(&o.stderr));
    assert!(o.stderr.is_empty());
    let (o, req) = exchange("json", &["--json", "wait", "j-000001"], vec![Reply::Line(prog.into()), Reply::Line(fin.into())]);
    assert_eq!(code(&o), 0);
    let id = Request::parse_line(req.as_bytes()).unwrap().id;
    let s = String::from_utf8(o.stdout).unwrap();
    assert_eq!(s, format!("{}\n{}\n", prog.replace("{id}", &id), fin.replace("{id}", &id)));
    assert!(o.stderr.is_empty());
}

/// IC-17: one case per row of the §4.5 exit-code table, plus every E_* code of each row.
#[test]
fn ic17_exit_code_table() {
    let _wd = init_testkit::watchdog::arm("ic17_exit_code_table", WD);
    // 1: ok:false with a domain code; E_BUDGET; a dotted validator code with details.
    for (c, data) in [("E_NOT_FOUND", "{}"), ("E_BUDGET", r#"{"key":"gate_usd","limit":4000000}"#), ("trailer.missing:Predicts", r#"{"sha":"4b1e"}"#), ("gate.g4.inconclusive_final", "{}"), ("E_SOMETHING_NEW", "{}")] {
        let (o, _) = exchange("x1", &["status"], vec![err_line(c, data)]);
        assert_eq!(code(&o), 1, "{c}");
        assert_eq!(stdout_json(&o)["error"]["code"], c);
    }
    // 1: gate with pass:false; task submit with admitted:false.
    let (o, _) = exchange("x1g", &["gate", "--quick", "evo/x"], vec![ok_line(r#"{"job":"j-000046","pass":false,"report":"r","sha":"s","stages":[],"warnings":[]}"#)]);
    assert_eq!(code(&o), 1);
    let (o, _) = exchange(
        "x1t",
        &["task", "submit", "/h/work/t"],
        vec![ok_line(r#"{"admitted":false,"checks":[],"family":"f","job":"j-000060","sealed":0,"seq":null,"visible":[]}"#)],
    );
    assert_eq!(code(&o), 1);
    // 2: wait expired (the server's timed_out record).
    let (o, _) = exchange(
        "x2",
        &["wait", "j-000047", "--timeout", "1"],
        vec![ok_line(r#"{"args":{},"cmd":"promote","created_at":"t","error":null,"finished_at":null,"id":"j-000047","lane":"live","progress":null,"result":null,"sha":null,"stages":[],"state":"running","timed_out":true,"v":1}"#)],
    );
    assert_eq!(code(&o), 2);
    // 64: E_ARGS, E_WOULD_DEADLOCK (IC-7).
    let (o, _) = exchange("x64", &["status"], vec![err_line("E_ARGS", "{}")]);
    assert_eq!(code(&o), 64);
    let (o, req) = exchange("ic7", &["reload", "--wait"], vec![err_line("E_WOULD_DEADLOCK", "{}")]);
    assert_eq!(code(&o), 64);
    assert!(req.contains(r#""args":{"wait":true},"cmd":"reload""#));
    // 70: E_INTERNAL.
    let (o, _) = exchange("x70", &["status"], vec![err_line("E_INTERNAL", "{}")]);
    assert_eq!(code(&o), 70);
    // 75: the temporary codes. IC-10: E_BUSY carries data.job.
    for c in ["E_BUSY", "E_RATE", "E_QUOTA", "E_DISK", "E_SHUTTING_DOWN", "E_NO_BODY", "E_ROOT"] {
        let (o, _) = exchange("x75", &["promote", "evo/pty-tool"], vec![err_line(c, r#"{"job":"j-000041"}"#)]);
        assert_eq!(code(&o), 75, "{c}");
        assert_eq!(stdout_json(&o)["error"]["data"]["job"], "j-000041");
    }
    // 76: the protocol codes.
    for c in ["E_PROTO", "E_VERSION", "E_UNKNOWN_CMD"] {
        let (o, _) = exchange("x76", &["status"], vec![err_line(c, "{}")]);
        assert_eq!(code(&o), 76, "{c}");
    }
    // 77: not permitted now. IC-13 (E_PROVISIONAL on vendor), IC-20 (E_LANE).
    for c in ["E_PEER", "E_LANE", "E_HALTED", "E_SAFE_MODE", "E_PROVISIONAL", "E_PROVISIONAL_DEPTH", "E_LEVEL", "E_CEILING"] {
        let (o, _) = exchange("x77", &["vendor", "vt100@0.16.2"], vec![err_line(c, r#"{"have":0,"need":1}"#)]);
        assert_eq!(code(&o), 77, "{c}");
    }
}

#[test]
fn exit_64_on_bad_cli_args_without_connecting() {
    let _wd = init_testkit::watchdog::arm("exit_64_on_bad_cli_args_without_connecting", WD);
    let home = Home::new("x64cli");
    let srv = serve(&home.sock(), vec![]);
    for args in [&["frobnicate"][..], &["promote"], &["vendor", "vt100"], &["gate", "evo/x"], &["wait"], &["--timeout", "abc", "status"]] {
        let o = initctl(&home, args, &[], None);
        assert_eq!(code(&o), 64, "{args:?}");
        assert!(o.stdout.is_empty());
        assert!(String::from_utf8_lossy(&o.stderr).contains("initctl:"));
    }
    // Nobody connected: the fake gives up after its accept deadline.
    drop(srv);
}

#[test]
fn exit_69_when_unavailable_or_dropped() {
    let _wd = init_testkit::watchdog::arm("exit_69_when_unavailable_or_dropped", WD);
    // No socket file.
    let home = Home::new("x69");
    let o = initctl(&home, &["status"], &[], None);
    assert_eq!(code(&o), 69);
    // Connection dropped before the final response (after a progress frame).
    let (o, _) = exchange(
        "x69drop",
        &["promote", "evo/pty-tool", "--wait"],
        vec![Reply::Line(r#"{"id":"{id}","progress":{"eta_s":null,"job":"j-000047","msg":"m","pct":1,"stage":"fetch","state":"running","ts":"t"},"v":1}"#.into())],
    );
    assert_eq!(code(&o), 69);
    // Dropped in the middle of a line.
    let home = Home::new("x69half");
    let srv = serve(&home.sock(), vec![]);
    let _ = srv; // the server closes right after reading the request
    let o = initctl(&home, &["status"], &[], None);
    assert_eq!(code(&o), 69);
}

#[test]
fn exit_76_on_unparsable_responses() {
    let _wd = init_testkit::watchdog::arm("exit_76_on_unparsable_responses", WD);
    for bad in [
        "garbage".to_string(),
        r#"{"id":"{id}","ok":true,"result":{},"v":2}"#.to_string(),
        r#"{"id":"{id}","v":1}"#.to_string(),
        r#"{"id":"someone-else","ok":true,"result":{},"v":1}"#.to_string(),
        r#"[1,2]"#.to_string(),
    ] {
        let (o, _) = exchange("x76r", &["status"], vec![Reply::Line(bad.clone())]);
        assert_eq!(code(&o), 76, "{bad}");
    }
    // A response line over 1 MiB.
    let big = format!(r#"{{"id":"{{id}}","ok":true,"result":{{"p":"{}"}},"v":1}}"#, "x".repeat(1024 * 1024));
    let (o, _) = exchange("x76big", &["status"], vec![Reply::Line(big)]);
    assert_eq!(code(&o), 76);
}

#[test]
fn e_peer_with_null_id_is_77() {
    let _wd = init_testkit::watchdog::arm("e_peer_with_null_id_is_77", WD);
    let (o, _) = exchange(
        "peer",
        &["status"],
        vec![Reply::Line(r#"{"error":{"code":"E_PEER","details":[],"message":"peer not attributable"},"id":null,"ok":false,"v":1}"#.into())],
    );
    assert_eq!(code(&o), 77);
    assert_eq!(stdout_json(&o), json!({"error": {"code": "E_PEER", "details": [], "message": "peer not attributable"}}));
}

#[test]
fn client_timeout_exits_2() {
    let _wd = init_testkit::watchdog::arm("client_timeout_exits_2", WD);
    let (o, _) = exchange(
        "ctimeout",
        &["--timeout", "1", "promote", "evo/pty-tool", "--wait"],
        vec![
            Reply::Line(r#"{"id":"{id}","progress":{"eta_s":null,"job":"j-000047","msg":"m","pct":1,"stage":"fetch","state":"running","ts":"t"},"v":1}"#.into()),
            Reply::Sleep(2500),
        ],
    );
    assert_eq!(code(&o), 2);
    assert!(String::from_utf8_lossy(&o.stderr).contains("initctl wait j-000047"));
}

#[test]
fn wait_failed_job_exits_with_its_code() {
    let _wd = init_testkit::watchdog::arm("wait_failed_job_exits_with_its_code", WD);
    let rec = |state: &str, err: &str| {
        ok_line(&format!(
            r#"{{"args":{{}},"cmd":"promote","created_at":"t","error":{err},"finished_at":"t","id":"j-000047","lane":"live","progress":null,"result":null,"sha":null,"stages":[],"state":"{state}","v":1}}"#
        ))
    };
    let (o, _) = exchange("wfail", &["wait", "j-000047"], vec![rec("failed", r#"{"code":"E_HANDOFF_FAILED","details":[],"message":"m"}"#)]);
    assert_eq!(code(&o), 1);
    let (o, _) = exchange("wok", &["wait", "j-000047"], vec![rec("succeeded", "null")]);
    assert_eq!(code(&o), 0);
}

#[test]
fn socket_selection_from_init_home_and_flag() {
    let _wd = init_testkit::watchdog::arm("socket_selection_from_init_home_and_flag", WD);
    let home = Home::new("sel");
    std::fs::create_dir_all(home.dir.join("run")).unwrap();
    let path = home.dir.join("run/kernel.sock");
    let srv = serve(&path, vec![ok_line("{}")]);
    let o = initctl(&home, &["status"], &[("INIT_KERNEL_SOCK", ""), ("INIT_HOME", home.dir.to_str().unwrap())], None);
    assert_eq!(code(&o), 0, "{}", String::from_utf8_lossy(&o.stderr));
    assert!(srv.join().unwrap().is_some());
    let other = home.dir.join("o.sock");
    let srv = serve(&other, vec![ok_line("{}")]);
    let o = initctl(&home, &["--sock", other.to_str().unwrap(), "status"], &[], None);
    assert_eq!(code(&o), 0);
    assert!(srv.join().unwrap().is_some());
}

fn git(dir: &Path, home: &Home, args: &[&str]) -> String {
    let o = init_testkit::watchdog::output(Proc::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", &home.dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", home.dir.join("gitconfig"))
        .env("GIT_CEILING_DIRECTORIES", &home.dir)
        .env("GIT_AUTHOR_NAME", "INIT Agent")
        .env("GIT_AUTHOR_EMAIL", "agent@init.local")
        .env("GIT_COMMITTER_NAME", "INIT Agent")
        .env("GIT_COMMITTER_EMAIL", "agent@init.local"))
        .unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap()
}

const GIT_ENV: &[(&str, &str)] = &[
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("GIT_AUTHOR_NAME", "INIT Agent"),
    ("GIT_AUTHOR_EMAIL", "agent@init.local"),
    ("GIT_COMMITTER_NAME", "INIT Agent"),
    ("GIT_COMMITTER_EMAIL", "agent@init.local"),
];

#[test]
fn ic11_gate_quick_snapshots_a_dirty_tree_without_touching_it() {
    let _wd = init_testkit::watchdog::arm("ic11_gate_quick_snapshots_a_dirty_tree_without_touching_it", WD);
    let home = Home::new("ic11");
    std::fs::write(home.dir.join("gitconfig"), "").unwrap();
    let repo = home.dir.join("work/wt-pty");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &home, &["-c", "init.defaultBranch=main", "init", "-q"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    std::fs::write(repo.join(".gitignore"), "ignored.txt\n").unwrap();
    git(&repo, &home, &["add", "-A"]);
    git(&repo, &home, &["commit", "-q", "-m", "feat: init"]);
    let head = git(&repo, &home, &["rev-parse", "HEAD"]).trim().to_string();
    let mut env: Vec<(&str, &str)> = GIT_ENV.to_vec();
    let gc = home.dir.join("gitconfig");
    env.push(("GIT_CONFIG_GLOBAL", gc.to_str().unwrap()));
    // The test home lies inside the repo checkout: never let git discover the outer repo.
    let ceiling = home.dir.to_str().unwrap().to_string();
    env.push(("GIT_CEILING_DIRECTORIES", &ceiling));

    // Clean tree: rev = HEAD, repo = the toplevel (from a subdirectory).
    std::fs::create_dir_all(repo.join("sub")).unwrap();
    let srv = serve(&home.sock(), vec![ok_line(r#"{"job":"j-000046","pass":true,"report":"r","sha":"s","stages":[],"warnings":[]}"#)]);
    let o = initctl(&home, &["gate", "--quick"], &env, Some(&repo.join("sub")));
    assert_eq!(code(&o), 0, "{}", String::from_utf8_lossy(&o.stderr));
    let req = Request::parse_line(srv.join().unwrap().unwrap().as_bytes()).unwrap();
    let top = git(&repo, &home, &["rev-parse", "--show-toplevel"]).trim().to_string();
    assert_eq!(req.args, json!({"quick": true, "repo": top, "rev": "HEAD"}));

    // Dirty tree: a modified file, an untracked file, an ignored file.
    std::fs::write(repo.join("a.txt"), "two\n").unwrap();
    std::fs::write(repo.join("new.txt"), "new\n").unwrap();
    std::fs::write(repo.join("ignored.txt"), "secret\n").unwrap();
    let status_before = git(&repo, &home, &["status", "--porcelain"]);
    let refs_before = git(&repo, &home, &["for-each-ref"]) + &git(&repo, &home, &["rev-parse", "HEAD"]);
    let index_before = std::fs::read(repo.join(".git/index")).unwrap();

    let srv = serve(&home.sock(), vec![ok_line(r#"{"job":"j-000047","pass":true,"report":"r","sha":"s","stages":[],"warnings":[]}"#)]);
    let o = initctl(&home, &["gate", "--quick", "work/wt-pty"], &env, Some(&home.dir));
    assert_eq!(code(&o), 0, "{}", String::from_utf8_lossy(&o.stderr));
    let req = Request::parse_line(srv.join().unwrap().unwrap().as_bytes()).unwrap();
    let snap = req.args["rev"].as_str().unwrap().to_string();
    assert!(init_proto::validate::is_sha40(&snap) && snap != head, "{snap}");
    assert_eq!(req.args["repo"], top.as_str());
    // The snapshot holds the dirty state on top of HEAD, without the ignored file.
    assert_eq!(git(&repo, &home, &["rev-parse", &format!("{snap}^")]).trim(), head);
    assert_eq!(git(&repo, &home, &["show", &format!("{snap}:a.txt")]), "two\n");
    assert_eq!(git(&repo, &home, &["show", &format!("{snap}:new.txt")]), "new\n");
    assert!(!git(&repo, &home, &["ls-tree", "--name-only", &snap]).contains("ignored.txt"));
    // Index, refs and worktree unchanged (the index is read before any `git status` of ours).
    assert_eq!(std::fs::read(repo.join(".git/index")).unwrap(), index_before);
    assert_eq!(git(&repo, &home, &["for-each-ref"]) + &git(&repo, &home, &["rev-parse", "HEAD"]), refs_before);
    assert_eq!(git(&repo, &home, &["status", "--porcelain"]), status_before);
    // The temporary index is gone.
    let leftovers: Vec<_> = std::fs::read_dir(home.dir.join("tmp")).unwrap().collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    // A ref that is not a directory: a ref in $INIT_HOME/body.
    let srv = serve(&home.sock(), vec![ok_line(r#"{"job":"j-000048","pass":true,"report":"r","sha":"s","stages":[],"warnings":[]}"#)]);
    let o = initctl(&home, &["gate", "--quick", "evo/pty-tool"], &env, Some(&home.dir));
    assert_eq!(code(&o), 0);
    let req = Request::parse_line(srv.join().unwrap().unwrap().as_bytes()).unwrap();
    assert_eq!(req.args, json!({"quick": true, "repo": "body", "rev": "evo/pty-tool"}));

    // Not a git worktree → usage error, nothing sent.
    std::fs::create_dir_all(home.dir.join("plain")).unwrap();
    let o = initctl(&home, &["gate", "--quick", "plain"], &env, Some(&home.dir));
    assert_eq!(code(&o), 64);
}

#[test]
fn every_cli_command_sends_a_parsable_request() {
    let _wd = init_testkit::watchdog::arm("every_cli_command_sends_a_parsable_request", WD);
    // Each CLI form produces a request the server-side parser accepts (shared init-proto types).
    let home = Home::new("allcmds");
    std::fs::write(home.dir.join("prompt.txt"), "try a pty tool").unwrap();
    std::fs::write(home.dir.join("why.txt"), "faster gate").unwrap();
    let forms: Vec<Vec<&str>> = vec![
        vec!["status"],
        vec!["ping"],
        vec!["level"],
        vec!["reload"],
        vec!["promote", "evo/pty-tool"],
        vec!["promote", "--sha", SHA, "--wait"],
        vec!["rollback", "--reason", "r"],
        vec!["vendor", "vt100@0.16.2"],
        vec!["task", "template"],
        vec!["task", "submit", "work/tasks/f"],
        vec!["task", "list"],
        vec!["task", "show", "f"],
        vec!["budget", "set", "day_musd=4000000"],
        vec!["model", "set", "deepseek/deepseek-flash"],
        vec!["fleet", "list"],
        vec!["fleet", "spawn", "--prompt-file", "prompt.txt", "--share-musd-day", "250000"],
        vec!["fleet", "send", "b1", "--text", "t"],
        vec!["fleet", "stop", "b1"],
        vec!["kernel", "propose", "work/k.diff", "--title", "t", "--rationale-file", "why.txt"],
        vec!["evo", "frictions", "--all"],
        vec!["evo", "propose", "work/p.json"],
        vec!["evo", "variant", "p-000001", "2", "work/v2.diff"],
        vec!["evo", "screen", "p-000001", "--wait"],
        vec!["evo", "status"],
        vec!["evo", "abandon", "p-000001", "keep_base"],
        vec!["propose-kernel", "work/k.diff", "--title", "t"],
        vec!["kernel", "promote", "kevo/x"],
        vec!["kernel", "status"],
        vec!["wait", "--last"],
        vec!["job", "status", "j-000001"],
        vec!["job", "wait", "j-000001"],
        vec!["job", "list", "--all"],
        vec!["job", "cancel", "j-000001"],
        vec!["grant", "extend.open_groups", "6", "--why", "five tools for one task"],
    ];
    let mut seen = std::collections::BTreeSet::new();
    for f in &forms {
        let srv = serve(&home.sock(), vec![ok_line(r#"{"state":"succeeded"}"#)]);
        let o = initctl(&home, f, &[], Some(&home.dir));
        assert_eq!(code(&o), 0, "{f:?}: {}", String::from_utf8_lossy(&o.stderr));
        let line = srv.join().unwrap().unwrap();
        std::fs::remove_file(home.sock()).unwrap();
        let req = Request::parse_line(line.as_bytes()).unwrap();
        let cmd = Command::from_request(&req).unwrap_or_else(|e| panic!("{f:?}: {e}"));
        seen.insert(cmd.name());
        match &cmd {
            Command::FleetSpawn(a) => assert_eq!((a.prompt.as_str(), a.budget_share_musd_day), ("try a pty tool", Some(250000))),
            Command::KernelPropose(a) => {
                assert!(Path::new(&a.patch).is_absolute() && a.patch.ends_with("work/k.diff"));
                assert!(a.rationale == "faster gate" || a.rationale.is_empty());
            }
            Command::TaskSubmit(a) => assert!(a.dir.ends_with("work/tasks/f") && Path::new(&a.dir).is_absolute()),
            Command::EvoVariant(a) => assert!(a.k == 2 && a.patch.ends_with("work/v2.diff") && Path::new(&a.patch).is_absolute()),
            _ => {}
        }
    }
    seen.insert(CommandName::Gate); // covered by ic11
    assert_eq!(seen.len(), CommandName::ALL.len());
    // Sanity: the final frames the fake sent are valid client frames.
    assert!(matches!(Frame::parse_line(br#"{"id":"1","ok":true,"result":{},"v":1}"#), Ok(Frame::Final(Response { .. }))));
}
