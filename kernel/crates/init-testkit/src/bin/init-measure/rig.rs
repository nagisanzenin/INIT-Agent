//! The socket-mode rig for M-4 and M-5 (testing.md §3.4.1 `socket` mode, §3.1a "live": body +
//! mock): a throwaway `INIT_HOME`, `init-mockllm --mode socket` on its proxy socket, and the real
//! `init-body` driven over stdio-jsonl/1 by this kernel stand-in. The shapes (boot.json, the
//! body environment, the frames) follow seed/tests/boot.rs, which the body's own suite uses.
//!
//! Every process here is this tool's own child and is stopped through its `Child` handle; every
//! home is under `target/test-homes/wp16/` and is removed by its exact path.

use crate::common;
use init_testkit::util;
use serde_json::{Value, json};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

pub const SID: &str = "20261020-060000-a1b2c3";
pub const LKG: &str = "3f2a1c0d9e8b3f2a1c0d9e8b3f2a1c0d9e8b3f2a";

pub struct Home {
    pub path: PathBuf,
    pub keep: bool,
}

impl Home {
    pub fn new(tag: &str, keep: bool) -> Result<Home, String> {
        let path = common::scratch_base().join(format!("{tag}-{}", std::process::id()));
        common::remove_exact(&path);
        let seed = common::repo().join("seed");
        let io = |e: std::io::Error| format!("{}: {e}", path.display());
        for d in ["body", "kernel", "state/kernel", "work/home"] {
            std::fs::create_dir_all(path.join(d)).map_err(io)?;
        }
        std::fs::copy(seed.join("GENESIS.md"), path.join("body/GENESIS.md")).map_err(io)?;
        std::fs::copy(seed.join("SELF.md"), path.join("body/SELF.md")).map_err(io)?;
        std::fs::copy(
            seed.join("src/boot/CONSTITUTION.md"),
            path.join("kernel/CONSTITUTION.md"),
        )
        .map_err(io)?;
        let h = Home { path, keep };
        if h.sock().as_os_str().len() > 103 {
            return Err(format!("socket path too long: {}", h.sock().display()));
        }
        Ok(h)
    }

    pub fn sock(&self) -> PathBuf {
        self.path.join("p.sock")
    }

    /// `state/kernel/boot.json` (supervise.md §4.3), as seed/tests/boot.rs writes it.
    pub fn boot_json(
        &self,
        boot_id: &str,
        reason: &str,
        sha: &str,
        extra: Value,
    ) -> Result<(), String> {
        let mut b = json!({
            "body_id": "b0", "boot_id": boot_id, "boot_seq": 1, "branch": null, "cause": null,
            "crash": null,
            "env": {
                "budget": {"cap_musd": 4000000, "day_starts_at": "2026-10-19T17:00:00Z", "today_musd": 0},
                "init_home": self.path.to_string_lossy(), "kernel_version": "0.1.0", "mem_total_mb": 16384,
                "model": {"context_window": 131072, "id": "mock-frontier-1", "max_output": 32768, "provider": "mock"},
                "network": {"egress": [], "mode": "proxy-only"},
                "os": {"arch": "arm64", "family": "macos", "version": "14.6"},
                "root": {"audited_good": LKG, "cap": 0, "ceilings": {"day_musd": 5000000, "parallel_bodies": 1,
                         "total_musd": 25000000, "total_used_musd": 0}, "classes": ["model-providers"],
                         "last_change": null, "level": 0, "max_level": 4, "next": null, "supported_max_level": 0},
                "sandbox": {"hidden": ["cache/", "probe/", "vault/"], "kind": "seatbelt", "landlock": null,
                            "landlock_abi": null, "network": "kernel-sockets-only",
                            "read_only": ["bin/", "kernel/", "lineage/", "state/kernel/"], "seccomp": false,
                            "sweep": "full", "write": ["body/", "state/", "work/"]}
            },
            "evolution": {}, "from_sha": null, "kernel": {"version": "0.1.0"}, "kernel_upgrade": null,
            "lkg_sha": LKG, "lkg_tree": "lineage/trees/3f2a1c0d9e8b", "mode": "normal",
            "model": {"effort": null, "model": "mock-frontier-1", "provider": "mock"},
            "panic": null, "pending": {"inputs": 0, "notices": 0}, "provisional": null, "reason": reason,
            "restarts_since_lkg": 0, "safe": null, "selfmap": {"body_tokens": 21480},
            "session": SID, "sha": sha, "spawned_at": util::ts(), "update_failed": null, "v": 1
        });
        if let (Value::Object(m), Value::Object(e)) = (&mut b, extra) {
            for (k, v) in e {
                m.insert(k, v);
            }
        }
        std::fs::write(
            self.path.join("state/kernel/boot.json"),
            serde_json::to_vec(&b).map_err(|e| e.to_string())?,
        )
        .map_err(|e| format!("boot.json: {e}"))
    }

