//! SBX-009 (ADR-009, build/specs/ADR-009-spec.md "Tests: Root"): workspace validation over owner.sock, full mode
//! and its typed confirmation, snapshots (APFS clones) with retention and the size skip, and `/undo`, against the
//! real Root daemon in test mode; with the fake kernel, a body spawned through the broker gets INIT_ACCESS and
//! INIT_WORKSPACE, writes the workspace (and nothing else in the home), and `/undo` restores it. ADR-012 (v0.3.1): full
//! access is the default (`sandbox.mode = "full"`); the ADR-009 confirmation flow runs under `mode = "workspace"`, and a
//! full body reads and writes the home but never ROOT_HOME.
//! Run with `cargo test --offline -p init-root --features test-fixtures`.
//!
//! Live-system safety (CONVENTIONS §1): every process is this test's Root daemon or one that Root spawned and holds
//! (the fake kernel, a `/bin/sh` body); the test signals only its own children, by handle. `HOME` is a fake home
//! under `root/target/test-homes/sbx009/`; nothing outside it is read or written.
#![cfg(feature = "test-fixtures")]

use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_init-root");
const FK: &str = env!("CARGO_BIN_EXE_init-root-fakekernel");
const ROOT_CFG: &str = "[root]\ntest_mode = true\nhb_interval_s = 1\nhb_stale_s = 3\nkernel_hello_timeout_s = 20\nbackoff_initial_ms = 50\n\
    backoff_max_ms = 200\nstop_grace_s = 5\n";

struct Home {
    dir: PathBuf,
    rh: PathBuf,
    ih: PathBuf,
}

/// Removed by its exact path when the test passes (after the daemon, dropped first); kept when it fails.
impl Drop for Home {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
}

/// A fresh fake HOME `root/target/test-homes/sbx009/<name>` (short: ROOT_HOME must stay ≤ 80 bytes).
fn home(name: &str) -> Home {
    let base = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap().join("test-homes/sbx009");
    let dir = base.join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap(); // exact path, created by an earlier run of this test
    }
    for d in ["rel", "tc/bin", "proj/src", "other", ".ssh/keys", ".local/bin"] {
        fs::create_dir_all(dir.join(d)).unwrap();
    }
    fs::copy(FK, dir.join("rel/init-kernel")).unwrap();
    fs::write(dir.join("proj/src/a.txt"), "v1\n").unwrap();
    fs::write(dir.join("proj/README"), "readme\n").unwrap();
    Home { rh: dir.join("rh"), ih: dir.join("ih"), dir }
}

fn charter(extra: &str) -> String {
    format!("schema_version = 1\n[purpose]\ntext = \"SBX-009 test home.\"\n[autonomy]\nmax_level = 4\n[quotas]\ndisk_min_free_gib = 0\n{extra}{ROOT_CFG}\n")
}

fn wait(secs: u64, what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(secs), "not within {secs} s: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
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
    fn install(&self, kernel: bool, extra: &str) {
        fs::create_dir_all(&self.rh).unwrap();
        fs::write(self.rh.join("charter.toml"), charter(extra)).unwrap();
        let (rel, tc) = (self.dir.join("rel"), self.dir.join("tc"));
        let o = if kernel {
            self.run(&["install", "--no-service", "--release", rel.to_str().unwrap(), "--toolchain", tc.to_str().unwrap()])
        } else {
            self.run(&["install", "--no-service"])
        };
        assert_eq!(o.status.code(), Some(0), "install: {}", String::from_utf8_lossy(&o.stderr));
    }
    /// One owner-socket request: the whole answer.
    fn req(&self, cmd: &str, args: Value) -> Value {
        let mut s = UnixStream::connect(self.rh.join("run/owner.sock")).unwrap();
        writeln!(s, "{}", json!({"args": args, "cmd": cmd, "id": "t-1", "v": 1})).unwrap();
        let mut line = String::new();
        BufReader::new(&s).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
    fn ok(&self, cmd: &str, args: Value) -> Value {
        let v = self.req(cmd, args);
        assert_eq!(v["ok"], true, "{cmd}: {v}");
        v["result"].clone()
    }
    fn refused(&self, cmd: &str, args: Value) -> Value {
        let v = self.req(cmd, args);
        assert_eq!(v["ok"], false, "{cmd} must be refused: {v}");
        v["error"].clone()
    }
    fn daemon(&self, fk: &str) -> Daemon {
        let info: Value = serde_json::from_str(&fs::read_to_string(self.rh.join("install.json")).unwrap()).unwrap();
        let s = |k: &str| info[k].as_str().unwrap_or("").to_string();
        let c = self.cmd().arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).env("INIT_TEST_FK", fk)
            .env("INIT_TEST_TOOLCHAIN", s("toolchain")).env("INIT_TEST_DEVDIR", s("devdir")).env("INIT_TEST_HOME_REAL", &self.dir)
            .env("INIT_ROOT_TEST_NOW", "2030-01-10T00:00:00Z").spawn().unwrap();
        let d = Daemon(c, self.rh.clone());
        wait(10, "owner.sock", || UnixStream::connect(self.rh.join("run/owner.sock")).is_ok());
        d
    }
    fn s(&self, rel: &str) -> String {
        self.dir.join(rel).to_string_lossy().into_owned()
    }
    fn snapshots(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(self.rh.join("snapshots")).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.ends_with(".json")).collect()).unwrap_or_default();
        v.sort();
        v
    }
    fn fk_events(&self, name: &str) -> Vec<Value> {
        fs::read_to_string(self.ih.join("state/kernel/fk.jsonl")).unwrap_or_default().lines().filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|v| v["ev"] == "event" && v["name"] == name).map(|v| v["data"].clone()).collect()
    }
}

