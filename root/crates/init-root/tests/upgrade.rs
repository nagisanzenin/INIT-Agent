//! Lane UPGRADE (v0.5.0): `init-root upgrade` against the real Root daemon in test mode, with the fake kernel.
//! - the replace step, the health probe, the commit and the rollback: the tuple, `lkg`, the Root binary, the kernel
//!   files and the launchers move and come back as the journal says, and the lineage verifies after each step; the
//!   refusals (a running Root, a downgrade, the same release, a binary that is not the release's own) change nothing;
//! - (macOS) nothing inside the zone can run the upgrade or write what it replaces: a body in full access and one in
//!   workspace mode (ADR-012, ADR-009) try to write, rename and unlink the Root's binary, the kernel's binaries,
//!   INIT's launchers, the upgrade's staging, journal and snapshot, and to run a release's `init-root upgrade` with
//!   this ROOT_HOME; every attempt fails and every file keeps its bytes.
//!
//! Run with `cargo test --offline -p init-root --features test-fixtures --test upgrade`.
//!
//! Live-system safety (CONVENTIONS §1): every process is this test's Root daemon, the probe's Root (a child of the
//! `upgrade --probe` process) or one those Roots spawned and hold; the test signals only its own children, by handle.
//! `HOME` is a fake home under `root/target/test-homes/upg/`; nothing outside it is read or written.
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

impl Drop for Home {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.dir); // exact path, made by home()
        }
    }
}

/// A fresh fake HOME `root/target/test-homes/upg/<name>` (short: ROOT_HOME must stay ≤ 80 bytes), with a release
/// directory `rel` (the fake kernel as init-kernel), a toolchain stand-in and INIT's launchers in `.local/bin`.
fn home(name: &str) -> Home {
    let base = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap().join("test-homes/upg");
    let dir = base.join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap(); // exact path, made by an earlier run of this test
    }
    for d in ["rel", "tc/bin", "proj", "other", ".local/bin"] {
        fs::create_dir_all(dir.join(d)).unwrap();
    }
    fs::copy(FK, dir.join("rel/init-kernel")).unwrap();
    for (n, text) in [("init-agent", "#!/bin/bash\nINIT_ROOT_HOME_DEFAULT=\"x\"\n"), ("init-tui", "tui v1\n"), ("init", "#!/bin/bash\nINIT_AGENT_SHIM=1\n")] {
        fs::write(dir.join(".local/bin").join(n), text).unwrap();
    }
    Home { rh: dir.join("rh"), ih: dir.join("ih"), dir }
}

