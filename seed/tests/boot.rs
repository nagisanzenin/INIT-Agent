#![forbid(unsafe_code)]
//! body/boot.md §8 end to end: the built `init-body` driven over stdio-jsonl/1 by an in-test
//! kernel stand-in, against `init-mockllm` ($0) on the proxy socket.
//!
//! Every test home lives under `<target>/test-homes/wp13/` (the test binary's target dir) and is
//! removed by its exact path, with its two socket files in `<target>/sk/`.
//! The only processes signalled are the children these tests spawn, by their `Child` handle.
//! Tests that need the mock print `SKIP` and pass when no `init-mockllm` binary exists (set
//! `INIT_MOCKLLM_BIN`, or build `kernel/` lane `t`: `kernel/target/t/debug/init-mockllm`).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const SHA: &str = "4b1e0c2d9a7f4b1e0c2d9a7f4b1e0c2d9a7f4b1e";
const SHA_B: &str = "7c1d2e9a0b3c7c1d2e9a0b3c7c1d2e9a0b3c7c1d";
const LKG: &str = "3f2a1c0d9e8b3f2a1c0d9e8b3f2a1c0d9e8b3f2a";
const SID: &str = "20261020-040000-a1b2c3";

static N: AtomicU64 = AtomicU64::new(0);

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn body_bin() -> PathBuf {
    PathBuf::from(
        option_env!("CARGO_BIN_EXE_init-body")
            .or(option_env!("CARGO_BIN_EXE_init-body-ov13"))
            .expect("the init-body binary"),
    )
}

/// The repository root (the directory holding `kernel/crates/init-testkit`).
fn repo_root() -> Option<PathBuf> {
    manifest_dir()
        .ancestors()
        .find(|a| a.join("kernel/crates/init-testkit").is_dir())
        .map(Path::to_path_buf)
}

fn mock_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("INIT_MOCKLLM_BIN").map(PathBuf::from) {
        assert!(p.exists(), "INIT_MOCKLLM_BIN={} does not exist", p.display());
        return Some(p);
    }
    // Any kernel lane's debug build (lanes use kernel/target/<lane>/).
    let mut found: Vec<PathBuf> = std::fs::read_dir(repo_root()?.join("kernel/target"))
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path().join("debug/init-mockllm")))
        .filter(|p| p.exists())
        .collect();
    found.sort();
    found.into_iter().next()
}