    pub fn session_log(&self) -> Vec<Value> {
        util::read_jsonl(&self.path.join(format!("state/sessions/{SID}.jsonl")))
    }

    pub fn ready(&self) -> Option<Value> {
        let b = std::fs::read(self.path.join("state/run/ready")).ok()?;
        serde_json::from_slice(&b).ok()
    }

    /// Polls `state/run/ready` for this boot id every 2 ms.
    pub fn wait_ready(
        &self,
        boot_id: &str,
        body: &mut Body,
        limit: Duration,
    ) -> Result<Value, String> {
        let t0 = Instant::now();
        loop {
            if let Some(r) = self.ready()
                && r["boot_id"] == boot_id
            {
                return Ok(r);
            }
            if let Some(code) = body.try_exit() {
                return Err(format!(
                    "body exited {code:?} before ready; stderr:\n{}",
                    body.stderr_tail()
                ));
            }
            if t0.elapsed() > limit {
                return Err(format!(
                    "no ready within {limit:?}; stderr:\n{}",
                    body.stderr_tail()
                ));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        if !self.keep {
            common::remove_exact(&self.path);
        }
    }
}

pub struct Mock {
    child: Child,
    pub rec: PathBuf,
}

impl Mock {
    pub fn start(bin: &Path, home: &Home, script: &str, args: &[&str]) -> Result<Mock, String> {
        let script_path = home.path.join("s.mock.jsonl");
        std::fs::write(&script_path, script).map_err(|e| e.to_string())?;
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
            .stderr(File::create(home.path.join("mock.stderr")).map_err(|e| e.to_string())?)
            .spawn()
            .map_err(|e| format!("spawn {}: {e}", bin.display()))?;
        let mut m = Mock { child, rec };
        let t0 = Instant::now();
        while UnixStream::connect(home.sock()).is_err() {
            if let Ok(Some(st)) = m.child.try_wait() {
                return Err(format!("init-mockllm exited early: {st}"));
            }
            if t0.elapsed() > Duration::from_secs(10) {
                return Err("init-mockllm did not listen within 10 s".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(m)
    }

    pub fn log(&self) -> Vec<Value> {
        util::read_jsonl(&self.rec.join("mock.log.jsonl"))
    }

    /// Requests of one purpose recorded so far (a cheap line count for the M-5 sampler).
    pub fn count(&self, purpose: &str) -> usize {
        let needle = format!("\"purpose\":\"{purpose}\"");
        std::fs::read_to_string(self.rec.join("mock.log.jsonl"))
            .map(|t| t.lines().filter(|l| l.contains(&needle)).count())
            .unwrap_or(0)
    }

    /// Recorded request bodies (`--record-bodies all`), in sequence order.
    pub fn bodies(&self) -> Vec<Value> {
        let mut names: Vec<PathBuf> = std::fs::read_dir(&self.rec)
            .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
            .unwrap_or_default();
        names.retain(|p| p.extension().is_some_and(|x| x == "json"));
        names.sort();
        names
            .iter()
            .filter_map(|p| serde_json::from_slice(&std::fs::read(p).ok()?).ok())
            .collect()
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        let _ = self.child.kill(); // our own child, by handle
        let _ = self.child.wait();
    }
}

pub struct Body {
    child: Child,
    stdin: Option<ChildStdin>,
    frames: Receiver<(Instant, Value)>,
    err_path: PathBuf,
}

/// The body's environment as supervise.md §4.1 builds it (test mode), per seed/tests/boot.rs.
pub fn body_env(home: &Home, reason: &str, boot_id: &str, sha: &str) -> Vec<(String, String)> {
    let h = home.path.to_string_lossy().into_owned();
    let mut v: Vec<(String, String)> = [
        ("INIT_SESSION", SID),
        ("INIT_SHA", sha),
        ("INIT_BOOT_REASON", reason),
        ("INIT_MODE", "normal"),
        ("INIT_BOOT_ID", boot_id),
        ("INIT_UI", "stdio-jsonl/1"),
        ("INIT_KERNEL_VERSION", "0.1.0"),
        ("INIT_TEST", "1"),
        ("INIT_TEST_RETRY_BASE_MS", "5"),
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
    v.push(("INIT_KERNEL_SOCK".into(), format!("{h}/k.sock")));
    v.push((
        "INIT_PROXY_SOCK".into(),
        home.sock().to_string_lossy().into_owned(),
    ));
    v.push(("HOME".into(), format!("{h}/work/home")));
    v.push(("GIT_CEILING_DIRECTORIES".into(), h));
    v.push(("PATH".into(), std::env::var("PATH").unwrap_or_default()));
    v
}

impl Body {
    pub fn spawn(
        bin: &Path,
        home: &Home,
        env: Vec<(String, String)>,
        tag: &str,
    ) -> Result<Body, String> {
        let err_path = home.path.join(format!("body-{tag}.stderr"));
        let mut child = Command::new(bin)
            .env_clear()
            .envs(env)
            .current_dir(home.path.join("work"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(File::create(&err_path).map_err(|e| e.to_string())?)
            .spawn()
            .map_err(|e| format!("spawn {}: {e}", bin.display()))?;
        let (tx, frames) = channel();
        let out = child.stdout.take().ok_or("no stdout")?;
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines() {
                let Ok(l) = line else { break };
                if let Ok(v) = serde_json::from_str::<Value>(&l) {
                    let _ = tx.send((Instant::now(), v));
                }
            }
        });
        Ok(Body {
            stdin: child.stdin.take(),
            child,
            frames,
            err_path,
        })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn stderr_tail(&self) -> String {
        let t = std::fs::read_to_string(&self.err_path).unwrap_or_default();
        let lines: Vec<&str> = t.lines().collect();
        lines[lines.len().saturating_sub(20)..].join("\n")
    }

    pub fn send(&mut self, mut v: Value) -> Result<(), String> {
        v["v"] = 1.into();
        let s = self.stdin.as_mut().ok_or("stdin closed")?;
        writeln!(s, "{v}").map_err(|e| format!("send: {e}"))?;
        s.flush().map_err(|e| format!("send: {e}"))
    }

    /// The next frame matching `pred` and the instant its line was read.
    pub fn wait_frame(
        &mut self,
        what: &str,
        limit: Duration,
        pred: impl Fn(&Value) -> bool,
    ) -> Result<(Instant, Value), String> {
        let t0 = Instant::now();
        loop {
            let left = limit.saturating_sub(t0.elapsed());
            match self.frames.recv_timeout(left.max(Duration::from_millis(1))) {
                Ok((at, v)) if pred(&v) => return Ok((at, v)),
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {
                    return Err(format!(
                        "no {what} frame within {limit:?}; stderr:\n{}",
                        self.stderr_tail()
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(format!(
                        "body stdout closed before {what}; stderr:\n{}",
                        self.stderr_tail()
                    ));
                }
            }
        }
    }

    /// A matching frame if one is already queued (non-blocking; others are dropped).
    pub fn poll_frame(&mut self, pred: impl Fn(&Value) -> bool) -> Option<(Instant, Value)> {
        while let Ok((at, v)) = self.frames.try_recv() {
            if pred(&v) {
                return Some((at, v));
            }
        }
        None
    }

    pub fn try_exit(&mut self) -> Option<Option<i32>> {
        self.child.try_wait().ok().flatten().map(|s| s.code())
    }

    pub fn wait_exit(&mut self, limit: Duration) -> Result<Option<i32>, String> {
        let t0 = Instant::now();
        loop {
            if let Some(c) = self.try_exit() {
                return Ok(c);
            }
            if t0.elapsed() > limit {
                return Err(format!("body still running after {limit:?}"));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Owner input (stdio-jsonl/1 `input`).
    pub fn input(&mut self, seq: u64, text: &str) -> Result<(), String> {
        self.send(json!({"t": "input", "id": util::random_hex(8), "seq": seq, "text": text, "ts": util::ts()}))
    }

    /// Waits for `state{busy:true}` then `state{busy:false}`: one run, start to idle.
    pub fn wait_run(&mut self, limit: Duration) -> Result<Instant, String> {
        self.wait_frame("busy", limit, |f| f["t"] == "state" && f["busy"] == true)?;
        Ok(self
            .wait_frame("idle", limit, |f| f["t"] == "state" && f["busy"] == false)?
            .0)
    }

    /// `stop` with a grace, then the exit code.
    pub fn stop(&mut self) -> Result<Option<i32>, String> {
        self.send(json!({"t": "stop", "reason": "measure", "grace_ms": 2000}))?;
        self.wait_exit(Duration::from_secs(15))
    }
}

impl Drop for Body {
    fn drop(&mut self) {
        let _ = self.child.kill(); // our own child, by handle
        let _ = self.child.wait();
    }
}

/// A random 40-hex commit-like sha (the candidate's `INIT_SHA`).
pub fn fake_sha() -> String {
    util::random_hex(20)
}
