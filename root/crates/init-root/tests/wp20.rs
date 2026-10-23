//! WP20 integration tests: install, charter check/apply, keys, the owner socket, attach, panic and
//! resume, test ledgers and the fake clock. Run with `cargo test --offline -p init-root --features test-fixtures`.
//! Every home is under `root/target/test-homes/wp20/` and `HOME` points there, so nothing touches the
//! owner's `~/.init-root`, `~/.init` or `~/Library/LaunchAgents`. Only processes spawned here are killed,
//! by their own handles.
#![cfg(feature = "test-fixtures")]

use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_init-root");

struct Home {
    dir: PathBuf,
    rh: PathBuf,
    ih: PathBuf,
}

/// A fresh home `root/target/test-homes/wp20/<name>` (short names: ROOT_HOME must stay ≤ 80 bytes).
fn home(name: &str) -> Home {
    let base = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap().join("test-homes/wp20");
    let dir = base.join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap(); // exact path, created by an earlier run of this test
    }
    fs::create_dir_all(&dir).unwrap();
    Home { rh: dir.join("rh"), ih: dir.join("ih"), dir }
}

impl Home {
    fn cmd(&self) -> Command {
        let mut c = Command::new(BIN);
        c.env_clear().env("HOME", &self.dir).env("INIT_ROOT_HOME", &self.rh).env("INIT_HOME", &self.ih).env("PATH", "/usr/bin:/bin");
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        self.cmd().args(args).stdin(Stdio::null()).output().unwrap()
    }
    fn charter_file(&self, extra: &str) -> PathBuf {
        let f = self.dir.join("charter.toml");
        fs::write(&f, charter(extra)).unwrap();
        f
    }
    fn install(&self, extra: &str) {
        let f = self.charter_file(extra);
        let o = self.run(&["install", "--charter", f.to_str().unwrap()]);
        assert_eq!(o.status.code(), Some(0), "install: {}", String::from_utf8_lossy(&o.stderr));
    }
    fn records(&self) -> Vec<Value> {
        let s = fs::read_to_string(self.rh.join("lineage/lineage.jsonl")).unwrap_or_default();
        s.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }
    fn kinds(&self) -> Vec<String> {
        self.records().iter().map(|r| r["kind"].as_str().unwrap().to_string()).collect()
    }
    fn status(&self) -> Value {
        let o = self.run(&["status", "--json"]);
        assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
        serde_json::from_slice(&o.stdout).unwrap()
    }
    fn daemon(&self, env: &[(&str, &str)]) -> Daemon {
        let mut c = self.cmd();
        c.arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        for (k, v) in env {
            c.env(k, v);
        }
        let d = Daemon(c.spawn().unwrap());
        let sock = self.rh.join("run/owner.sock");
        wait(10, || std::os::unix::net::UnixStream::connect(&sock).is_ok());
        d
    }
}

/// A daemon this test spawned; killed by its own handle.
struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn charter(extra: &str) -> String {
    format!("schema_version = 1\n[purpose]\ntext = \"WP20 test home.\"\n[autonomy]\nmax_level = 4\n[quotas]\ndisk_min_free_gib = 0\n{extra}\n")
}

