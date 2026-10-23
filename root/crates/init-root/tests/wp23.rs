//! WP23 integration tests: R4 through the `init-root` binary (root/root.md §3.10, §3.11, §8 RR-17, RR-19)
//! and `r4.verify` (`init-root audit verify --full`, and `--view` for the published copy) at the end of
//! every test. The mirror is a local `file://` bare repository in the test home, so nothing reaches a
//! network. Homes live under `root/target/test-homes/wp23/` (or `$WP23_HOMES`) with `HOME` pointed
//! there; daemons are killed by their own handles. Run with `cargo test -p init-root --features test-fixtures`.
#![cfg(feature = "test-fixtures")]

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_init-root");

struct Home {
    dir: PathBuf,
    rh: PathBuf,
    ih: PathBuf,
}

/// A fresh home `<base>/<name>` (short names: ROOT_HOME must stay ≤ 80 bytes).
fn home(name: &str) -> Home {
    let base = std::env::var_os("WP23_HOMES").map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/wp23"));
    fs::create_dir_all(&base).unwrap();
    let dir = fs::canonicalize(base).unwrap().join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap(); // exact path, created by an earlier run of this test
    }
    fs::create_dir_all(&dir).unwrap();
    Home { rh: dir.join("rh"), ih: dir.join("ih"), dir }
}