fn wait(secs: u64, what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(secs), "not within {secs} s: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A file's bytes as a short id (or ABSENT), to compare two reads of the same path: FNV-1a over 128 bits, a test's
/// equality check, not a security hash.
fn sha(p: &Path) -> String {
    fs::read(p).map(|b| format!("{:x}", fnv(&b))).unwrap_or_else(|_| "ABSENT".into())
}

fn fnv(b: &[u8]) -> u128 {
    let mut h: u128 = 0x6c62272e07bb014262b821756295c58d;
    for x in b {
        h ^= u128::from(*x);
        h = h.wrapping_mul(0x0000000001000000000000000000013B);
    }
    h
}

impl Home {
    fn cmd(&self, bin: &Path) -> Command {
        let info: Value = fs::read_to_string(self.rh.join("install.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let s = |k: &str| info[k].as_str().unwrap_or("").to_string();
        let mut c = Command::new(bin);
        c.env_clear().env("HOME", &self.dir).env("INIT_ROOT_HOME", &self.rh).env("INIT_HOME", &self.ih).env("PATH", "/usr/bin:/bin")
            .env("INIT_TEST_FK", "").env("INIT_TEST_TOOLCHAIN", s("toolchain")).env("INIT_TEST_DEVDIR", s("devdir"))
            .env("INIT_TEST_HOME_REAL", &self.dir).env("INIT_ROOT_TEST_NOW", "2030-01-10T00:00:00Z");
        c
    }
    fn run(&self, bin: &Path, args: &[&str]) -> Output {
        self.cmd(bin).args(args).stdin(Stdio::null()).output().unwrap()
    }
    fn install(&self) {
        fs::create_dir_all(&self.rh).unwrap();
        fs::write(self.rh.join("charter.toml"), format!("schema_version = 1\n[purpose]\ntext = \"UPGRADE test home.\"\n[autonomy]\n\
            max_level = 4\n[quotas]\ndisk_min_free_gib = 0\n{ROOT_CFG}\n")).unwrap();
        let (rel, tc, bin) = (self.dir.join("rel"), self.dir.join("tc"), self.dir.join(".local/bin"));
        let o = self.run(Path::new(BIN), &["install", "--no-service", "--release", rel.to_str().unwrap(), "--toolchain", tc.to_str().unwrap(),
            "--bin-dir", bin.to_str().unwrap()]);
        assert_eq!(o.status.code(), Some(0), "install: {}", String::from_utf8_lossy(&o.stderr));
    }
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
    fn daemon(&self) -> Daemon {
        let c = self.cmd(Path::new(BIN)).arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let d = Daemon(c, self.rh.clone());
        wait(10, "owner.sock", || UnixStream::connect(self.rh.join("run/owner.sock")).is_ok());
        wait(20, "running", || self.ok("status", json!({}))["slot"] == "running");
        d
    }
    fn s(&self, rel: &str) -> String {
        self.dir.join(rel).to_string_lossy().into_owned()
    }
    fn journal(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.rh.join("state/upgrade.json")).unwrap()).unwrap()
    }
    /// The current and lkg tuples, folded from the lineage as the Root folds them.
    fn ptrs(&self) -> (String, String) {
        let (mut cur, mut lkg) = (String::new(), String::new());
        for l in fs::read_to_string(self.rh.join("lineage/lineage.jsonl")).unwrap().lines() {
            let r: Value = serde_json::from_str(l).unwrap();
            let s = |k: &str| r["data"][k].as_str().unwrap_or("").to_string();
            match r["kind"].as_str() {
                Some("tuple") => cur = s("id"),
                Some("rollback") => cur = s("to"),
                Some("lkg") => lkg = s("tuple"),
                _ => {}
            }
        }
        (cur, lkg)
    }
    fn verify(&self) {
        let o = self.run(Path::new(BIN), &["audit", "verify", "--full"]);
        assert_eq!(o.status.code(), Some(0), "audit verify --full: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    }
    /// A release directory `name`: this test's init-root, the fake kernel with `marker` appended (a different binary
    /// that runs the same) or `kernel` instead, and VERSION.
    fn release(&self, name: &str, version: &str, marker: &str, kernel: Option<&str>) -> PathBuf {
        let d = self.dir.join(name);
        fs::create_dir_all(&d).unwrap();
        fs::copy(BIN, d.join("init-root")).unwrap();
        match kernel {
            Some(text) => fs::write(d.join("init-kernel"), text).unwrap(),
            None => {
                let mut k = fs::read(FK).unwrap();
                k.extend_from_slice(format!("\n# {marker}\n").as_bytes());
                fs::write(d.join("init-kernel"), k).unwrap();
            }
        }
        fs::set_permissions(d.join("init-kernel"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        fs::write(d.join("VERSION"), format!("{version}\n")).unwrap();
        d
    }
    fn upgrade(&self, rel: &Path, extra: &[&str]) -> Output {
        let mut a = vec!["upgrade", "--release", rel.to_str().unwrap()];
        a.extend_from_slice(extra);
        self.run(&rel.join("init-root"), &a)
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

fn code(o: &Output) -> Option<i32> {
    o.status.code()
}

fn err(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

#[test]
fn replace_probe_commit_then_a_failed_probe_rolls_back() {
    let h = home("r");
    h.install();
    { let _d = h.daemon(); } // the seedless genesis tuple forms at the first start
    let (genesis, _) = h.ptrs();
    let launchers = |h: &Home| ["init-agent", "init-tui", "init"].map(|n| sha(&h.dir.join(".local/bin").join(n)));
    let (root0, kernel0, launch0) = (sha(&h.rh.join("bin/init-root")), sha(&h.ih.join("kernel/bin/init-kernel")), launchers(&h));
    let keys0 = sha(&h.rh.join("keys/lineage.pub"));
    let charter0 = sha(&h.rh.join("charter.toml"));

    // refusals change nothing
    let r1 = h.release("rel1", "9.1.0-test", "release one", None);
    {
        let _d = h.daemon();
        let o = h.upgrade(&r1, &[]);
        assert_eq!(code(&o), Some(1), "{}", err(&o));
        assert!(err(&o).contains("the Root is running"), "{}", err(&o));
    }
    let old = h.release("rel-old", "0.0.1", "an old release", None);
    let o = h.upgrade(&old, &[]);
    assert_eq!(code(&o), Some(3), "a downgrade: {}", err(&o));
    let mut other = fs::read(BIN).unwrap();
    other.extend_from_slice(b"\n# not the release's binary\n");
    fs::write(h.dir.join("other/init-root"), other).unwrap();
    fs::set_permissions(h.dir.join("other/init-root"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let o = h.run(&h.dir.join("other/init-root"), &["upgrade", "--release", r1.to_str().unwrap()]);
    assert_eq!(code(&o), Some(1), "{}", err(&o));
    assert!(err(&o).contains("the release's own"), "{}", err(&o));
    assert!(!h.rh.join("state/upgrade.json").exists(), "a refusal writes no journal");
    assert_eq!((sha(&h.ih.join("kernel/bin/init-kernel")), h.ptrs().0.clone()), (kernel0.clone(), genesis.clone()));

    // the replace step: a new tuple (the new kernel, the same body and ext), current moves, lkg stays
    let o = h.upgrade(&r1, &["--bin-dir", h.dir.join(".local/bin").to_str().unwrap()]);
    assert_eq!(code(&o), Some(0), "{}", err(&o));
    let j = h.journal();
    assert_eq!(j["phase"], "replaced", "{j}");
    let (cur, lkg) = h.ptrs();
    assert_ne!(cur, genesis);
    assert_eq!(lkg, genesis, "lkg moves only at the commit");
    assert_eq!(sha(&h.ih.join("kernel/bin/init-kernel")), sha(&r1.join("init-kernel")));
    let snap = h.rh.join("state/upgrade").join(j["id"].as_str().unwrap());
    assert_eq!(sha(&snap.join("ih/kernel/bin/init-kernel")), kernel0, "the snapshot holds the old kernel");
    assert_eq!(sha(&snap.join("bin/init")), launch0[2], "and INIT's launchers");
    let o = h.upgrade(&r1, &[]);
    assert_eq!(code(&o), Some(1), "a pending upgrade blocks another: {}", err(&o));
    h.verify();
    // the probe: the new Root brings the zone up on the new tuple, then stops it
    let o = h.run(&h.rh.join("bin/init-root"), &["upgrade", "--probe", "--timeout", "40"]);
    assert_eq!(code(&o), Some(0), "probe: {}", err(&o));
    let p: Value = serde_json::from_str(String::from_utf8_lossy(&o.stdout).lines().last().unwrap()).unwrap();
    assert_eq!((p["ok"].as_bool(), p["tuple"].as_str(), p["verify"].as_i64()), (Some(true), Some(cur.as_str()), Some(0)), "{p}");
    let o = h.run(&h.rh.join("bin/init-root"), &["upgrade", "--commit"]);
    assert_eq!(code(&o), Some(0), "commit: {}", err(&o));
    assert_eq!(h.ptrs(), (cur.clone(), cur.clone()), "lkg is the upgraded tuple");
    assert_eq!(h.journal()["phase"], "committed");
    assert!(!snap.exists(), "the snapshot goes at the commit");
    assert_eq!(h.upgrade(&r1, &[]).status.code(), Some(4), "the same release again: nothing to do");
    h.verify();

    // a release whose kernel does not run: the probe fails, the rollback restores everything
    let (root1, kernel1, launch1) = (sha(&h.rh.join("bin/init-root")), sha(&h.ih.join("kernel/bin/init-kernel")), launchers(&h));
    let r2 = h.release("rel2", "9.2.0-test", "", Some("#!/bin/sh\necho 'a deliberately broken kernel' >&2\nexit 1\n"));
    let o = h.upgrade(&r2, &[]);
    assert_eq!(code(&o), Some(0), "{}", err(&o));
    fs::write(h.dir.join(".local/bin/init-agent"), "#!/bin/bash\nINIT_ROOT_HOME_DEFAULT=\"new\"\n").unwrap(); // install.sh's step
    fs::remove_file(h.dir.join(".local/bin/init")).unwrap();
    let o = h.run(&h.rh.join("bin/init-root"), &["upgrade", "--probe", "--timeout", "20"]);
    assert_eq!(code(&o), Some(1), "the probe must fail: {}", err(&o));
    let o = h.run(&r2.join("init-root"), &["upgrade", "--rollback"]);
    assert_eq!(code(&o), Some(0), "rollback: {}", err(&o));
    assert_eq!(h.ptrs().0, cur, "the tuple from before");
    assert_eq!((sha(&h.rh.join("bin/init-root")), sha(&h.ih.join("kernel/bin/init-kernel")), launchers(&h)), (root1, kernel1, launch1),
        "the Root, the kernel and the launchers from before");
    assert_eq!(h.journal()["phase"], "rolled_back");
    let last_rb = fs::read_to_string(h.rh.join("lineage/lineage.jsonl")).unwrap().lines().rev()
        .map(|l| serde_json::from_str::<Value>(l).unwrap()).find(|r| r["kind"] == "rollback").expect("a rollback record");
    assert_eq!(last_rb["data"]["to"], cur.as_str(), "{last_rb}");
    h.verify();
    assert_eq!((sha(&h.rh.join("keys/lineage.pub")), sha(&h.rh.join("charter.toml"))), (keys0, charter0), "keys and charter untouched");
    assert_ne!(root0, "ABSENT");
    // the old install still runs
    let _d = h.daemon();
    assert_eq!(h.ok("status", json!({}))["tuples"]["current"], cur.as_str());
}

/// The kernel's spawn request for a body (`spawn_args` of the fake kernel), running `/bin/sh -c script`.
fn body_request(h: &Home, script: &str) -> Value {
    let i = fs::canonicalize(&h.ih).unwrap().to_string_lossy().into_owned();
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
    json!({"args": ["-c", script], "cwd": format!("{i}/work"), "env": {"PATH": "/usr/bin:/bin", "INIT_ACCESS": "none", "INIT_WORKSPACE": "/"},
        "kind": "body", "lane": "live", "params": p, "profile": "body", "program": "/bin/sh"})
}

/// The Root-protected files an upgrade writes, with their bytes (or ABSENT).
fn protected(h: &Home) -> Vec<(String, String)> {
    let mut v: Vec<PathBuf> = vec![h.rh.join("bin/init-root"), h.ih.join("kernel/bin/init-kernel"), h.ih.join("kernel/bin/initctl"),
        h.ih.join("kernel/bin/init-evil"), h.rh.join("cache/get.sh"), h.rh.join("cache/upgrade/evil"), h.rh.join("state/upgrade.json"),
        h.rh.join("state/upgrade")];
    v.extend(["init-agent", "init-tui", "init", "init-agent.moved"].map(|n| h.dir.join(".local/bin").join(n)));
    v.into_iter().map(|p| (p.display().to_string(), sha(&p))).collect()
}

/// ADR-012 and ADR-009 (macOS): a body in full access and one in workspace mode try to write, rename and unlink every
/// file the upgrade replaces, and to run the upgrade itself with this ROOT_HOME; all of it fails.
#[cfg(target_os = "macos")]
#[test]
fn the_zone_cannot_run_the_upgrade_or_write_what_it_replaces() {
    let h = home("z");
    h.install();
    fs::write(h.rh.join("cache/get.sh"), "#!/bin/sh\n# the kept get.sh\n").unwrap();
    fs::create_dir_all(h.rh.join("cache/upgrade")).unwrap();
    let rel = h.release("rel1", "9.1.0-test", "release one", None);
    // a copy of a release the zone can read and run (the fake home's other/ is outside every deny list)
    fs::create_dir_all(h.dir.join("other/rel")).unwrap();
    for f in ["init-root", "init-kernel", "VERSION"] {
        fs::copy(rel.join(f), h.dir.join("other/rel").join(f)).unwrap();
    }
    let _d = h.daemon();
    let before = protected(&h);
    let i = fs::canonicalize(&h.ih).unwrap().to_string_lossy().into_owned();
    let (rh, bin, orel) = (fs::canonicalize(&h.rh).unwrap().display().to_string(), h.s(".local/bin"), h.s("other/rel"));
    for mode in ["full", "workspace"] {
        if mode == "workspace" {
            let r = h.ok("workspace.set", json!({"cwd": h.s("proj"), "mode": "workspace"}));
            assert_eq!(r["access"]["access"], "workspace", "{r}");
        } else {
            let r = h.ok("workspace.set", json!({"cwd": h.s("proj")}));
            assert_eq!(r["access"]["access"], "full", "{r}");
        }
        let out = format!("{i}/work/attack-{mode}.txt");
        let script = format!(
            "t() {{ n=$1; shift; ( \"$@\" ) >/dev/null 2>&1; echo \"$n $?\" >> '{out}'; }}; \
             echo \"access $INIT_ACCESS\" > '{out}'; \
             t write_root_bin sh -c 'printf x >> {rh}/bin/init-root'; \
             t rename_root_bin mv {rh}/bin/init-root {rh}/bin/x; \
             t write_kernel_bin sh -c 'printf x >> {i}/kernel/bin/init-kernel'; \
             t new_kernel_file sh -c 'printf x > {i}/kernel/bin/init-evil'; \
             t unlink_initctl rm {i}/kernel/bin/initctl; \
             t write_launcher sh -c 'printf x >> {bin}/init-agent'; \
             t write_tui sh -c 'printf x >> {bin}/init-tui'; \
             t write_init sh -c 'printf x >> {bin}/init'; \
             t rename_launcher mv {bin}/init-agent {bin}/init-agent.moved; \
             t unlink_tui rm {bin}/init-tui; \
             t stage_dir mkdir {rh}/cache/upgrade/evil; \
             t get_sh sh -c 'printf x >> {rh}/cache/get.sh'; \
             t journal sh -c 'printf x > {rh}/state/upgrade.json'; \
             t snapshot_dir mkdir {rh}/state/upgrade; \
             t replace_step env INIT_ROOT_HOME={rh} {orel}/init-root upgrade --release {orel} --bin-dir {bin}; \
             echo end >> '{out}'"
        );
        let (tmp, dst) = (h.ih.join("work/fk-call.tmp"), h.ih.join("work/fk-call.json"));
        wait(20, "the previous call taken", || !dst.exists());
        fs::write(&tmp, json!({"cmd": "spawn", "args": body_request(&h, &script)}).to_string()).unwrap();
        fs::rename(tmp, dst).unwrap();
        wait(60, "the body ran", || fs::read_to_string(&out).is_ok_and(|s| s.ends_with("end\n")));
        let rec = fs::read_to_string(&out).unwrap();
        let want_access = if mode == "full" { "access full" } else { "access workspace" };
        assert!(rec.starts_with(want_access), "{mode}: {rec}");
        let tries: Vec<&str> = rec.lines().filter(|l| !l.starts_with("access ") && *l != "end").collect();
        assert_eq!(tries.len(), 15, "{rec}");
        for l in &tries {
            assert!(!l.ends_with(" 0"), "{mode}: an attempt succeeded inside the zone: {l}\n{rec}");
        }
        assert_eq!(protected(&h), before, "{mode}: a protected file changed");
    }
    assert!(!h.rh.join("state/upgrade.json").exists(), "no upgrade ran");
}