fn wait(secs: u64, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(secs), "condition not met within {secs} s");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn mode(p: &Path) -> u32 {
    fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn install_lays_out_both_homes_and_records_genesis() {
    let h = home("inst");
    h.install("[root]\ntest_mode = true\n");
    for d in ["", "keys", "keys/providers", "lineage", "state", "run", "store/blobs"] {
        assert_eq!(mode(&h.rh.join(d)), 0o700, "ROOT_HOME/{d}");
    }
    assert_eq!(mode(&h.rh.join("charter.toml")), 0o600);
    assert_eq!(mode(&h.rh.join("keys/lineage.ed25519")), 0o600);
    assert_eq!(fs::metadata(h.rh.join("keys/lineage.ed25519")).unwrap().len(), 32);
    assert_eq!(mode(&h.rh.join("run/canary")), 0o400);
    assert!(h.rh.join("keys/mirror_ed25519.pub").is_file() && h.rh.join("bin/init-root").is_file());
    for d in ["kernel/bin", "bin", "body", "work", "state/kernel", "probe", "cache", "vault", "run/mark", "root", "lineage/kernel.git"] {
        assert!(h.ih.join(d).is_dir(), "INIT_HOME/{d}");
    }
    assert!(h.ih.join("run/mark/decoy").is_file() && h.ih.join("run/zone-mark").is_file());
    let constitution = fs::read_to_string(h.ih.join("kernel/CONSTITUTION.md")).unwrap();
    assert!(constitution.starts_with("# INIT Constitution") && constitution.len() == 3930);
    assert_eq!(h.kinds(), ["genesis", "charter"]);
    let r = h.records();
    assert_eq!(r[1]["data"]["source"], "install");
    assert_eq!(r[1]["data"]["money"]["day_musd"], 5_000_000);
    assert!(!fs::read_to_string(h.rh.join("lineage/lineage.jsonl")).unwrap().contains(h.dir.to_str().unwrap()), "no paths in lineage");
    // the view (§3.10.6)
    assert_eq!(fs::read(h.ih.join("root/lineage.jsonl")).unwrap(), fs::read(h.rh.join("lineage/lineage.jsonl")).unwrap());
    let view: Value = serde_json::from_str(&fs::read_to_string(h.ih.join("root/charter.json")).unwrap()).unwrap();
    assert_eq!(view["charter_sha256"], r[1]["data"]["digest"]);
    assert_eq!(view["supported_max_level"], 4); // a test build
    let st: Value = serde_json::from_str(&fs::read_to_string(h.ih.join("root/status.json")).unwrap()).unwrap();
    assert_eq!((st["slot"].as_str(), st["level"].as_i64()), (Some("stopped"), Some(0)));
    assert!(h.dir.join("Library/LaunchAgents/dev.init.root.plist").is_file(), "service file under the test HOME");
    assert_eq!(h.run(&["install"]).status.code(), Some(1), "a second install is refused");
}

#[test]
fn install_with_the_template_stops_until_the_purpose_is_written() {
    let h = home("tmpl");
    let o = h.run(&["install"]);
    assert_eq!(o.status.code(), Some(2));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("purpose.text") && err.contains("then run install again"), "{err}");
    let tpl = fs::read_to_string(h.rh.join("charter.toml")).unwrap();
    assert!(tpl.contains("max_level = 4") && tpl.contains("nagisanzenin/INIT-Lineage") && tpl.contains("# INIT Constitution"));
    let fixed = tpl.replace("REPLACE ME: what INIT is for, in your words.", "Test purpose.")
        .replace("[notify]\ncmd = [\"/usr/bin/osascript\"", "[notify]\ncmd = [] #");
    // test mode: no Time Machine exclusion and no notification on the owner's screen
    fs::write(h.rh.join("charter.toml"), format!("{fixed}\n[root]\ntest_mode = true\n")).unwrap();
    let o = h.run(&["install"]);
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
}