/// The Root daemon this test spawned. On drop: an orderly `stop`, then the daemon by its own handle.
struct Daemon(Child, PathBuf);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = Command::new(BIN).env_clear().env("HOME", "/nonexistent").env("INIT_ROOT_HOME", &self.1).args(["stop"]).output();
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(8) {
            let o = Command::new(BIN).env_clear().env("HOME", "/nonexistent").env("INIT_ROOT_HOME", &self.1).args(["status", "--json"]).output();
            let v: Value = o.ok().and_then(|o| serde_json::from_slice(&o.stdout).ok()).unwrap_or_default();
            if v["instances"].as_array().is_none_or(Vec::is_empty) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Spec item 2 over owner.sock: each refusal reason, a symlinked cwd canonicalized, the workspace in `status`.
/// Then full mode under a `sandbox.mode = "workspace"` charter (RR-39): `root.confirm` until the phrase, once per
/// session; a new `workspace.set` ends it.
/// Then the snapshot retention (5 per workspace) and the 2 GiB skip (a sparse file: no disk is used).
// LINUX: workspace mode and full mode are capped off on Linux (access.rs LINUX_HOME_GRANTS); macOS only
#[cfg(target_os = "macos")]
#[test]
fn workspace_validation_full_mode_retention_and_size_skip() {
    let h = home("v");
    h.install(false, "[sandbox]\nmode = \"workspace\"\n");
    let _d = h.daemon("");
    std::os::unix::fs::symlink(h.dir.join("proj"), h.dir.join("link")).unwrap();
    fs::write(h.dir.join("other/file"), "x").unwrap();
    let cases: Vec<(String, &str)> = vec![
        ("proj".into(), "relative"),
        (h.s("missing"), "missing"),
        (h.s("other/file"), "not_dir"),
        ("/usr/bin".into(), "outside_home"),
        (h.s(""), "home"),
        (h.dir.to_string_lossy().into_owned(), "home"),
        (h.s("rh"), "root_path"),
        (h.s("ih/work"), "root_path"),
        (h.s(".ssh/keys"), "deny_list"),
        (h.s(".local/bin"), "deny_list"),
    ];
    for (cwd, code) in &cases {
        let r = h.ok("workspace.set", json!({"cwd": cwd}));
        assert_eq!((r["refused"]["code"].as_str(), &r["access"]["workspace"]), (Some(*code), &Value::Null), "{cwd}: {r}");
        assert_eq!(r["access"]["access"], "read_home", "no workspace: the home read only");
    }
    assert_eq!(h.refused("workspace.set", json!({}))["code"], "root.args");
    let proj = fs::canonicalize(h.dir.join("proj")).unwrap().to_string_lossy().into_owned();
    let r = h.ok("workspace.set", json!({"cwd": h.s("link")}));
    assert_eq!((r["refused"].clone(), r["access"]["workspace"].as_str(), r["changed"].as_bool()), (Value::Null, Some(proj.as_str()), Some(true)));
    assert_eq!(r["access"]["access"], "workspace");
    assert_eq!(r["access"]["snapshot"]["reason"], "session", "{r}");
    let st = h.ok("status", json!({}));
    assert_eq!((st["access"]["workspace"].as_str(), st["access"]["full"].as_bool()), (Some(proj.as_str()), Some(false)));

    // full mode: a typed confirmation once per owner session
    let e = h.refused("full.set", json!({"on": true}));
    assert_eq!((e["code"].as_str(), e["data"]["phrase"].as_str()), (Some("root.confirm"), Some("confirm")), "{e}");
    assert!(e["message"].as_str().unwrap().contains("snapshotted first"), "{e}");
    assert_eq!(h.refused("full.set", json!({"on": true, "confirm": "yes"}))["code"], "root.confirm");
    let r = h.ok("full.set", json!({"on": true, "confirm": "confirm"}));
    assert_eq!((r["access"]["access"].as_str(), r["access"]["full"].as_bool(), r["changed"].as_bool()), (Some("full"), Some(true), Some(true)));
    assert_eq!(r["snapshot"]["reason"], "full_on");
    assert_eq!(h.ok("status", json!({}))["access"]["full"], true, "the status bar's FULL");
    assert_eq!(h.ok("full.set", json!({"on": false}))["access"]["access"], "workspace");
    assert_eq!(h.ok("full.set", json!({"on": true}))["access"]["full"], true, "confirmed once this session");
    h.ok("workspace.set", json!({"cwd": proj}));
    assert_eq!(h.ok("access.get", json!({}))["full"], false, "a new session ends full mode");
    assert_eq!(h.refused("full.set", json!({"on": true}))["code"], "root.confirm", "and forgets the confirmation");
    // retention: the last 5 snapshots of the workspace (session, full_on, session, and four more sessions)
    for _ in 0..4 {
        h.ok("workspace.set", json!({"cwd": proj}));
    }
    let snaps = h.snapshots();
    assert_eq!(snaps.len(), 5, "{snaps:?}");
    for s in &snaps {
        let meta: Value = serde_json::from_str(&fs::read_to_string(h.rh.join("snapshots").join(format!("{s}.json"))).unwrap()).unwrap();
        assert_eq!((meta["workspace"].as_str(), meta["id"].as_str()), (Some(proj.as_str()), Some(s.as_str())));
        assert_eq!(fs::read_to_string(h.rh.join("snapshots").join(s).join("src/a.txt")).unwrap(), "v1\n");
    }
    // the size skip: a sparse 3 GiB file (no blocks are written) makes the next workspace too big to snapshot
    fs::create_dir_all(h.dir.join("big")).unwrap();
    fs::File::create(h.dir.join("big/sparse")).unwrap().set_len(3 << 30).unwrap();
    let r = h.ok("workspace.set", json!({"cwd": h.s("big")}));
    assert_eq!((r["access"]["access"].as_str(), &r["access"]["snapshot"]), (Some("workspace"), &Value::Null), "{r}");
    assert_eq!(h.snapshots().len(), 5, "no snapshot of the big workspace");
    let e = h.refused("undo", json!({"confirm": "confirm"}));
    assert_eq!(e["code"], "root.not_found", "{e}");
    fs::remove_file(h.dir.join("big/sparse")).unwrap(); // exact path, created above
    // `/full on` with no workspace warns that nothing covers it; `/undo` with none is root.mode
    h.ok("workspace.set", json!({"cwd": h.s("")}));
    assert!(h.refused("full.set", json!({"on": true}))["message"].as_str().unwrap().contains("no snapshot covers it"));
    let e = h.refused("undo", json!({}));
    assert_eq!((e["code"].as_str(), e["data"]["reason"].as_str()), (Some("root.mode"), Some("no_workspace")));
    // the access records (no paths: the lineage is mirrored)
    let lin = fs::read_to_string(h.ih.join("root/lineage.jsonl")).unwrap();
    let recs: Vec<Value> = lin.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["kind"] == "access").collect();
    assert!(recs.iter().any(|r| r["data"]["event"] == "full_on") && recs.iter().any(|r| r["data"]["event"] == "workspace"), "{recs:?}");
    assert!(!lin.contains(&proj), "no workspace path in the lineage");
}

