//! Lane L2b (v0.5.3, PLAN-052): the Root's invariant gaps found in v0.5.1's review, against the real `init-root` daemon in
//! test mode with `init-root-fakekernel` (tests/support) as the kernel.
//! - INV-2 / ADR-011 A0: `root_start` records the Root's binary, its compiled-in profiles and its active charter; a
//!   change is refused (`root.boundary`) and the zone halted when one of them changed under the running Root; a start
//!   on a Root binary that no start or upgrade recorded never runs the zone until the owner's `/resume`.
//! - INV-3: a free-path `promotion{lkg:true}` is matched to the watched tuple by content, not by its commit sha.
//!
//! Run with `cargo test --offline -p init-root --features test-fixtures --test l2b`.
//!
//! Live-system safety (CONVENTIONS §1): every process is this test's Root daemon or one that Root spawned and holds; the
//! test signals only its own children, by handle. Each home runs its own copy of `init-root` (so a test may replace the
//! binary a daemon runs from, and a concurrent rebuild cannot); homes live under `root/target/test-homes/l2b/`, with
//! `HOME` there, removed by exact path on pass. $0: no model, no network.
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
const CHARTER: &str = "schema_version = 1\n[purpose]\ntext = \"L2b test home.\"\n[autonomy]\nmax_level = 0\n[quotas]\ndisk_min_free_gib = 0\n\
    [sandbox]\ndisk_floor_gib = 0\n[extend]\nmode = \"free\"\n[root]\ntest_mode = true\nhb_interval_s = 1\nhb_stale_s = 3\nkernel_hello_timeout_s = 20\n\
    backoff_initial_ms = 50\nbackoff_max_ms = 200\nstop_grace_s = 5\n";

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

/// A fresh home `root/target/test-homes/l2b/<name>` (short: ROOT_HOME must stay ≤ 80 bytes) with its own `init-root`.
fn home(name: &str) -> Home {
    let base = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap().join("test-homes/l2b");
    let dir = base.join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap(); // exact path, created by an earlier run of this test
    }
    fs::create_dir_all(dir.join("rel")).unwrap();
    fs::create_dir_all(dir.join("tc/bin")).unwrap();
    fs::copy(FK, dir.join("rel/init-kernel")).unwrap();
    fs::copy(BIN, dir.join("init-root")).unwrap();
    Home { rh: dir.join("rh"), ih: dir.join("ih"), dir }
}