#[test]
fn charter_check_reports_errors_and_apply_records_changes() {
    let h = home("chk");
    h.install("[root]\ntest_mode = true\n");
    assert_eq!(h.run(&["charter", "check"]).status.code(), Some(0));
    let bad = h.dir.join("bad.toml");
    fs::write(&bad, charter("[root]\ntest_mode = true\n[bogus]\nx = 1\n[money]\nday_usd = 5.0\n").replace("max_level = 4", "max_level = 7")).unwrap();
    let o = h.run(&["charter", "check", bad.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(2));
    let err = String::from_utf8_lossy(&o.stderr);
    for key in ["bogus", "money", "autonomy.max_level"] {
        assert!(err.contains(key), "{key} missing in: {err}");
    }
    // edit, check (diff), apply without --yes on a non-TTY, then with --yes (no daemon: under the Root's lock)
    let edited = charter("[root]\ntest_mode = true\n[money]\nday_usd = \"4.00\"\n");
    fs::write(h.rh.join("charter.toml"), &edited).unwrap();
    let o = h.run(&["charter", "check"]);
    assert!(String::from_utf8_lossy(&o.stdout).contains("money.day_usd: \"5.00\" → \"4.00\""));
    assert_eq!(h.run(&["charter", "check", "--apply"]).status.code(), Some(2));
    assert_eq!(h.kinds(), ["genesis", "charter"], "nothing applied yet");
    let o = h.run(&["charter", "check", "--apply", "--yes"]);
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    let r = h.records();
    assert_eq!((r[2]["kind"].as_str(), r[2]["data"]["source"].as_str()), (Some("charter"), Some("owner")));
    assert_eq!(r[2]["data"]["changed"], serde_json::json!(["money.day_usd"]));
    assert_eq!(fs::read_to_string(h.rh.join("state/charter.active.toml")).unwrap(), edited);
    let o = h.run(&["charter", "check", "--apply", "--yes"]);
    assert!(String::from_utf8_lossy(&o.stdout).contains("nothing to apply"));
    assert_eq!(h.records().len(), 3);
}

#[test]
fn keys_set_reads_only_a_tty_and_stores_0600() {
    let h = home("keys");
    h.install("[root]\ntest_mode = true\n");
    let o = h.cmd().args(["keys", "set", "deepseek"]).stdin(Stdio::piped()).output().unwrap();
    assert_eq!(o.status.code(), Some(2), "a pipe is not a TTY");
    assert!(!h.rh.join("keys/providers/deepseek").exists());
    assert_eq!(h.run(&["keys", "set", "Bad/Id"]).status.code(), Some(64));
    // through a pseudo-terminal: /usr/bin/script runs the CLI on a pty fed from our pipe
    let mut c = Command::new("/usr/bin/script");
    c.args(["-q", "/dev/null", BIN, "keys", "set", "deepseek"]).env_clear()
        .env("HOME", &h.dir).env("INIT_ROOT_HOME", &h.rh).env("PATH", "/usr/bin:/bin");
    let mut child = c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let rx = chunks(child.stdout.take().unwrap());
    let mut seen = String::new();
    wait(10, || { seen.push_str(&rx.try_recv().unwrap_or_default()); seen.contains("input hidden") });
    stdin.write_all(b"sk-test-wp20-not-a-real-key\n").unwrap();
    let status = child.wait().unwrap();
    drop(stdin);
    seen.extend(rx.try_iter());
    assert!(status.success(), "{seen}");
    assert!(!seen.contains("sk-test-wp20"), "the secret was echoed: {seen}");
    let k = h.rh.join("keys/providers/deepseek");
    assert_eq!((mode(&k), fs::read_to_string(&k).unwrap()), (0o600, "sk-test-wp20-not-a-real-key".to_string()));
    let check = |h: &Home| String::from_utf8(h.run(&["keys", "check"]).stdout).unwrap();
    assert!(check(&h).contains("providers/deepseek: present") && !check(&h).contains("sk-test"));
    fs::set_permissions(&k, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(check(&h).contains("providers/deepseek: insecure"));
}

/// Stdout of a child as a channel of chunks (one per line).
fn lines(r: impl std::io::Read + Send + 'static) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for l in BufReader::new(r).lines().map_while(Result::ok) {
            if tx.send(l + "\n").is_err() {
                return;
            }
        }
    });
    rx
}

/// Raw output chunks (a prompt has no newline).
fn chunks(mut r: impl std::io::Read + Send + 'static) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut b = [0u8; 4096];
        while let Ok(n) = r.read(&mut b) {
            if n == 0 || tx.send(String::from_utf8_lossy(&b[..n]).into_owned()).is_err() {
                return;
            }
        }
    });
    rx
}

fn expect(rx: &mpsc::Receiver<String>, needle: &str) -> String {
    let mut seen = String::new();
    wait(10, || { seen.extend(rx.try_iter()); seen.contains(needle) });
    seen
}

