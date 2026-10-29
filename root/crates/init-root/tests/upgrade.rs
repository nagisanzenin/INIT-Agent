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
            if l.trim_matches('\0').is_empty() {
                continue; // a torn tail that the SIGKILL of the test's daemon left (NUL bytes): the Root trims it at its next open
            }
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
    /// The installed Root's `upgrade <verb>`, with `INIT_FAULT` set when `fault` is not empty (root.md §4.7).
    fn verb(&self, verb: &str, fault: &str) -> Output {
        let mut c = self.cmd(&self.rh.join("bin/init-root"));
        if !fault.is_empty() {
            c.env("INIT_FAULT", fault);
        }
        c.args(["upgrade", verb]).stdin(Stdio::null()).output().unwrap()
    }
    fn probe(&self) {
        let o = self.run(&self.rh.join("bin/init-root"), &["upgrade", "--probe", "--timeout", "40"]);
        assert_eq!(code(&o), Some(0), "probe: {}", err(&o));
    }
    fn records(&self) -> Vec<Value> {
        fs::read_to_string(self.rh.join("lineage/lineage.jsonl")).unwrap().lines().filter(|l| !l.trim_matches('\0').is_empty()) // a torn tail
            .map(|l| serde_json::from_str(l).unwrap()).collect()
    }
    fn last(&self, kind: &str) -> Value {
        self.records().into_iter().rev().find(|r| r["kind"] == kind).unwrap_or_default()
    }
    fn tuple(&self, id: &str) -> Value {
        serde_json::from_str(&fs::read_to_string(self.rh.join("store/tuples").join(format!("{id}.json"))).unwrap()).unwrap()
    }
    /// Records through the running Root's own append path (test builds: `test.append`).
    fn append(&self, records: Value) {
        self.ok("test.append", json!({"records": records}));
    }
    /// An owner line on `owner.sock` (a Root line such as `/rollback twin`).
    fn say(&self, text: &str) {
        let mut s = UnixStream::connect(self.rh.join("run/owner.sock")).unwrap();
        writeln!(s, "{}", json!({"t": "line", "text": text})).unwrap();
        s.flush().unwrap();
        std::thread::sleep(Duration::from_millis(300));
    }
    /// Every file and directory an upgrade replaces, with its bytes (directories by their listing).
    fn live(&self) -> Vec<(String, String)> {
        let mut v = vec![self.rh.join("bin/init-root"), self.rh.join("install.json"), self.ih.join("kernel/bin/init-kernel"), self.ih.join("kernel/bin/initctl")];
        v.extend(["init-agent", "init-tui", "init"].map(|n| self.dir.join(".local/bin").join(n)));
        let mut out: Vec<(String, String)> = v.iter().map(|p| (p.display().to_string(), sha(p))).collect();
        let list = |d: &Path| fs::read_dir(d).map(|r| r.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect::<std::collections::BTreeSet<_>>())
            .map(|s| format!("{s:?}")).unwrap_or_else(|_| "ABSENT".into());
        out.extend(["bin", "docs", "wit", "src"].map(|d| (format!("kernel/{d}"), list(&self.ih.join("kernel").join(d)))));
        out
    }
}

/// The kernel source `src` of a release (or of the install): one commit on refs/heads/release, `name` its message.
fn source_bundle(h: &Home, rel: &Path, version: &str, name: &str) -> String {
    let git = |dir: &Path, args: &[&str]| {
        let o = Command::new("/usr/bin/git").env_clear().env("PATH", "/usr/bin:/bin").env("HOME", &h.dir).env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null").current_dir(dir).args(args).output().unwrap();
        assert!(o.status.success(), "git {args:?}: {}", err(&o));
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    let src = h.dir.join(format!("ksrc-{version}"));
    fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "-q", "-b", "release"]);
    git(&src, &["-c", "user.name=t", "-c", "user.email=t@t.invalid", "commit", "-q", "--allow-empty", "-m", name]);
    fs::create_dir_all(rel.join("src")).unwrap();
    git(&src, &["bundle", "create", "-q", rel.join(format!("src/{version}.bundle")).to_str().unwrap(), "refs/heads/release"]);
    git(&src, &["rev-parse", "HEAD"])
}

/// A ref of the Root's `lineage/kernel.git`.
fn kref(h: &Home, r: &str) -> String {
    let o = Command::new("/usr/bin/git").env_clear().env("PATH", "/usr/bin:/bin").env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null")
        .arg(format!("--git-dir={}", h.ih.join("lineage/kernel.git").display())).args(["rev-parse", "-q", "--verify", r]).output().unwrap();
    String::from_utf8_lossy(&o.stdout).trim().to_string()
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
    let last_rb = fs::read_to_string(h.rh.join("lineage/lineage.jsonl")).unwrap().lines().rev().filter(|l| !l.trim_matches('\0').is_empty())
        .map(|l| serde_json::from_str::<Value>(l).unwrap()).find(|r| r["kind"] == "rollback").expect("a rollback record");
    assert_eq!(last_rb["data"]["to"], cur.as_str(), "{last_rb}");
    h.verify();
    assert_eq!((sha(&h.rh.join("keys/lineage.pub")), sha(&h.rh.join("charter.toml"))), (keys0, charter0), "keys and charter untouched");
    assert_ne!(root0, "ABSENT");
    // the old install still runs
    let _d = h.daemon();
    assert_eq!(h.ok("status", json!({}))["tuples"]["current"], cur.as_str());
}