/// The seed's head files: from the crate (GENESIS.md, SELF.md) and the seed constitution.
fn crate_file(rel: &str) -> Vec<u8> {
    std::fs::read(manifest_dir().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// The longest Unix socket path the body accepts (boot.md §3.2; `sun_path` is 104 bytes on macOS).
const MAX_SOCK: usize = 103;

struct Home {
    path: PathBuf,
    /// The proxy socket (`INIT_PROXY_SOCK`, where the mock listens) and `INIT_KERNEL_SOCK`.
    proxy_sock: PathBuf,
    kernel_sock: PathBuf,
}

impl Home {
    /// A fresh INIT_HOME with body/ (head files), kernel/CONSTITUTION.md, state/kernel/, work/.
    ///
    /// Homes live in the test binary's target dir, the one tree that is writable both in the dev
    /// repo (`seed/target`) and under G0 (`cache/build/slot/out/target`; the checkout itself is
    /// read-only there). The sockets get short names in `<target>/sk/`, not in the home: under G0 a
    /// home path alone can take 100 bytes, and the body refuses socket paths over [`MAX_SOCK`].
    fn new(tag: &str) -> Home {
        let n = N.fetch_add(1, Ordering::Relaxed);
        let path = target_dir()
            .join("test-homes/wp13")
            .join(format!("{tag}{n}"));
        if path.exists() {
            std::fs::remove_dir_all(&path).unwrap();
        }
        let socks = target_dir().join("sk");
        std::fs::create_dir_all(&socks).unwrap();
        let proxy_sock = socks.join(format!("{tag}{n}.p"));
        let kernel_sock = socks.join(format!("{tag}{n}.k"));
        for s in [&proxy_sock, &kernel_sock] {
            assert!(
                s.as_os_str().len() <= MAX_SOCK,
                "socket path too long ({} > {MAX_SOCK} bytes): {}",
                s.as_os_str().len(),
                s.display()
            );
            let _ = std::fs::remove_file(s); // a stale socket from an aborted run, by its exact path
        }
        for d in ["body", "kernel", "state/kernel", "work/home"] {
            std::fs::create_dir_all(path.join(d)).unwrap();
        }
        std::fs::write(path.join("body/GENESIS.md"), crate_file("GENESIS.md")).unwrap();
        std::fs::write(path.join("body/SELF.md"), crate_file("SELF.md")).unwrap();
        std::fs::write(
            path.join("kernel/CONSTITUTION.md"),
            crate_file("src/boot/CONSTITUTION.md"),
        )
        .unwrap();
        Home {
            path,
            proxy_sock,
            kernel_sock,
        }
    }

    fn sock(&self) -> PathBuf {
        self.proxy_sock.clone()
    }

    /// `state/kernel/boot.json` in supervise.md §4.3's shape, merged with `extra`.
    fn boot_json(&self, boot_id: &str, mode: &str, reason: &str, extra: Value) {
        let mut b = json!({
            "body_id": "b0", "boot_id": boot_id, "boot_seq": 1, "branch": null, "cause": null,
            "crash": null,
            "env": {
                "budget": {"cap_musd": 4000000, "day_starts_at": "2026-10-19T17:00:00Z", "today_musd": 1230000},
                "init_home": self.path.to_string_lossy(), "kernel_version": "0.1.0", "mem_total_mb": 16384,
                "model": {"context_window": 131072, "id": "mock-frontier-1", "max_output": 32768, "provider": "mock"},
                "network": {"egress": [], "mode": "proxy-only"},
                "os": {"arch": "arm64", "family": "macos", "version": "14.6"},
                "root": {"audited_good": LKG, "cap": 0, "ceilings": {"day_musd": 5000000, "parallel_bodies": 1,
                         "total_musd": 25000000, "total_used_musd": 7400000}, "classes": ["model-providers"],
                         "last_change": null, "level": 0, "max_level": 4, "next": null, "supported_max_level": 0},
                "sandbox": {"hidden": ["cache/", "probe/", "vault/"], "kind": "seatbelt", "landlock": null,
                            "landlock_abi": null, "network": "kernel-sockets-only",
                            "read_only": ["bin/", "kernel/", "lineage/", "state/kernel/"], "seccomp": false,
                            "sweep": "full", "write": ["body/", "state/", "work/"]}
            },
            "evolution": {}, "from_sha": null, "kernel": {"version": "0.1.0"}, "kernel_upgrade": null,
            "lkg_sha": LKG, "lkg_tree": "lineage/trees/3f2a1c0d9e8b", "mode": mode,
            "model": {"effort": null, "model": "mock-frontier-1", "provider": "mock"},
            "panic": null, "pending": {"inputs": 0, "notices": 0}, "provisional": null, "reason": reason,
            "restarts_since_lkg": 0, "safe": null, "selfmap": {"body_tokens": 21480},
            "session": SID, "sha": SHA, "spawned_at": "2026-10-20T04:00:00.000Z", "update_failed": null,
            "v": 1, "x_extra_field": {"kept": true}
        });
        if let (Value::Object(m), Value::Object(e)) = (&mut b, extra) {
            for (k, v) in e {
                m.insert(k, v);
            }
        }
        std::fs::write(
            self.path.join("state/kernel/boot.json"),
            serde_json::to_vec(&b).unwrap(),
        )
        .unwrap();
    }

    fn log(&self) -> Vec<Value> {
        std::fs::read_to_string(self.path.join(format!("state/sessions/{SID}.jsonl")))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    fn ready(&self) -> Option<Value> {
        let b = std::fs::read(self.path.join("state/run/ready")).ok()?;
        serde_json::from_slice(&b).ok()
    }

    fn heartbeat(&self) -> Option<Value> {
        let b = std::fs::read(self.path.join("state/run/heartbeat")).ok()?;
        serde_json::from_slice(&b).ok()
    }

    /// Polls for a `ready` with this boot id.
    fn wait_ready(&self, boot_id: &str, body: &mut Body, limit: Duration) -> Value {
        let t0 = Instant::now();
        loop {
            if let Some(r) = self.ready()
                && r["boot_id"] == boot_id
            {
                return r;
            }
            if let Some(code) = body.try_exit() {
                panic!("body exited {code:?} before ready; stderr:\n{}", body.stderr());
            }
            assert!(
                t0.elapsed() < limit,
                "no ready within {limit:?}; stderr:\n{}",
                body.stderr()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        if std::env::var("INIT_WP13_KEEP").as_deref() != Ok("1") {
            let _ = std::fs::remove_dir_all(&self.path);
            let _ = std::fs::remove_file(&self.proxy_sock);
            let _ = std::fs::remove_file(&self.kernel_sock);
        }
    }
}

struct Mock {
    child: Child,
    rec: PathBuf,
}

impl Mock {
    /// None (test skipped) when no mock binary exists.
    fn start(home: &Home, name: &str, script: &str, args: &[&str]) -> Option<Mock> {
        let Some(bin) = mock_bin() else {
            eprintln!("SKIP {name}: no init-mockllm (set INIT_MOCKLLM_BIN)");
            return None;
        };
        let script_path = home.path.join("s.mock.jsonl");
        std::fs::write(&script_path, script).unwrap();
        let rec = home.path.join("rec");
        let child = Command::new(bin)
            .args(["--mode", "socket", "--no-g3-responder"])
            .args(args)
            .arg("--socket")
            .arg(home.sock())
            .arg("--script")
            .arg(&script_path)
            .arg("--record-dir")
            .arg(&rec)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn init-mockllm");
        let mut m = Mock { child, rec };
        let t0 = Instant::now();
        while UnixStream::connect(home.sock()).is_err() {
            if let Ok(Some(st)) = m.child.try_wait() {
                panic!("init-mockllm exited early: {st}");
            }
            assert!(t0.elapsed() < Duration::from_secs(10), "mock did not listen");
            std::thread::sleep(Duration::from_millis(10));
        }
        Some(m)
    }

    fn log(&self) -> Vec<Value> {
        std::fs::read_to_string(self.rec.join("mock.log.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    fn bodies(&self) -> Vec<Value> {
        let mut names: Vec<PathBuf> = std::fs::read_dir(&self.rec)
            .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
            .unwrap_or_default();
        names.retain(|p| p.extension().is_some_and(|x| x == "json"));
        names.sort();
        names
            .iter()
            .map(|p| serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap())
            .collect()
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        let _ = self.child.kill(); // our own child, by handle
        let _ = self.child.wait();
    }
}

struct Body {
    child: Child,
    stdin: Option<ChildStdin>,
    frames: Receiver<Value>,
    seen: Vec<Value>,
    err: Arc<Mutex<String>>,
    /// Set when the stderr reader saw EOF (every line the body wrote is in `err`).
    err_eof: Arc<std::sync::atomic::AtomicBool>,
}

/// The body's environment as supervise.md §4.1 builds it (test mode).
fn body_env(home: &Home, mode: &str, reason: &str, boot_id: &str) -> Vec<(String, String)> {
    let h = home.path.to_string_lossy().into_owned();
    let mut v: Vec<(String, String)> = [
        ("INIT_SESSION", SID),
        ("INIT_SHA", SHA),
        ("INIT_BOOT_REASON", reason),
        ("INIT_MODE", mode),
        ("INIT_BOOT_ID", boot_id),
        ("INIT_UI", "stdio-jsonl/1"),
        ("INIT_KERNEL_VERSION", "0.1.0"),
        ("INIT_TEST", "1"),
        ("INIT_TEST_RETRY_BASE_MS", "5"),
        ("INIT_TEST_IDLE_TIMEOUT_MS", "5000"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("LANG", "C"),
        ("TERM", "dumb"),
        ("NO_COLOR", "1"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    v.push(("INIT_HOME".into(), h.clone()));
    v.push(("INIT_KERNEL_SOCK".into(), home.kernel_sock.to_string_lossy().into_owned()));
    v.push(("INIT_PROXY_SOCK".into(), home.sock().to_string_lossy().into_owned()));
    v.push(("HOME".into(), format!("{h}/work/home")));
    v.push(("GIT_CEILING_DIRECTORIES".into(), h));
    v.push(("PATH".into(), std::env::var("PATH").unwrap_or_default()));
    v
}

impl Body {
    fn spawn(home: &Home, mode: &str, reason: &str, boot_id: &str, extra: &[(&str, &str)]) -> Body {
        Self::spawn_env(body_env(home, mode, reason, boot_id), home, extra)
    }

    fn spawn_env(mut env: Vec<(String, String)>, home: &Home, extra: &[(&str, &str)]) -> Body {
        for (k, v) in extra {
            env.retain(|(ek, _)| ek != k);
            env.push((k.to_string(), v.to_string()));
        }
        let mut child = Command::new(body_bin())
            .env_clear()
            .envs(env)
            .current_dir(home.path.join("work"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn init-body");
        let (tx, frames) = channel();
        let out = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines() {
                let Ok(l) = line else { break };
                if let Ok(v) = serde_json::from_str::<Value>(&l) {
                    let _ = tx.send(v);
                }
            }
        });
        let err = Arc::new(Mutex::new(String::new()));
        let err_eof = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (e2, eof2) = (err.clone(), err_eof.clone());
        let se = child.stderr.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(se).lines() {
                let Ok(l) = line else { break };
                let mut s = e2.lock().unwrap();
                s.push_str(&l);
                s.push('\n');
            }
            eof2.store(true, Ordering::SeqCst);
        });
        Body {
            stdin: child.stdin.take(),
            child,
            frames,
            seen: Vec::new(),
            err,
            err_eof,
        }
    }

    fn stderr(&self) -> String {
        self.err.lock().unwrap().clone()
    }

    /// All of stderr, once the reader thread saw EOF (an exited body's last line may still be in
    /// the pipe when `wait_exit` returns).
    fn stderr_complete(&self, limit: Duration) -> String {
        let t0 = Instant::now();
        while !self.err_eof.load(Ordering::SeqCst) && t0.elapsed() < limit {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.stderr()
    }

    fn send(&mut self, mut v: Value) {
        v["v"] = 1.into();
        let s = self.stdin.as_mut().expect("stdin open");
        writeln!(s, "{v}").unwrap();
        s.flush().unwrap();
    }

    /// The next frame matching `pred` (earlier non-matching frames are kept in `seen`).
    fn wait_frame(&mut self, what: &str, limit: Duration, pred: impl Fn(&Value) -> bool) -> Value {
        let t0 = Instant::now();
        loop {
            let left = limit.saturating_sub(t0.elapsed());
            match self.frames.recv_timeout(left.max(Duration::from_millis(1))) {
                Ok(v) => {
                    if pred(&v) {
                        self.seen.push(v.clone());
                        return v;
                    }
                    self.seen.push(v);
                }
                Err(_) => panic!(
                    "no {what} frame within {limit:?}; seen {:?}; stderr:\n{}",
                    self.seen,
                    self.stderr()
                ),
            }
        }
    }

    fn try_exit(&mut self) -> Option<Option<i32>> {
        self.child.try_wait().ok().flatten().map(|s| s.code())
    }

    fn wait_exit(&mut self, limit: Duration) -> Option<i32> {
        let t0 = Instant::now();
        loop {
            if let Some(c) = self.try_exit() {
                return c;
            }
            assert!(
                t0.elapsed() < limit,
                "body still running after {limit:?}; stderr:\n{}",
                self.stderr()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// SIGKILL our own child (a crash), by handle.
    fn kill9(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Body {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn boot_events(log: &[Value]) -> Vec<&Value> {
    log.iter().filter(|e| e["type"] == "boot").collect()
}

fn stop(body: &mut Body) {
    body.send(json!({"t": "stop", "reason": "test", "grace_ms": 1000}));
    assert_eq!(body.wait_exit(Duration::from_secs(10)), Some(0), "{}", body.stderr());
}

fn last_message_text(body: &Value) -> String {
    let m = body["messages"].as_array().unwrap().last().unwrap();
    match &m["content"] {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------------------------
// argv, manifest, config errors (no mock needed)
// ---------------------------------------------------------------------------------------------

/// §8 item 11: `env -i ./init-body --manifest` with INIT_HOME pointing nowhere.
#[test]
fn manifest_reads_no_env() {
    let out = Command::new(body_bin())
        .arg("--manifest")
        .env_clear()
        .env("INIT_HOME", "/nonexistent/wp13-home")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let s = String::from_utf8(out.stdout).unwrap();
    assert_eq!(s.matches('\n').count(), 1, "one line: {s}");
    let m: Value = serde_json::from_str(s.trim_end()).unwrap();
    assert_eq!(m["manifest"], 1);
    assert_eq!(m["control_channel"], "stdio-jsonl/1");
    assert_eq!(m["modes"], json!(["normal", "probe", "safe", "check"]));
    assert_eq!(
        m["subcommands"],
        json!(["--manifest", "--version", "ext call", "ext describe", "ext inspect"])
    );
    // tools = tools.md §4.1 (the registry golden), tools_sha256 = init-tools-v1, cross-checked
    // with the system's shasum (an independent SHA-256).
    let golden: Value =
        serde_json::from_slice(&crate_file("src/context/testdata/tools.json")).unwrap();
    assert_eq!(m["tools"], golden);
    let icj = serde_json::to_string(&golden).unwrap(); // BTreeMap keys: sorted, no spaces
    assert_eq!(icj.len(), 3_405);
    let mut sh = Command::new("/usr/bin/shasum")
        .args(["-a", "256"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    sh.stdin.take().unwrap().write_all(icj.as_bytes()).unwrap();
    let hex = String::from_utf8(sh.wait_with_output().unwrap().stdout).unwrap();
    assert_eq!(
        m["tools_sha256"].as_str().unwrap(),
        format!("sha256:{}", &hex[..64])
    );
}

/// §8 item 12: `--version`, bad argv → 64, `ext …` dispatched to the host CLI.
#[test]
fn argv_dispatch() {
    let out = Command::new(body_bin()).arg("--version").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("init-body {}\n", env!("CARGO_PKG_VERSION")));
    for bad in [&["--frobnicate"][..], &["--version", "x"], &["--manifest", "--version"]] {
        let out = Command::new(body_bin()).args(bad).output().unwrap();
        assert_eq!(out.status.code(), Some(64), "{bad:?}");
    }
    let out = Command::new(body_bin()).args(["ext", "describe"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2), "ext usage (host.md §4.5)");
    let out = Command::new(body_bin()).args(["ext", "frobnicate", "x.wasm"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2), "unknown ext subcommand");
}

fn expect_boot_error(body: &mut Body, code: i32, step: &str, contains: &str) {
    assert_eq!(body.wait_exit(Duration::from_secs(10)), Some(code));
    let err = body.stderr_complete(Duration::from_secs(5));
    let line = err
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v["type"] == "boot_error")
        .unwrap_or_else(|| panic!("no boot_error line: {err}"));
    assert_eq!(line["code"], code);
    assert_eq!(line["step"], step, "{line}");
    assert!(
        line["message"].as_str().unwrap().contains(contains),
        "{line}"
    );
}

/// §8 items 1–2 through the binary: env and boot.json errors exit 78 with a boot_error line.
#[test]
fn config_errors_exit_78() {
    let home = Home::new("cf");
    let bid = "1111111111111111";
    // No boot.json.
    let mut b = Body::spawn(&home, "normal", "cold", bid, &[]);
    expect_boot_error(&mut b, 78, "boot_json", "boot.json");
    // INIT_UI of another protocol version.
    home.boot_json(bid, "normal", "cold", json!({}));
    let mut b = Body::spawn(&home, "normal", "cold", bid, &[("INIT_UI", "stdio-jsonl/2")]);
    expect_boot_error(&mut b, 78, "env", "INIT_UI");
    // boot_id mismatch.
    let mut b = Body::spawn(&home, "normal", "cold", "2222222222222222", &[]);
    expect_boot_error(&mut b, 78, "boot_json", "INIT_BOOT_ID");
    // Unknown model, listing the known ones.
    home.boot_json(
        bid,
        "normal",
        "cold",
        json!({"model": {"effort": null, "model": "deepseek-chat", "provider": "deepseek"}}),
    );
    let mut b = Body::spawn(&home, "normal", "cold", bid, &[]);
    expect_boot_error(
        &mut b,
        78,
        "model",
        "unknown model \"deepseek/deepseek-chat\"; known: deepseek/deepseek-flash, deepseek/deepseek-v4-pro,",
    );
    // A bad effort.
    home.boot_json(
        bid,
        "normal",
        "cold",
        json!({"model": {"effort": "turbo", "model": "mock-frontier-1", "provider": "mock"}}),
    );
    let mut b = Body::spawn(&home, "normal", "cold", bid, &[]);
    expect_boot_error(&mut b, 78, "model", "bad effort \"turbo\"");
    // Nothing was written to the log by a failed config boot.
    assert!(home.log().is_empty());
}

/// §8 item 14: the seed build configuration (§3.16).
#[test]
fn seed_build_configuration() {
    let raw = String::from_utf8(crate_file("Cargo.toml")).unwrap();
    // Comments stripped (the manifest's own comment names the forbidden feature).
    let toml: String = raw
        .lines()
        .map(|l| l.split(" #").next().unwrap_or("").trim_end())
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!toml.contains("preserve_order"));
    assert!(toml.contains("[profile.dev]\npanic = \"unwind\""));
    assert!(toml.contains("[profile.dev.package.\"*\"]"));
    assert!(toml.contains("opt-level = 2"));
    assert!(toml.contains("exclude = [\"ext\"]"));
    assert!(toml.contains("serde_json = \"=1.0.154\""));
    let lock = String::from_utf8(crate_file("Cargo.lock")).unwrap();
    assert!(!lock_has_dep(&lock, "serde_json", "indexmap"), "serde_json without preserve_order");
    let main = String::from_utf8(crate_file("src/main.rs")).unwrap();
    assert_eq!(main.lines().next(), Some("#![forbid(unsafe_code)]"));
    // .cargo/config.toml: vendored sources, offline, nothing else (G0's build-config policy).
    let cfg = manifest_dir()
        .ancestors()
        .map(|a| a.join(".cargo/config.toml"))
        .find(|p| p.exists())
        .expect(".cargo/config.toml");
    let text = std::fs::read_to_string(cfg).unwrap();
    for l in text.lines().filter(|l| !l.trim().is_empty()) {
        assert!(
            [
                "[source.crates-io]",
                "replace-with = \"vendored-sources\"",
                "[source.vendored-sources]",
                "[net]",
                "offline = true",
            ]
            .contains(&l)
                || l.starts_with("directory = "),
            "unexpected line in .cargo/config.toml: {l}"
        );
    }
}

/// Whether `pkg`'s `[[package]]` block in Cargo.lock lists `dep`.
fn lock_has_dep(lock: &str, pkg: &str, dep: &str) -> bool {
    lock.split("[[package]]").any(|b| {
        b.contains(&format!("name = \"{pkg}\"\n")) && b.contains(&format!(" \"{dep}\""))
    })
}

// ---------------------------------------------------------------------------------------------
// normal boot: ready, heartbeat, pause, stop, lock
// ---------------------------------------------------------------------------------------------

/// §8 items 5 and 6, D11: `ready` is atomic and matches; the boot event is on disk before `ready`;
/// the heartbeat starts at once and runs every 5 s, also while paused; `stop` while paused exits 0
/// without writing to the log.
#[test]
fn ready_heartbeat_pause_stop() {
    let home = Home::new("rh");
    let bid = "a3f09c1e5b7d2468";
    // evolution-v2 §4.6: the engine counts reach the boot event and its text.
    let evo = json!({"frictions_eligible": 3, "frictions_top": {"instances": 6, "signature": "path.home_relative"},
                     "proposals_by_state": {"screened": 1}, "proposals_open": 1});
    home.boot_json(bid, "normal", "cold", json!({"evolution": evo}));
    let Some(_mock) = Mock::start(&home, "ready_heartbeat_pause_stop", "", &[]) else {
        return;
    };
    let mut body = Body::spawn(&home, "normal", "cold", bid, &[]);
    let ready = home.wait_ready(bid, &mut body, Duration::from_secs(20));
    let t_ready = Instant::now();
    assert_eq!(ready["pid"], body.pid());
    assert_eq!(ready["mode"], "normal");
    assert_eq!(ready["probe"], Value::Null);
    assert_eq!(ready["session"], SID);
    assert_eq!(ready["sha"], SHA);
    assert_eq!(ready["v"], 1);
    assert!(ready["prefix_hash"].as_str().unwrap().starts_with("sha256:"));
    // The boot event is durable before ready.
    let log = home.log();
    let boots = boot_events(&log);
    assert_eq!(boots.len(), 1);
    let boot = boots[0];
    assert_eq!(boot["id"], ready["boot_event_id"]);
    assert_eq!(boot["prefix_hash"], ready["prefix_hash"]);
    assert_eq!(boot["boot_id"], bid);
    assert_eq!(boot["facts"]["kernel"]["x_extra_field"], json!({"kept": true}));
    assert_eq!(boot["facts"]["mode"], "normal");
    assert_eq!(boot["facts"]["model"], json!({"effort": "high", "model": "mock-frontier-1", "provider": "mock"}));
    assert_eq!(boot["facts"]["ext"], json!({"status": "loading"}), "WP14's host, Boot load queued");
    assert!(boot["text"].as_str().unwrap().starts_with("[init:boot] "));
    assert!(boot["constitution_sha256"].as_str().unwrap().starts_with("sha256:"));
    assert_eq!((boot["frictions_eligible"].clone(), boot["proposals_open"].clone()), (json!(3), json!(1)));
    assert!(boot["text"].as_str().unwrap().contains(
        "\n- evolution: 3 eligible frictions (top path.home_relative ×6); 1 proposal screened\n"
    ));
    // Heartbeat: the first write follows ready at once.
    let hb = loop {
        if let Some(h) = home.heartbeat() {
            break h;
        }
        assert!(t_ready.elapsed() < Duration::from_millis(500), "no heartbeat");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(hb["boot_id"], bid);
    assert_eq!(hb["busy"], false);
    assert_eq!(hb["session"], SID);
    // Every 5 s ± 0.5 s.
    let mut stamps = vec![(Instant::now(), hb["ts"].clone())];
    while stamps.len() < 3 {
        if let Some(h) = home.heartbeat()
            && h["ts"] != stamps.last().unwrap().1
        {
            stamps.push((Instant::now(), h["ts"].clone()));
        }
        assert!(t_ready.elapsed() < Duration::from_secs(14), "heartbeat too slow");
        std::thread::sleep(Duration::from_millis(20));
    }
    for w in stamps.windows(2) {
        let d = w[1].0 - w[0].0;
        assert!(
            d >= Duration::from_millis(4_400) && d <= Duration::from_millis(5_600),
            "heartbeat interval {d:?}"
        );
    }
    // Pause while idle: paused at once, heartbeat goes on with phase paused.
    body.send(json!({"t": "pause", "token": "5f0d22aa91c3e7b4", "reason": "handoff"}));
    let p = body.wait_frame("paused", Duration::from_secs(5), |f| f["t"] == "paused");
    assert_eq!(p["token"], "5f0d22aa91c3e7b4");
    let len_paused = std::fs::metadata(home.path.join(format!("state/sessions/{SID}.jsonl")))
        .unwrap()
        .len();
    assert_eq!(p["log_len"], len_paused);
    let t = Instant::now();
    loop {
        if let Some(h) = home.heartbeat()
            && h["phase"] == "paused"
        {
            assert_eq!(h["busy"], false);
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(7), "no paused heartbeat");
        std::thread::sleep(Duration::from_millis(50));
    }
    // D11: stop while paused exits 0 at once and writes nothing more.
    stop(&mut body);
    let len_after = std::fs::metadata(home.path.join(format!("state/sessions/{SID}.jsonl")))
        .unwrap()
        .len();
    assert_eq!(len_after, len_paused);
    // No kept extensions: the boot-queued loads (Boot, SessionStart) changed nothing, so 10 s of
    // idle boundaries logged no `[init:reload] … (no changes)` event.
    let log = home.log();
    assert!(!log.iter().any(|e| e["type"] == "reload"), "{log:?}");
}

/// §8 item 3: a second body on the same session exits 1 (lock wait shortened by the test
/// override); after the first pauses (unlocks), the next one acquires the lock and boots.
#[test]
fn lock_second_writer() {
    let home = Home::new("lk");
    home.boot_json("1000000000000001", "normal", "cold", json!({}));
    let Some(_mock) = Mock::start(&home, "lock_second_writer", "", &[]) else {
        return;
    };
    let mut a = Body::spawn(&home, "normal", "cold", "1000000000000001", &[]);
    home.wait_ready("1000000000000001", &mut a, Duration::from_secs(20));
    home.boot_json("1000000000000002", "normal", "crash", json!({}));
    let t = Instant::now();
    let mut b = Body::spawn(
        &home,
        "normal",
        "crash",
        "1000000000000002",
        &[("INIT_TEST_LOCK_WAIT_MS", "400")],
    );
    assert_eq!(b.wait_exit(Duration::from_secs(10)), Some(1));
    assert!(t.elapsed() >= Duration::from_millis(400));
    let err = b.stderr_complete(Duration::from_secs(5));
    assert!(
        err.contains(&format!("log: session {SID} locked by another writer")),
        "{err}"
    );
    // A pauses (sync + unlock); C boots on the same session.
    a.send(json!({"t": "pause", "token": "t1", "reason": "handoff"}));
    a.wait_frame("paused", Duration::from_secs(5), |f| f["t"] == "paused");
    home.boot_json("1000000000000003", "normal", "promote", json!({}));
    let mut c = Body::spawn(
        &home,
        "normal",
        "promote",
        "1000000000000003",
        &[("INIT_TEST_LOCK_WAIT_MS", "2000")],
    );
    home.wait_ready("1000000000000003", &mut c, Duration::from_secs(20));
    a.send(json!({"t": "exit", "token": "t1", "code": 75}));
    assert_eq!(a.wait_exit(Duration::from_secs(10)), Some(75));
    stop(&mut c);
    assert_eq!(boot_events(&home.log()).len(), 2, "B's boot event is in no log");
}

// ---------------------------------------------------------------------------------------------
// probe mode (§3.9, §8 item 7)
// ---------------------------------------------------------------------------------------------

/// Boots a normal body once (so the session has history and a live head hash), stops it, and
/// returns the live `ready`.
fn live_boot_then_stop(home: &Home) -> Value {
    home.boot_json("aaaaaaaaaaaaaaaa", "normal", "cold", json!({}));
    let mut live = Body::spawn(home, "normal", "cold", "aaaaaaaaaaaaaaaa", &[]);
    let r = home.wait_ready("aaaaaaaaaaaaaaaa", &mut live, Duration::from_secs(20));
    stop(&mut live);
    r
}

/// 7(a) and 7(f): `bash true`, then `ready`; two requests with `x-init-purpose: probe`, the real
/// tools and effort, the whole projected conversation ending with the `[init:probe]` item; the
/// head hash equals the live body's (gate criterion 5). Strict DeepSeek wire checks are on.
///
/// Scripted rather than the mock's built-in probe responder: that responder answers `ready` only
/// when the last message is a tool result, but on the DeepSeek layout the self-signal note (a user
/// message, log-context §3.13) follows the results, so it calls `bash true` again and the probe
/// takes 3 requests (still ok). Reported to the testkit owner in build/status/WP13.md.
#[test]
fn probe_round_trip() {
    let home = Home::new("pa");
    let script = [
        r#"{"v":1,"kind":"turn","purpose":"probe","expect":{"last_text_contains":"[init:probe] Gate probe","tools":["bash","edit","ext","read","write"]},"reply":{"calls":[{"name":"bash","input":{"command":"true"}}],"stop":"tool_use"}}"#,
        r#"{"v":1,"kind":"turn","purpose":"probe","expect":{"prefix":"same"},"reply":{"text":"ready","stop":"end_turn"}}"#,
    ]
    .join("\n");
    let Some(mock) = Mock::start(
        &home,
        "probe_round_trip",
        &script,
        &["--strict", "deepseek", "--no-probe-responder"],
    ) else {
        return;
    };
    let live = live_boot_then_stop(&home);
    let bid = "bbbbbbbbbbbbbbbb";
    home.boot_json(bid, "probe", "promote", json!({"from_sha": SHA}));
    let mut body = Body::spawn(&home, "probe", "promote", bid, &[]);
    let r = home.wait_ready(bid, &mut body, Duration::from_secs(30));
    let p = &r["probe"];
    assert_eq!(p["ok"], true, "{r}");
    assert_eq!(p["requests"], 2);
    assert_eq!(p["tool_round_trip"], true);
    assert_eq!(p["model_called_tool"], true);
    assert_eq!(r["mode"], "probe");
    assert_eq!(p["head_hash"], r["prefix_hash"]);
    assert_eq!(r["prefix_hash"], live["prefix_hash"], "probe head == live head");
    let log = mock.log();
    assert_eq!(log.len(), 2, "{log:?}");
    for l in &log {
        assert_eq!(l["purpose"], "probe");
        assert_eq!(l["expect_ok"], true, "{l}");
        assert_eq!(l["session"], SID);
        assert_eq!(l["status"], 200);
        assert!(l["protocol_error"].is_null(), "{l}");
    }
    let bodies = mock.bodies();
    let b1 = &bodies[0];
    let names: Vec<&str> = b1["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["bash", "edit", "ext", "read", "write"]);
    assert_eq!(b1["reasoning_effort"], "high");
    let msgs = b1["messages"].as_array().unwrap();
    assert_eq!(msgs[0]["role"], "system");
    assert!(
        msgs.iter().any(|m| m["content"].as_str().is_some_and(|c| c.contains("[init:boot] "))),
        "the boot blocks are in the projected conversation"
    );
    assert!(last_message_text(b1).contains(
        "[init:probe] Gate probe: call the bash tool once with the command \"true\", then reply with the single word ready."
    ));
    let b2 = &bodies[1];
    let last = b2["messages"].as_array().unwrap().iter().rev().find(|m| m["role"] == "tool").unwrap();
    let call_id = b2["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|m| m["role"] == "assistant")
        .unwrap()["tool_calls"][0]["id"]
        .clone();
    assert_eq!(last["tool_call_id"], call_id, "the result answers the emitted call id");
    // The exchange is in the (staged) log after the probe's boot event.
    let ev = home.log();
    let probe_boot = ev.iter().position(|e| e["type"] == "boot" && e["boot_id"] == bid).unwrap();
    assert!(ev[probe_boot..].iter().any(|e| e["type"] == "tool_result"));
    assert!(ev[probe_boot..].iter().any(|e| e["type"] == "tool_call"));
    // After ready the probe body serves; no auto-continue; stop exits 0.
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(mock.log().len(), 2);
    stop(&mut body);
}

/// 7(b): a text-only reply, then a text-only reply to the nudge: `ready` with
/// `tool_round_trip:false` (the gate reports probe_no_round_trip). Two requests (deviation: no
/// results remain to send, so a third request would end on an assistant turn).
#[test]
fn probe_no_tool_call() {
    let home = Home::new("pb");
    let script = [
        r#"{"v":1,"kind":"turn","purpose":"probe","expect":{"last_text_contains":"[init:probe] Gate probe"},"reply":{"text":"hello","stop":"end_turn"}}"#,
        r#"{"v":1,"kind":"turn","purpose":"probe","expect":{"last_text_contains":"[init:probe] Call the bash tool"},"reply":{"text":"still no","stop":"end_turn"}}"#,
    ]
    .join("\n");
    let Some(mock) = Mock::start(&home, "probe_no_tool_call", &script, &["--no-probe-responder"]) else {
        return;
    };
    let bid = "cccccccccccccccc";
    home.boot_json(bid, "probe", "promote", json!({}));
    let mut body = Body::spawn(&home, "probe", "promote", bid, &[]);
    let r = home.wait_ready(bid, &mut body, Duration::from_secs(30));
    let p = &r["probe"];
    assert_eq!(p["ok"], true);
    assert_eq!(p["tool_round_trip"], false);
    assert_eq!(p["model_called_tool"], false);
    assert_eq!(p["requests"], 2);
    let log = mock.log();
    assert_eq!(log.len(), 2);
    assert!(log.iter().all(|l| l["expect_ok"] == true), "{log:?}");
    stop(&mut body);
}

/// 7(c): `bash rm -rf x` is not executed; it gets `error: probe_policy: …`; the round trip counts.
#[test]
fn probe_policy() {
    let home = Home::new("pc");
    std::fs::create_dir_all(home.path.join("work/x")).unwrap();
    std::fs::write(home.path.join("work/x/keep"), "keep").unwrap();
    let script = [
        r#"{"v":1,"kind":"turn","purpose":"probe","reply":{"calls":[{"name":"bash","input":{"command":"rm -rf x"}}],"stop":"tool_use"}}"#,
        r#"{"v":1,"kind":"turn","purpose":"probe","reply":{"text":"ready","stop":"end_turn"}}"#,
    ]
    .join("\n");
    let Some(mock) = Mock::start(&home, "probe_policy", &script, &["--no-probe-responder"]) else {
        return;
    };
    let bid = "dddddddddddddddd";
    home.boot_json(bid, "probe", "promote", json!({}));
    let mut body = Body::spawn(&home, "probe", "promote", bid, &[]);
    let r = home.wait_ready(bid, &mut body, Duration::from_secs(30));
    assert_eq!(r["probe"]["ok"], true);
    assert_eq!(r["probe"]["tool_round_trip"], true);
    assert_eq!(r["probe"]["model_called_tool"], true);
    assert!(home.path.join("work/x/keep").exists(), "rm -rf x must not run");
    let b2 = &mock.bodies()[1];
    let tool = b2["messages"].as_array().unwrap().iter().rev().find(|m| m["role"] == "tool").unwrap();
    assert!(
        tool["content"].as_str().unwrap().contains("error: probe_policy: not executed in probe mode"),
        "{tool}"
    );
    stop(&mut body);
}

/// 7(d): the mock returns 500 forever: no `ready`, exit 70 with a probe boot_error. And the
/// watchdog: a stalled stream is aborted at the (shortened) watchdog, exit 70.
#[test]
fn probe_failures_exit_70() {
    let home = Home::new("pd");
    let Some(mock) = Mock::start(&home, "probe_failures_exit_70", "", &["--no-probe-responder"]) else {
        return;
    };
    let bid = "eeeeeeeeeeeeeeee";
    home.boot_json(bid, "probe", "promote", json!({}));
    let t = Instant::now();
    let mut body = Body::spawn(&home, "probe", "promote", bid, &[]);
    expect_boot_error(&mut body, 70, "probe", "request 1 failed");
    assert!(t.elapsed() < Duration::from_secs(61));
    assert!(home.ready().is_none(), "no ready after a failed probe");
    assert!(mock.log().len() >= 2, "retried: {:?}", mock.log());
    drop(mock);

    let home = Home::new("pw");
    let script = r#"{"v":1,"kind":"turn","purpose":"probe","fault":{"kind":"stall","after_events":1,"ms":20000},"reply":{"text":"slow","stop":"end_turn"}}"#;
    let Some(_mock) = Mock::start(&home, "probe_watchdog", script, &["--no-probe-responder"]) else {
        return;
    };
    home.boot_json(bid, "probe", "promote", json!({}));
    let t = Instant::now();
    let mut body = Body::spawn(&home, "probe", "promote", bid, &[("INIT_TEST_PROBE_WATCHDOG_MS", "800")]);
    expect_boot_error(&mut body, 70, "probe", "watchdog");
    assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
    assert!(home.ready().is_none());
}

/// 7(e) and item 12's panic line: a candidate that panics at boot exits 101 with no `ready`, and
/// stderr carries one `{"type":"panic",…}` line.
#[test]
fn panic_at_boot_exits_101() {
    let home = Home::new("pp");
    let bid = "ffffffffffffffff";
    home.boot_json(bid, "normal", "promote", json!({}));
    let mut body = Body::spawn(
        &home,
        "normal",
        "promote",
        bid,
        &[("INIT_FAULT", "boot.before_ready:panic")],
    );
    assert_eq!(body.wait_exit(Duration::from_secs(20)), Some(101));
    assert!(home.ready().is_none());
    let err = body.stderr_complete(Duration::from_secs(5));
    let line: Value = err
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v["type"] == "panic")
        .unwrap_or_else(|| panic!("{err}"));
    assert_eq!(line["thread"], "main");
    assert!(line["message"].as_str().unwrap().contains("INIT_FAULT"));
    assert!(line["location"].as_str().unwrap().contains("src/boot/mod.rs:"));
}

// ---------------------------------------------------------------------------------------------
// crash repair, safe mode, handoff (§8 items 4, 8, 9, 10)
// ---------------------------------------------------------------------------------------------

/// Runs a normal body, sends one owner input whose model request stalls, waits until the owner
/// message is logged and the request reached the mock, then SIGKILLs the body: the leaf is the
/// owner message (pending work).
fn crash_with_pending_owner_message(home: &Home, mock: &Mock) {
    home.boot_json("0000000000000a01", "normal", "cold", json!({}));
    let mut a = Body::spawn(home, "normal", "cold", "0000000000000a01", &[]);
    home.wait_ready("0000000000000a01", &mut a, Duration::from_secs(20));
    a.send(json!({"t": "input", "id": "9c41e07a2b5d13f0", "seq": 1, "text": "Say hi.", "ts": "2026-10-20T04:00:00.000Z"}));
    a.wait_frame("ack", Duration::from_secs(10), |f| f["t"] == "ack");
    let t = Instant::now();
    while mock.log().is_empty() && mock.bodies().is_empty() {
        assert!(t.elapsed() < Duration::from_secs(10), "no request reached the mock");
        std::thread::sleep(Duration::from_millis(20));
    }
    a.kill9();
}

/// §8 item 8 (A5 body side): safe mode reads T0 from `lkg_tree` (a broken `body/SELF.md` does
/// not matter), has no extension host, does not continue pending work, refuses reload.
#[test]
fn safe_mode() {
    let home = Home::new("sm");
    let script = r#"{"v":1,"kind":"turn","fault":{"kind":"stall","after_events":0,"ms":60000},"reply":{"text":"never","stop":"end_turn"}}"#;
    let Some(mock) = Mock::start(&home, "safe_mode", script, &[]) else {
        return;
    };
    crash_with_pending_owner_message(&home, &mock);
    let requests_before = mock.bodies().len();
    // LKG tree with good T0 files; body/SELF.md broken (not UTF-8).
    let lkg = home.path.join("lineage/trees/3f2a1c0d9e8b");
    std::fs::create_dir_all(&lkg).unwrap();
    std::fs::write(lkg.join("GENESIS.md"), crate_file("GENESIS.md")).unwrap();
    std::fs::write(lkg.join("SELF.md"), crate_file("SELF.md")).unwrap();
    std::fs::write(home.path.join("body/SELF.md"), [0xff, 0xfe, 0x00]).unwrap();
    let bid = "5afe5afe5afe5afe";
    home.boot_json(bid, "safe", "crash", json!({"safe": {"since": "2026-10-20T04:00:00.000Z"}}));
    let mut body = Body::spawn(&home, "safe", "crash", bid, &[]);
    let r = home.wait_ready(bid, &mut body, Duration::from_secs(20));
    assert_eq!(r["mode"], "safe");
    let log = home.log();
    let boot = boot_events(&log).into_iter().last().unwrap();
    assert_eq!(boot["mode"], "safe");
    assert_eq!(boot["facts"]["mode"], "safe");
    assert_eq!(boot["facts"]["ext"], json!({"status": "disabled"}));
    // No auto-continue: no new model request.
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(mock.bodies().len(), requests_before);
    // reload is refused in safe mode.
    body.send(json!({"t": "reload", "job": "j1", "kind": "request"}));
    let rd = body.wait_frame("reload_done", Duration::from_secs(10), |f| f["t"] == "reload_done");
    assert_eq!(rd["ok"], false, "{rd}");
    stop(&mut body);
}

/// §8 item 4 through boot, plus normal-mode auto-continue: a crash with an owner message as the
/// leaf is pending; the next normal boot appends its boot event and continues the run at once.
#[test]
fn crash_restart_continues_pending_run() {
    let home = Home::new("cr");
    let script = [
        r#"{"v":1,"kind":"turn","fault":{"kind":"stall","after_events":0,"ms":60000},"reply":{"text":"never","stop":"end_turn"}}"#,
        r#"{"v":1,"kind":"turn","expect":{"last_role":"user"},"reply":{"text":"hi after restart","stop":"end_turn"}}"#,
    ]
    .join("\n");
    let Some(mock) = Mock::start(&home, "crash_restart", &script, &[]) else {
        return;
    };
    crash_with_pending_owner_message(&home, &mock);
    let bid = "c4a5c4a5c4a5c4a5";
    home.boot_json(bid, "normal", "crash", json!({"crash": {"exit_code": null, "signal": "SIGKILL", "log_tail": ""}, "restarts_since_lkg": 1}));
    let mut body = Body::spawn(&home, "normal", "crash", bid, &[]);
    home.wait_ready(bid, &mut body, Duration::from_secs(20));
    body.wait_frame("turn_end", Duration::from_secs(20), |f| f["kind"] == "turn_end");
    let log = home.log();
    let owner = log.iter().position(|e| e["type"] == "message" && e["source"] == "owner").unwrap();
    let boot2 = log.iter().rposition(|e| e["type"] == "boot").unwrap();
    assert!(boot2 > owner);
    assert_eq!(log[boot2]["reason"], "crash");
    assert_eq!(log[boot2]["crash"]["signal"], "SIGKILL");
    let reply = log.iter().rposition(|e| e["type"] == "message" && e["role"] == "assistant").unwrap();
    assert!(reply > boot2);
    assert_eq!(mock.log().last().unwrap()["expect_ok"], true);
    stop(&mut body);
}

/// §8 items 9 and 10 (A3/A4 body side), with the kernel's side scripted here: A pauses after its
/// tool batch; B boots on the same session (its boot event follows A's last event), A exits 75 on
/// `exit`, B continues the pending run. Then B pauses, a C fails before ready (exit 70), and B
/// resumes with `update_failed` (branch-back to its leaf) and continues.
#[test]
fn handoff_overlap_and_failed_handoff() {
    let home = Home::new("ho");
    let script = [
        r#"{"v":1,"kind":"turn","reply":{"calls":[{"name":"bash","input":{"command":"sleep 0.6; echo done"}}],"stop":"tool_use"}}"#,
        r#"{"v":1,"kind":"turn","reply":{"calls":[{"name":"bash","input":{"command":"sleep 0.6; echo again"}}],"stop":"tool_use"}}"#,
        r#"{"v":1,"kind":"turn","reply":{"text":"finished","stop":"end_turn"}}"#,
    ]
    .join("\n");
    let Some(mock) = Mock::start(&home, "handoff", &script, &[]) else {
        return;
    };
    home.boot_json("0000000000000a0a", "normal", "cold", json!({}));
    let mut a = Body::spawn(&home, "normal", "cold", "0000000000000a0a", &[]);
    home.wait_ready("0000000000000a0a", &mut a, Duration::from_secs(20));
    a.send(json!({"t": "input", "id": "1111222233334444", "seq": 7, "text": "Run the step.", "ts": "2026-10-20T04:00:00.000Z"}));
    a.wait_frame("tool_start", Duration::from_secs(10), |f| f["kind"] == "tool_start");
    a.send(json!({"t": "pause", "token": "aaaa000011112222", "reason": "handoff"}));
    let paused = a.wait_frame("paused", Duration::from_secs(10), |f| f["t"] == "paused");
    let leaf_a = paused["leaf"].clone();
    // The batch result is in the log before paused.
    let log = home.log();
    assert_eq!(log.last().unwrap()["id"], leaf_a);
    assert_eq!(log.last().unwrap()["type"], "tool_result");
    // B (promote) boots while A is paused.
    home.boot_json("0000000000000b0b", "normal", "promote", json!({"from_sha": SHA, "branch": "evo/0007",
        "provisional": {"depth": 1, "g4": "queued"}}));
    let mut env_b = body_env(&home, "normal", "promote", "0000000000000b0b");
    env_b.retain(|(k, _)| k != "INIT_SHA");
    env_b.push(("INIT_SHA".into(), SHA_B.into()));
    let mut b = Body::spawn_env(env_b, &home, &[]);
    let rb = home.wait_ready("0000000000000b0b", &mut b, Duration::from_secs(20));
    let log = home.log();
    let boot_b = log.iter().find(|e| e["id"] == rb["boot_event_id"]).unwrap();
    assert_eq!(boot_b["parent_id"], leaf_a, "B's boot event follows A's last event");
    assert_eq!(boot_b["sha"], SHA_B);
    assert_eq!(boot_b["branch"], "evo/0007");
    assert_eq!(boot_b["provisional"], true);
    a.send(json!({"t": "exit", "token": "aaaa000011112222", "code": 75}));
    assert_eq!(a.wait_exit(Duration::from_secs(10)), Some(75));
    // B continues the pending run: request 2 carries A's tool result.
    b.wait_frame("tool_start", Duration::from_secs(20), |f| f["kind"] == "tool_start");
    let t = Instant::now();
    while mock.bodies().len() < 2 {
        assert!(t.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        mock.bodies()[1].to_string().contains("done"),
        "B's first request carries A's tool result"
    );
    // B pauses after this batch; a C would fail before ready (exit 70): B resumes with update_failed.
    b.send(json!({"t": "pause", "token": "bbbb000011112222", "reason": "handoff"}));
    let pb = b.wait_frame("paused", Duration::from_secs(10), |f| f["t"] == "paused");
    home.boot_json("0000000000000c0c", "normal", "promote", json!({}));
    let mut c = Body::spawn(&home, "normal", "promote", "0000000000000c0c", &[("INIT_FAULT", "boot.before_ready:exit70")]);
    assert_eq!(c.wait_exit(Duration::from_secs(20)), Some(70));
    let uf = json!({"at": "2026-10-20T04:01:00.000Z", "exit_code": 70, "from_sha": SHA_B, "reason": "no ready",
                    "seq": 12, "sha": SHA, "signal": null, "stage": "handoff", "v": 1});
    b.send(json!({"t": "resume", "token": "bbbb000011112222", "update_failed": uf}));
    b.wait_frame("turn_end after resume", Duration::from_secs(20), |f| {
        f["kind"] == "turn_end"
    });
    let log = home.log();
    let ufe = log.iter().find(|e| e["type"] == "update_failed").expect("update_failed event");
    assert_eq!(ufe["parent_id"], pb["leaf"], "branch-back to B's leaf");
    assert_eq!(ufe["seq"], 12);
    let t = Instant::now();
    while mock.bodies().len() < 3 {
        assert!(t.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        mock.bodies()[2].to_string().contains("[init:update-failed]"),
        "the model reads the failure at its next request (A4)"
    );
    stop(&mut b);
}

// ---------------------------------------------------------------------------------------------
// The extension host wired into the body (WP14 through boot; testing.md A2's core path)
// ---------------------------------------------------------------------------------------------

/// The seed's target dir, from this test binary (`<target>/debug/deps/boot-…`).
fn target_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.ancestors().nth(3).map(Path::to_path_buf))
        .unwrap_or_else(|| manifest_dir().join("target"))
}

/// The real `count-lines` component: `CARGO_TARGET_DIR=seed/target cargo build --offline
/// --target wasm32-wasip2` in `seed/ext` (ext/authoring.md §3.5), as the ext_host tests build it.
fn count_lines_wasm() -> PathBuf {
    static R: std::sync::OnceLock<Result<PathBuf, String>> = std::sync::OnceLock::new();
    R.get_or_init(|| {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let out = Command::new(cargo)
            .args(["build", "--offline", "--target", "wasm32-wasip2"])
            .current_dir(manifest_dir().join("ext"))
            .env("CARGO_TARGET_DIR", target_dir())
            .env_remove("RUSTFLAGS")
            .env_remove("CARGO_BUILD_TARGET")
            .output()
            .map_err(|e| format!("cargo: {e}"))?;
        if !out.status.success() {
            return Err(format!("count-lines build failed:\n{}", String::from_utf8_lossy(&out.stderr)));
        }
        let p = target_dir().join("wasm32-wasip2/debug/count_lines.wasm");
        p.is_file().then_some(p.clone()).ok_or(format!("{} missing after build", p.display()))
    })
    .clone()
    .unwrap_or_else(|e| panic!("{e}"))
}

/// The workspace file both tests count in: "fn " on lines 1, 3 and 4.
const NOTES: &str = "fn a() {}\nlet b = 1;\nfn c() {}\n// fn d\n";

/// `init-body ext describe|call|inspect` run the real host on the real component (host.md §4.5).
#[test]
fn ext_cli_on_count_lines() {
    let wasm = count_lines_wasm();
    let home = Home::new("ec");
    std::fs::write(home.path.join("work/notes.txt"), NOTES).unwrap();
    let run = |args: &[&str]| -> (Option<i32>, Value) {
        let out = Command::new(body_bin())
            .arg("ext")
            .args(args)
            .env_clear()
            .env("INIT_HOME", &home.path)
            .output()
            .unwrap();
        let v = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("{args:?}: {e}; stderr {}", String::from_utf8_lossy(&out.stderr)));
        (out.status.code(), v)
    };
    let w = wasm.to_str().unwrap();
    let (code, d) = run(&["describe", w]);
    assert_eq!(code, Some(0), "{d}");
    assert_eq!(d["api"], "init:ext@1");
    assert_eq!(d["info"]["name"], "count-lines");
    assert_eq!(d["info"]["input_schema"]["required"], json!(["path", "needle"]));
    let work = home.path.join("work");
    let (code, c) = run(&["call", w, "--input", r#"{"needle":"fn ","path":"notes.txt"}"#, "--workspace", work.to_str().unwrap()]);
    assert_eq!(code, Some(0), "{c}");
    assert_eq!((c["is_error"].clone(), c["kind"].clone()), (json!(false), json!("ok")), "{c}");
    assert_eq!(serde_json::from_str::<Value>(c["text"].as_str().unwrap()).unwrap(), json!({"count": 3, "lines": [1, 3, 4]}));
    assert!(home.path.join("state/ext/count-lines/data").is_dir(), "data dir from INIT_HOME");
    let (code, i) = run(&["inspect", w]);
    assert_eq!(code, Some(0), "{i}");
    assert_eq!(i["exports"], json!(["call", "describe"]), "world init:ext/tool exports");
    assert!(!i["imports"].to_string().contains("init:ext/process"), "count-lines imports no process host");
    let (code, e) = run(&["describe", home.path.join("missing.wasm").to_str().unwrap()]);
    assert_eq!(code, Some(1), "load error exits 1: {e}");
    assert!(e["error"].is_string());
}

/// testing.md A2's core path without the build step: the kernel published a kept set
/// (`bin/ext` → `bin/ext.d/<set>/count_lines.wasm`, supervise.md §3.6.5), the body's Boot load
/// compiles it while the first model request runs (host.md §3.9 rule 4), and a scripted turn calls
/// it through the `ext` tool. Checks: boot facts `ext.status = loading`; a T1 `reload` event with
/// `count-lines` and no prefix change; the `ext` result is the component's real output, not an
/// error; the prompt prefix and the tools array are unchanged across the turn; the host's per-call
/// log and catalog are written under `state/ext/`.
#[test]
fn ext_tool_calls_kept_count_lines() {
    let home = Home::new("ex");
    let script = [
        r#"{"v":1,"kind":"turn","expect":{"last_role":"user","tools":["bash","edit","ext","read","write"]},"reply":{"calls":[{"name":"ext","input":{"name":"count-lines","input":{"needle":"fn ","path":"a2/notes.txt"}}}],"stop":"tool_use"}}"#,
        r#"{"v":1,"kind":"turn","expect":{"prefix":"same","tools":["bash","edit","ext","read","write"]},"reply":{"text":"3 lines contain fn: 1, 3 and 4.","stop":"end_turn"}}"#,
    ]
    .join("\n");
    let Some(mock) = Mock::start(&home, "ext_tool_calls_kept_count_lines", &script, &[]) else {
        return;
    };
    let wasm = count_lines_wasm();
    let set = home.path.join("bin/ext.d/0001");
    std::fs::create_dir_all(&set).unwrap();
    std::fs::copy(&wasm, set.join("count_lines.wasm")).unwrap();
    std::os::unix::fs::symlink("ext.d/0001", home.path.join("bin/ext")).unwrap();
    // The source dir as the seed ships it (`body/ext/count-lines/`): a new session's reload
    // (host.md §3.8 `SessionStart`) lists names from the source dirs and loads their kept builds.
    let src = home.path.join("body/ext/count-lines");
    std::fs::create_dir_all(src.join("src")).unwrap();
    for f in ["Cargo.toml", "src/lib.rs"] {
        std::fs::copy(manifest_dir().join("ext/count-lines").join(f), src.join(f)).unwrap();
    }
    std::fs::create_dir_all(home.path.join("work/a2")).unwrap();
    std::fs::write(home.path.join("work/a2/notes.txt"), NOTES).unwrap();

    let bid = "e0e0e0e0e0e0e0e0";
    home.boot_json(bid, "normal", "cold", json!({}));
    let mut body = Body::spawn(&home, "normal", "cold", bid, &[]);
    home.wait_ready(bid, &mut body, Duration::from_secs(30));
    let log = home.log();
    let boot = boot_events(&log).into_iter().last().unwrap();
    assert_eq!(boot["facts"]["ext"], json!({"status": "loading"}), "{}", boot["facts"]);

    body.send(json!({"t": "input", "id": "a2a2a2a2a2a2a2a2", "seq": 1, "text": "How many lines of work/a2/notes.txt contain \"fn \"? Use the count-lines extension.", "ts": "2026-10-20T04:00:00.000Z"}));
    // Two model turns (the ext call, then the answer); the run ends with state{busy:false}.
    body.wait_frame("busy", Duration::from_secs(20), |f| f["t"] == "state" && f["busy"] == true);
    body.wait_frame("idle", Duration::from_secs(60), |f| f["t"] == "state" && f["busy"] == false);

    let log = home.log();
    let pos = |pred: &dyn Fn(&Value) -> bool| log.iter().position(pred);
    let call = pos(&|e| e["type"] == "tool_call" && e["name"] == "ext").expect("ext tool_call");
    let result = pos(&|e| e["type"] == "tool_result" && e["name"] == "ext").expect("ext tool_result");
    let reload = pos(&|e| e["type"] == "reload" && e["tier"] == "T1").expect("T1 reload event");
    let r = &log[result];
    assert_eq!(r["is_error"], false, "{r}");
    let text = r["output"][0]["text"].as_str().unwrap_or_else(|| panic!("{r}"));
    assert_eq!(serde_json::from_str::<Value>(text).unwrap(), json!({"count": 3, "lines": [1, 3, 4]}), "{r}");
    assert!(call < result);
    let rl = &log[reload];
    assert_eq!(rl["ok"], true, "{rl}");
    assert!(rl["components"].as_array().unwrap().iter().any(|c| c == "count-lines"), "{rl}");
    assert_eq!(rl["prefix_changed"], false, "{rl}");
    assert_eq!(rl["head_hash_before"], rl["head_hash_after"], "{rl}");
    assert!(rl["text"].as_str().unwrap().contains("count-lines"), "{rl}");

    // The mock saw two turns, both expectations held (tools, prefix unchanged), and the second
    // request carries the component's output as the tool result.
    let ml = mock.log();
    assert_eq!(ml.len(), 2, "{ml:?}");
    assert!(ml.iter().all(|l| l["expect_ok"] == true), "{ml:?}");
    let bodies = mock.bodies();
    let tool_msg = bodies[1]["messages"].as_array().unwrap().iter().rev().find(|m| m["role"] == "tool").unwrap();
    assert!(tool_msg["content"].to_string().contains(r#"\"count\":3"#), "{tool_msg}");
    let reply = log.iter().rposition(|e| e["type"] == "message" && e["role"] == "assistant").unwrap();
    assert!(reply > result);

    // The host's own records (host.md §3.12 per-call log, §4.4 catalog).
    let call_log = std::fs::read_to_string(home.path.join("state/ext/count-lines/log")).expect("per-call log");
    assert_eq!(call_log.lines().count(), 1, "{call_log}");
    let catalog: Value = serde_json::from_slice(&std::fs::read(home.path.join("state/ext/catalog.json")).expect("catalog")).unwrap();
    assert!(catalog.to_string().contains("count-lines"), "{catalog}");
    stop(&mut body);
}

// ---------------------------------------------------------------------------------------------
// ADR-009 (WIRE-009): the workspace and the access grant
// ---------------------------------------------------------------------------------------------

/// The `- access:` line of a boot event's text, checked to sit right after the ceilings line.
fn access_line(boot: &Value) -> String {
    let text = boot["text"].as_str().unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let i = lines
        .iter()
        .position(|l| l.starts_with("- ceilings (Root): "))
        .unwrap_or_else(|| panic!("no ceilings line: {text}"));
    assert!(lines[i + 1].starts_with("- access: "), "{text}");
    lines[i + 1].to_string()
}

/// The Root sets `INIT_WORKSPACE` and `INIT_ACCESS` (SBX-009 §D): the boot event names the grant,
/// the boot facts' cwd is the workspace while the process cwd stays `INIT_HOME/work`, and a
/// mock-scripted relative `write` and `pwd` land in the workspace. Then the owner's workspace
/// changes: a `handoff` boot with cause `workspace_change` continues the session in the new one.
#[test]
fn workspace_access_boot_and_tools() {
    let home = Home::new("ws");
    let ws = home.path.join("work/home/projects/app");
    let ws2 = home.path.join("work/home/projects/other");
    for d in [&ws, &ws2] {
        std::fs::create_dir_all(d).unwrap();
    }
    let (ws_s, ws2_s) = (ws.to_str().unwrap(), ws2.to_str().unwrap());
    let script = [
        r#"{"v":1,"kind":"turn","expect":{"last_role":"user"},"reply":{"calls":[{"name":"write","input":{"path":"hello.txt","content":"hi\n"}},{"name":"bash","input":{"command":"pwd"}}],"stop":"tool_use"}}"#,
        r#"{"v":1,"kind":"turn","reply":{"text":"wrote hello.txt","stop":"end_turn"}}"#,
    ]
    .join("\n");
    let Some(_mock) = Mock::start(&home, "workspace_access_boot_and_tools", &script, &[]) else {
        return;
    };
    let bid = "5a5a5a5a5a5a5a5a";
    home.boot_json(bid, "normal", "cold", json!({}));
    let grant = [("INIT_WORKSPACE", ws_s), ("INIT_ACCESS", "workspace")];
    let mut body = Body::spawn(&home, "normal", "cold", bid, &grant);
    home.wait_ready(bid, &mut body, Duration::from_secs(20));
    let log = home.log();
    let boot = boot_events(&log)[0];
    assert_eq!(boot["facts"]["cwd"], ws_s, "{}", boot["facts"]);
    let warns = boot["facts"]["warnings"].to_string();
    assert!(!warns.contains("INIT_WORKSPACE") && !warns.contains("INIT_ACCESS"), "{warns}");
    assert_eq!(
        access_line(boot),
        format!("- access: write the workspace {ws_s} and INIT_HOME (your tools start in the workspace) · read the owner's home, secret stores denied")
    );
    assert!(boot["text"].as_str().unwrap().contains(&format!("· cwd: {ws_s} · bash: ")));

    body.send(json!({"t": "input", "id": "5a5a000000000001", "seq": 1, "text": "Write hello.txt.", "ts": "2026-10-20T04:00:00.000Z"}));
    body.wait_frame("busy", Duration::from_secs(20), |f| f["t"] == "state" && f["busy"] == true);
    body.wait_frame("idle", Duration::from_secs(60), |f| f["t"] == "state" && f["busy"] == false);
    assert_eq!(std::fs::read_to_string(ws.join("hello.txt")).unwrap(), "hi\n");
    assert!(!home.path.join("work/hello.txt").exists());
    let log = home.log();
    let pwd = log
        .iter()
        .find(|e| e["type"] == "tool_result" && e["name"] == "bash")
        .expect("bash tool_result");
    let out = pwd["output"][0]["text"].as_str().unwrap_or_else(|| panic!("{pwd}"));
    assert!(out.starts_with(&format!("{ws_s}\n")), "{out}");
    stop(&mut body);

    // The workspace changed: same sha, same session, reason handoff.
    let bid2 = "5a5a5a5a5a5a5a5b";
    home.boot_json(bid2, "normal", "handoff", json!({"cause": {"kind": "workspace_change"}}));
    let grant2 = [("INIT_WORKSPACE", ws2_s), ("INIT_ACCESS", "workspace")];
    let mut body = Body::spawn(&home, "normal", "handoff", bid2, &grant2);
    home.wait_ready(bid2, &mut body, Duration::from_secs(20));
    let log = home.log();
    let boots = boot_events(&log);
    assert_eq!(boots.len(), 2);
    let b2 = boots[1];
    assert_eq!(b2["facts"]["cwd"], ws2_s);
    let text = b2["text"].as_str().unwrap();
    assert!(
        text.starts_with("[init:boot] The kernel restarted you without a body change (your owner's workspace changed). The conversation continues."),
        "{text}"
    );
    assert!(access_line(b2).contains(&format!("write the workspace {ws2_s} and INIT_HOME")));
    let lacks_cause = |e: &Value| {
        e["type"] == "error"
            && e["code"] == "boot_json_field"
            && e["data"]["missing"].as_array().is_some_and(|m| m.iter().any(|x| x == "cause"))
    };
    assert!(!log.iter().any(lacks_cause), "{log:?}");
    stop(&mut body);

    // No workspace (`INIT_WORKSPACE=""`) and `none`, as the Root sets for a probe or a fleet
    // member: the none line, tools in the process cwd.
    let bid3 = "5a5a5a5a5a5a5a5c";
    home.boot_json(bid3, "normal", "cold", json!({}));
    let mut body = Body::spawn(&home, "normal", "cold", bid3, &[("INIT_WORKSPACE", ""), ("INIT_ACCESS", "none")]);
    home.wait_ready(bid3, &mut body, Duration::from_secs(20));
    let log = home.log();
    let b3 = *boot_events(&log).last().unwrap();
    assert_eq!(b3["facts"]["cwd"], home.path.join("work").to_str().unwrap());
    assert_eq!(access_line(b3), "- access: INIT_HOME only (no home access; the v0.1 sandbox)");
    stop(&mut body);
}

/// ADR-011 RF-18 (lane X4): mode `check` (the Root's A4 rollback probe) boots, rehydrates the session named by
/// `INIT_SESSION` (the second boot finds the first one's boot event and appends its own), writes `ready` with mode
/// `check` and exits 0 without serving, loading no extension and making no model call: nothing ever connects to
/// the proxy socket, a listener this test holds, and no kernel socket exists.
#[test]
fn check_mode_makes_no_model_call() {
    let home = Home::new("check");
    let proxy = std::os::unix::net::UnixListener::bind(home.sock()).unwrap();
    proxy.set_nonblocking(true).unwrap();
    let mut hashes = Vec::new();
    for (i, bid) in ["c0c0c0c0c0c0c0c0", "c1c1c1c1c1c1c1c1"].into_iter().enumerate() {
        home.boot_json(bid, "check", "cold", json!({}));
        let mut b = Body::spawn(&home, "check", "cold", bid, &[]);
        let code = b.wait_exit(Duration::from_secs(20));
        assert_eq!(code, Some(0), "{}", b.stderr_complete(Duration::from_secs(2)));
        let r = home.ready().expect("ready");
        assert_eq!((r["mode"].as_str(), r["boot_id"].as_str(), r["session"].as_str(), r["probe"].clone()),
                   (Some("check"), Some(bid), Some(SID), Value::Null));
        let log = home.log();
        let boots = boot_events(&log);
        assert_eq!(boots.len(), i + 1, "the session is rehydrated, not replaced");
        let boot = boots[i];
        assert_eq!((boot["id"].clone(), boot["facts"]["mode"].as_str()), (r["boot_event_id"].clone(), Some("check")));
        assert_eq!(boot["facts"]["ext"], json!({"status": "disabled"}));
        hashes.push(r["prefix_hash"].clone());
    }
    assert_eq!(hashes[0], hashes[1], "the same head");
    assert!(home.heartbeat().is_none(), "mode check never serves, so it writes no heartbeat");
    assert!(
        matches!(proxy.accept(), Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "mode check made no model call"
    );
    assert!(!home.kernel_sock.exists());
}

/// ADR-011 X11, T-AW: the free path's `- extend ·` line comes from the Root's published views
/// (`root/{charter,extend,caps}.json`) and sits in the boot block at the tail, so the head's
/// `prefix_hash` is the same whatever state the line shows: gated (no line), free with a group open,
/// and quarantine debt. No mock: a normal boot makes no model call.
#[test]
fn x11_extend_line_keeps_the_head() {
    let home = Home::new("x11");
    let root = home.path.join("root");
    std::fs::create_dir_all(&root).unwrap();
    let w = |name: &str, v: Value| std::fs::write(root.join(name), v.to_string()).unwrap();
    let caps = |extra: Vec<Value>| {
        let mut rows: Vec<Value> = (0..3)
            .map(|i| json!({"evidence": "root", "id": format!("ext/t{i}"), "registered": true, "state": "active"}))
            .collect();
        rows.extend(extra);
        json!({"caps": rows, "day": "2026-10-20", "v": 1})
    };
    let group = json!({"caps": ["ext/t2"], "changes": 1, "kernel": false, "last_join": "2026-10-20T03:00:00Z",
        "opened_at": "2026-10-20T03:00:00Z", "parent_tuple": "t-1", "seq": 2, "snapshot": null, "tuple": "t-2", "turns": 7});
    let states: [(&str, Option<&str>); 3] = [
        ("b0b0b0b0b0b0b0b0", None),
        ("b1b1b1b1b1b1b1b1", Some("- extend · groups 1/4 · window 7/20 · caps 3/64 · L0 at cap")),
        ("b2b2b2b2b2b2b2b2", Some("- extend · groups 0/4 · window none · caps 4/64 · L0 at cap · QUARANTINED ext/bad: move to quarantine/ or remove; every other change is refused until then")),
    ];
    let mut hashes = Vec::new();
    for (i, (bid, want)) in states.into_iter().enumerate() {
        match i {
            0 => w("charter.json", json!({"autonomy": {"max_level": 4}, "supported_max_level": 0})),
            1 => {
                w("charter.json", json!({"autonomy": {"max_level": 4}, "extend": {"mode": "free"}, "supported_max_level": 0}));
                w("extend.json", json!({"freeze_until": null, "groups": [group.clone()], "rollback": null, "v": 1}));
                w("caps.json", caps(vec![]));
            }
            _ => {
                w("extend.json", json!({"freeze_until": null, "groups": [], "rollback": null, "v": 1}));
                w("caps.json", caps(vec![json!({"id": "ext/bad", "registered": true, "state": "quarantined"})]));
            }
        }
        home.boot_json(bid, "normal", "cold", json!({}));
        let mut body = Body::spawn(&home, "normal", "cold", bid, &[]);
        let r = home.wait_ready(bid, &mut body, Duration::from_secs(20));
        stop(&mut body);
        let log = home.log();
        let boot = *boot_events(&log).last().unwrap();
        let text = boot["text"].as_str().unwrap();
        let line = text.lines().find(|l| l.starts_with("- extend "));
        assert_eq!(line, want, "{text}");
        assert_eq!(boot["prefix_hash"], r["prefix_hash"]);
        hashes.push(r["prefix_hash"].clone());
    }
    assert!(hashes[0].as_str().unwrap().starts_with("sha256:"));
    assert!(hashes.iter().all(|h| *h == hashes[0]), "the head's prefix_hash is unchanged: {hashes:?}");
}
