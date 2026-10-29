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

#[path = "common/shown.rs"]
mod shown;
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
            unlock(&self.dir);
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
}

/// v0.5.3: the `/undo` tests leave read-only folders and locked files in their home (in the workspace and in the
/// Root's clones of it); this makes them removable again: the lock flag off, then the owner's permissions on, under
/// `dir` only (a test home this file made).
fn unlock(dir: &Path) {
    if cfg!(target_os = "macos") {
        let _ = Command::new("/usr/bin/chflags").args(["-R", "nouchg"]).arg(dir).stderr(Stdio::null()).status();
        let _ = Command::new("/bin/chmod").args(["-R", "-N"]).arg(dir).stderr(Stdio::null()).status(); // and every ACL
    }
    let _ = Command::new("/bin/chmod").args(["-R", "u+rwX"]).arg(dir).stderr(Stdio::null()).status();
}

/// A fresh fake HOME `root/target/test-homes/sbx009/<name>` (short: ROOT_HOME must stay ≤ 80 bytes).
fn home(name: &str) -> Home {
    let base = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap().join("test-homes/sbx009");
    let dir = base.join(name);
    if dir.exists() {
        unlock(&dir);
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
        // TZ: the Root prints snapshot times on the owner's clock (R-UNDO); a fixed UTC+7 makes them exact here
        c.env_clear().env("HOME", &self.dir).env("INIT_ROOT_HOME", &self.rh).env("INIT_HOME", &self.ih).env("PATH", "/usr/bin:/bin").env("TZ", "ICT-7");
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
        self.daemon_with(fk, &[])
    }
    /// [`Home::daemon`] with more environment (v0.5.3: `INIT_FAULT`, the Root's fault points in test builds).
    fn daemon_with(&self, fk: &str, env: &[(&str, &str)]) -> Daemon {
        let info: Value = serde_json::from_str(&fs::read_to_string(self.rh.join("install.json")).unwrap()).unwrap();
        let s = |k: &str| info[k].as_str().unwrap_or("").to_string();
        let mut c = self.cmd();
        c.arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).env("INIT_TEST_FK", fk)
            .env("INIT_TEST_TOOLCHAIN", s("toolchain")).env("INIT_TEST_DEVDIR", s("devdir")).env("INIT_TEST_HOME_REAL", &self.dir)
            .env("INIT_ROOT_TEST_NOW", "2030-01-10T00:00:00Z");
        c.envs(env.iter().copied());
        let d = Daemon(c.spawn().unwrap(), self.rh.clone());
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
    // R-UNDO (v0.5.2): nothing changed since the session snapshot, so full_on keeps none of its own
    assert_eq!((r["snapshot"]["reason"].as_str(), r["snapshot"]["unchanged"].as_bool()), (Some("session"), Some(true)), "{r}");
    assert_eq!(h.snapshots().len(), 1);
    fs::write(h.dir.join("proj/full.txt"), "x\n").unwrap();
    h.ok("full.set", json!({"on": false}));
    let r = h.ok("full.set", json!({"on": true}));
    assert_eq!((r["snapshot"]["reason"].as_str(), r["snapshot"]["unchanged"].as_bool()), (Some("full_on"), None), "a changed folder: {r}");
    assert_eq!(h.ok("status", json!({}))["access"]["full"], true, "the status bar's FULL");
    assert_eq!(h.ok("full.set", json!({"on": false}))["access"]["access"], "workspace");
    assert_eq!(h.ok("full.set", json!({"on": true}))["access"]["full"], true, "confirmed once this session");
    h.ok("workspace.set", json!({"cwd": proj}));
    assert_eq!(h.ok("access.get", json!({}))["full"], false, "a new session ends full mode");
    assert_eq!(h.refused("full.set", json!({"on": true}))["code"], "root.confirm", "and forgets the confirmation");
    // retention: the last 5 snapshots of the workspace (session, full_on, and six more sessions of a folder changed each
    // time; the clock moves, as it does between launches). An unchanged folder takes none (R-UNDO, v0.5.2).
    assert_eq!(h.snapshots().len(), 2, "the new session's folder is unchanged since full_on");
    for i in 0..6 {
        h.ok("test.clock", json!({"advance_ms": 60_000}));
        fs::write(h.dir.join("proj/n.txt"), format!("{i}\n")).unwrap();
        h.ok("workspace.set", json!({"cwd": proj}));
    }
    let snaps = h.snapshots();
    assert_eq!(snaps.len(), 5, "{snaps:?}");
    assert_eq!(fs::read_to_string(h.rh.join("snapshots").join(&snaps[4]).join("n.txt")).unwrap(), "5\n", "the newest is kept");
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
    // v0.5.3 (lane H): a workspace without the home read is `workspace_only`, which the body's boot block states (until
    // v0.5.3 it was `workspace`, and the boot block told the model it could read the owner's home)
    assert_eq!(w["access"]["access"], if linux { "none" } else { "workspace_only" });
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
    // the undo can itself be undone, by the save's id (v0.5.3: plain /undo never goes back to a state an /undo replaced)
    assert_eq!(h.refused("undo", json!({}))["data"]["reason"], "replaced");
    let sid = r["saved"]["id"].as_str().unwrap();
    let mut a = Attach::new(&h);
    a.say(&format!("/undo {sid} confirm"));
    a.notice(&format!("[root] /undo restored {} to snapshot {sid},", proj.display()));
    assert_eq!(fs::read_to_string(proj.join("agent.txt")).unwrap(), "agent\n");
    assert_eq!(fs::read_to_string(proj.join("src/a.txt")).unwrap(), "v2\n");
    // full access while that body still runs (no confirmation: the charter's default is full): a restart at idle
    assert_eq!(h.ok("access.get", json!({}))["restart_pending"], false, "the body holds the current grant");
    h.ok("full.set", json!({"on": true}));
    wait(10, "access{restart:idle}", || h.fk_events("access").iter().any(|d| d["full"] == true && d["restart"] == "idle"));
    assert_eq!(h.ok("access.get", json!({}))["restart_pending"], true);
}