fn wait(secs: u64, what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(secs), "not within {secs} s: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn sha256(b: &[u8]) -> String {
    let o = Command::new("/usr/bin/shasum").args(["-a", "256"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().and_then(|mut c| {
        c.stdin.take().unwrap().write_all(b)?;
        c.wait_with_output()
    }).unwrap();
    String::from_utf8_lossy(&o.stdout).split_whitespace().next().unwrap().to_string()
}

impl Home {
    fn exe(&self) -> PathBuf {
        self.dir.join("init-root")
    }
    fn cmd(&self) -> Command {
        let mut c = Command::new(self.exe());
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
    fn install(&self) {
        self.install_with(CHARTER);
    }
    fn install_with(&self, charter: &str) {
        fs::create_dir_all(&self.rh).unwrap();
        fs::write(self.rh.join("charter.toml"), charter).unwrap();
        let spec = self.dir.join("spec.json");
        fs::write(&spec, json!({"start_at": "2030-01-01T00:00:00Z", "start_level": 0, "records": []}).to_string()).unwrap();
        self.ok(&["test", "ledger", "--spec", spec.to_str().unwrap()]);
        let (rel, tc) = (self.dir.join("rel"), self.dir.join("tc"));
        let o = self.run(&["install", "--release", rel.to_str().unwrap(), "--toolchain", tc.to_str().unwrap()]);
        assert_eq!(o.status.code(), Some(0), "install: {}", String::from_utf8_lossy(&o.stderr));
    }
    fn records(&self) -> Vec<Value> {
        let s = fs::read_to_string(self.rh.join("lineage/lineage.jsonl")).unwrap_or_default();
        s.lines().filter_map(|l| serde_json::from_str(l).ok()).collect() // a line being appended may be torn
    }
    fn of(&self, kind: &str) -> Vec<Value> {
        self.records().into_iter().filter(|r| r["kind"] == kind).collect()
    }
    fn status(&self) -> Value {
        self.ok(&["status", "--json"])
    }
    fn fk(&self, ev: &str) -> Vec<Value> {
        let s = fs::read_to_string(self.ih.join("state/kernel/fk.jsonl")).unwrap_or_default();
        s.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["ev"] == ev).collect()
    }
    /// A request the fake kernel sends over the control link (`work/fk-call.json`), and its logged outcome.
    fn kernel_call(&self, cmd: &str, args: Value) -> Value {
        let n = self.fk("call").len();
        let (tmp, dst) = (self.ih.join("work/fk-call.tmp"), self.ih.join("work/fk-call.json"));
        fs::create_dir_all(self.ih.join("work")).unwrap();
        fs::write(&tmp, json!({"cmd": cmd, "args": args}).to_string()).unwrap();
        fs::rename(tmp, dst).unwrap();
        wait(30, &format!("{cmd} answered"), || self.fk("call").len() > n);
        self.fk("call").into_iter().nth(n).unwrap()
    }
    /// An owner request on `owner.sock`.
    fn req(&self, cmd: &str, args: Value) -> Value {
        let mut s = UnixStream::connect(self.rh.join("run/owner.sock")).unwrap();
        writeln!(s, "{}", json!({"args": args, "cmd": cmd, "id": "t-1", "v": 1})).unwrap();
        let mut line = String::new();
        BufReader::new(&s).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
    /// The daemon, up to its owner socket (the zone may or may not start).
    fn spawn(&self) -> Daemon {
        let info: Value = serde_json::from_str(&fs::read_to_string(self.rh.join("install.json")).unwrap()).unwrap();
        let s = |k: &str| info[k].as_str().unwrap_or("").to_string();
        let c = self.cmd().arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).env("INIT_TEST_FK", "body")
            .env("INIT_TEST_TOOLCHAIN", s("toolchain")).env("INIT_TEST_DEVDIR", s("devdir")).env("INIT_TEST_HOME_REAL", &self.dir)
            .env("INIT_ROOT_TEST_NOW", "2030-01-02T00:00:00Z").spawn().unwrap();
        let d = Daemon(c, self.rh.clone(), self.exe());
        wait(10, "owner.sock", || UnixStream::connect(self.rh.join("run/owner.sock")).is_ok());
        d
    }
    /// The daemon with its zone running.
    fn daemon(&self) -> Daemon {
        let n = self.of("zone_start").len();
        let d = self.spawn();
        wait(30, "the zone running", || self.status()["slot"] == "running" && self.of("zone_start").len() > n);
        d
    }
    fn change(sha: char, binary: &str) -> Value {
        let sha = sha.to_string().repeat(40);
        json!({"sha": sha, "from_sha": "", "scope": "t0", "prefix_change": false,
            "candidate": {"body": {"binary": binary, "sha": sha}, "ext": {"components": [], "set": null}}, "caps": [{"id": "skill/l2b", "op": "add"}]})
    }
    /// `change.open` for a T0 change on the current tuple's kernel, with body binary `binary` ("" or a stored blob).
    fn open_change(&self, sha: char, binary: &str) -> Value {
        self.kernel_call("change.open", Self::change(sha, binary))
    }
    /// A `change.open` that A0 refuses: the Root halts the zone, so the kernel is killed before it logs the answer; the
    /// Root's own records and alerts tell the outcome.
    fn refused_change(&self, sha: char) {
        let opens = self.of("change_open").len();
        let (tmp, dst) = (self.ih.join("work/fk-call.tmp"), self.ih.join("work/fk-call.json"));
        fs::create_dir_all(self.ih.join("work")).unwrap();
        fs::write(&tmp, json!({"cmd": "change.open", "args": Self::change(sha, "")}).to_string()).unwrap();
        fs::rename(tmp, dst).unwrap();
        wait(30, "Halted{boundary}", || self.status()["halted"] == "boundary");
        assert_eq!(self.of("change_open").len(), opens, "nothing opens");
        let halt = self.of("halt").pop().unwrap();
        assert_eq!((halt["data"]["on"].as_bool(), halt["data"]["reason"].as_str()), (Some(true), Some("boundary")), "{halt}");
        assert!(self.alerts().contains("Halted{boundary}: A0: "), "{}", self.alerts());
        wait(10, "the zone stopped", || self.status()["instances"].as_array().is_some_and(Vec::is_empty));
    }
    /// A blob in the Root's store, as `store.put` leaves it (0444), by its sha256.
    fn blob(&self, bytes: &[u8]) -> String {
        let sha = sha256(bytes);
        let p = self.rh.join("store/blobs").join(&sha);
        if !p.exists() {
            fs::write(&p, bytes).unwrap();
            fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o444)).unwrap();
        }
        sha
    }
    /// A different binary in place of the home's `init-root` (a new inode, as `mv` or an upgrade leaves it), one that
    /// still runs: the same bytes plus a trailing marker.
    fn replace_exe(&self, marker: &str) {
        let mut b = fs::read(BIN).unwrap();
        b.extend_from_slice(format!("\n# {marker}\n").as_bytes());
        let tmp = self.dir.join("init-root.new");
        fs::write(&tmp, b).unwrap();
        fs::set_permissions(&tmp, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        fs::rename(&tmp, self.exe()).unwrap();
    }
    fn alerts(&self) -> String {
        fs::read_to_string(self.rh.join("alerts.jsonl")).unwrap_or_default()
    }
}

/// The Root daemon this test spawned. On drop: an orderly `stop`, then the daemon by its own handle.
struct Daemon(Child, PathBuf, PathBuf);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = Command::new(&self.2).env_clear().env("HOME", "/nonexistent").env("INIT_ROOT_HOME", &self.1).args(["stop"]).output();
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(8) {
            let o = Command::new(&self.2).env_clear().env("HOME", "/nonexistent").env("INIT_ROOT_HOME", &self.1).args(["status", "--json"]).output();
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

/// INV-2 / A0: every daemon start records `root_start` with the binary it runs from, the compiled-in profile texts and
/// the active charter; a change's `checks` say A0 and A5 passed.
#[test]
fn a0_root_start_records_the_binary_the_profiles_and_the_charter() {
    let h = home("rs");
    h.install();
    let _d = h.daemon();
    let starts = h.of("root_start");
    assert_eq!(starts.len(), 1, "one root_start per daemon start");
    let d = &starts[0]["data"];
    assert_eq!(d["root_sha256"], sha256(&fs::read(h.exe()).unwrap()).as_str(), "{d}");
    assert_eq!(d["root_version"], h.status()["root"]["version"], "{d}");
    assert!(d["profiles_sha256"].as_str().is_some_and(|s| s.len() == 64), "{d}");
    assert_eq!(d["charter"], h.of("charter").last().unwrap()["data"]["digest"], "the active charter: {d}");
    assert_eq!(starts[0]["by"], "root");
    let r = h.open_change('b', "");
    assert_eq!(r["ok"], true, "change.open: {r}");
    let checks = &h.of("change_open")[0]["data"]["checks"];
    assert_eq!((checks["A0"].as_str(), checks["A5"].as_str()), (Some("pass"), Some("pass")), "{checks}");
}

/// INV-2 / A0: the binary the running Root started from is replaced (a new inode at its path, as a zone escape writing
/// `ROOT_HOME/bin/init-root` would leave it). The next change is refused (`root.boundary`), nothing opens, the zone stops
/// (`Halted{boundary}`, recorded and alerted), and only the owner's `/resume` brings it back.
#[test]
fn a0_refuses_a_change_after_the_root_binary_is_replaced_and_halts_the_zone() {
    let h = home("bin");
    h.install();
    let _d = h.daemon();
    assert_eq!(h.open_change('b', "")["ok"], true, "before: A0 passes");
    h.replace_exe("not the binary this Root started from");
    h.refused_change('c');
    assert!(h.alerts().contains("the Root binary"), "{}", h.alerts());
    // the owner's /resume restarts the zone; the binary still differs, so A0 still refuses
    let n = h.of("zone_start").len();
    assert_eq!(h.req("resume", json!({}))["ok"], true);
    wait(30, "the zone after /resume", || h.of("zone_start").len() > n && h.status()["slot"] == "running");
    h.refused_change('d');
}

/// INV-2 / A0: the active charter file changes without `charter check --apply` (which moves the digest with it).
#[test]
fn a0_refuses_a_change_after_the_active_charter_is_rewritten() {
    let h = home("ch");
    h.install();
    let _d = h.daemon();
    let active = h.rh.join("state/charter.active.toml");
    let text = fs::read_to_string(&active).unwrap().replace("L2b test home.", "L2b test home, rewritten.");
    fs::write(&active, text).unwrap();
    h.refused_change('b');
    assert!(h.alerts().contains("active charter"), "{}", h.alerts());
}

/// V3 (fails on c4797cc, where A0 ran a full `charter::load`): a folder the charter's `sandbox.extra_read` names is
/// removed (the agent can, in full access). Rule C14 wants it to exist, but no Root file changed, so A0 still passes and
/// the change opens; an edit of the active charter file itself is still refused.
#[test]
fn a0_does_not_halt_when_an_extra_read_folder_goes_away() {
    let h = home("xr");
    let xr = h.dir.join("xr");
    fs::create_dir_all(&xr).unwrap();
    h.install_with(&CHARTER.replace("[sandbox]\n", &format!("[sandbox]\nextra_read = [\"{}\"]\n", xr.display())));
    let _d = h.daemon();
    assert!(fs::read_to_string(h.rh.join("state/charter.active.toml")).unwrap().contains("extra_read"), "the active charter names it");
    assert_eq!(h.open_change('b', "")["ok"], true, "before: A0 passes");
    fs::remove_dir(&xr).unwrap(); // exact path, made above
    let r = h.open_change('c', "");
    assert_eq!(r["ok"], true, "A0 after the folder went away: {r} {}", h.alerts());
    let opened = h.of("change_open");
    let checks = &opened.last().unwrap()["data"]["checks"];
    assert_eq!(checks["A0"].as_str(), Some("pass"), "{checks}");
    assert_ne!(h.status()["halted"], "boundary");
    let active = h.rh.join("state/charter.active.toml");
    let text = fs::read_to_string(&active).unwrap().replace("L2b test home.", "L2b test home, rewritten.");
    fs::write(&active, text).unwrap();
    h.refused_change('d');
    assert!(h.alerts().contains("active charter"), "{}", h.alerts());
}

/// V2 (fails on c4797cc, where any `halt{on:true}` ended it): a binary no start recorded is refused (`Halted{boundary}`);
/// a restart on it with a bad active charter halts `layout` too (Root::boot); the charter fixed, the next restart on the
/// same binary is still `Halted{boundary}`: only the owner's `/resume` accepts the binary.
#[test]
fn a_boundary_halt_outlives_a_later_integrity_halt() {
    let h = home("bl");
    h.install();
    { let _d = h.daemon(); }
    let zones = h.of("zone_start").len();
    h.replace_exe("put here while the Root was stopped");
    {
        let _d = h.spawn();
        wait(10, "Halted{boundary}", || h.status()["halted"] == "boundary");
    }
    let active = h.rh.join("state/charter.active.toml");
    let good = fs::read_to_string(&active).unwrap();
    fs::write(&active, "schema_version = 1\n[purpose]\n").unwrap(); // no [autonomy]: the active charter fails to load
    {
        let _d = h.spawn();
        wait(10, "Halted{layout}", || h.status()["halted"] == "layout");
    }
    assert_eq!(h.of("halt").last().unwrap()["data"]["reason"], "layout");
    fs::write(&active, good).unwrap();
    {
        let _d = h.spawn();
        wait(10, "still Halted{boundary}", || h.status()["halted"] == "boundary");
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(h.of("zone_start").len(), zones, "the zone never ran on the refused binary");
        assert_eq!(h.req("resume", json!({}))["ok"], true);
        wait(30, "the zone after the owner's /resume", || h.of("zone_start").len() > zones && h.status()["slot"] == "running");
    }
}

/// INV-2 / A0 at start: the Root binary is replaced while the Root is stopped, and no upgrade installed it. That start
/// records the binary in `root_start` but never runs the zone, also across a restart, until the owner's `/resume`
/// accepts it; from then on that binary is the recorded one and starts normally.
#[test]
fn a_root_binary_no_start_or_upgrade_recorded_never_runs_the_zone() {
    let h = home("st");
    h.install();
    { let _d = h.daemon(); }
    let zones = h.of("zone_start").len();
    h.replace_exe("put here while the Root was stopped");
    {
        let _d = h.spawn();
        wait(10, "Halted{boundary}", || h.status()["halted"] == "boundary");
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(h.of("zone_start").len(), zones, "the zone never started");
        assert!(h.alerts().contains("not the one the last Root start recorded"), "{}", h.alerts());
        let starts = h.of("root_start");
        assert_eq!(starts.last().unwrap()["data"]["root_sha256"], sha256(&fs::read(h.exe()).unwrap()).as_str());
    }
    {
        let _d = h.spawn(); // a restart alone does not clear it
        wait(10, "still Halted{boundary}", || h.status()["halted"] == "boundary");
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(h.of("zone_start").len(), zones, "the zone still never started");
        assert_eq!(h.req("resume", json!({}))["ok"], true);
        wait(30, "the zone after the owner's /resume", || h.of("zone_start").len() > zones && h.status()["slot"] == "running");
    }
    let _d = h.daemon(); // the accepted binary is the recorded one now
    assert_eq!(h.open_change('b', "")["ok"], true);
}

/// INV-3: on the free path a kernel `promotion{lkg:true}` was matched to a closed change group by commit sha alone, so a
/// promotion naming a closed change's sha with other blobs formed a tuple nobody admitted or watched, and moved `current`
/// and `lkg` onto it. It must now name that change's content: the tuple it forms must be the one `change.open` admitted.
#[test]
fn a_free_path_promotion_must_name_the_content_of_a_closed_change() {
    let h = home("pr");
    h.install();
    let _d = h.daemon();
    let (b1, b2) = (h.blob(b"the watched body\n"), h.blob(b"a body nobody watched\n"));
    let r = h.open_change('b', &b1);
    assert_eq!(r["ok"], true, "change.open: {r}");
    let open = h.of("change_open").pop().unwrap();
    let (group, watched) = (open["data"]["group"].clone(), open["data"]["tuple"].as_str().unwrap().to_string());
    // the window closes clean (recorded through the Root's own append path)
    let close = json!({"attribution": null, "group": group, "live": {}, "outcome": "clean", "trigger": null, "turns": 0});
    assert_eq!(h.req("test.append", json!({"records": [{"kind": "change_close", "data": close}]}))["ok"], true);
    let lkg_before = h.status()["tuples"]["lkg"].clone();
    let promo = |idem: &str, sha: &str, body: &str| json!({"kind": "promotion", "idem": idem, "data": {"sha": sha, "from_sha": "",
        "gate_job": null, "scope": null, "lkg": true, "body_binary": body, "ext_set": null, "components": []}});
    // the closed change's commit sha, another body: refused, nothing moves
    let r = h.kernel_call("lineage.append", promo("l2b-1", &"b".repeat(40), &b2));
    assert_eq!((r["ok"].as_bool(), r["error"]["code"].as_str()), (Some(false), Some("root.lineage")), "{r}");
    // the closed change's body under another commit sha: refused too
    let r = h.kernel_call("lineage.append", promo("l2b-2", &"e".repeat(40), &b1));
    assert_eq!(r["ok"], false, "{r}");
    let st = h.status();
    assert_eq!((&st["tuples"]["lkg"], st["tuples"]["current"].as_str()), (&lkg_before, Some(watched.as_str())), "nothing moved: {st}");
    assert!(h.of("promotion").is_empty(), "no promotion recorded");
    // the watched content itself: accepted, and lkg is the tuple the Root admitted
    let r = h.kernel_call("lineage.append", promo("l2b-3", &"b".repeat(40), &b1));
    assert_eq!(r["ok"], true, "{r}");
    wait(10, "lkg on the watched tuple", || h.status()["tuples"]["lkg"] == watched.as_str());
}