/// `sandbox.allow_full = false` refuses full mode with `root.mode`; `read_home = false` gives `none`.
#[test]
fn charter_disables_full_mode_and_home_reads() {
    let h = home("c");
    h.install(false, "[sandbox]\nread_home = false\nallow_full = false\nread_deny = [\"/Users/nobody-sbx009/Private\"]\n");
    let _d = h.daemon("");
    let v = h.ok("access.get", json!({}));
    assert_eq!((v["access"].as_str(), v["allow_full"].as_bool(), v["read_home"].as_bool()), (Some("none"), Some(false), Some(false)));
    let e = h.refused("full.set", json!({"on": true, "confirm": "confirm"}));
    // ADR-012: full access is possible on Linux too, so the charter answers on every platform
    let linux = cfg!(target_os = "linux");
    assert_eq!((e["code"].as_str(), e["data"]["reason"].as_str()), (Some("root.mode"), Some("charter")));
    assert_eq!((v["full"].as_bool(), v["mode"].as_str()), (Some(false), Some("full")), "allow_full = false wins over the default mode");
    let w = h.ok("workspace.set", json!({"cwd": h.s("proj")}));
    assert_eq!(w["access"]["access"], if linux { "none" } else { "workspace" });
    if linux {
        assert_eq!(w["refused"]["code"], "platform", "{w}");
    }
}