/// v0.5.3 (lane H, the cancelled demo's setup): with a workspace set, turning `sandbox.read_home` off is a change of the
/// live body's grant. The running body is restarted at its next idle point (`access{restart:"idle"}`, `restart_pending`)
/// and the next one gets `INIT_ACCESS=workspace_only`, so its boot block no longer says it can read the owner's home. In
/// v0.5.2 both grants were `workspace`, so the body kept the home read and the boot block kept the claim.
#[cfg(target_os = "macos")]
#[test]
fn turning_read_home_off_under_a_workspace_restarts_the_body_and_says_so() {
    let h = home("rh");
    h.install(true, "");
    let _d = h.daemon("");
    wait(20, "running", || h.ok("status", json!({}))["slot"] == "running");
    h.ok("workspace.set", json!({"cwd": h.s("proj"), "mode": "workspace"}));
    let i = fs::canonicalize(&h.ih).unwrap().to_string_lossy().into_owned();
    let (env1, first) = live_body(&h, &format!("env > '{i}/work/e.tmp' && mv '{i}/work/e.tmp' '{i}/work/env1.txt'; exec /bin/sleep 60"), Value::Null);
    wait(20, "the body ran", || h.ih.join("work/env1.txt").exists());
    let env = fs::read_to_string(h.ih.join("work/env1.txt")).unwrap();
    assert!(env.lines().any(|l| l == "INIT_ACCESS=workspace"), "{env} {env1}");
    assert_eq!(h.ok("access.get", json!({}))["restart_pending"], false, "the body holds the current grant");
    fs::write(h.rh.join("charter.toml"), charter("[sandbox]\nread_home = false\n")).unwrap();
    let o = h.run(&["charter", "check", "--apply", "--yes"]);
    assert_eq!(o.status.code(), Some(0), "charter apply: {}", String::from_utf8_lossy(&o.stderr));
    wait(10, "access{restart:idle}", || h.fk_events("access").iter().any(|d| d["access"] == "workspace_only" && d["restart"] == "idle"));
    let v = h.ok("access.get", json!({}));
    assert_eq!((v["access"].as_str(), v["read_home"].as_bool(), v["restart_pending"].as_bool()), (Some("workspace_only"), Some(false), Some(true)), "{v}");
    live_body(&h, &format!("env > '{i}/work/e.tmp' && mv '{i}/work/e.tmp' '{i}/work/env2.txt'; exec /bin/sleep 60"), first);
    wait(20, "the second body ran", || h.ih.join("work/env2.txt").exists());
    let env = fs::read_to_string(h.ih.join("work/env2.txt")).unwrap();
    assert!(env.lines().any(|l| l == "INIT_ACCESS=workspace_only"), "the next body is told it has no home read: {env}");
    assert_eq!(h.ok("access.get", json!({}))["restart_pending"], false, "the new body holds the new grant");
}

/// v0.5.3 (lane H, the cancelled demo's setup): another INIT install on the account (here `~/.init-root-demo`, the
/// default `~/.init`, and a ROOT_HOME under another name with the INIT_HOME its install.json records) is denied to this
/// install's live body in full access (the default) and in workspace mode: it reads none of their keys and cannot
/// replace their Root. Neither mode reads the maintainer's release signing key in `~/.init-release`. Workspace mode also
/// denies the credential stores the v0.5.2 list missed; full access reads them (ADR-012). In v0.5.2 a full body could
/// write the other install's Root binary.
#[cfg(target_os = "macos")]
#[test]
fn another_installs_homes_are_denied_in_both_modes() {
    let h = home("o");
    h.install(true, "");
    let (demo_rh, demo_ih) = (h.dir.join("demo-rh"), h.dir.join("demo-ih"));
    for d in [".init-root-demo/keys", ".init-root-demo/bin", ".init/kernel/bin", "demo-rh/keys", "demo-ih/state", ".config/gh", ".init-release"] {
        fs::create_dir_all(h.dir.join(d)).unwrap();
    }
    for f in [".init-root-demo/keys/lineage.key", ".init-root-demo/bin/init-root", ".init/kernel/bin/init-kernel", "demo-rh/keys/lineage.key",
        "demo-ih/state/x", ".git-credentials", ".config/gh/hosts.yml", ".init-release/release.key"] {
        fs::write(h.dir.join(f), "fake, not a real secret\n").unwrap();
    }
    fs::write(demo_rh.join("install.json"), json!({"init_home": demo_ih, "root_home": demo_rh}).to_string()).unwrap();
    let _d = h.daemon("");
    wait(20, "running", || h.ok("status", json!({}))["slot"] == "running");
    let i = fs::canonicalize(&h.ih).unwrap().to_string_lossy().into_owned();
    let probe = |tag: &str| {
        let mut sh = String::new();
        for (n, f) in [(1, ".init-root-demo/keys/lineage.key"), (2, ".init/kernel/bin/init-kernel"), (3, "demo-rh/keys/lineage.key"),
            (4, "demo-ih/state/x"), (5, ".git-credentials"), (6, ".config/gh/hosts.yml"), (7, ".init-release/release.key")] {
            sh.push_str(&format!("( cat '{}' ) >/dev/null 2>&1; echo r{n}=$? >> '{i}/work/{tag}.t'; ", h.s(f)));
        }
        sh.push_str(&format!("( echo x > '{}' ) 2>/dev/null; echo w1=$? >> '{i}/work/{tag}.t'; ", h.s(".init-root-demo/bin/init-root")));
        sh.push_str(&format!("mv '{i}/work/{tag}.t' '{i}/work/{tag}.rc'; exec /bin/sleep 60"));
        sh
    };
    let rc = |tag: &str| {
        wait(20, tag, || h.ih.join(format!("work/{tag}.rc")).exists());
        let t = fs::read_to_string(h.ih.join(format!("work/{tag}.rc"))).unwrap();
        t.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.to_string(), v == "0")).collect::<std::collections::BTreeMap<String, bool>>()
    };
    h.ok("workspace.set", json!({"cwd": h.s("proj")})); // the default: full access
    let (_, first) = live_body(&h, &probe("full"), Value::Null);
    let full = rc("full");
    for k in ["r1", "r2", "r3", "r4", "r7", "w1"] {
        assert!(!full[k], "full access: {k} must be denied: {full:?}");
    }
    assert!(full["r5"] && full["r6"], "full access reads the owner's credential stores (ADR-012): {full:?}");
    assert_eq!(fs::read_to_string(h.dir.join(".init-root-demo/bin/init-root")).unwrap(), "fake, not a real secret\n");
    h.ok("workspace.set", json!({"cwd": h.s("proj"), "mode": "workspace"}));
    live_body(&h, &probe("ws"), first);
    let ws = rc("ws");
    assert!(ws.values().all(|ok| !ok), "workspace mode denies all of them: {ws:?}");
}