#[test]
fn owner_socket_attach_panic_resume_and_restart() {
    let h = home("pnc");
    let notified = h.dir.join("notified.jsonl");
    let watch = h.dir.join("watch-panic");
    h.install(&format!("[root]\ntest_mode = true\n[notify]\ncmd = [\"/usr/bin/tee\", \"-a\", \"{}\"]\n[panic]\nwatch_paths = [\"{}\"]\n",
        notified.display(), watch.display()));
    let d = h.daemon(&[]);
    let st = h.status();
    assert_eq!((st["slot"].as_str(), st["level"].as_i64(), st["charter_pending"].as_bool()), (Some("stopped"), Some(0), Some(false)));
    assert_eq!(h.run(&["start"]).status.code(), Some(0), "a second start reports the running Root");
    // attach: a line is queued and echoed; /panic is handled by the Root
    let mut a = h.cmd().arg("attach").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let rx = lines(a.stdout.take().unwrap());
    let mut ain = a.stdin.take().unwrap();
    expect(&rx, "[root] attached");
    writeln!(ain, "hello root").unwrap();
    let seen = expect(&rx, "queued (1)");
    assert!(seen.contains("» hello root"), "{seen}");
    writeln!(ain, "/panic").unwrap();
    expect(&rx, "PANIC at");
    drop(ain);
    assert_eq!(a.wait().unwrap().code(), Some(0), "attach detaches on EOF");
    let st = h.status();
    assert_eq!((st["slot"].as_str(), st["panic"]["channel"].as_str()), (Some("panicked"), Some("owner_socket")));
    let latch: Value = serde_json::from_str(&fs::read_to_string(h.rh.join("run/panic")).unwrap()).unwrap();
    assert_eq!(latch["channel"], "owner_socket");
    let r = h.records();
    let n = r.len();
    assert_eq!((r[n - 2]["kind"].as_str(), r[n - 1]["kind"].as_str()), (Some("panic"), Some("ladder")));
    assert_eq!(r[n - 1]["data"]["trigger"], serde_json::json!({"kind": "panic", "seq": r[n - 2]["seq"]}));
    assert_eq!((r[n - 1]["data"]["from"].as_i64(), r[n - 1]["data"]["to"].as_i64()), (Some(0), Some(0)), "L0 stays 0→0");
    assert!(fs::read_to_string(h.rh.join("alerts.jsonl")).unwrap().contains("\"code\":\"panic\""));
    wait(5, || fs::read_to_string(&notified).is_ok_and(|s| s.contains("PANIC at")));
    assert_eq!(fs::read_to_string(h.rh.join("state/owner-queue.jsonl")).unwrap().lines().count(), 1);
    // a second panic only reports the first
    let o = h.run(&["panic"]);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(h.records().len(), n);
    // resume, then the latch-file channel
    assert_eq!(h.run(&["resume"]).status.code(), Some(0));
    assert!(!h.rh.join("run/panic").exists());
    assert_eq!(h.status()["slot"], "stopped");
    assert_eq!(h.kinds().last().map(String::as_str), Some("resume"));
    fs::write(h.rh.join("run/panic"), "").unwrap(); // `touch ~/.init-root/run/panic`
    wait(5, || h.status()["panic"]["channel"] == "latch_file");
    assert_eq!(h.run(&["resume"]).status.code(), Some(0));
    // the watch-path channel; resume is refused while the file exists
    fs::write(&watch, "").unwrap();
    wait(5, || h.status()["panic"]["channel"] == "watch_path");
    let o = h.run(&["resume"]);
    assert_eq!(o.status.code(), Some(77), "{}", String::from_utf8_lossy(&o.stderr));
    fs::remove_file(&watch).unwrap();
    assert_eq!(h.run(&["resume"]).status.code(), Some(0));
    // the latch survives a Root restart, and the panic is not recorded twice
    assert_eq!(h.run(&["panic"]).status.code(), Some(0));
    let panics = |h: &Home| h.kinds().iter().filter(|k| *k == "panic").count();
    let before = panics(&h);
    drop(d);
    let _d = h.daemon(&[]);
    assert_eq!(h.status()["slot"], "panicked");
    assert_eq!(panics(&h), before);
    assert_eq!(h.run(&["resume"]).status.code(), Some(0));
    assert_eq!(h.run(&["stop"]).status.code(), Some(0));
}

#[test]
fn panic_without_a_root_latches_and_is_recorded_once_at_start() {
    let h = home("latch");
    h.install("[root]\ntest_mode = true\n");
    let o = h.run(&["panic"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&o.stdout).contains("PANIC latched"));
    let st = h.status();
    assert_eq!((st["slot"].as_str(), st["panic"]["channel"].as_str()), (Some("down"), Some("cli_fallback")));
    for _ in 0..2 {
        let _d = h.daemon(&[]);
        assert_eq!(h.status()["panic"]["channel"], "cli_fallback");
    }
    assert_eq!(h.kinds().iter().filter(|k| *k == "panic").count(), 1);
}

#[test]
fn sandboxed_peers_are_rejected() {
    let h = home("peer");
    h.install("[root]\ntest_mode = true\n");
    let _d = h.daemon(&[]);
    // a sandbox applied only to a process this test spawns
    let o = Command::new("/usr/bin/sandbox-exec").args(["-p", "(version 1)(allow default)", BIN, "status", "--json"])
        .env_clear().env("HOME", &h.dir).env("INIT_ROOT_HOME", &h.rh).env("PATH", "/usr/bin:/bin").output().unwrap();
    assert_ne!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stdout));
    assert!(fs::read_to_string(h.rh.join("logs/root.jsonl")).unwrap().contains("peer rejected"));
    assert_eq!(h.status()["slot"], "stopped", "an unsandboxed owner still gets through");
}