/// The post-release review of v0.5.0: `init-root upgrade --release` read the release's kernel source with
/// `git bundle list-heads` from the caller's working directory, so an upgrade started inside a repository read that
/// repository's `.git/config`, which the agent can write in full access (ADR-012). Every Root git call runs from `/`
/// now. Here the caller stands in a repository whose config git cannot parse; the upgrade reads the source bundle.
#[test]
fn an_upgrade_started_inside_a_repository_reads_none_of_its_git_config() {
    let h = home("g");
    h.install();
    { let _d = h.daemon(); } // the genesis tuple
    let rel = h.release("rel1", "9.1.0-test", "release one", None);
    let git = |dir: &Path, args: &[&str]| {
        let o = Command::new("/usr/bin/git").env_clear().env("PATH", "/usr/bin:/bin").env("HOME", &h.dir).env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null").current_dir(dir).args(args).output().unwrap();
        assert!(o.status.success(), "git {args:?}: {}", err(&o));
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    // the release's kernel source: one commit on refs/heads/release
    let src = h.dir.join("ksrc");
    fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "-q", "-b", "release"]);
    git(&src, &["-c", "user.name=t", "-c", "user.email=t@t.invalid", "commit", "-q", "--allow-empty", "-m", "kernel source"]);
    let head = git(&src, &["rev-parse", "HEAD"]);
    fs::create_dir_all(rel.join("src")).unwrap();
    git(&src, &["bundle", "create", "-q", rel.join("src/9.1.0-test.bundle").to_str().unwrap(), "refs/heads/release"]);
    // the agent's repository, where the owner runs the upgrade (its config would name core.fsmonitor or a pager the same way)
    let repo = h.dir.join("proj");
    git(&repo, &["init", "-q"]);
    fs::write(repo.join(".git/config"), "[core\n\tfsmonitor = /bin/false\n").unwrap();
    let o = h.cmd(&rel.join("init-root")).args(["upgrade", "--release", rel.to_str().unwrap()]).current_dir(&repo).stdin(Stdio::null())
        .output().unwrap();
    assert_eq!(code(&o), Some(0), "{}", err(&o));
    let j = h.journal();
    assert_eq!((j["phase"].as_str(), j["to"]["main"].as_str()), (Some("replaced"), Some(head.as_str())), "{j}");
    h.verify();
}

/// v0.5.1 (docs/user-sessions/v0.5.0-returning.md, operator note 3): a panic after a committed upgrade resumes on the
/// upgraded kernel. Resume rolls back to audited-good (root.md §3.9.5), and before the fix `audited` still named the
/// tuple from before the upgrade, so the resume reinstalled the old kernel under the new Root. The commit now moves the
/// rollback anchors onto the release's kernel, each with the body and ext it had (§3.7.2).
#[test]
fn a_panic_after_a_committed_upgrade_resumes_on_the_upgraded_kernel() {
    let h = home("pr");
    h.install();
    { let _d = h.daemon(); } // the seedless genesis tuple: current, lkg and audited
    let (genesis, _) = h.ptrs();
    let tuple = |id: &str| -> Value {
        serde_json::from_str(&fs::read_to_string(h.rh.join("store/tuples").join(format!("{id}.json"))).unwrap()).unwrap()
    };
    let r1 = h.release("rel1", "9.1.0-test", "release one", None);
    let o = h.upgrade(&r1, &["--bin-dir", h.dir.join(".local/bin").to_str().unwrap()]);
    assert_eq!(code(&o), Some(0), "{}", err(&o));
    let o = h.run(&h.rh.join("bin/init-root"), &["upgrade", "--probe", "--timeout", "40"]);
    assert_eq!(code(&o), Some(0), "probe: {}", err(&o));
    let o = h.run(&h.rh.join("bin/init-root"), &["upgrade", "--commit"]);
    assert_eq!(code(&o), Some(0), "commit: {}", err(&o));
    let (to, lkg) = h.ptrs();
    assert!(to != genesis && lkg == to, "the upgraded tuple is current and lkg");
    let (new_kernel, upgraded) = (sha(&r1.join("init-kernel")), tuple(&to));
    let _d = h.daemon();
    let st = h.ok("status", json!({}));
    let audited = st["tuples"]["audited"].as_str().unwrap_or_default().to_string();
    assert_eq!(tuple(&audited)["kernel"], upgraded["kernel"], "the commit moved audited-good onto the release's kernel: {st}");
    assert_eq!((&tuple(&audited)["body"], &tuple(&audited)["ext"]), (&tuple(&genesis)["body"], &tuple(&genesis)["ext"]),
        "audited-good keeps the body and ext it had");
    h.ok("panic", json!({}));
    wait(10, "panicked", || h.ok("status", json!({}))["slot"] == "panicked");
    let r = h.ok("resume", json!({}));
    wait(20, "running after the resume", || h.ok("status", json!({}))["slot"] == "running");
    let st = h.ok("status", json!({}));
    let cur = st["tuples"]["current"].as_str().unwrap_or_default().to_string();
    assert_eq!(tuple(&cur)["kernel"], upgraded["kernel"], "the resume runs the release's kernel, not the one from before: {r} {st}");
    assert_ne!(cur, genesis, "{st}");
    assert_eq!(sha(&h.ih.join("kernel/bin/init-kernel")), new_kernel, "kernel/bin holds the release's kernel after the resume");
    let rb = fs::read_to_string(h.rh.join("lineage/lineage.jsonl")).unwrap().lines().rev().filter(|l| !l.trim_matches('\0').is_empty())
        .map(|l| serde_json::from_str::<Value>(l).unwrap()).find(|r| r["kind"] == "rollback").expect("the resume's rollback record");
    assert_eq!((rb["data"]["reason"].as_str(), rb["data"]["target"].as_str(), rb["data"]["to"].as_str()), (Some("panic"), Some("audited"), Some(cur.as_str())), "{rb}");
    drop(_d);
    h.verify();
}