/// A live body through the broker, as the kernel spawns one (`replaces`: the body it restarts): the request's id is
/// answered in the fake kernel's log; returns (the `call` outcome, the instance id).
fn live_body(h: &Home, script: &str, replaces: Value) -> (Value, Value) {
    let calls = || fs::read_to_string(h.ih.join("state/kernel/fk.jsonl")).unwrap_or_default().lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["ev"] == "call").collect::<Vec<Value>>();
    let n = calls().len();
    let mut req = body_request(h, script, json!({"PATH": "/usr/bin:/bin"}));
    req["replaces"] = replaces;
    let (tmp, dst) = (h.ih.join("work/fk-call.tmp"), h.ih.join("work/fk-call.json"));
    fs::write(&tmp, json!({"cmd": "spawn", "args": req}).to_string()).unwrap();
    fs::rename(tmp, dst).unwrap();
    wait(20, "the spawn answered", || calls().len() > n);
    let c = calls()[n].clone();
    assert_eq!(c["ok"], true, "{c}");
    let id = c["result"]["instance"].clone();
    (c, id)
}

/// R-UNDO (v0.5.2; docs/user-sessions/v0.5.1-new.md): after INIT is reopened, `/undo` still goes back to before the
/// agent's edits. Every launch snapshots the folder, so the newest snapshot already holds those edits: `/undo` restores
/// the newest snapshot that differs from the folder now, an unchanged folder takes no new snapshot, `/undo list` names
/// each snapshot (id, local time, trigger, folder) and `/undo <id>` restores a chosen one. Each command the Root prints
/// here is run as printed, on an attached client's line path or by the request the TUI sends.
#[cfg(target_os = "macos")]
#[test]
fn a_relaunch_still_undoes_the_agents_edits() {
    let h = home("u");
    h.install(false, "");
    let _d = h.daemon("");
    let proj = fs::canonicalize(h.dir.join("proj")).unwrap();
    let p = proj.display().to_string();
    let mut a = UnixStream::connect(h.rh.join("run/owner.sock")).unwrap();
    writeln!(a, "{}", json!({"client": "undo-test", "replay": false, "t": "attach"})).unwrap();
    a.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut rd = BufReader::new(a.try_clone().unwrap());
    let mut notice = |prefix: &str| loop {
        let mut l = String::new();
        assert!(rd.read_line(&mut l).unwrap() > 0, "the attach connection closed before {prefix:?}");
        let v: Value = serde_json::from_str(&l).unwrap_or_default();
        if v["t"] == "notice" && v["text"].as_str().is_some_and(|t| t.starts_with(prefix)) {
            break v["text"].as_str().unwrap().to_string();
        }
    };
    let mut say = |line: &str| writeln!(a, "{}", json!({"t": "line", "text": line})).unwrap();
    let tick = || h.ok("test.clock", json!({"advance_ms": 60_000}));
    let snap = |r: &Value| r["access"]["snapshot"]["id"].as_str().unwrap().to_string();
    // launch 1: the folder before the agent; the notice no longer claims "/undo restores it"
    let first = snap(&h.ok("workspace.set", json!({"cwd": p})));
    assert_eq!(notice("[root] snapshot "), format!("[root] snapshot {first} of {p} taken at launch (/undo list shows what /undo can restore)"));
    // the agent's edits, then INIT reopened: launch 2's snapshot already holds them
    tick();
    fs::write(proj.join("hello.py"), "print('hello')\n").unwrap();
    fs::write(proj.join("src/a.txt"), "v2\n").unwrap();
    let second = snap(&h.ok("workspace.set", json!({"cwd": p})));
    assert_ne!(second, first);
    // launch 3, nothing changed since: no new snapshot, and it says so
    tick();
    assert_eq!(snap(&h.ok("workspace.set", json!({"cwd": p}))), second);
    assert_eq!(h.snapshots().len(), 2, "an unchanged folder takes no snapshot");
    assert_eq!(notice(&format!("[root] {p} is unchanged")), format!("[root] {p} is unchanged since snapshot {second}, taken 2030-01-10 \
        07:01:00 +07:00 at launch, so no new snapshot was taken (/undo list shows what /undo can restore)"));

    // the TUI's plain /undo: the snapshot from before the agent's edits, not the newest (which equals the folder now)
    let e = h.refused("undo", json!({}));
    assert_eq!((e["code"].as_str(), e["data"]["snapshot"]["id"].as_str()), (Some("root.confirm"), Some(first.as_str())), "{e}");
    assert_eq!(e["message"].as_str().unwrap(), format!("/undo restores {p} to snapshot {first}, taken 2030-01-10 07:00:00 +07:00 at \
        launch: the newest snapshot that differs from the folder now (/undo list shows the others). Everything changed in the folder \
        since then, by you or the agent, is replaced; the folder as it is now is saved first, so this can be undone. Type `confirm` to \
        restore it."));

    // `/undo list`: newest first, one line each, the default marked
    say("/undo list");
    let list = notice("[root] /undo list:");
    assert_eq!(list, format!("[root] /undo list: 2 snapshots of {p}, newest first (INIT keeps the last 5). /undo restores the one marked \
        \"← /undo\"; /undo <id> restores another.\n  {second} · 2030-01-10 07:01:00 +07:00 · at launch · ~/proj · same as the folder now\n  \
        {first} · 2030-01-10 07:00:00 +07:00 · at launch · ~/proj · ← /undo"));
    // `/undo <id>` as listed, then the confirmation exactly as printed
    tick();
    say(&format!("/undo {first}"));
    let ask = notice(&format!("[root] /undo {first} restores"));
    let typed = ask.split("Type ").nth(1).and_then(|t| t.strip_suffix(" to restore it.")).unwrap().to_string();
    assert_eq!(typed, format!("/undo {first} confirm"), "{ask}");
    say(&typed);
    let done = notice("[root] /undo restored");
    let saved = h.snapshots().into_iter().find(|n| *n != first && *n != second).unwrap();
    assert_eq!(done, format!("[root] /undo restored {p} to snapshot {first}, taken 2030-01-10 07:00:00 +07:00 at launch; the folder as it \
        was is saved as snapshot {saved}, so /undo {saved} brings it back (/undo list shows every snapshot)"));
    assert!(!proj.join("hello.py").exists(), "the agent's new file is gone");
    assert_eq!(fs::read_to_string(proj.join("src/a.txt")).unwrap(), "v1\n");
    // v0.5.3: plain /undo never goes back to what an /undo replaced; the request the TUI sends names the line that does
    tick();
    let e = h.refused("undo", json!({}));
    assert_eq!((e["code"].as_str(), e["data"]["reason"].as_str()), (Some("root.not_found"), Some("replaced")), "{e}");
    let m = e["message"].as_str().unwrap();
    assert!(m.starts_with(&format!("/undo has nothing to restore: the snapshots that differ from {p} hold only what an /undo replaced")), "{m}");
    assert!(m.contains(&format!("/undo {saved} brings back the newest of them, taken ")) && m.contains("before an /undo (the folder it replaced)"), "{m}");
    // "/undo <id> brings it back", as printed
    say(&format!("/undo {saved} confirm"));
    let done = notice(&format!("[root] /undo restored {p} to snapshot {saved},"));
    assert_eq!(fs::read_to_string(proj.join("hello.py")).unwrap(), "print('hello')\n");
    let back = h.snapshots().into_iter().find(|n| ![&first, &second, &saved].contains(&n)).unwrap();
    assert!(done.contains(&format!("saved as snapshot {back}, so /undo {back} brings it back")), "{done}");

    // refusals name /undo list: an id that is not a snapshot, one that equals the folder now, a malformed line
    say("/undo nope");
    assert_eq!(notice("[root] /undo: nope"), format!("[root] /undo: nope is not a snapshot of {p}; /undo list shows the ones /undo can restore"));
    say(&format!("/undo {second}"));
    assert_eq!(notice(&format!("[root] /undo {second}:")), format!("[root] /undo {second}: that snapshot is the same as {p} now, so there \
        is nothing to restore; /undo list shows the others"));
    say("/undo a b c");
    assert!(notice("[root] /undo takes").contains("/undo list"));

    // the folder changes between the question and the confirmation: the confirmation asks again, and changes nothing
    tick();
    fs::write(proj.join("notes.txt"), "notes\n").unwrap();
    let third = snap(&h.ok("workspace.set", json!({"cwd": p})));
    tick();
    let e = h.refused("undo", json!({}));
    assert_eq!(e["data"]["snapshot"]["id"].as_str(), Some(saved.as_str()), "before the notes, passing over {back}, which an /undo replaced: {e}");
    assert!(e["message"].as_str().unwrap().contains("passing over 1 that an /undo replaced"), "{e}");
    fs::remove_file(proj.join("notes.txt")).unwrap(); // exact path, written above: the folder is now `saved`'s
    let n = h.snapshots().len();
    let e = h.refused("undo", json!({"confirm": "confirm"}));
    assert_eq!((e["code"].as_str(), e["data"]["snapshot"]["id"].as_str()), (Some("root.confirm"), Some(third.as_str())), "{e}");
    assert!(e["message"].as_str().unwrap().starts_with("The folder changed since /undo asked. /undo restores "), "{e}");
    assert_eq!(h.snapshots().len(), n, "the refused confirmation keeps no snapshot");
    assert!(!proj.join("notes.txt").exists(), "and restores nothing");
    let r = h.ok("undo", json!({"confirm": "confirm"}));
    assert_eq!(r["restored"]["id"].as_str(), Some(third.as_str()), "the second confirmation restores what it named");
    assert!(proj.join("notes.txt").exists());

    // with five kept, restoring the oldest keeps it (the pre_undo save makes six; the next oldest goes)
    let all = h.snapshots();
    assert_eq!(all.len(), 5, "{all:?}");
    assert_eq!(all[0], second, "the save of the last restore made six, so the oldest went: {all:?}");
    tick();
    say(&format!("/undo {second} confirm"));
    notice(&format!("[root] /undo restored {p} to snapshot {second},")); // the earlier restores' notices are passed over
    let left = h.snapshots();
    assert_eq!(left.len(), 5, "{left:?}");
    assert!(left.contains(&second) && !left.contains(&saved), "the restored snapshot is kept, the next oldest pruned: {left:?}");
    assert!(!proj.join("notes.txt").exists());
    // no workspace: `/undo list` says why, as `/undo` does, with the launcher's path (v0.5.3)
    h.ok("workspace.set", json!({"cwd": h.s("")}));
    say("/undo list");
    let launcher = shown::agent(&h.dir.join(".local/bin"), "");
    assert!(notice("[root] /undo has nothing to restore: this session has no workspace").contains(&format!("run {launcher} there")));
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
    let confine = format!("{}, /full off, or sandbox.mode = \"workspace\" in ", shown::agent(&h.dir.join(".local/bin"), "--workspace"));
    assert!(out.contains("access: FULL (default) · workspace ") && out.contains(&confine), "{out}");
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

// ---------------------------------------------------------------------------------------------
// v0.5.3 (REL-052 §7.5 findings 1, 2 and the low finding on /undo after an /undo): `/undo` never loses files

/// An attached owner client: lines on the Root's line path, and the Root's notices by their prefix.
struct Attach(UnixStream, BufReader<UnixStream>);
impl Attach {
    fn new(h: &Home) -> Attach {
        let mut a = UnixStream::connect(h.rh.join("run/owner.sock")).unwrap();
        writeln!(a, "{}", json!({"client": "undo-test", "replay": false, "t": "attach"})).unwrap();
        a.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let r = BufReader::new(a.try_clone().unwrap());
        Attach(a, r)
    }
    fn say(&mut self, line: &str) {
        writeln!(self.0, "{}", json!({"t": "line", "text": line})).unwrap();
    }
    fn notice(&mut self, prefix: &str) -> String {
        loop {
            let mut l = String::new();
            assert!(self.1.read_line(&mut l).unwrap() > 0, "the attach connection closed before {prefix:?}");
            let v: Value = serde_json::from_str(&l).unwrap_or_default();
            if v["t"] == "notice" && v["text"].as_str().is_some_and(|t| t.starts_with(prefix)) {
                return v["text"].as_str().unwrap().to_string();
            }
        }
    }
}

/// Every entry below `dir`, sorted, never following a link: its path, mode, and a file's bytes or a link's target.
fn tree(dir: &Path) -> Vec<String> {
    use std::os::unix::fs::PermissionsExt;
    let (mut out, mut stack) = (Vec::new(), vec![PathBuf::new()]);
    while let Some(rel) = stack.pop() {
        for e in fs::read_dir(dir.join(&rel)).unwrap() {
            let r = rel.join(e.unwrap().file_name());
            let (p, md) = (dir.join(&r), fs::symlink_metadata(dir.join(&r)).unwrap());
            let what = if md.file_type().is_symlink() {
                format!("link {}", fs::read_link(&p).unwrap().display())
            } else if md.is_dir() {
                stack.push(r.clone());
                "dir".into()
            } else {
                format!("file {:?}", String::from_utf8_lossy(&fs::read(&p).unwrap()))
            };
            out.push(format!("{} {:o} {what}", r.display(), md.permissions().mode() & 0o7777));
        }
    }
    out.sort();
    out
}

fn chmod(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
}

/// ROOT_HOME/snapshots holds snapshots only: each tree has its record, and no probe clone, staging tree or orphan clone
/// is left (the v0.5.2 findings: a clone that could not be removed stayed as `.probe` or as an orphan `<id>/`).
fn only_snapshots(h: &Home) {
    let names: Vec<String> = fs::read_dir(h.rh.join("snapshots")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    for n in &names {
        if !n.ends_with(".json") {
            assert!(!n.starts_with('.') && names.contains(&format!("{n}.json")), "{n} is not a snapshot with its record: {names:?}");
        }
    }
}

/// A project as a Go developer has it: a vendored module cache (`vendor/pkg` 0555 with a 0444 file, as `go mod vendor`
/// and the module cache leave them), a nested read-only tree, a read-only folder at the top (a copied release), a link
/// to a file and one to a folder outside the project.
fn read_only_project(h: &Home) -> PathBuf {
    let proj = fs::canonicalize(h.dir.join("proj")).unwrap();
    fs::write(proj.join("main.go"), "package main\n").unwrap();
    fs::write(proj.join("zz.txt"), "zz\n").unwrap();
    for (d, f) in [("vendor/pkg", "mod.go"), ("cache/a/b", "c.txt"), ("rel", "VERSION")] {
        fs::create_dir_all(proj.join(d)).unwrap();
        fs::write(proj.join(d).join(f), format!("{d}/{f}\n")).unwrap();
        chmod(&proj.join(d).join(f), 0o444);
    }
    for d in ["vendor/pkg", "cache/a/b", "cache/a", "rel"] {
        chmod(&proj.join(d), 0o555);
    }
    std::os::unix::fs::symlink("main.go", proj.join("link")).unwrap();
    fs::write(h.dir.join("other/keep.txt"), "outside\n").unwrap();
    std::os::unix::fs::symlink(h.dir.join("other"), proj.join("out")).unwrap();
    proj
}

/// v0.5.3 (REL-052 §7.5 finding 1, FG-1): `/undo` in a folder with read-only folders restores the launch snapshot
/// whole: every file, mode and link as it was, the folder a link points to untouched. v0.5.2 removed the folder's
/// entries before cloning the snapshot back, and EACCES in `vendor/pkg` stopped it with the files already gone. The
/// state it replaced is saved and comes back by its id; nothing but snapshots is left in ROOT_HOME/snapshots.
#[cfg(target_os = "macos")]
#[test]
fn undo_with_read_only_folders_loses_nothing() {
    let h = home("ro");
    h.install(false, "");
    let _d = h.daemon("");
    let proj = read_only_project(&h);
    let before = tree(&proj);
    let first = h.ok("workspace.set", json!({"cwd": proj.to_str().unwrap()}))["access"]["snapshot"]["id"].as_str().unwrap().to_string();
    // the agent's edits
    fs::write(proj.join("main.go"), "package main // v2\n").unwrap();
    fs::write(proj.join("new.txt"), "new\n").unwrap();
    fs::remove_file(proj.join("zz.txt")).unwrap(); // exact path, written above
    let edited = tree(&proj);
    assert_eq!(h.refused("undo", json!({}))["data"]["snapshot"]["id"].as_str(), Some(first.as_str()));
    let v = h.req("undo", json!({"confirm": "confirm"}));
    assert_eq!(v["ok"], true, "the restore completes: {v}");
    assert_eq!(tree(&proj), before, "every file, mode and link of the launch snapshot");
    assert_eq!(fs::read_to_string(h.dir.join("other/keep.txt")).unwrap(), "outside\n", "a link's target is never touched");
    let saved = v["result"]["saved"]["id"].as_str().unwrap().to_string();
    assert_eq!(tree(&h.rh.join("snapshots").join(&saved)), edited, "the state /undo replaced is saved whole");
    only_snapshots(&h);
    // and it comes back by its id, read-only folders and all
    let mut a = Attach::new(&h);
    a.say(&format!("/undo {saved} confirm"));
    let done = a.notice("[root] /undo restored");
    assert!(done.contains(&format!("to snapshot {saved},")), "{done}");
    assert_eq!(tree(&proj), edited);
    only_snapshots(&h);
}

/// v0.5.3 (lane U, class 1): the state an `/undo` replaced is saved in inodes of its own. c4797cc made the entries
/// the restore moved out of the workspace the `pre_undo` snapshot's tree by rename, so a file in it was still the one
/// the zone could reach through a hard link kept outside the folder (full access writes the home) or a descriptor
/// it held open: a write there after the `/undo` changed the saved state behind its recorded digest, and
/// `/undo <id>` brought back content that no longer matched it. Now the save is a clone: those writes reach the
/// moved-out files only, and `/undo <id>` brings back the folder exactly as the `/undo` replaced it.
#[cfg(target_os = "macos")]
#[test]
fn the_state_an_undo_replaced_shares_no_inode_with_the_zone() {
    let h = home("hl");
    h.install(false, "");
    let _d = h.daemon("");
    let proj = fs::canonicalize(h.dir.join("proj")).unwrap();
    h.ok("workspace.set", json!({"cwd": proj.to_str().unwrap()}));
    fs::write(proj.join("src/a.txt"), "the agent's edit\n").unwrap();
    let link = h.dir.join("other/a.link");
    fs::hard_link(proj.join("src/a.txt"), &link).unwrap(); // outside the folder, as full access allows
    let mut open = fs::OpenOptions::new().write(true).open(proj.join("README")).unwrap(); // a descriptor the zone keeps
    let edited = tree(&proj);
    let r = h.ok("undo", json!({"confirm": "confirm"}));
    let saved = r["saved"]["id"].as_str().unwrap().to_string();
    let snap = h.rh.join("snapshots").join(&saved);
    assert_eq!(tree(&snap), edited, "the state /undo replaced is saved whole");
    // the zone writes through its hard link and its descriptor after the /undo
    fs::write(&link, "written after the /undo\n").unwrap();
    open.write_all(b"also written after the /undo\n").unwrap();
    drop(open);
    assert_eq!(tree(&snap), edited, "the saved state is the folder as the /undo replaced it");
    only_snapshots(&h);
    // and `/undo <id>` brings it back as recorded
    let mut a = Attach::new(&h);
    a.say(&format!("/undo {saved} confirm"));
    a.notice(&format!("[root] /undo restored {} to snapshot {saved},", proj.display()));
    assert_eq!(tree(&proj), edited);
    assert_eq!(fs::read_to_string(&link).unwrap(), "written after the /undo\n", "the zone's own file is its own");
    only_snapshots(&h);
}

/// v0.5.3 (lane U, reviewer 2): no owner line goes to the agent while a confirmed `/undo` restores. The busy check
/// comes before the confirmation and the restore lock, and the owner socket serves one thread per client, so in
/// c4797cc a line typed on another attached client during the restore went to the agent at once and could start a
/// turn that wrote the folder while its entries moved. Now it stays queued, the notice says why, and it goes to the
/// agent when the restore ends. (`root.undo.out@1:sleep1500` holds the restore before its first move.)
#[cfg(target_os = "macos")]
#[test]
fn a_line_typed_during_an_undo_waits_for_the_restore() {
    let h = home("hd");
    h.install(true, "");
    let _d = h.daemon_with("", &[("INIT_FAULT", "root.undo.out@1:sleep1500")]);
    wait(20, "running", || h.ok("status", json!({}))["slot"] == "running");
    let proj = fs::canonicalize(h.dir.join("proj")).unwrap();
    h.ok("workspace.set", json!({"cwd": proj.to_str().unwrap()}));
    fs::write(proj.join("src/a.txt"), "v2\n").unwrap();
    let text = "typed during the undo";
    let sent = || fs::read_to_string(h.ih.join("state/kernel/fk.jsonl")).unwrap_or_default().lines().filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .any(|v| v["ev"] == "owner.in" && v["frame"]["text"] == text);
    std::thread::scope(|s| {
        let undo = s.spawn(|| h.req("undo", json!({"confirm": "confirm"})));
        wait(10, "the restore runs", || fs::read_dir(h.rh.join("snapshots")).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with(".undo-")));
        let mut a = Attach::new(&h);
        a.say(text);
        let n = a.notice("[root] /undo is restoring the workspace;");
        assert!(n.ends_with("your message is queued (1) and goes to the agent when it is done"), "{n}");
        assert!(!sent(), "held while the entries move");
        let v = undo.join().unwrap();
        assert_eq!(v["ok"], true, "the restore completes: {v}");
    });
    assert_eq!(fs::read_to_string(proj.join("src/a.txt")).unwrap(), "v1\n");
    wait(10, "the held line goes to the agent", sent);
}

/// v0.5.3 (finding 1): a restore that cannot complete leaves the folder exactly as it was and says why and what the
/// owner can do. Refused before anything moves: a locked file (`chflags uchg`), and a read-only workspace folder itself.
/// Rolled back: a move that fails midway (the fault points `root.undo.out`, while the folder's entries move aside, and
/// `root.undo.in`, while the snapshot's move in; each after one entry has moved). Each time the folder is unchanged,
/// the save of it is dropped and nothing but snapshots is left; then a restore completes.
#[cfg(target_os = "macos")]
#[test]
fn an_undo_that_cannot_finish_changes_nothing() {
    let h = home("rb");
    h.install(false, "");
    let _d = h.daemon_with("", &[("INIT_FAULT", "root.undo.out@2:enospc,root.undo.in@2:enospc")]);
    let proj = fs::canonicalize(h.dir.join("proj")).unwrap();
    let p = proj.display().to_string();
    let first = h.ok("workspace.set", json!({"cwd": p}))["access"]["snapshot"]["id"].as_str().unwrap().to_string();
    let launch = tree(&proj);
    fs::write(proj.join("src/a.txt"), "v2\n").unwrap();
    fs::write(proj.join("b.txt"), "b\n").unwrap();
    fs::write(proj.join("locked.txt"), "l\n").unwrap();
    assert!(Command::new("/usr/bin/chflags").args(["uchg"]).arg(proj.join("locked.txt")).status().unwrap().success());
    let edited = tree(&proj);
    let n = h.snapshots().len();
    let unchanged = |why: &str| {
        assert_eq!(tree(&proj), edited, "{why}: the folder is exactly as it was");
        assert_eq!(h.snapshots().len(), n, "{why}: the save of the folder is dropped");
        only_snapshots(&h);
    };
    // a locked file: refused before anything moves
    let e = h.refused("undo", json!({"confirm": "confirm"}));
    assert_eq!((e["code"].as_str(), e["data"]["reason"].as_str()), (Some("root.mode"), Some("restore_refused")), "{e}");
    let m = e["message"].as_str().unwrap();
    assert!(m.starts_with(&format!("/undo did not restore {p}: {p}/locked.txt is locked")) && m.contains("chflags nouchg")
        && m.ends_with("Nothing changed."), "{m}");
    unchanged("a locked file");
    assert!(Command::new("/usr/bin/chflags").args(["nouchg"]).arg(proj.join("locked.txt")).status().unwrap().success());
    // the workspace folder itself read-only
    chmod(&proj, 0o555);
    let e = h.refused("undo", json!({"confirm": "confirm"}));
    chmod(&proj, 0o755);
    assert_eq!(e["data"]["reason"].as_str(), Some("restore_refused"), "{e}");
    let m = e["message"].as_str().unwrap();
    assert!(m.contains(&format!("{p} itself is read-only (mode 555)")) && m.contains(&format!("chmod u+w {}", shown::sh_word(&p))), "{m}");
    unchanged("a read-only workspace");
    // a move that fails after one entry moved aside, then one that fails after one entry moved in: all put back
    for (point, why) in [("out", "moving the folder's entries aside"), ("in", "moving the snapshot's entries in")] {
        let e = h.refused("undo", json!({"confirm": "confirm"}));
        assert_eq!((e["code"].as_str(), e["data"]["reason"].as_str()), (Some("root.internal"), Some("rolled_back")), "{point}: {e}");
        let m = e["message"].as_str().unwrap();
        assert!(m.starts_with(&format!("/undo could not restore {p}: ")) && m.contains("so every entry was put back and the folder is exactly as it \
            was before /undo"), "{point}: {m}");
        unchanged(why);
    }
    // then the restore completes
    let r = h.ok("undo", json!({"confirm": "confirm"}));
    assert_eq!(r["restored"]["id"].as_str(), Some(first.as_str()));
    assert_eq!(tree(&proj), launch);
    only_snapshots(&h);
}

/// v0.5.3 (REL-052 §7.5 finding 2): the content check never depends on one shared probe clone. A `snapshots/.probe`
/// that a v0.5.2 Root could not remove (it holds a read-only folder) made every check fail with EEXIST, so `/undo`
/// named the newest snapshot (the folder as it is) and the confirmation asked again; an unchanged launch kept an
/// orphan clone. Now every check names the snapshot from before the edits, the confirmation restores it, an unchanged
/// launch keeps no clone, and a probe a crashed call left is removed. One that cannot be removed at all (here an ACL
/// denies the delete, which the Root does not change) stays, and the checks go on around it.
#[cfg(target_os = "macos")]
#[test]
fn the_content_check_survives_a_stuck_probe() {
    let h = home("pr");
    h.install(false, "");
    // v0.5.2's shared probe, which nothing can remove, and a probe of a crashed v0.5.3 call, read-only inside
    let (stuck, left) = (h.rh.join("snapshots/.probe/vendor/pkg"), h.rh.join("snapshots/.probe-1-0/vendor/pkg"));
    for d in [&stuck, &left] {
        fs::create_dir_all(d).unwrap();
        fs::write(d.join("mod.go"), "x\n").unwrap();
        chmod(d, 0o555);
    }
    assert!(Command::new("/bin/chmod").args(["+a", "everyone deny delete"]).arg(stuck.join("mod.go")).status().unwrap().success());
    let _d = h.daemon("");
    let proj = fs::canonicalize(h.dir.join("proj")).unwrap();
    fs::create_dir_all(proj.join("vendor/pkg")).unwrap();
    fs::write(proj.join("vendor/pkg/mod.go"), "package pkg\n").unwrap();
    chmod(&proj.join("vendor/pkg"), 0o555);
    let p = proj.display().to_string();
    let snap = |r: &Value| r["access"]["snapshot"]["id"].as_str().unwrap().to_string();
    let first = snap(&h.ok("workspace.set", json!({"cwd": p})));
    fs::write(proj.join("src/a.txt"), "v2\n").unwrap();
    h.ok("test.clock", json!({"advance_ms": 60_000}));
    let second = snap(&h.ok("workspace.set", json!({"cwd": p})));
    assert_ne!(second, first);
    h.ok("test.clock", json!({"advance_ms": 60_000}));
    assert_eq!(snap(&h.ok("workspace.set", json!({"cwd": p}))), second, "an unchanged launch");
    let names: Vec<String> = fs::read_dir(h.rh.join("snapshots")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    for n in names.iter().filter(|n| !n.starts_with('.') && !n.ends_with(".json")) {
        assert!(names.contains(&format!("{n}.json")), "the unchanged launch left its clone {n}: {names:?}");
    }
    let probes = || -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(h.rh.join("snapshots")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".probe")).collect();
        v.sort();
        v
    };
    for _ in 0..2 {
        let e = h.refused("undo", json!({}));
        assert_eq!(e["data"]["snapshot"]["id"].as_str(), Some(first.as_str()), "every check sees the folder: {e}");
        assert_eq!(probes(), [".probe"], "the crashed call's probe and this check's are gone; only the one nothing can remove is left");
    }
    let mut a = Attach::new(&h);
    a.say("/undo list");
    let list = a.notice("[root] /undo list:");
    assert!(list.contains(&format!("{first} · ")) && list.lines().any(|l| l.contains(&first) && l.ends_with("← /undo")), "{list}");
    assert!(list.lines().any(|l| l.contains(&second) && l.ends_with("same as the folder now")), "{list}");
    let r = h.ok("undo", json!({"confirm": "confirm"}));
    assert_eq!(r["restored"]["id"].as_str(), Some(first.as_str()), "the confirmation restores what the question named, without asking again");
    assert_eq!(fs::read_to_string(proj.join("src/a.txt")).unwrap(), "v1\n");
    assert_eq!(probes(), [".probe"]);
    assert!(Command::new("/bin/chmod").args(["-N"]).arg(stuck.join("mod.go")).status().unwrap().success());
    h.refused("undo", json!({}));
    only_snapshots(&h); // once it can be removed, the next check removes it
}