/// The e2e core at the Root (spec "Tests: e2e"): with the fake kernel, a body spawned through the broker gets the
/// Root's INIT_ACCESS / INIT_WORKSPACE (over the kernel's), writes a file in the workspace and cannot write
/// elsewhere in the home; the kernel hears `access` with `restart:"idle"` while that body runs; `/undo` is refused
/// while the body is busy, needs the typed confirmation, restores the snapshot and can itself be undone.
// LINUX: workspace mode and full mode are capped off on Linux (access.rs LINUX_HOME_GRANTS); macOS only
#[cfg(target_os = "macos")]
#[test]
fn body_writes_the_workspace_and_undo_restores_it() {
    let h = home("e");
    h.install(true, "");
    let _d = h.daemon("");
    wait(20, "running", || h.ok("status", json!({}))["slot"] == "running");
    let proj = fs::canonicalize(h.dir.join("proj")).unwrap();
    // `init-agent --workspace`: the session is confined to ADR-009's workspace mode under the default full charter
    let r = h.ok("workspace.set", json!({"cwd": h.s("proj"), "mode": "workspace"}));
    assert_eq!((r["access"]["access"].as_str(), r["access"]["mode"].as_str()), (Some("workspace"), Some("full")), "{r}");
    let first = r["access"]["snapshot"]["id"].as_str().unwrap().to_string();
    wait(10, "access event after the change", || h.fk_events("access").iter().any(|d| d["workspace"].as_str() == proj.to_str()));
    // a body (the kernel's own spawn request, as the fake kernel's `spawn_args`, running /bin/sh)
    let ih = fs::canonicalize(&h.ih).unwrap();
    let i = ih.to_string_lossy().into_owned();
    let info: Value = serde_json::from_str(&fs::read_to_string(h.rh.join("install.json")).unwrap()).unwrap();
    let mut p = serde_json::Map::new();
    for (k, sub) in [("BIN", "/bin"), ("BODY", "/body"), ("CACHE", "/cache"), ("INIT_HOME", ""), ("KERNEL", "/kernel"),
        ("KERNEL_SOCK", "/run/kernel.sock"), ("LINEAGE", "/lineage"), ("LINK_BIN", "/bin"), ("LINK_KERNEL", "/kernel"),
        ("LINK_LINEAGE", "/lineage"), ("PROXY_SOCK", "/run/proxy.sock"), ("PUB", "/root"), ("RUN", "/run"),
        ("STATE", "/state"), ("STATE_KERNEL", "/state/kernel"), ("VAULT", "/vault"), ("WORK", "/work")] {
        p.insert(k.into(), json!(format!("{i}{sub}")));
    }
    p.insert("TOOLCHAIN".into(), info["toolchain"].clone());
    p.insert("DEVDIR".into(), info["devdir"].clone());
    p.insert("HOME_REAL".into(), json!(h.dir));
    let other = h.s("other/agent.txt");
    let script = format!(
        "env > '{i}/work/env.txt'; echo agent > \"$INIT_WORKSPACE/agent.txt\"; echo v2 > \"$INIT_WORKSPACE/src/a.txt\"; \
         ( echo no > '{other}' ) 2>/dev/null; echo $? > '{i}/work/other.rc'; exec /bin/sleep 30"
    );
    let req = json!({"args": ["-c", script], "cwd": format!("{i}/work"), "env": {"PATH": "/usr/bin:/bin", "INIT_ACCESS": "full", "INIT_WORKSPACE": "/"},
        "kind": "body", "lane": "live", "params": p, "profile": "body", "program": "/bin/sh"});
    let (tmp, dst) = (h.ih.join("work/fk-call.tmp"), h.ih.join("work/fk-call.json"));
    fs::write(&tmp, json!({"cmd": "spawn", "args": req}).to_string()).unwrap();
    fs::rename(tmp, dst).unwrap();
    wait(20, "the body ran", || h.ih.join("work/other.rc").exists());
    let env = fs::read_to_string(h.ih.join("work/env.txt")).unwrap();
    assert!(env.lines().any(|l| l == "INIT_ACCESS=workspace"), "the Root's value, not the kernel's: {env}");
    assert!(env.lines().any(|l| l == format!("INIT_WORKSPACE={}", proj.display())), "{env}");
    assert_eq!(fs::read_to_string(proj.join("agent.txt")).unwrap(), "agent\n", "the body wrote its workspace");
    assert_eq!(fs::read_to_string(proj.join("src/a.txt")).unwrap(), "v2\n");
    assert_ne!(fs::read_to_string(h.ih.join("work/other.rc")).unwrap().trim(), "0", "elsewhere in the home is denied");
    assert!(!h.dir.join("other/agent.txt").exists());
    // /undo: refused while a turn runs, then the typed confirmation
    fs::write(h.ih.join("work/fk-busy"), "").unwrap();
    wait(10, "busy", || h.req("undo", json!({}))["error"]["code"] == "root.busy"); // checked before the confirmation
    let e = h.refused("undo", json!({"confirm": "confirm"}));
    assert_eq!((e["code"].as_str(), e["data"]["reason"].as_str()), (Some("root.busy"), Some("body_busy")));
    fs::remove_file(h.ih.join("work/fk-busy")).unwrap(); // exact path, created above
    wait(10, "idle", || h.req("undo", json!({}))["error"]["code"] == "root.confirm");
    let e = h.refused("undo", json!({}));
    let latest = e["data"]["snapshot"]["id"].as_str().unwrap().to_string();
    assert_eq!(e["data"]["phrase"], "confirm");
    assert_eq!(latest, first, "the session snapshot, taken before the body's first write");
    let r = h.ok("undo", json!({"confirm": "confirm"}));
    assert_eq!((r["restored"]["id"].as_str(), r["saved"]["reason"].as_str()), (Some(latest.as_str()), Some("pre_undo")));
    assert!(!proj.join("agent.txt").exists(), "restored: the agent's new file is gone");
    assert_eq!(fs::read_to_string(proj.join("src/a.txt")).unwrap(), "v1\n");
    assert_eq!(fs::read_to_string(proj.join("README")).unwrap(), "readme\n");
    let saved = h.rh.join("snapshots").join(r["saved"]["id"].as_str().unwrap());
    assert_eq!(fs::read_to_string(saved.join("agent.txt")).unwrap(), "agent\n", "the state before /undo was saved");
    assert!(h.snapshots().contains(&first), "the session snapshot is still kept");
    // the undo can itself be undone: the latest snapshot is now the pre_undo one
    let r2 = h.ok("undo", json!({"confirm": "confirm"}));
    assert_eq!(r2["restored"]["id"], r["saved"]["id"]);
    assert_eq!(fs::read_to_string(proj.join("agent.txt")).unwrap(), "agent\n");
    assert_eq!(fs::read_to_string(proj.join("src/a.txt")).unwrap(), "v2\n");
    // full access while that body still runs (no confirmation: the charter's default is full): a restart at idle
    assert_eq!(h.ok("access.get", json!({}))["restart_pending"], false, "the body holds the current grant");
    h.ok("full.set", json!({"on": true}));
    wait(10, "access{restart:idle}", || h.fk_events("access").iter().any(|d| d["full"] == true && d["restart"] == "idle"));
    assert_eq!(h.ok("access.get", json!({}))["restart_pending"], true);
}