fn wait(secs: u64, what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(secs), "not within {secs} s: {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A test-made git command on `dir`: no global or system config, a test identity.
fn tgit(h: &Home, dir: &Path, args: &[&str]) -> String {
    let o = Command::new("/usr/bin/git").env_clear().env("PATH", "/usr/bin:/bin").env("HOME", &h.dir)
        .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(["-c", "user.name=wp23-test", "-c", "user.email=test@example.invalid"])
        .arg(format!("--git-dir={}", dir.display())).args(args).output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
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
    fn install(&self, extra: &str) {
        let f = self.dir.join("charter.toml");
        fs::write(&f, format!("schema_version = 1\n[purpose]\ntext = \"WP23 test home.\"\n[autonomy]\nmax_level = 4\n\
            [quotas]\ndisk_min_free_gib = 0\n[root]\ntest_mode = true\n{extra}\n")).unwrap();
        let o = self.run(&["install", "--charter", f.to_str().unwrap()]);
        assert_eq!(o.status.code(), Some(0), "install: {}", String::from_utf8_lossy(&o.stderr));
    }
    fn records(&self) -> Vec<Value> {
        let s = fs::read_to_string(self.rh.join("lineage/lineage.jsonl")).unwrap_or_default();
        s.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    }
    fn status(&self) -> Value {
        serde_json::from_slice(&self.run(&["status", "--json"]).stdout).unwrap_or(Value::Null)
    }
    fn daemon(&self, env: &[(&str, &str)]) -> Daemon {
        let mut c = self.cmd();
        c.arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        for (k, v) in env {
            c.env(k, v);
        }
        let d = Daemon(c.spawn().unwrap());
        wait(10, "owner.sock", || std::os::unix::net::UnixStream::connect(self.rh.join("run/owner.sock")).is_ok());
        d
    }
    fn alerts(&self) -> String {
        fs::read_to_string(self.rh.join("alerts.jsonl")).unwrap_or_default()
    }
    /// `r4.verify` (testing.md §3.13): the Root's chain in full, `head.json`, the mirror when one is on,
    /// and the published copy under the published key, as the agent checks it.
    fn r4_verify(&self) {
        for args in [["audit", "verify", "--full"], ["audit", "verify", "--view"]] {
            let o = self.run(&args);
            assert_eq!(o.status.code(), Some(0), "r4.verify {args:?}: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        }
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

fn url(p: &Path) -> String {
    format!("file://{}", p.display().to_string().replace(' ', "%20"))
}

/// RR-19 through the daemon: the lineage thread pushes at start and at once after urgent kinds, the
/// mirror-ref channel panics the Root, and a rewritten remote stops the pushes (`mirror_diverged`),
/// never forced; `audit verify` checks rule 8.
#[test]
fn rr19_the_daemon_mirrors_panics_from_the_mirror_and_never_forces() {
    let h = home("mir");
    let remote = h.dir.join("remote.git");
    tgit(&h, &remote, &["init", "--bare", "-q"]);
    h.install(&format!("[mirror]\nurl = \"{}\"\npush_interval_s = 1\n[panic]\nmirror_ref = true\nmirror_poll_s = 1\n", url(&remote)));
    let head_of = |h: &Home| h.records().last().map(|r| r["hash"].as_str().unwrap().to_string());
    let remote_head = |_: &Home| -> Option<String> {
        let o = Command::new("/usr/bin/git").env_clear().env("GIT_CONFIG_GLOBAL", "/dev/null")
            .arg(format!("--git-dir={}", remote.display())).args(["show", "lineage:HEAD.json"]).output().unwrap();
        serde_json::from_slice::<Value>(&o.stdout).ok().and_then(|v| v["hash"].as_str().map(String::from))
            .filter(|_| o.status.success())
    };
    let d = h.daemon(&[]);
    wait(15, "the first push", || remote_head(&h).is_some() && remote_head(&h) == head_of(&h));
    wait(10, "mirror state ok", || h.status()["mirror"]["state"] == "ok");
    let o = h.run(&["audit", "verify", "--full"]);
    assert!(String::from_utf8_lossy(&o.stdout).contains("mirror: a prefix"), "{}", String::from_utf8_lossy(&o.stdout));
    // the mirror-ref channel: a panic branch on the remote panics the Root (≤ mirror_poll_s), which pushes at once
    let c1 = tgit(&h, &remote, &["rev-parse", "refs/heads/lineage"]);
    tgit(&h, &remote, &["update-ref", "refs/heads/panic", &c1]);
    wait(15, "a mirror_ref panic", || h.records().iter().any(|r| r["kind"] == "panic" && r["data"]["channel"] == "mirror_ref"));
    wait(10, "the panic pushed", || remote_head(&h) == head_of(&h));
    // §3.9.5 step 1 (build-deviations §5 L5, R-SAFE): resume is refused while the panic branch exists
    let o = h.run(&["resume"]);
    assert_eq!(o.status.code(), Some(77), "resume while refs/heads/panic exists");
    assert!(String::from_utf8_lossy(&o.stderr).contains("refs/heads/panic still exists"), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(h.status()["slot"], "panicked");
    tgit(&h, &remote, &["update-ref", "-d", "refs/heads/panic"]);
    wait(10, "resume once the branch is gone", || h.run(&["resume"]).status.code() == Some(0));
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(h.records().iter().filter(|r| r["kind"] == "panic").count(), 1, "the same panic ref fires once");
    // the remote's history is rewritten: the next push is refused, alerted, and never forced
    let empty = tgit(&h, &remote, &["hash-object", "-w", "-t", "tree", "/dev/null"]);
    let rewritten = tgit(&h, &remote, &["commit-tree", &empty, "-m", "rewritten"]);
    tgit(&h, &remote, &["update-ref", "refs/heads/lineage", &rewritten]);
    assert_eq!(h.run(&["panic"]).status.code(), Some(0));
    wait(15, "mirror_diverged", || h.alerts().contains("mirror_diverged"));
    assert_eq!(h.status()["mirror"]["state"], "diverged");
    assert_eq!(tgit(&h, &remote, &["rev-parse", "refs/heads/lineage"]), rewritten, "never forced");
    assert_eq!(h.run(&["audit", "verify", "--full"]).status.code(), Some(1), "rule 8: the mirror is no prefix");
    drop(d);
    // the owner restores the mirror from the Root's own copy; the chain is valid again
    let local = tgit(&h, &h.rh.join("lineage/mirror.git"), &["rev-parse", "refs/heads/lineage"]);
    tgit(&h, &h.rh.join("lineage/mirror.git"), &["push", "-q", "--force", &url(&remote), &format!("{local}:refs/heads/lineage")]);
    h.r4_verify();
}

/// RR-17: `root.lineage.mid_append` aborts the process with half a line written; the next Root start
/// copies the torn tail to `lineage/torn-<ts>.bin`, truncates it, and the chain verifies.
#[test]
fn rr17_a_torn_tail_after_an_abort_is_repaired_at_start() {
    let h = home("torn");
    h.install("");
    let before = fs::read_to_string(h.rh.join("lineage/lineage.jsonl")).unwrap();
    let ch = fs::read_to_string(h.rh.join("charter.toml")).unwrap();
    fs::write(h.rh.join("charter.toml"), ch.replace("WP23 test home.", "WP23 test home, edited.")).unwrap();
    let o = h.cmd().args(["charter", "check", "--apply", "--yes"]).env("INIT_FAULT", "root.lineage.mid_append:abort")
        .stdin(Stdio::null()).output().unwrap();
    assert_eq!(o.status.code(), None, "aborted by the fault point");
    let torn = fs::read_to_string(h.rh.join("lineage/lineage.jsonl")).unwrap();
    assert!(torn.starts_with(&before) && !torn.ends_with('\n'));
    let o = h.run(&["audit", "verify"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stdout).contains("torn tail"));
    let d = h.daemon(&[]);
    let files: Vec<String> = fs::read_dir(h.rh.join("lineage")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert!(files.iter().any(|f| f.starts_with("torn-") && f.ends_with(".bin")), "{files:?}");
    assert!(fs::read_to_string(h.rh.join("lineage/lineage.jsonl")).unwrap().starts_with(&before));
    drop(d);
    h.r4_verify();
}

/// §3.10.4: the lineage thread verifies the whole chain once a day; a corrupt complete line drops the
/// lineage handle, alerts `lineage_corrupt` and halts the Root (`Halted{lineage}`).
#[test]
fn the_daily_full_verification_halts_on_a_corrupt_chain() {
    let h = home("daily");
    h.install("");
    let d = h.daemon(&[("INIT_ROOT_TEST_NOW", "2026-10-20T00:00:00Z")]);
    wait(10, "the first full verification", || {
        let hj: Value = serde_json::from_str(&fs::read_to_string(h.rh.join("lineage/head.json")).unwrap_or_default()).unwrap_or(Value::Null);
        hj["verified_full_at"].is_string()
    });
    h.r4_verify();
    let f = h.rh.join("lineage/lineage.jsonl");
    let t = fs::read_to_string(&f).unwrap();
    fs::write(&f, t.replacen("\"source\":\"install\"", "\"source\":\"instal_\"", 1)).unwrap();
    assert_eq!(h.run(&["audit", "verify", "--full"]).status.code(), Some(1));
    assert_eq!(h.run(&["test", "clock", "--advance", "86401"]).status.code(), Some(0));
    wait(10, "lineage_corrupt", || h.alerts().contains("lineage_corrupt"));
    wait(10, "Halted{lineage}", || h.status()["halted"] == "lineage");
    drop(d);
}

/// D7, §3.8.1: `install --release` imports the release's kernel source bundle into the Root-written
/// `INIT_HOME/lineage/kernel.git` as `refs/upstream/<version>`, and `main` names it.
#[test]
fn install_imports_the_kernel_source_bundle() {
    let h = home("kgit");
    let src = h.dir.join("src.git");
    tgit(&h, &src, &["init", "--bare", "-q"]);
    let tree = tgit(&h, &src, &["hash-object", "-w", "-t", "tree", "/dev/null"]);
    let c = tgit(&h, &src, &["commit-tree", &tree, "-m", "kernel 0.1.0"]);
    tgit(&h, &src, &["update-ref", "refs/heads/release", &c]);
    fs::create_dir_all(h.dir.join("rel/src")).unwrap();
    tgit(&h, &src, &["bundle", "create", h.dir.join("rel/src/0.1.0.bundle").to_str().unwrap(), "refs/heads/release"]);
    let f = h.dir.join("charter.toml");
    fs::write(&f, "schema_version = 1\n[purpose]\ntext = \"WP23 test home.\"\n[autonomy]\nmax_level = 4\n[quotas]\n\
        disk_min_free_gib = 0\n[root]\ntest_mode = true\n").unwrap();
    let o = h.run(&["install", "--charter", f.to_str().unwrap(), "--release", h.dir.join("rel").to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "install: {}", String::from_utf8_lossy(&o.stderr));
    let kgit = h.ih.join("lineage/kernel.git");
    assert_eq!(tgit(&h, &kgit, &["rev-parse", "refs/upstream/0.1.0"]), c);
    assert_eq!(tgit(&h, &kgit, &["rev-parse", "refs/heads/main"]), c);
    assert_eq!(tgit(&h, &kgit, &["rev-parse", "refs/heads/lkg"]), c, "§4.3 step 6: lkg names the imported release too (§5 L4)");
    h.r4_verify();
}