/// v0.5.3 (REL-052 §7.5, low): plain `/undo` never goes back to a state an `/undo` replaced. After an `/undo` and new
/// edits, v0.5.2's "newest snapshot that differs from the folder" was the `pre_undo` save, so `/undo` brought back the
/// edits the owner had just undone. Now it goes back to the folder as the `/undo` left it; right after an `/undo`,
/// plain `/undo` has nothing to restore and names the `/undo <id>` that brings the replaced state back; those states
/// stay in `/undo list`, and restoring one by its id makes it a default target again.
#[cfg(target_os = "macos")]
#[test]
fn undo_never_defaults_to_a_state_an_undo_replaced() {
    let h = home("rj");
    h.install(false, "");
    let _d = h.daemon("");
    let proj = fs::canonicalize(h.dir.join("proj")).unwrap();
    let p = proj.display().to_string();
    let tick = || h.ok("test.clock", json!({"advance_ms": 60_000}));
    let first = h.ok("workspace.set", json!({"cwd": p}))["access"]["snapshot"]["id"].as_str().unwrap().to_string();
    let s1 = tree(&proj);
    tick();
    fs::write(proj.join("agent.txt"), "the agent's edit\n").unwrap();
    let s2 = tree(&proj);
    tick();
    let second = h.ok("workspace.set", json!({"cwd": p}))["access"]["snapshot"]["id"].as_str().unwrap().to_string();
    // the owner undoes the agent's edit
    tick();
    let r = h.ok("undo", json!({"confirm": "confirm"}));
    assert_eq!(r["restored"]["id"].as_str(), Some(first.as_str()));
    let p1 = r["saved"]["id"].as_str().unwrap().to_string();
    assert_eq!(tree(&proj), s1);
    // right after it, plain /undo has nothing to restore, and names the line that brings the replaced state back
    let e = h.refused("undo", json!({}));
    assert_eq!((e["code"].as_str(), e["data"]["reason"].as_str()), (Some("root.not_found"), Some("replaced")), "{e}");
    assert!(e["message"].as_str().unwrap().contains(&format!("/undo {p1} brings back")), "{e}");
    // new edits, then /undo: back to the folder as the /undo left it, not to the edits it undid
    tick();
    fs::write(proj.join("new.txt"), "new\n").unwrap();
    let e = h.refused("undo", json!({}));
    assert_eq!(e["data"]["snapshot"]["id"].as_str(), Some(first.as_str()), "not {p1} or {second}, which hold the undone edit: {e}");
    let r = h.ok("undo", json!({"confirm": "confirm"}));
    assert_eq!(r["restored"]["id"].as_str(), Some(first.as_str()));
    let p2 = r["saved"]["id"].as_str().unwrap().to_string();
    assert_eq!(tree(&proj), s1);
    let e = h.refused("undo", json!({}));
    assert_eq!(e["data"]["reason"].as_str(), Some("replaced"), "{e}");
    assert!(e["message"].as_str().unwrap().contains(&format!("/undo {p2} brings back")), "the newest replaced state: {e}");
    // they stay listed, and restoring one by its id makes it a default target again
    let mut a = Attach::new(&h);
    a.say("/undo list");
    let list = a.notice("[root] /undo list:");
    for id in [&p1, &p2, &second] {
        assert!(list.lines().any(|l| l.contains(id.as_str()) && !l.ends_with("← /undo")), "{id} listed, not the default: {list}");
    }
    tick();
    a.say(&format!("/undo {p1} confirm"));
    a.notice(&format!("[root] /undo restored {p} to snapshot {p1},"));
    assert_eq!(tree(&proj), s2, "the agent's edit is back, as asked");
    tick();
    fs::write(proj.join("more.txt"), "more\n").unwrap();
    let e = h.refused("undo", json!({}));
    let back = e["data"]["snapshot"]["id"].as_str().unwrap();
    assert!(back == p1 || back == second, "back to the state restored by its id: {e}");
}
