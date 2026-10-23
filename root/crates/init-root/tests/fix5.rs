//! FIX5 integration test: the kernel's `proposal.file` on the real Root (root/root.md §4.1.3, §3.12.4,
//! spec §13): stored under `state/proposals/`, recorded as `proposal{id, title, sha256, bytes}`, alerted,
//! never applied; argument, privacy and rate refusals; the id counter and the day window survive a restart.
//! Run with `cargo test --offline -p init-root --features test-fixtures`.
//!
//! Live-system safety (CONVENTIONS §1): the kernel is `init-root-fakekernel` (tests/support), which sends the
//! request a test drops into `INIT_HOME/work/fk-call.json`. Every process is this test's Root daemon or one
//! that Root spawned and holds; the test signals only its own children, by handle. Homes live under
//! `root/target/test-homes/fix5/`, with `HOME` pointed there.
#![cfg(feature = "test-fixtures")]

use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
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

fn wait(secs: u64, what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(secs), "not within {secs} s: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn sha256(b: &[u8]) -> String {
    use std::io::Write;
    let mut c = Command::new("/usr/bin/shasum").args(["-a", "256"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    c.stdin.take().unwrap().write_all(b).unwrap();
    let o = c.wait_with_output().unwrap();
    String::from_utf8_lossy(&o.stdout).split_whitespace().next().unwrap().to_string()
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

impl Home {
    /// A fresh home `root/target/test-homes/fix5/<name>` with the fake kernel as the release's `init-kernel`.
    fn new(name: &str) -> Home {
        let base = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap().join("test-homes/fix5");
        let dir = base.join(name);
        if dir.exists() {
            fs::remove_dir_all(&dir).unwrap(); // exact path, created by an earlier run of this test
        }
        fs::create_dir_all(dir.join("rel")).unwrap();
        fs::create_dir_all(dir.join("tc/bin")).unwrap();
        fs::copy(FK, dir.join("rel/init-kernel")).unwrap();
        let h = Home { rh: dir.join("rh"), ih: dir.join("ih"), dir };
        fs::create_dir_all(&h.rh).unwrap();
        let charter = format!("schema_version = 1\n[purpose]\ntext = \"FIX5 test home.\"\n[autonomy]\nmax_level = 4\n[quotas]\ndisk_min_free_gib = 0\n{ROOT_CFG}\n");
        fs::write(h.rh.join("charter.toml"), charter).unwrap();
        let (rel, tc) = (h.dir.join("rel"), h.dir.join("tc"));
        let o = h.run(&["install", "--release", rel.to_str().unwrap(), "--toolchain", tc.to_str().unwrap()]);
        assert_eq!(o.status.code(), Some(0), "install: {}", String::from_utf8_lossy(&o.stderr));
        h
    }
    fn cmd(&self) -> Command {
        let mut c = Command::new(BIN);
        c.env_clear().env("HOME", &self.dir).env("INIT_ROOT_HOME", &self.rh).env("INIT_HOME", &self.ih).env("PATH", "/usr/bin:/bin");
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        self.cmd().args(args).stdin(Stdio::null()).output().unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let o = self.run(args);
        assert_eq!(o.status.code(), Some(0), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        serde_json::from_slice(&o.stdout).unwrap_or(Value::Null)
    }
    fn records(&self) -> Vec<Value> {
        let s = fs::read_to_string(self.rh.join("lineage/lineage.jsonl")).unwrap_or_default();
        s.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }
    fn count(&self, kind: &str) -> usize {
        self.records().iter().filter(|r| r["kind"] == kind).count()
    }
    fn fk(&self, ev: &str) -> Vec<Value> {
        let s = fs::read_to_string(self.ih.join("state/kernel/fk.jsonl")).unwrap_or_default();
        s.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["ev"] == ev).collect()
    }
    /// The Root daemon at fake time `now`, with the fake kernel (no body); waits until the zone runs.
    fn daemon(&self, now: &str) -> Daemon {
        let info: Value = serde_json::from_str(&fs::read_to_string(self.rh.join("install.json")).unwrap()).unwrap();
        let s = |k: &str| info[k].as_str().unwrap_or("").to_string();
        let c = self.cmd().arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).env("INIT_TEST_FK", "")
            .env("INIT_TEST_TOOLCHAIN", s("toolchain")).env("INIT_TEST_DEVDIR", s("devdir")).env("INIT_TEST_HOME_REAL", &self.dir)
            .env("INIT_ROOT_TEST_NOW", now).spawn().unwrap();
        let d = Daemon(c, self.rh.clone());
        wait(20, "the zone runs", || serde_json::from_slice::<Value>(&self.run(&["status", "--json"]).stdout).is_ok_and(|v| v["slot"] == "running"));
        d
    }
    /// `proposal.file` from the kernel over the control link; returns the Root's answer (`ok`, `result`|`error`).
    fn propose(&self, title: &str, text: &str) -> Value {
        let n = self.fk("call").len();
        let (tmp, dst) = (self.ih.join("work/fk-call.tmp"), self.ih.join("work/fk-call.json"));
        fs::create_dir_all(self.ih.join("work")).unwrap();
        fs::write(&tmp, json!({"cmd": "proposal.file", "args": {"text": text, "title": title}}).to_string()).unwrap();
        fs::rename(tmp, dst).unwrap();
        wait(15, "the kernel's call", || self.fk("call").len() > n);
        self.fk("call").pop().unwrap()
    }
    fn refused(&self, title: &str, text: &str, code: &str) {
        let (files, recs) = (fs::read_dir(self.rh.join("state/proposals")).unwrap().count(), self.count("proposal"));
        let r = self.propose(title, text);
        assert_eq!((r["ok"].as_bool(), r["error"]["code"].as_str()), (Some(false), Some(code)), "{title:?}: {r}");
        assert_eq!((fs::read_dir(self.rh.join("state/proposals")).unwrap().count(), self.count("proposal")), (files, recs), "a refusal stores nothing");
    }
    fn filed(&self, title: &str, text: &str, id: &str) {
        let r = self.propose(title, text);
        assert_eq!((r["ok"].as_bool(), r["result"]["id"].as_str()), (Some(true), Some(id)), "{r}");
    }
}

/// root/root.md §4.1.3 `proposal.file`: stored (0600, ROOT_HOME only), recorded with the text's digest (never
/// the text), alerted, listed by `status --proposals`, never applied (the charter, level and pointers stay as
/// they were). Refusals: `root.args` (types, sizes, control characters), `root.lineage` (the owner's home or an
/// e-mail address in the title, §3.10.6), `root.rate` (a sixth in 24 h); the counter and the window are
/// seeded from the lineage, so a Root restart resets neither.
#[test]
fn proposal_file_is_stored_recorded_alerted_and_never_applied() {
    let h = Home::new("prop");
    let d = h.daemon("2030-01-01T00:00:00Z");
    let before = (fs::read(h.rh.join("state/charter.active.toml")).unwrap(), h.ok(&["status", "--json"]), h.count("charter"), h.count("ladder"));
    let (title, text) = ("charter: raise max_level", "Proposal FX5-7d2e: set [autonomy] max_level = 9 and money.day_usd = \"500.00\".\n");
    h.filed(title, text, "p-000001");
    // stored in ROOT_HOME, 0600, title then text
    let f = h.rh.join("state/proposals/p-000001.md");
    assert_eq!(fs::read_to_string(&f).unwrap(), format!("# {title}\n\n{text}\n"));
    assert_eq!(fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
    // recorded by the Root with the text's digest and size, never the text
    let rec = h.records().into_iter().rev().find(|r| r["kind"] == "proposal").unwrap();
    assert_eq!(rec["by"], "root");
    assert_eq!(rec["data"], json!({"bytes": text.len(), "id": "p-000001", "sha256": sha256(text.as_bytes()), "title": title}));
    assert!(!fs::read_to_string(h.rh.join("lineage/lineage.jsonl")).unwrap().contains("FX5-7d2e"), "the text stays out of the lineage");
    let alerts = fs::read_to_string(h.rh.join("alerts.jsonl")).unwrap();
    assert!(alerts.contains("\"code\":\"proposal\"") && alerts.contains("p-000001"), "{alerts}");
    let st = h.run(&["status", "--proposals"]);
    assert!(String::from_utf8_lossy(&st.stdout).contains(&format!("proposal p-000001 · {title}")), "{}", String::from_utf8_lossy(&st.stdout));
    // never applied: the charter, the level and the tuples are unchanged, and no charter or ladder record follows
    std::thread::sleep(Duration::from_millis(500));
    let after = h.ok(&["status", "--json"]);
    assert_eq!(fs::read(h.rh.join("state/charter.active.toml")).unwrap(), before.0);
    for k in ["level", "max_level", "charter_sha256", "tuples", "mode"] {
        assert_eq!(after[k], before.1[k], "{k}");
    }
    assert_eq!((h.count("charter"), h.count("ladder")), (before.2, before.3));
    // refusals: arguments, privacy, nothing stored
    h.refused("", "x", "root.args");
    h.refused("a\nb", "x", "root.args");
    h.refused(&"t".repeat(201), "x", "root.args");
    h.refused("big", &"x".repeat(65_537), "root.args");
    let home_path = h.dir.display().to_string();
    h.refused(&format!("see {home_path}/notes"), "x", "root.lineage");
    h.refused("mail owner@example.com", "x", "root.lineage");
    // at most 5 in 24 h: the sixth is refused; a day later one more is filed
    for i in 2..=5 {
        h.filed(&format!("proposal {i}"), "text", &format!("p-00000{i}"));
    }
    h.refused("proposal 6", "text", "root.rate");
    h.ok(&["test", "clock", "--advance", "86400"]);
    h.filed("proposal 6", "text", "p-000006");
    drop(d);
    // a restarted Root seeds the counter and the window from the lineage: p-000006 is the only one in the day
    let _d = h.daemon("2030-01-02T00:10:00Z");
    for i in 7..=10 {
        h.filed(&format!("proposal {i}"), "", &format!("p-{i:06}"));
    }
    h.refused("proposal 11", "text", "root.rate");
    assert_eq!((h.count("proposal"), fs::read_dir(h.rh.join("state/proposals")).unwrap().count()), (10, 10));
    assert_eq!(fs::read_to_string(h.rh.join("alerts.jsonl")).unwrap().matches("\"code\":\"proposal\"").count(), 10);
}