/// R-UNDO (v0.5.2, the post-release review of v0.5.1): a Root started while an upgrade is pending (a login after a
/// terminal closed during the probe) never runs the zone on the upgrade's tuple with the old kernel's anchors. In
/// `replaced` the start rolls the upgrade back and exits (the service then starts the old Root); in `committing` it
/// finishes the commit and runs. The probe's own Root still runs in `replaced`. Also: install.sh's replace step
/// (`--by-installer`) prints no "next" (the bare step does), and `--commit` installs the release's get.sh into
/// ROOT_HOME/cache, also when the owner commits by hand.
#[test]
fn a_start_mid_upgrade_never_runs_the_zone_on_mixed_anchors() {
    let h = home("ms");
    h.install();
    { let _d = h.daemon(); }
    let (genesis, _) = h.ptrs();
    let kernel0 = sha(&h.ih.join("kernel/bin/init-kernel"));
    fs::create_dir_all(h.rh.join("cache")).unwrap();
    fs::write(h.rh.join("cache/get.sh"), "#!/bin/sh\n# the old release's get.sh\n").unwrap();
    let r1 = h.release("rel1", "9.1.0-test", "release one", None);
    fs::create_dir_all(r1.join("scripts")).unwrap();
    fs::write(r1.join("scripts/get.sh"), "#!/bin/sh\n# the new release's get.sh\n").unwrap();
    let bin = h.dir.join(".local/bin");
    let fk_lines = || fs::read_to_string(h.ih.join("state/kernel/fk.jsonl")).map_or(0, |t| t.lines().count());
    // a start that must exit by itself (one that runs the zone would not)
    let start = |h: &Home| {
        let mut c = h.cmd(Path::new(BIN)).arg("start").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let t = Instant::now();
        while c.try_wait().unwrap().is_none() {
            if t.elapsed() > Duration::from_secs(30) {
                let _ = c.kill(); // our own child, by its handle
                let _ = c.wait();
                panic!("start ran on: it did not stop for the pending upgrade");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        c.wait_with_output().unwrap()
    };

    // install.sh's replace step: no "next"
    let o = h.upgrade(&r1, &["--bin-dir", bin.to_str().unwrap(), "--by-installer"]);
    assert_eq!(code(&o), Some(0), "{}", err(&o));
    let out: Value = serde_json::from_str(String::from_utf8_lossy(&o.stdout).lines().last().unwrap()).unwrap();
    assert!(out.get("next").is_none() && out["upgrade"].is_string(), "{out}");
    assert_eq!(h.journal()["phase"], "replaced");
    let (n0, (upgraded, _)) = (fk_lines(), h.ptrs());
    assert_ne!(upgraded, genesis);
    // a start in `replaced`: rolled back, the zone never ran, and it says what happened and what to run
    let o = start(&h);
    assert_eq!(code(&o), Some(75), "{}", err(&o));
    let e = String::from_utf8_lossy(&o.stderr).into_owned();
    for want in ["it was cut off after its replace step, before its health probe and commit, so this start rolled it back",
        "Start the Root again: run this in a terminal of its own and leave it open: ", "start --foreground",
        format!("run the upgrade again: {}.", shown::agent(&bin, "upgrade")).as_str()] {
        assert!(e.contains(want), "{want:?} in {e}");
    }
    assert_eq!(h.journal()["phase"], "rolled_back");
    assert_eq!((h.ptrs().0, sha(&h.ih.join("kernel/bin/init-kernel"))), (genesis.clone(), kernel0.clone()), "the install from before");
    assert_eq!(fk_lines(), n0, "no kernel ran on the half-upgraded install");
    assert!(fs::read_to_string(h.rh.join("alerts.jsonl")).unwrap().contains("rolled it back"), "an alert says so");
    assert!(fs::read_to_string(h.rh.join("cache/get.sh")).unwrap().contains("the old release's"), "a rollback keeps the old get.sh");
    { let _d = h.daemon(); } // the old install runs as before

    // the bare replace step names the next steps; the probe's own Root runs in `replaced`
    let o = h.upgrade(&r1, &["--bin-dir", bin.to_str().unwrap()]);
    assert_eq!(code(&o), Some(0), "{}", err(&o));
    let out: Value = serde_json::from_str(String::from_utf8_lossy(&o.stdout).lines().last().unwrap()).unwrap();
    assert!(out["next"].as_str().is_some_and(|n| n.ends_with("upgrade --probe, then --commit (or --rollback)")), "{out}");
    h.probe();
    // a commit cut off after `committing`: get.sh is already the release's, and a start finishes the commit, then runs
    let o = h.verb("--commit", "root.upgrade.after_snapshot_remove:exit7");
    assert_eq!(code(&o), Some(7), "{}", err(&o));
    assert_eq!(h.journal()["phase"], "committing");
    assert_eq!(fs::read_to_string(h.rh.join("cache/get.sh")).unwrap(), "#!/bin/sh\n# the new release's get.sh\n", "the commit installs it");
    let to = h.journal()["to"]["tuple"].as_str().unwrap().to_string();
    let _d = h.daemon();
    assert_eq!(h.journal()["phase"], "committed", "the start finished the commit");
    let st = h.ok("status", json!({}));
    assert_eq!((st["tuples"]["current"].as_str(), st["tuples"]["lkg"].as_str()), (Some(to.as_str()), Some(to.as_str())), "{st}");
    drop(_d);
    h.verify();
}

/// Our own child, killed and reaped by its handle when a test fails while it runs.
struct Kid(Child);
impl Drop for Kid {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

/// v0.5.3 (REL-052 §7.5, low): a start that cannot finish a pending upgrade never exits into a restart loop. launchd's
/// `KeepAlive` and systemd's `Restart=always` start an exited Root again, so v0.5.2's exit 1 repeated the refused
/// rollback, its alert and its log line every 10 s for as long as the service was loaded. Now the service's `start`
/// says it once, never runs the zone, and waits; once the owner finishes the upgrade as it says, it exits by itself
/// (75: the service starts the Root again, which runs). `start --foreground`, run by hand in a terminal, exits 1.
#[test]
fn a_start_that_cannot_finish_an_upgrade_waits_instead_of_looping() {
    let h = home("wl");
    h.install();
    { let _d = h.daemon(); }
    let r1 = h.release("rel1", "9.1.0-test", "release one", None);
    let bin = h.dir.join(".local/bin");
    let o = h.upgrade(&r1, &["--bin-dir", bin.to_str().unwrap(), "--by-installer"]);
    assert_eq!(code(&o), Some(0), "{}", err(&o));
    let j = h.journal();
    assert_eq!(j["phase"], "replaced");
    // the snapshot loses a file, so the rollback a start does in `replaced` is refused
    let snap = h.rh.join("state/upgrade").join(j["id"].as_str().unwrap());
    let key = j["items"].as_array().unwrap().iter().filter(|i| i["kind"] == "file").filter_map(|i| i["key"].as_str())
        .find(|k| snap.join(k).is_file()).unwrap().to_string();
    fs::remove_file(snap.join(&key)).unwrap(); // exact path, in this test's home
    let fk_lines = || fs::read_to_string(h.ih.join("state/kernel/fk.jsonl")).map_or(0, |t| t.lines().count());
    let n0 = fk_lines();
    let said = || fs::read_to_string(h.rh.join("alerts.jsonl")).unwrap_or_default().matches("is not finished").count();
    // by hand in a terminal: exits 1 at once, and says what to run
    let o = h.run(Path::new(BIN), &["start", "--foreground"]);
    assert_eq!(code(&o), Some(1), "{}", err(&o));
    assert!(err(&o).contains("upgrade --commit"), "{}", err(&o));
    assert_eq!(said(), 1);
    // the service's start: says it once, then waits instead of exiting into a restart loop
    let mut c = Kid(h.cmd(Path::new(BIN)).arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap());
    wait(10, "the message", || said() == 2);
    std::thread::sleep(Duration::from_secs(3));
    assert!(c.0.try_wait().unwrap().is_none(), "the service's start exited: launchd and systemd would start it again, in a loop");
    assert_eq!(said(), 2, "said once");
    let m = fs::read_to_string(h.rh.join("alerts.jsonl")).unwrap().lines().last().unwrap().to_string();
    assert!(m.contains("so this start waits instead of exiting") && m.contains("upgrade --commit"), "{m}");
    assert_eq!(fk_lines(), n0, "the zone never ran");
    // the owner finishes the upgrade as the message says: the waiting start exits by itself, and the Root runs
    let o = h.verb("--commit", "");
    assert_eq!(code(&o), Some(0), "{}", err(&o));
    wait(20, "the waiting start exits", || c.0.try_wait().unwrap().is_some());
    assert_eq!(c.0.wait().unwrap().code(), Some(75), "75: its service starts the Root again");
    assert_eq!(said(), 2);
    let _d = h.daemon();
    assert_eq!(h.journal()["phase"], "committed");
}

/// v0.5.1: an anchor whose tuple file cannot be read at the commit is never skipped silently. The upgrade itself is
/// sound, so the commit succeeds; the anchor is reported (`anchors[].skipped` in the output and the journal, one plain
/// stderr line, a `kernel_upgrade` alert) and cleared by an `audited{tuple:null, skipped}` record, so a resume falls
/// back to LKG on the new kernel even when the old tuple's file comes back later.
#[test]
fn an_unreadable_anchor_is_reported_and_cleared_at_the_commit() {
    let h = home("ps");
    h.install();
    { let _d = h.daemon(); } // the seedless genesis tuple: current, lkg and audited
    let (genesis, _) = h.ptrs();
    let r1 = h.release("rel1", "9.1.0-test", "release one", None);
    let o = h.upgrade(&r1, &["--bin-dir", h.dir.join(".local/bin").to_str().unwrap()]);
    assert_eq!(code(&o), Some(0), "{}", err(&o));
    // audited-good's tuple file goes missing before the commit (current is already the upgraded tuple)
    let (file, aside) = (h.rh.join("store/tuples").join(format!("{genesis}.json")), h.dir.join("genesis-tuple.aside"));
    fs::rename(&file, &aside).unwrap();
    let o = h.run(&h.rh.join("bin/init-root"), &["upgrade", "--probe", "--timeout", "40"]);
    assert_eq!(code(&o), Some(0), "probe: {}", err(&o));
    // the second review of v0.5.1: a commit that dies after it cleared the anchor (before its report) is re-run; the
    // re-run finds the anchor already null, and still reports it, because the report is read from the records
    let o = h.verb("--commit", "root.upgrade.after_anchors:exit7");
    assert_eq!(code(&o), Some(7), "cut short after the records: {}", err(&o));
    assert_eq!(h.last("audited")["data"]["skipped"].as_str().map(|s| s.contains("missing")), Some(true), "the clear is recorded");
    let o = h.run(&h.rh.join("bin/init-root"), &["upgrade", "--commit"]);
    assert_eq!(code(&o), Some(0), "the commit still succeeds: {}", err(&o));
    let out: Value = serde_json::from_str(String::from_utf8_lossy(&o.stdout).lines().last().unwrap()).unwrap();
    let a = &out["anchors"][0];
    assert_eq!((a["name"].as_str(), a["from"].as_str(), a["tuple"].as_str()), (Some("audited"), Some(genesis.as_str()), None), "{out}");
    assert!(a["skipped"].as_str().is_some_and(|s| s.contains("missing")), "the reason: {out}");
    let line = String::from_utf8_lossy(&o.stderr).into_owned();
    assert!(line.contains(&genesis) && line.contains("last-known-good"), "one plain line for the owner: {line}");
    assert_eq!(h.journal()["to"]["anchors"], out["anchors"], "journaled");
    let alerts = fs::read_to_string(h.rh.join("alerts.jsonl")).unwrap_or_default();
    assert!(alerts.lines().any(|l| l.contains("\"kernel_upgrade\"") && l.contains(&genesis)), "alerted: {alerts}");
    let to = h.ptrs().0;
    fs::rename(&aside, &file).unwrap(); // the file comes back (a restore from a backup)
    let _d = h.daemon();
    assert_eq!(h.ok("status", json!({}))["tuples"]["audited"], Value::Null, "the unreadable anchor is cleared");
    h.ok("panic", json!({}));
    wait(10, "panicked", || h.ok("status", json!({}))["slot"] == "panicked");
    h.ok("resume", json!({}));
    wait(20, "running after the resume", || h.ok("status", json!({}))["slot"] == "running");
    assert_eq!(h.ok("status", json!({}))["tuples"]["current"], to.as_str(), "the resume falls back to LKG, the upgraded tuple");
    assert_eq!(sha(&h.ih.join("kernel/bin/init-kernel")), sha(&r1.join("init-kernel")), "and runs the release's kernel");
    drop(_d);
    h.verify();
}

/// The pre-release review of v0.5.1: the owner's `/rollback twin` is a whole-system rollback to the twin tuple, and the
/// commit did not move the twin, so after an upgrade it reinstalled the kernel from before. The twin now takes the
/// release's kernel at the commit like the rollback anchors, keeping its body, ext and freeze (its replays never run
/// its kernel: twin.rs). The same commit ends a goal reading in flight on a change from before the upgrade, with a record.
#[test]
fn the_commit_moves_the_twin_so_rollback_twin_runs_the_release_kernel() {
    let h = home("tw");
    h.install();
    let genesis = {
        let _d = h.daemon(); // the seedless genesis tuple: current, lkg and audited
        let g = h.ptrs().0;
        h.append(json!([{"kind": "twin", "data": {"reason": "enable", "snapshot": null, "tuple": g}}]));
        g
    };
    let frozen = h.last("twin");
    // a pending card reading on the genesis change (the seedless body's sha is ""; goal.rs matches a reading's change by it)
    fs::create_dir_all(h.rh.join("state/goals")).unwrap();
    fs::write(h.rh.join("state/goals/state.json"), json!({"goals": [], "preds": [], "obs": [], "next": [0, 0, 0, 1], "checkin_alerted": null,
        "measures": [{"id": "M-000001", "card": "cost_turn", "sha": "", "goal": null, "n": 5, "gate": true, "guard": null}]}).to_string()).unwrap();
    let r1 = h.release("rel1", "9.1.0-test", "release one", None);
    let o = h.upgrade(&r1, &["--bin-dir", h.dir.join(".local/bin").to_str().unwrap()]);
    assert_eq!(code(&o), Some(0), "{}", err(&o));
    h.probe();
    let o = h.verb("--commit", "");
    assert_eq!(code(&o), Some(0), "commit: {}", err(&o));
    let out: Value = serde_json::from_str(String::from_utf8_lossy(&o.stdout).lines().last().unwrap()).unwrap();
    let to = h.ptrs().0;
    let twin = h.last("twin");
    assert!(out["anchors"].as_array().unwrap().iter().any(|a| a["name"] == "twin" && a["from"] == genesis.as_str() && a["tuple"] == twin["data"]["tuple"]), "{out}");
    let t = twin["data"]["tuple"].as_str().unwrap().to_string();
    assert_eq!(h.tuple(&t)["kernel"], h.tuple(&to)["kernel"], "the twin runs the release's kernel: {twin}");
    assert_eq!((&h.tuple(&t)["body"], &h.tuple(&t)["ext"]), (&h.tuple(&genesis)["body"], &h.tuple(&genesis)["ext"]), "with its own body and ext");
    assert_eq!((&twin["data"]["reason"], &twin["data"]["snapshot"], twin["data"]["frozen_at"].as_str()), (&json!("enable"), &Value::Null, frozen["at"].as_str()),
        "the twin keeps its freeze: {twin}");
    // the goal reading
    assert_eq!(out["goal_readings_cancelled"], json!(["M-000001"]), "{out}");
    let c = h.last("card_reading");
    assert_eq!((c["data"]["action"].as_str(), c["data"]["measure"].as_str(), c["data"]["upgrade"].clone()), (Some("cancelled"), Some("M-000001"), out["upgrade"].clone()), "{c}");
    assert!(err(&o).contains("goal reading"), "{}", err(&o));
    let st: Value = serde_json::from_str(&fs::read_to_string(h.rh.join("state/goals/state.json")).unwrap()).unwrap();
    assert_eq!(st["measures"], json!([]), "{st}");
    // `/rollback twin` on the new Root: the release's kernel stays in kernel/bin
    let _d = h.daemon();
    assert_eq!(h.ok("status", json!({}))["tuples"]["twin"], t.as_str());
    let n = h.records().iter().filter(|r| r["kind"] == "rollback").count();
    h.say("/rollback twin");
    wait(20, "the twin rollback", || h.records().iter().filter(|r| r["kind"] == "rollback").count() > n);
    wait(20, "running after the rollback", || h.ok("status", json!({}))["slot"] == "running");
    let rb = h.last("rollback");
    assert_eq!(rb["data"]["to"], t.as_str(), "{rb}");
    assert_eq!(sha(&h.ih.join("kernel/bin/init-kernel")), sha(&r1.join("init-kernel")), "kernel/bin holds the release's kernel after /rollback twin");
    drop(_d);
    h.verify();
}

/// The second review of v0.5.1: the commit re-anchored only when the upgrade changed the kernel. An anchor a v0.5.0
/// commit left on the kernel before it must move at the next commit even when that release keeps the kernel.
#[test]
fn a_commit_that_keeps_the_kernel_still_moves_an_anchor_left_on_an_older_one() {
    let h = home("ks");
    h.install();
    { let _d = h.daemon(); }
    let (genesis, _) = h.ptrs();
    let r1 = h.release("rel1", "9.1.0-test", "release one", None);
    assert_eq!(code(&h.upgrade(&r1, &["--bin-dir", h.dir.join(".local/bin").to_str().unwrap()])), Some(0));
    h.probe();
    assert_eq!(code(&h.verb("--commit", "")), Some(0));
    let up = h.ptrs().0;
    {
        let _d = h.daemon();
        h.append(json!([{"kind": "audited", "data": {"tuple": genesis}}])); // as a v0.5.0 commit left it: the old kernel
    }
    // a release with the same kernel and another Root
    let r2 = h.release("rel2", "9.1.1-test", "", None);
    fs::copy(r1.join("init-kernel"), r2.join("init-kernel")).unwrap();
    let mut root = fs::read(BIN).unwrap();
    root.extend_from_slice(b"\n# release two\n");
    fs::write(r2.join("init-root"), root).unwrap();
    fs::set_permissions(r2.join("init-root"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let o = h.upgrade(&r2, &[]);
    assert_eq!(code(&o), Some(0), "{}", err(&o));
    assert_eq!(h.journal()["to"]["tuple"], up.as_str(), "the kernel is unchanged: the same tuple");
    h.probe();
    let o = h.verb("--commit", "");
    assert_eq!(code(&o), Some(0), "commit: {}", err(&o));
    let a = h.last("audited");
    assert_eq!((a["data"]["from"].as_str(), a["data"]["upgrade"].clone()), (Some(genesis.as_str()), h.journal()["id"].clone()), "{a}");
    assert_eq!(h.tuple(a["data"]["tuple"].as_str().unwrap())["kernel"], h.tuple(&up)["kernel"], "audited-good is on the release's kernel");
    h.verify();
}

/// The second review of v0.5.1: the commit removed the snapshot before it recorded the commit, so a commit cut short
/// there left the journal `replaced`, and a `--rollback` then removed each live file before it found no snapshot to
/// clone back: the Root, install.json, the kernel files and the launchers were gone. The journal now says `committing`
/// before the snapshot goes; from then on only `--commit` is accepted, and a rollback never starts on an incomplete one.
#[test]
fn a_commit_cut_short_while_the_snapshot_goes_finishes_and_is_never_rolled_back() {
    let h = home("cs");
    h.install();
    { let _d = h.daemon(); }
    let r1 = h.release("rel1", "9.1.0-test", "release one", None);
    assert_eq!(code(&h.upgrade(&r1, &["--bin-dir", h.dir.join(".local/bin").to_str().unwrap()])), Some(0));
    h.probe();
    let live = h.live();
    let snap = h.rh.join("state/upgrade").join(h.journal()["id"].as_str().unwrap());
    // a commit cut short right after it recorded `committing`
    let mut j = h.journal();
    j["phase"] = json!("committing");
    fs::write(h.rh.join("state/upgrade.json"), j.to_string()).unwrap();
    let o = h.verb("--rollback", "");
    assert_eq!(code(&o), Some(1), "{}", err(&o));
    assert!(err(&o).contains("its commit has begun removing the snapshot") && err(&o).contains("upgrade --commit"), "{}", err(&o));
    assert!(snap.is_dir() && h.live() == live, "nothing changed");
    j["phase"] = json!("replaced");
    fs::write(h.rh.join("state/upgrade.json"), j.to_string()).unwrap();
    // a commit cut short after the snapshot went: the commit itself recorded `committing` first
    let o = h.verb("--commit", "root.upgrade.after_snapshot_remove:exit7");
    assert_eq!(code(&o), Some(7), "{}", err(&o));
    assert_eq!((h.journal()["phase"].as_str(), snap.exists()), (Some("committing"), false));
    let o = h.verb("--rollback", "");
    assert_eq!(code(&o), Some(1), "{}", err(&o));
    assert!(err(&o).contains("cannot be rolled back") && err(&o).contains("upgrade --commit"), "{}", err(&o));
    assert_eq!(h.live(), live, "every live file is kept");
    let o = h.verb("--commit", "");
    assert_eq!(code(&o), Some(0), "the re-run finishes it: {}", err(&o));
    assert_eq!(h.journal()["phase"], "committed");
    assert_eq!(h.live(), live);
    h.verify();
}

/// The second review of v0.5.1: `undo` removed each live item before it cloned it back from the snapshot. A snapshot
/// that a commit (here: of v0.5.0, which recorded nothing first) had partly removed now refuses the rollback whole.
#[test]
fn a_rollback_on_an_incomplete_snapshot_changes_nothing() {
    let h = home("is");
    h.install();
    { let _d = h.daemon(); }
    let r1 = h.release("rel1", "9.1.0-test", "release one", None);
    assert_eq!(code(&h.upgrade(&r1, &["--bin-dir", h.dir.join(".local/bin").to_str().unwrap()])), Some(0));
    let live = h.live();
    let snap = h.rh.join("state/upgrade").join(h.journal()["id"].as_str().unwrap());
    fs::remove_dir_all(snap.join("ih/kernel/bin")).unwrap(); // exact path, inside this test's snapshot
    let o = h.verb("--rollback", "");
    assert_eq!(code(&o), Some(1), "{}", err(&o));
    assert!(err(&o).contains("no longer holds ih/kernel/bin") && err(&o).contains("upgrade --commit"), "{}", err(&o));
    assert_eq!(h.live(), live, "no live file was removed");
    assert_eq!(h.journal()["phase"], "replaced");
}

/// The second review of v0.5.1: a commit cut short after it moved `lkg`, the anchors, the twin and `refs/heads/lkg`
/// left the journal `replaced`, and a `--rollback` then restored the tuple, the Root and `main` but not those, so the
/// old Root's panic resume installed the release's kernel under it. The rollback now puts them back from the journal.
#[test]
fn a_rollback_after_a_commit_cut_short_restores_lkg_the_anchors_and_the_twin() {
    let h = home("pc");
    let rel = h.dir.join("rel");
    let k0 = source_bundle(&h, &rel, "9.0.0-test", "the installed kernel source");
    h.install();
    let genesis = {
        let _d = h.daemon();
        let g = h.ptrs().0;
        h.append(json!([{"kind": "settled", "data": {"tuple": g}}, {"kind": "twin", "data": {"reason": "enable", "snapshot": null, "tuple": g}}]));
        g
    };
    let frozen = h.last("twin");
    assert_eq!((kref(&h, "refs/heads/lkg"), kref(&h, "refs/heads/main")), (k0.clone(), k0.clone()), "the install's source");
    let old_kernel = sha(&h.ih.join("kernel/bin/init-kernel"));
    let r1 = h.release("rel1", "9.1.0-test", "release one", None);
    let k1 = source_bundle(&h, &r1, "9.1.0-test", "the release's kernel source");
    assert_eq!(code(&h.upgrade(&r1, &["--bin-dir", h.dir.join(".local/bin").to_str().unwrap()])), Some(0));
    h.probe();
    let o = h.verb("--commit", "root.upgrade.after_anchors:exit7");
    assert_eq!(code(&o), Some(7), "{}", err(&o));
    let st = |h: &Home| {
        let mut p = (String::new(), String::new(), String::new(), String::new());
        for r in h.records() {
            let t = r["data"]["tuple"].as_str().unwrap_or("").to_string();
            match r["kind"].as_str() {
                Some("lkg") => p.0 = t,
                Some("audited") => p.1 = t,
                Some("settled") => p.2 = t,
                Some("twin") => p.3 = t,
                _ => {}
            }
        }
        p
    };
    let moved = st(&h);
    assert!(moved.0 != genesis && moved.1 != genesis && moved.2 != genesis && moved.3 != genesis, "the cut-short commit moved them: {moved:?}");
    assert_eq!((kref(&h, "refs/heads/lkg"), h.journal()["phase"].as_str()), (k1, Some("replaced")));
    let o = h.verb("--rollback", "");
    assert_eq!(code(&o), Some(0), "rollback: {}", err(&o));
    assert_eq!(st(&h), (genesis.clone(), genesis.clone(), genesis.clone(), genesis.clone()), "lkg, audited, settled and the twin are back");
    assert_eq!((kref(&h, "refs/heads/lkg"), kref(&h, "refs/heads/main")), (k0.clone(), k0), "and the kernel source's lkg and main");
    let twin = h.last("twin");
    assert_eq!((twin["data"]["reason"].as_str(), twin["data"]["frozen_at"].as_str(), twin["data"]["rolled_back"].as_bool()), (Some("enable"), frozen["at"].as_str(), Some(true)), "{twin}");
    assert_eq!(h.last("lkg")["data"]["upgrade"], h.journal()["id"], "settled_of's boundary: no tuple of the upgrade settles later");
    h.verify();
    // the old Root's panic resume runs the old kernel
    let _d = h.daemon();
    h.ok("panic", json!({}));
    wait(10, "panicked", || h.ok("status", json!({}))["slot"] == "panicked");
    h.ok("resume", json!({}));
    wait(20, "running after the resume", || h.ok("status", json!({}))["slot"] == "running");
    assert_eq!(h.ok("status", json!({}))["tuples"]["current"], genesis.as_str());
    assert_eq!(sha(&h.ih.join("kernel/bin/init-kernel")), old_kernel, "the kernel from before the upgrade");
    drop(_d);
    h.verify();
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

/// L2b (v0.5.3): SHA-256 of a file's bytes (a blob's name in the Root's store), from /usr/bin/shasum.
fn sha256(b: &[u8]) -> String {
    let o = Command::new("/usr/bin/shasum").args(["-a", "256"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().and_then(|mut c| {
        c.stdin.take().unwrap().write_all(b)?;
        c.wait_with_output()
    }).unwrap();
    String::from_utf8_lossy(&o.stdout).split_whitespace().next().unwrap().to_string()
}

/// L2b: a copy of this test's `init-root` with a trailing marker (a different Root binary that runs the same).
fn root_variant(at: &Path, marker: &str) {
    let mut b = fs::read(BIN).unwrap();
    b.extend_from_slice(format!("\n# {marker}\n").as_bytes());
    let tmp = at.with_extension("new");
    fs::write(&tmp, b).unwrap();
    fs::set_permissions(&tmp, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    fs::rename(&tmp, at).unwrap(); // a new inode, as `mv` or an upgrade leaves it
}

impl Home {
    /// The daemon started from the installed `ROOT_HOME/bin/init-root`, as the service starts it.
    fn installed_daemon(&self) -> Daemon {
        let c = self.cmd(&self.rh.join("bin/init-root")).arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let d = Daemon(c, self.rh.clone());
        wait(10, "owner.sock", || UnixStream::connect(self.rh.join("run/owner.sock")).is_ok());
        wait(20, "running", || self.ok("status", json!({}))["slot"] == "running");
        d
    }
}

/// L2b (INV-2 / A0 at start; "after a legitimate upgrade the new binary's hash is the expected one"): a release that
/// ships a new Root binary. Its probe starts it (the journal's `to.root_sha256`), the commit keeps it, and the service's
/// next start runs it as the recorded binary; a later upgrade rolled back after its probe restores the binary before it,
/// which starts too (the journal's `from.root_sha256`). Each start records `root_start` with the binary it ran.
#[test]
fn an_upgrade_that_ships_a_new_root_binary_starts_it_and_its_rollback_restores_the_old_one() {
    let h = home("nb");
    h.install();
    { let _d = h.daemon(); }
    let r1 = h.release("rel1", "9.1.0-test", "release one", None);
    root_variant(&r1.join("init-root"), "release one's Root");
    let new_root = sha256(&fs::read(r1.join("init-root")).unwrap());
    let o = h.upgrade(&r1, &["--bin-dir", h.dir.join(".local/bin").to_str().unwrap()]);
    assert_eq!(code(&o), Some(0), "{}", err(&o));
    assert_eq!(h.journal()["to"]["root_sha256"], new_root.as_str());
    h.probe();
    assert_eq!(h.last("root_start")["data"]["root_sha256"], new_root.as_str(), "the probe's Root is the release's binary");
    let o = h.verb("--commit", "");
    assert_eq!(code(&o), Some(0), "commit: {}", err(&o));
    { let _d = h.installed_daemon(); }
    // a second release, probed and then rolled back: the binary from before comes back and starts
    let r2 = h.release("rel2", "9.2.0-test", "release two", None);
    root_variant(&r2.join("init-root"), "release two's Root");
    let o = h.upgrade(&r2, &[]);
    assert_eq!(code(&o), Some(0), "{}", err(&o));
    h.probe();
    let o = h.run(&r2.join("init-root"), &["upgrade", "--rollback"]);
    assert_eq!(code(&o), Some(0), "rollback: {}", err(&o));
    assert_eq!(sha256(&fs::read(h.rh.join("bin/init-root")).unwrap()), new_root);
    { let _d = h.installed_daemon(); }
    assert_eq!(h.last("root_start")["data"]["root_sha256"], new_root.as_str());
    assert!(!h.records().iter().any(|r| r["kind"] == "halt" && r["data"]["reason"] == "boundary"), "no start was refused");
    h.verify();
}

/// L2b (INV-2 / A0 at start): something other than the upgrade puts another binary at `ROOT_HOME/bin/init-root` after
/// the replace step. The probe's Root is then a binary neither the last start nor the upgrade recorded: it never runs
/// the zone (`Halted{boundary}`), so the probe fails and the owner rolls the upgrade back instead of committing it.
#[test]
fn a_root_binary_swapped_after_the_replace_step_fails_the_probe() {
    let h = home("sw");
    h.install();
    { let _d = h.daemon(); }
    let r1 = h.release("rel1", "9.1.0-test", "release one", None);
    let o = h.upgrade(&r1, &[]);
    assert_eq!(code(&o), Some(0), "{}", err(&o));
    root_variant(&h.rh.join("bin/init-root"), "not the release's binary");
    let o = h.run(&h.rh.join("bin/init-root"), &["upgrade", "--probe", "--timeout", "30"]);
    assert_eq!(code(&o), Some(1), "the probe must fail: {}", err(&o));
    assert!(err(&o).contains("boundary"), "{}", err(&o));
    let o = h.run(&r1.join("init-root"), &["upgrade", "--rollback"]);
    assert_eq!(code(&o), Some(0), "rollback: {}", err(&o));
    let _d = h.daemon(); // the install from before runs
}

/// L2b, INV-10 / D9: the genesis last resort after an upgrade. `upgrade --commit` moves `lkg`, `audited`, `settled` and
/// the twin onto the release's kernel, but the rollback order (root.md §3.7.3) ends at genesis, the install's first
/// tuple, which still named the install's kernel. With every other anchor unreadable, a rollback installed that old
/// kernel under the new Root. Now genesis runs with the kernel of the newest upgrade and its own body and ext; and if
/// that kernel's blob is gone too, the Root halts (`no_tuple`) rather than run the old one.
#[test]
fn the_genesis_last_resort_after_an_upgrade_runs_the_release_kernel() {
    let h = home("gl");
    h.install();
    { let _d = h.daemon(); } // the seedless genesis tuple: current, lkg and audited
    let (genesis, _) = h.ptrs();
    let (g, kernel0) = (h.tuple(&genesis), sha(&h.ih.join("kernel/bin/init-kernel")));
    // a later tuple of its own (another commit, the same empty body binary) is current, lkg and audited-good
    let mut tc = g.clone();
    tc["body"]["sha"] = json!("c".repeat(40));
    let id = format!("t{}", &sha256(serde_json::to_string(&tc).unwrap().as_bytes())[..12]);
    let f = h.rh.join("store/tuples").join(format!("{id}.json"));
    fs::write(&f, format!("{}\n", serde_json::to_string(&tc).unwrap())).unwrap();
    {
        let _d = h.daemon();
        h.append(json!([{"kind": "tuple", "data": {"id": id, "tuple": tc}}, {"kind": "lkg", "data": {"tuple": id}},
            {"kind": "audited", "data": {"tuple": id}}]));
    }
    let r1 = h.release("rel1", "9.1.0-test", "release one", None);
    let o = h.upgrade(&r1, &["--bin-dir", h.dir.join(".local/bin").to_str().unwrap()]);
    assert_eq!(code(&o), Some(0), "{}", err(&o));
    h.probe();
    let o = h.verb("--commit", "");
    assert_eq!(code(&o), Some(0), "commit: {}", err(&o));
    let (to, lkg) = h.ptrs();
    let upgraded = h.tuple(&to);
    assert!(lkg == to && h.tuple(&to)["body"] == tc["body"], "lkg is the upgraded tuple");
    assert_eq!(h.last("audited")["data"]["tuple"], to.as_str(), "audited-good moved onto the release's kernel");
    // the upgraded tuple's file is damaged: lkg and audited-good both fail to verify; genesis is the last resort
    let tf = h.rh.join("store/tuples").join(format!("{to}.json"));
    fs::set_permissions(&tf, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
    fs::write(&tf, "{\"damaged\": true}\n").unwrap();
    {
        let _d = h.daemon();
        let rb = h.last("rollback");
        assert_eq!((rb["data"]["target"].as_str(), rb["data"]["reason"].as_str()), (Some("genesis"), Some("blob_missing")), "{rb}");
        let landed = rb["data"]["to"].as_str().unwrap().to_string();
        assert_ne!(landed, genesis, "not the install's tuple with the install's kernel: {rb}");
        let t = h.tuple(&landed);
        assert_eq!(t["kernel"], upgraded["kernel"], "the release's kernel: {rb}");
        assert_eq!((&t["body"], &t["ext"]), (&g["body"], &g["ext"]), "genesis's own body and ext");
        assert_eq!(sha(&h.ih.join("kernel/bin/init-kernel")), sha(&r1.join("init-kernel")), "kernel/bin holds the release's kernel");
        assert_ne!(sha(&h.ih.join("kernel/bin/init-kernel")), kernel0);
        let st = h.ok("status", json!({}));
        assert_eq!((st["slot"].as_str(), st["tuples"]["current"].as_str()), (Some("running"), Some(landed.as_str())), "{st}");
    }
    // the release kernel's blob is gone as well: no tuple of the release's kernel verifies, and the old one never runs
    let kblob = h.rh.join("store/blobs").join(upgraded["kernel"]["init_kernel"].as_str().unwrap());
    fs::remove_file(&kblob).unwrap(); // exact path, a blob of this test's store
    let c = h.cmd(Path::new(BIN)).arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let _d = Daemon(c, h.rh.clone());
    wait(10, "owner.sock", || UnixStream::connect(h.rh.join("run/owner.sock")).is_ok());
    wait(30, "Halted{no_tuple}", || h.ok("status", json!({}))["halted"] == "no_tuple");
    assert_ne!(sha(&h.ih.join("kernel/bin/init-kernel")), kernel0, "the install's kernel was never installed again");
}