/// The body's spawn request as the fake kernel sends it (`spawn_args`): kind body, the live lane, running `/bin/sh -c`.
fn body_request(h: &Home, script: &str, env: Value) -> Value {
    let ih = fs::canonicalize(&h.ih).unwrap();
    let i = ih.to_string_lossy().into_owned();
    let info: Value = serde_json::from_str(&fs::read_to_string(h.rh.join("install.json")).unwrap()).unwrap();
    let mut p = serde_json::Map::new();
    for (k, sub) in [("BIN", "/bin"), ("BODY", "/body"), ("CACHE", "/cache"), ("INIT_HOME", ""), ("KERNEL", "/kernel"),
        ("KERNEL_SOCK", "/run/kernel.sock"), ("LINEAGE", "/lineage"), ("LINK_BIN", "/bin"), ("LINK_KERNEL", "/kernel"),
        ("LINK_LINEAGE", "/lineage"), ("PROXY_SOCK", "/run/proxy.sock"), ("PUB", "/root"), ("RUN", "/run"),
        ("STATE", "/state"), ("STATE_KERNEL", "/state/kernel"), ("VAULT", "/vault"), ("WORK", "/work")] {
        p.insert(k.into(), json!(format!("{i}{sub}")));
    }
    p.insert("TOOLCHAIN".into(), info["toolchain"].clone());
    p.insert("DEVDIR".into(), info["devdir"].clone());
    p.insert("HOME_REAL".into(), json!(h.dir));
    json!({"args": ["-c", script], "cwd": format!("{i}/work"), "env": env, "kind": "body", "lane": "live", "params": p, "profile": "body",
        "program": "/bin/sh"})
}