#[test]
fn layout_violation_halts_the_root() {
    let h = home("halt");
    h.install("[root]\ntest_mode = true\n");
    fs::set_permissions(h.rh.join("keys"), fs::Permissions::from_mode(0o755)).unwrap();
    let _d = h.daemon(&[]);
    let st = h.status();
    assert_eq!((st["slot"].as_str(), st["halted"].as_str()), (Some("halted"), Some("layout")));
    assert!(fs::read_to_string(h.rh.join("alerts.jsonl")).unwrap().contains("\"code\":\"halted\""));
    assert_eq!(h.run(&["resume"]).status.code(), Some(77), "an integrity halt needs a fix, not a resume");
    assert_eq!(h.kinds().last().map(String::as_str), Some("halt"));
}

#[test]
fn test_ledger_fake_clock_cap_and_panic_demotion() {
    let h = home("tl");
    let spec = h.dir.join("spec.json");
    fs::write(&spec, r#"{"start_at":"2026-09-01T00:00:00Z","start_level":2,"records":[
        {"at":"2026-09-02T00:00:00Z","kind":"promotion","count":3,"step_s":3600,
         "data":{"sha":"{i}aaaa","from_sha":"x","gate_job":"g-{i}","scope":"t1","lkg":true,"body_binary":"b","ext_set":"e","components":[]}},
        {"at":"2026-09-01T12:00:00Z","kind":"audit","data":{"tuple":"t1","score_permille":900,"regression":false,"trigger":"manual"}}]}"#).unwrap();
    // refused without test_mode
    fs::create_dir_all(&h.rh).unwrap();
    fs::write(h.rh.join("charter.toml"), charter("")).unwrap();
    let o = h.run(&["test", "ledger", "--spec", spec.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(2), "{}", String::from_utf8_lossy(&o.stderr));
    fs::write(h.rh.join("charter.toml"), charter("[root]\ntest_mode = true\n")).unwrap();
    let o = h.run(&["test", "ledger", "--spec", spec.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(h.run(&["test", "ledger", "--spec", spec.to_str().unwrap()]).status.code(), Some(2), "lineage not empty");
    let r = h.records();
    assert_eq!(r[0]["data"]["test"], true);
    assert_eq!(h.kinds(), ["genesis", "ladder", "ladder", "audit", "promotion", "promotion", "promotion"]);
    assert_eq!(r[2]["data"]["trigger"]["kind"], "test_fixture");
    assert_eq!((r[4]["by"].as_str(), r[6]["data"]["gate_job"].as_str()), (Some("kernel:000000000000"), Some("g-2")));
    assert_eq!(r[6]["at"], "2026-09-02T02:00:00.000Z");
    // install keeps the test chain and appends the charter record; the Root verifies it at start
    let o = h.run(&["install"]);
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(h.kinds().last().map(String::as_str), Some("charter"));
    let d = h.daemon(&[("INIT_ROOT_TEST_NOW", "2026-11-01T00:00:00Z")]);
    let st = h.status();
    assert_eq!((st["root"]["now"].as_str(), st["level"].as_i64(), st["slot"].as_str()), (Some("2026-11-01T00:00:00.000Z"), Some(2), Some("stopped")));
    let o = h.run(&["test", "clock", "--advance", "3600"]);
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(h.status()["root"]["now"], "2026-11-01T01:00:00.000Z");
    assert_eq!(h.run(&["test", "clock", "--set", "2026-10-01T00:00:00Z"]).status.code(), Some(2), "never backwards");
    // lowering max_level below the level caps it (RT-L5), with no rollback
    fs::write(h.rh.join("charter.toml"), charter("[root]\ntest_mode = true\n").replace("max_level = 4", "max_level = 1")).unwrap();
    let o = h.run(&["charter", "check", "--apply", "--yes"]);
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    let last = h.records().pop().unwrap();
    assert_eq!((last["kind"].as_str(), last["data"]["direction"].as_str(), last["data"]["to"].as_i64()), (Some("ladder"), Some("cap"), Some(1)));
    assert_eq!(last["at"], "2026-11-01T01:00:00.000Z", "records follow the fake clock");
    assert_eq!((h.status()["level"].as_i64(), h.status()["cap"].as_i64()), (Some(1), Some(1)));
    // a panic demotes one level (CH-2)
    assert_eq!(h.run(&["panic"]).status.code(), Some(0));
    let last = h.records().pop().unwrap();
    assert_eq!((last["data"]["from"].as_i64(), last["data"]["to"].as_i64()), (Some(1), Some(0)));
    let ledger: Value = serde_json::from_slice(&h.run(&["ledger"]).stdout).unwrap();
    assert_eq!((ledger["level"].as_i64(), ledger["last_change"]["trigger"]["kind"].as_str()), (Some(0), Some("panic")));
    drop(d);
}