/// ADR-012 (RR-39b): the default `sandbox.mode = "full"`: every session starts full with no confirmation, `init
/// --workspace` (`workspace.set{mode:"workspace"}`) and `/full off` confine it, `/full on` lifts that at once, the
/// confinement survives a Root restart, and `init-root status` names the mode and how to change it.
#[test]
fn default_full_access_and_session_choices() {
    let h = home("d");
    h.install(false, "");
    let linux = cfg!(target_os = "linux");
    let confined = if linux { "none" } else { "workspace" }; // LINUX: workspace mode has no home grants
    let proj = fs::canonicalize(h.dir.join("proj")).unwrap().to_string_lossy().into_owned();
    {
        let _d = h.daemon("");
        let v = h.ok("access.get", json!({}));
        assert_eq!((v["access"].as_str(), v["full"].as_bool(), v["mode"].as_str(), v["workspace"].clone()), (Some("full"), Some(true), Some("full"), Value::Null), "{v}");
        let r = h.ok("workspace.set", json!({"cwd": h.s("proj")}));
        assert_eq!((r["access"]["access"].as_str(), r["access"]["workspace"].as_str(), r["refused"].clone()), (Some("full"), Some(proj.as_str()), Value::Null), "{r}");
        assert_eq!(h.refused("workspace.set", json!({"cwd": h.s("proj"), "mode": "open"}))["code"], "root.args");
        let r = h.ok("workspace.set", json!({"cwd": h.s("proj"), "mode": "workspace"}));
        assert_eq!((r["access"]["access"].as_str(), r["access"]["full"].as_bool(), r["changed"].as_bool()), (Some(confined), Some(false), Some(true)), "{r}");
        if linux {
            assert_eq!(r["refused"]["code"], "platform", "{r}");
        }
        let r = h.ok("full.set", json!({"on": true}));
        assert_eq!((r["access"]["access"].as_str(), r["changed"].as_bool()), (Some("full"), Some(true)), "no confirmation under the default: {r}");
        assert_eq!(r["access"]["workspace"].as_str(), Some(proj.as_str()), "the session's workspace is kept");
        let r = h.ok("full.set", json!({"on": false}));
        assert_eq!((r["access"]["access"].as_str(), r["changed"].as_bool()), (Some(confined), Some(true)));
        assert_eq!(h.ok("status", json!({}))["access"]["mode"], "full");
        let o = h.run(&["status"]);
        let out = String::from_utf8_lossy(&o.stdout);
        assert!(out.contains("access: WORKSPACE (this session)"), "{out}");
    }
    // a Root restart keeps the session's confinement (it never widens access); a new `init-agent` launch takes the default
    let _d = h.daemon("");
    assert_eq!(h.ok("access.get", json!({}))["full"], false, "confined across a Root restart");
    let r = h.ok("workspace.set", json!({"cwd": h.s("proj")}));
    assert_eq!(r["access"]["access"], "full", "{r}");
    let out = String::from_utf8_lossy(&h.run(&["status"]).stdout).into_owned();
    assert!(out.contains("access: FULL (default) · workspace ") && out.contains("init-agent --workspace, /full off, or sandbox.mode = \"workspace\" in "), "{out}");
    let lin = fs::read_to_string(h.ih.join("root/lineage.jsonl")).unwrap();
    assert!(!lin.contains(&proj), "no workspace path in the lineage");
}

/// ADR-012 (RR-39b, macOS): a full body spawned through the broker gets the Root's `INIT_ACCESS=full`, the owner's
/// `HOME` and `PATH` additions, no `CARGO_NET_OFFLINE`; it writes elsewhere in the home but cannot read ROOT_HOME or
/// replace INIT's launcher.
#[cfg(target_os = "macos")]
#[test]
fn full_body_writes_the_home_not_the_root() {
    let h = home("f");
    h.install(true, "");
    let _d = h.daemon("");
    wait(20, "running", || h.ok("status", json!({}))["slot"] == "running");
    h.ok("workspace.set", json!({"cwd": h.s("proj")}));
    let i = fs::canonicalize(&h.ih).unwrap().to_string_lossy().into_owned();
    let (other, charter) = (h.s("other/full.txt"), h.rh.join("charter.toml").to_string_lossy().into_owned());
    let (launcher, old_name) = (h.s(".local/bin/init-agent"), h.s(".local/bin/init")); // issue #1: and the macOS shim's name
    let script = format!(
        "env > '{i}/work/env.txt'; echo full > '{other}'; ( cat '{charter}' ) >/dev/null 2>&1; echo $? > '{i}/work/rh.rc'; \
         ( echo x > '{old_name}' ) 2>/dev/null; echo $? > '{i}/work/old.rc'; \
         ( echo x > '{launcher}' ) 2>/dev/null; echo $? > '{i}/work/init.rc'; exec /bin/sleep 30"
    );
    let env = json!({"PATH": "/usr/bin:/bin", "INIT_ACCESS": "none", "INIT_WORKSPACE": "/", "CARGO_NET_OFFLINE": "true", "HOME": format!("{i}/work/home")});
    let (tmp, dst) = (h.ih.join("work/fk-call.tmp"), h.ih.join("work/fk-call.json"));
    fs::write(&tmp, json!({"cmd": "spawn", "args": body_request(&h, &script, env)}).to_string()).unwrap();
    fs::rename(tmp, dst).unwrap();
    wait(20, "the body ran", || h.ih.join("work/init.rc").exists());
    let env = fs::read_to_string(h.ih.join("work/env.txt")).unwrap();
    let home = fs::canonicalize(&h.dir).unwrap().to_string_lossy().into_owned();
    assert!(env.lines().any(|l| l == "INIT_ACCESS=full"), "the Root's value: {env}");
    assert!(env.lines().any(|l| l == format!("HOME={home}")), "the owner's home: {env}");
    assert!(env.lines().any(|l| l == format!("PATH=/usr/bin:/bin:{home}/.local/bin:{home}/.cargo/bin")), "{env}");
    assert!(!env.contains("CARGO_NET_OFFLINE"), "{env}");
    assert_eq!(fs::read_to_string(h.dir.join("other/full.txt")).unwrap(), "full\n", "full access writes elsewhere in the home");
    assert_ne!(fs::read_to_string(h.ih.join("work/rh.rc")).unwrap().trim(), "0", "ROOT_HOME stays unreadable");
    assert_ne!(fs::read_to_string(h.ih.join("work/init.rc")).unwrap().trim(), "0", "INIT's launcher stays unwritable");
    assert_ne!(fs::read_to_string(h.ih.join("work/old.rc")).unwrap().trim(), "0", "so does its old name, init");
    assert!(!h.dir.join(".local/bin/init-agent").exists() && !h.dir.join(".local/bin/init").exists());
}
