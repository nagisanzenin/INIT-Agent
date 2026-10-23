//! M7 lean (lane ROOT-S1a; build/specs/PROOF-LEAN.md, docs/m7-design.md §3.1.5–§3.2.13, §8): S v1's draw from a
//! pack, the G4 slice (canaries, tamper, leak, the incumbent cache and its sentinel, sealed-lane caps, coarse
//! records), `sealed.g4_slice` on the control link, calibration, the cost probe and the milestone check.
//! Run with `cargo test --offline -p init-root --features test-fixtures --test m7`.
//!
//! Everything runs at $0: the fixture pack `tests/fixtures/sealed/s1fx/` (tiny generated templates, never a real
//! S pack), the Perl fixture body `s1fx/body` in variants, and `init-mockllm` on loopback with a fake `test-` key.
//! Homes live under `root/target/test-homes/m7/` and are deleted by exact path. Every process is this test's own
//! child or one the Root under test spawned and holds.
#![cfg(feature = "test-fixtures")]

use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_init-root");
const FK: &str = env!("CARGO_BIN_EXE_init-root-fakekernel");
const KEY: &str = "test-key-m7-5e5e5e5e5e";
/// The body commit of every fixture tuple. A sealed body run takes its T0 from the lkg tuple's kernel-exported tree
/// `INIT_HOME/lineage/trees/<sha12>/` (REAL-M7's `body_home`), so `install_tc` lays that tree down, as the kernel
/// does on a real install. The Perl fixture bodies ignore T0.
const T0_SHA: &str = "a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7";

fn fx() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sealed/s1fx")
}

fn base() -> PathBuf {
    fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap().join("test-homes/m7")
}

fn mock_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("INIT_MOCKLLM_BIN") {
        return Some(PathBuf::from(p)).filter(|p| p.is_file());
    }
    let k = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../kernel/target");
    let mut c = vec![k.join("debug/init-mockllm")];
    c.extend(fs::read_dir(&k).into_iter().flatten().flatten().map(|e| e.path().join("debug/init-mockllm")));
    c.into_iter().filter(|p| p.is_file()).max_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok())
}

struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill(); // our own child, by its handle
        let _ = self.0.wait();
    }
}

struct Home {
    dir: PathBuf,
    rh: PathBuf,
    ih: PathBuf,
    port: u16,
    mock: Option<Daemon>,
}

impl Drop for Home {
    fn drop(&mut self) {
        drop(self.mock.take());
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.dir); // exact path, this test's own home
        }
    }
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
    String::from_utf8_lossy(&c.wait_with_output().unwrap().stdout).split_whitespace().next().unwrap().to_string()
}

/// A fresh home `test-homes/m7/<name>` (≤ 4 bytes, so lane sockets fit `sun_path`) with its own mock.
fn home(name: &str) -> Option<Home> {
    let Some(mock) = mock_bin() else {
        eprintln!("SKIP: init-mockllm is not built (set INIT_MOCKLLM_BIN)");
        return None;
    };
    let dir = base().join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap(); // exact path, left by an earlier run of this test
    }
    fs::create_dir_all(dir.join("rh")).unwrap();
    fs::create_dir_all(dir.join("tasks")).unwrap(); // no task scripts: every fixture prompt gets the mock's give-up turn
    fs::set_permissions(dir.join("rh"), fs::Permissions::from_mode(0o700)).unwrap();
    let pf = dir.join("mock.port");
    let m = Daemon(Command::new(mock).args(["--mode", "upstream", "--listen", "127.0.0.1:0", "--record-bodies", "none", "--port-file"]).arg(&pf)
        .arg("--tasks-dir").arg(dir.join("tasks")).arg("--record-dir").arg(dir.join("mock")).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
    wait(10, "mock port", || fs::read_to_string(&pf).is_ok_and(|s| s.trim().parse::<u16>().is_ok()));
    let port = fs::read_to_string(&pf).unwrap().trim().parse().unwrap();
    Some(Home { rh: dir.join("rh"), ih: dir.join("ih"), dir, port, mock: Some(m) })
}

impl Home {
    fn charter(&self) -> String {
        format!("schema_version = 1\n[purpose]\ntext = \"M7 test home.\"\n[autonomy]\nmax_level = 4\n[quotas]\ndisk_min_free_gib = 0\n\
            [root]\ntest_mode = true\n[money]\nday_usd = \"5.00\"\ntotal_usd = \"10.00\"\n[actions.providers]\ndestinations = [\"127.0.0.1:{p}\"]\n\
            paths = [\"chat/completions\"]\n[actions.providers.models.\"deepseek-flash\"]\nmin_level = 0\ninput_musd_per_mtok = 300000\n\
            output_musd_per_mtok = 1200000\n[actions.providers.providers.deepseek]\nhost = \"127.0.0.1:{p}\"\n\
            [sealed]\naudit_schedule = \"manual\"\naudit_usd = \"2.00\"\nreserve_usd = \"3.00\"\nparallel = 4\n", p = self.port)
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
    fn err(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert_ne!(o.status.code(), Some(0), "{args:?} should fail: {}", String::from_utf8_lossy(&o.stdout));
        String::from_utf8_lossy(&o.stderr).to_string()
    }
    /// The store, a test chain (tuples, lkg, audited) and the install with the provider key.
    fn install(&self, records: Vec<Value>) {
        self.install_tc(records, &self.dir.join("tc"));
    }
    fn install_tc(&self, records: Vec<Value>, tc: &Path) {
        fs::write(self.rh.join("charter.toml"), self.charter()).unwrap();
        let spec = self.dir.join("spec.json");
        fs::write(&spec, json!({"start_at": "2026-10-20T00:00:00Z", "start_level": 0, "records": records}).to_string()).unwrap();
        let o = self.run(&["test", "ledger", "--root-home", self.rh.to_str().unwrap(), "--spec", spec.to_str().unwrap()]);
        assert_eq!(o.status.code(), Some(0), "test ledger: {}", String::from_utf8_lossy(&o.stderr));
        let ch = self.dir.join("charter.toml");
        fs::write(&ch, self.charter()).unwrap();
        fs::create_dir_all(self.dir.join("tc/bin")).unwrap();
        self.ok(&["install", "--charter", ch.to_str().unwrap(), "--toolchain", tc.to_str().unwrap()]);
        let tree = self.ih.join("lineage/trees").join(&T0_SHA[..12]);
        fs::create_dir_all(&tree).unwrap();
        for f in ["GENESIS.md", "SELF.md"] {
            fs::write(tree.join(f), format!("# {f} (m7 fixture T0)\n")).unwrap();
        }
        let k = self.rh.join("keys/providers/deepseek");
        fs::write(&k, KEY).unwrap();
        fs::set_permissions(&k, fs::Permissions::from_mode(0o600)).unwrap();
    }
    fn store(&self) {
        for d in ["store", "store/blobs", "store/tuples"] {
            fs::create_dir_all(self.rh.join(d)).unwrap();
            fs::set_permissions(self.rh.join(d), fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    fn put(&self, b: &[u8]) -> String {
        let sha = sha256(b);
        let p = self.rh.join("store/blobs").join(&sha);
        if !p.exists() {
            fs::write(&p, b).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o444)).unwrap();
        }
        sha
    }
    /// A fixture body variant (`MODE` replaced; `extra` appended as a comment) in the store: (sha256, bytes).
    fn body(&self, mode: &str, extra: &str) -> String {
        let text = fs::read_to_string(fx().join("body")).unwrap().replace("my $MODE = 'good';", &format!("my $MODE = '{mode}';"));
        self.put(format!("{text}# {extra}\n").as_bytes())
    }
    /// A tuple whose body is `sha` (the kernel is the fake kernel): its id and JSON.
    fn tuple(&self, sha: &str) -> (String, Value) {
        let k = self.put(&fs::read(FK).unwrap());
        let t = json!({"body": {"binary": sha, "sha": T0_SHA}, "ext": {"components": [], "set": null},
            "kernel": {"built_from": "", "init_kernel": k, "initctl": "", "source": "maintainer", "version": "m7"}, "v": 1});
        let id = format!("t{}", &sha256(serde_json::to_string(&t).unwrap().as_bytes())[..12]);
        fs::write(self.rh.join("store/tuples").join(format!("{id}.json")), format!("{}\n", serde_json::to_string(&t).unwrap())).unwrap();
        (id, t)
    }
    fn records(&self) -> Vec<Value> {
        fs::read_to_string(self.rh.join("lineage/lineage.jsonl")).unwrap_or_default().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }
    fn kinds(&self, k: &str) -> Vec<Value> {
        self.records().into_iter().filter(|r| r["kind"] == k).map(|r| r["data"].clone()).collect()
    }
    fn s(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.rh.join("sealed/S.json")).unwrap()).unwrap()
    }
    fn alerts(&self) -> String {
        fs::read_to_string(self.rh.join("alerts.jsonl")).unwrap_or_default()
    }
    /// The fixture pack in this home: E 12 (Python and Rust labels), C 10, T 10, two broken B templates and the
    /// 4 canaries; composition E 10+2, C 8+2, T 8+2, B 2+0 (so both broken ones are drawn and skipped).
    fn pack(&self) -> PathBuf {
        let p = self.dir.join("pack");
        fs::create_dir_all(p.join("lib")).unwrap();
        for f in ["fxgen.py", "run.py"] {
            fs::copy(fx().join("lib").join(f), p.join("lib").join(f)).unwrap();
        }
        let comp = json!({"E": {"g4": 10, "reserve": 2}, "C": {"g4": 8, "reserve": 2}, "T": {"g4": 8, "reserve": 2}, "B": {"g4": 2, "reserve": 0},
            "canary": {"g4": 4}, "note": "s1fx"});
        fs::write(p.join("pack.json"), json!({"pack": "s1fx", "version": "1", "composition": comp}).to_string()).unwrap();
        let tpl = |fam: &str, id: &str, t: Value| {
            let d = p.join(fam).join(id);
            fs::create_dir_all(d.join("gen")).unwrap();
            fs::copy(fx().join("lib/run.py"), d.join("gen/run")).unwrap();
            fs::set_permissions(d.join("gen/run"), fs::Permissions::from_mode(0o755)).unwrap();
            let mut v = json!({"v": 1, "id": id, "family": fam, "lang": "py", "title": id, "difficulty": {"default": 3, "min": 1, "max": 5}, "licence": "INIT"});
            v.as_object_mut().unwrap().extend(t.as_object().unwrap().clone());
            fs::write(d.join("template.json"), v.to_string()).unwrap();
        };
        for i in 0..12 {
            tpl("E", &format!("fx-e-{i:02}"), json!({"lang": if i % 2 == 0 { "py" } else { "rust" }}));
        }
        for i in 0..10 {
            tpl("C", &format!("fx-c-{i:02}"), json!({}));
            tpl("T", &format!("fx-t-{i:02}"), json!({"lang": "sh"}));
        }
        for i in 0..2 {
            tpl("B", &format!("fx-b-{i:02}"), json!({"broken": true}));
        }
        for k in ["health", "impossible", "tamper", "leak"] {
            tpl("canary", k, json!({"canary": k, "family": "canary"}));
        }
        fs::create_dir_all(p.join("E/_helper")).unwrap(); // a `_` helper dir is never a template
        p
    }
    fn draw(&self) -> Value {
        let p = self.pack();
        self.ok(&["sealed", "draw", "--pack-dir", p.to_str().unwrap()])
    }
    fn slice(&self, gate: &str, body: &str, inc: &str, n: u64, r: u64) -> Output {
        let (n, r) = (n.to_string(), r.to_string());
        self.run(&["sealed", "slice", "--gate-id", gate, "--body", body, "--incumbent", inc, "--tier", "lean", "--n", &n, "--repeats", &r])
    }
    fn slice_ok(&self, gate: &str, body: &str, inc: &str, n: u64, r: u64) -> Value {
        let o = self.slice(gate, body, inc, n, r);
        assert_eq!(o.status.code(), Some(0), "{gate}: {}", String::from_utf8_lossy(&o.stderr));
        serde_json::from_slice(&o.stdout).unwrap()
    }
    fn detail(&self, gate: &str) -> Value {
        self.ok(&["sealed", "show", "--g4", gate])
    }
    fn owner(&self, req: Value) -> Option<Value> {
        use std::io::{BufRead, Write};
        let mut s = std::os::unix::net::UnixStream::connect(self.rh.join("run/owner.sock")).ok()?;
        writeln!(s, "{req}").ok()?;
        let mut line = String::new();
        std::io::BufReader::new(&s).read_line(&mut line).ok()?;
        serde_json::from_str(&line).ok()
    }
}

/// This install's GUID is in every drawn instance's workspace (the generators got `INIT_S1_GUID`), the leak
/// canary's included. Only bytes are compared; no instance is read as content.
fn guid_everywhere(h: &Home) -> bool {
    let g = fs::read_to_string(h.rh.join("sealed/guid")).unwrap();
    let g = g.trim().as_bytes();
    let s = h.s();
    let mut ids: Vec<String> = s["canary"].as_object().unwrap().values().map(|v| v.as_str().unwrap().to_string()).collect();
    assert_eq!(ids.len(), 4);
    ids.extend(s["g4"].as_array().unwrap().iter().chain(s["reserve"].as_array().unwrap()).map(|v| v.as_str().unwrap().to_string()));
    ids.iter().all(|id| fs::read(h.rh.join("sealed/S").join(id).join("workspace.tar")).unwrap().windows(32).any(|w| w == g))
}

fn chain(tuples: &[(&str, &Value)], lkg: &str) -> Vec<Value> {
    let mut v: Vec<Value> = tuples.iter().enumerate().map(|(i, (id, t))| {
        json!({"at": format!("2026-10-20T00:0{i}:00Z"), "kind": "tuple", "data": {"id": id, "tuple": t}})
    }).collect();
    v.push(json!({"at": "2026-10-20T00:05:00Z", "kind": "rollback", "data": {"from": null, "reason": "test", "target": "test", "to": lkg}}));
    v.push(json!({"at": "2026-10-20T00:05:00Z", "kind": "lkg", "data": {"tuple": lkg}}));
    v.push(json!({"at": "2026-10-20T00:05:00Z", "kind": "audited", "data": {"tuple": lkg}}));
    v
}

/// No S instance id, template name or the GUID in any lineage record (m7-design.md §4.2, §3.2.11).
fn records_clean(h: &Home) {
    let text = fs::read_to_string(h.rh.join("lineage/lineage.jsonl")).unwrap();
    let s = h.s();
    for list in [&s["g4"], &s["reserve"], &s["out_of_band"]] {
        for id in list.as_array().unwrap() {
            assert!(!text.contains(id.as_str().unwrap()), "an S id is in the lineage");
        }
    }
    let guid = fs::read_to_string(h.rh.join("sealed/guid")).unwrap();
    assert!(!text.contains("fx-e-") && !text.contains("fx-c-") && !text.contains(guid.trim()), "template or GUID in the lineage");
}

/// M7-R01, R02 (draw: composition, provenance, broken templates skipped, `_` helpers ignored, < 24 → sealed_none);
/// M7-R04 (superior, tie; inferior through the control link), the canaries (M7-P06 on the Root side: impossible and
/// tamper canaries trip `canary`, `health`, `leak`), the orchestrator's tamper check (a protected test edited →
/// `tamper`), M7-R08/R09 lean (cache reuse, sentinel drift), M7-R11 (the turn cap on actuals), M7-R13 (coarse
/// records, full detail for the owner), M7-R12 (incumbent run errors), M7-G13's Root half (`error:harness_health`)
/// and the refusals.
#[test]
fn m7_draw_and_g4_slice() {
    let Some(h) = home("ds") else { return };
    h.store();
    let (bad, good, nohealth, silent) = (h.body("bad", "bad"), h.body("good", "good"), h.body("nohealth", "nohealth"), h.body("silent", "silent"));
    let ((tb, t_b), (tg, t_g), (tn, t_n), (ts, t_s)) = (h.tuple(&bad), h.tuple(&good), h.tuple(&nohealth), h.tuple(&silent));
    h.install(chain(&[(&tg, &t_g), (&tn, &t_n), (&ts, &t_s), (&tb, &t_b)], &tb));
    // M7-R01/R02: the draw
    let d = h.draw();
    let c = &d["sealed_draw"]["counts"];
    assert_eq!(c, &json!({"B": {"g4": 0, "reserve": 0}, "C": {"g4": 8, "reserve": 2}, "E": {"g4": 10, "reserve": 2}, "T": {"g4": 8, "reserve": 2},
        "canary": 4}), "{d}");
    assert_eq!((d["sealed_draw"]["skipped_templates"].as_u64(), d["sealed_none"].as_bool()), (Some(2), Some(false)), "{d}");
    assert_eq!(h.kinds("sealed_draw"), vec![d["sealed_draw"].clone()]);
    let s = h.s();
    assert_eq!((s["g4"].as_array().unwrap().len(), s["reserve"].as_array().unwrap().len(), s["s_version"].as_str()), (26, 6, Some("s1fx.0")));
    let id0 = s["g4"][0].as_str().unwrap();
    let prov: Value = serde_json::from_str(&fs::read_to_string(h.rh.join("sealed/S").join(id0).join("provenance.json")).unwrap()).unwrap();
    for k in ["pack", "pack_version", "family", "template", "difficulty", "licence", "partition", "drawn_at", "s_version", "check_digest"] {
        assert!(!prov[k].is_null(), "provenance {k}: {prov}");
    }
    let seeds = h.rh.join("sealed/seeds").join(format!("{id0}.json"));
    assert_eq!(fs::metadata(&seeds).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(fs::read_to_string(h.rh.join("logs/root.jsonl")).unwrap_or_default().contains("fx-b-"), "skipped templates are logged");
    assert!(h.alerts().contains("sealed_template"));
    assert!(h.err(&["sealed", "draw"]).contains("already drawn"));
    assert!(guid_everywhere(&h), "INIT_S1_GUID reaches every generator");
    // refusals: incumbent not lkg, a missing blob, fewer than 24 G4 tasks (since the 2026-10-23 observe-only money
    // policy a budget below the estimate is no longer a refusal)
    assert!(String::from_utf8_lossy(&h.slice("g-args", &good, &tg, 8, 1).stderr).contains("incumbent must be the lkg"));
    assert!(String::from_utf8_lossy(&h.slice("g-store", &"0".repeat(64), &tb, 8, 1).stderr).contains("not in the store"));
    let full = h.s();
    let mut small = full.clone();
    small["g4"] = json!(full["g4"].as_array().unwrap()[..20].to_vec());
    fs::write(h.rh.join("sealed/S.json"), small.to_string()).unwrap();
    assert!(String::from_utf8_lossy(&h.slice("g-none", &good, &tb, 8, 1).stderr).contains("needs 24"));
    fs::write(h.rh.join("sealed/S.json"), full.to_string()).unwrap();
    // tie (the same body), all 26 tasks: every incumbent run enters the cache
    let v = h.slice_ok("g-tie", &bad, &tb, 26, 1);
    assert_eq!((v["status"].as_str(), v["n"].as_u64(), v["d_permille"].as_i64(), v["dup"].as_bool(), v["looks_left"].clone()),
        (Some("tie"), Some(26), Some(0), Some(false), Value::Null), "{v}");
    // superior, reusing the cached incumbent runs, with a fresh sentinel
    let v = h.slice_ok("g-sup", &good, &tb, 8, 1);
    assert_eq!((v["status"].as_str(), v["ratchet_signal"].as_bool(), v["d_permille"].as_i64(), v["ratchet_k"].as_u64(), v["unconfirmed"].as_u64()),
        (Some("superior"), Some(true), Some(1000), Some(4), Some(0)), "{v}");
    let g4 = h.kinds("sealed_g4");
    let rec = g4.last().unwrap();
    assert_eq!(rec["cache"], json!({"reused": true, "sentinel_ok": true}), "{rec}");
    assert!(rec["d_permille"].as_i64().unwrap() % 50 == 0 && rec["m_permille"].as_i64().unwrap() % 10 == 0 && rec["cpp_ratio_permille"].as_i64().unwrap() % 50 == 0);
    let det = h.detail("g-sup");
    assert_eq!(det["tasks"].as_array().unwrap().len(), 8, "the owner sees per-task detail");
    assert!(det["tasks"].as_array().unwrap().iter().all(|t| t["inc_cached"] == 1 && t["k"] == 1000), "{det}");
    assert_eq!(det["sentinel"]["drift"], false);
    // the orchestrator's red-team check: a candidate that edits a protected test is rejected `tamper`, and the slice
    // stops after the half in which it was seen
    let tam = h.body("tamper", "tamper");
    let v = h.slice_ok("g-tam", &tam, &tb, 8, 1);
    assert_eq!((v["status"].as_str(), v["n"].as_u64(), v["d_permille"].as_i64()), (Some("tamper"), Some(0), Some(0)), "{v}");
    assert!(h.detail("g-tam")["tasks"].is_null(), "no statistics after a tamper");
    // canaries: special-casing (impossible) and a test edit on the tamper canary trip `canary`; no health → `health`;
    // this install's GUID inside the body → `leak`
    for (mode, want) in [("special", "canary"), ("tampercanary", "canary"), ("nohealth", "health")] {
        let b = h.body(mode, mode);
        let v = h.slice_ok(&format!("g-{mode}"), &b, &tb, 8, 1);
        assert_eq!(v["status"].as_str(), Some(want), "{mode}: {v}");
    }
    let guid = fs::read_to_string(h.rh.join("sealed/guid")).unwrap();
    let leaky = h.body("good", guid.trim());
    assert_eq!(h.slice_ok("g-leak", &leaky, &tb, 8, 1)["status"], "leak");
    assert!(h.alerts().contains("sealed_leak"));
    // M7-R11: the turn cap on actuals: 4 turns admitted, the 5th refused 429; the runs are valid and scored
    let caps = h.body("caps", "caps");
    let v = h.slice_ok("g-caps", &caps, &tb, 8, 1);
    assert_eq!(v["status"], "superior", "{v}");
    let det = h.detail("g-caps");
    assert_eq!(det["stats"]["caps"]["cand"], 8, "{det}");
    assert_eq!(det["stats"]["caps"]["cand_turns"], 32, "4 turns per run");
    // M7-R09: a 100-point drift in the cached incumbent runs → the sentinel (6 of 26 tasks) drops the cache, alert,
    // fresh arm
    for e in fs::read_dir(h.rh.join("sealed/cache")).unwrap().flatten() {
        let t = fs::read_to_string(e.path()).unwrap().replace("\"score_permille\":0", "\"score_permille\":1000");
        fs::write(e.path(), t).unwrap();
    }
    let v = h.slice_ok("g-drift", &good, &tb, 26, 1);
    assert_eq!(v["status"], "superior", "{v}");
    assert_eq!(h.kinds("sealed_g4").last().unwrap()["cache"], json!({"reused": false, "sentinel_ok": false}));
    assert!(h.alerts().contains("incumbent_drift"));
    assert!(h.detail("g-drift")["tasks"].as_array().unwrap().iter().all(|t| t["inc_cached"] == 0));
    records_clean(&h);
    assert!(fs::read_dir(h.rh.join("sealed/run")).unwrap().next().is_none(), "every run root is deleted");
    // the control link: a running Root with the fake kernel; `sealed.g4_slice` answers asynchronously while the
    // kernel's link stays live. M7-R04 inferior; then M7-G13's Root half: an incumbent without health → error
    let info: Value = serde_json::from_str(&fs::read_to_string(h.rh.join("install.json")).unwrap()).unwrap();
    let _d = Daemon(h.cmd().arg("start").env("INIT_TEST_FK", "").env("INIT_TEST_TOOLCHAIN", info["toolchain"].as_str().unwrap())
        .env("INIT_TEST_DEVDIR", info["devdir"].as_str().unwrap()).env("INIT_TEST_HOME_REAL", &h.dir)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
    wait(30, "owner.sock", || h.owner(json!({"args": {}, "cmd": "status", "id": "s", "v": 1})).is_some());
    let r = h.owner(json!({"args": {"records": [{"kind": "lkg", "data": {"tuple": tg}}]}, "cmd": "test.append", "id": "t", "v": 1}));
    assert_eq!(r.as_ref().map(|v| v["ok"].clone()), Some(json!(true)), "{r:?}");
    let fk_log = h.ih.join("state/kernel/fk.jsonl");
    wait(30, "the fake kernel", || fs::read_to_string(&fk_log).is_ok_and(|s| s.contains("ready")));
    fs::create_dir_all(h.ih.join("work")).unwrap();
    let call = |args: Value| {
        let n = fs::read_to_string(&fk_log).unwrap_or_default().matches("\"ev\":\"call\"").count();
        let tmp = h.ih.join("work/fk-call.tmp");
        fs::write(&tmp, json!({"cmd": "sealed.g4_slice", "args": args}).to_string()).unwrap();
        fs::rename(&tmp, h.ih.join("work/fk-call.json")).unwrap();
        wait(60, "the slice's answer on the control link", || fs::read_to_string(&fk_log).unwrap_or_default().matches("\"ev\":\"call\"").count() > n);
        let log = fs::read_to_string(&fk_log).unwrap();
        serde_json::from_str::<Value>(log.lines().rev().find(|l| l.contains("\"ev\":\"call\"")).unwrap()).unwrap()
    };
    let args = |gate: &str, body: &str, inc: &str| json!({"gate_id": gate, "candidate": {"body": body, "ext": {"set": null, "components": []}, "t0": null},
        "incumbent": inc, "tier": {"name": "lean", "n": 8, "repeats": 1}, "budget_musd": 3_000_000, "wall_s": 600});
    let r = call(args("g-inf", &bad, &tg));
    assert_eq!(r["ok"], true, "{r}");
    assert!(["inferior", "inferior_early"].contains(&r["result"]["status"].as_str().unwrap()), "{r}");
    for k in ["seq", "status", "ratchet_signal", "unconfirmed", "ratchet_k", "tier", "n", "repeats", "d_permille", "m_permille", "cpp_ratio_permille",
        "looks_left", "s_version", "dup", "spent_musd"] {
        assert!(r["result"].get(k).is_some(), "result.{k}: {r}");
    }
    let r = call(json!({"gate_id": "g-x"}));
    assert_eq!(r["error"]["code"], "root.args", "{r}");
    let r = h.owner(json!({"args": {"records": [{"kind": "lkg", "data": {"tuple": tn}}]}, "cmd": "test.append", "id": "t", "v": 1}));
    assert_eq!(r.as_ref().map(|v| v["ok"].clone()), Some(json!(true)), "{r:?}");
    let r = call(args("g-hh", &good, &tn));
    assert_eq!(r["result"]["status"], "error:harness_health", "{r}");
    // M7-R12: an incumbent whose runs make no model request → `error:runs_failed:<outcome>` (c712701)
    let r = h.owner(json!({"args": {"records": [{"kind": "lkg", "data": {"tuple": ts}}]}, "cmd": "test.append", "id": "t", "v": 1}));
    assert_eq!(r.as_ref().map(|v| v["ok"].clone()), Some(json!(true)), "{r:?}");
    let r = call(args("g-rf", &good, &ts));
    assert!(r["result"]["status"].as_str().unwrap().starts_with("error:runs_failed:"), "{r}");
    assert!(h.kinds("sealed_g4").iter().all(|g| g["looks_left"].is_null() && g["dup"] == false));
}

/// M7-R14 (calibration on the LKG: band selection, the pseudo-A/A record, δ_S into noise.json, selection runs never
/// reused), M7-R18 (the cost probe: throw-away instances deleted, per-family aggregates, S untouched), M7-R17 lean
/// (milestone: confirmed, the reserve used up, regression with its alert, partial keeps the reserve).
#[test]
fn m7_calibrate_probe_milestone() {
    let Some(h) = home("cm") else { return };
    h.store();
    let (bad, good) = (h.body("bad", "bad"), h.body("good", "good"));
    let ((tb, t_b), (tg, t_g)) = (h.tuple(&bad), h.tuple(&good));
    h.install(chain(&[(&tb, &t_b), (&tg, &t_g)], &tg));
    h.draw();
    let before = h.s();
    // M7-R18
    let v = h.ok(&["sealed", "probe", "--per-family", "1"]);
    let pf = &v["sealed_calibrate"]["probe"]["per_family"];
    for g in ["E.py", "E.rust", "C", "T"] {
        assert_eq!((pf[g]["runs"].as_u64(), pf[g]["passes"].as_u64()), (Some(1), Some(1)), "{g}: {v}");
        assert!(pf[g]["requests"].as_u64() >= Some(1) && pf[g]["cost_musd"].as_u64() > Some(0), "{v}");
    }
    assert_eq!(pf["B"]["failed"], 1, "the broken family fails to generate: {v}");
    assert_eq!(h.s(), before, "the probe changes no partition");
    assert!(fs::read_dir(h.rh.join("sealed/run")).unwrap().next().is_none(), "throw-away instances deleted by exact path");
    // M7-R14 with a deterministic LKG: every task 6/6, so nothing is in band and S stays unselected
    let v = h.ok(&["sealed", "calibrate", "--repeats", "6", "--splits", "50"]);
    let c = &v["sealed_calibrate"];
    assert_eq!((c["selected"].as_bool(), c["n_kept"].as_u64(), c["fa_permille"].as_u64(), c["partial"].as_bool()), (Some(false), Some(26), Some(0), Some(false)), "{v}");
    assert_eq!(v["in_band"], 0);
    for k in ["delta_permille", "mde_permille", "var_between_permille", "var_within_permille", "spent_musd", "s_version", "verdict"] {
        assert!(!c[k].is_null(), "{k}: {v}");
    }
    let nz: Value = serde_json::from_str(&fs::read_to_string(h.rh.join("sealed/noise.json")).unwrap()).unwrap();
    assert_eq!((nz["s1"]["n"].as_u64(), nz["s1"]["r3_permille"].as_i64()), (Some(26), Some(0)), "{nz}");
    // selection runs are never reused: the next slice runs its incumbent fresh
    let v = h.slice_ok("g-cal", &good, &tg, 8, 1);
    assert_eq!(v["status"], "tie");
    assert_eq!(h.kinds("sealed_g4").last().unwrap()["cache"]["reused"], false);
    // M7-R17 lean: current (good) against genesis (bad) on the 6 reserve tasks: confirmed, the reserve is used up
    let v = h.ok(&["sealed", "milestone", "--repeats", "2"]);
    let m = &v["milestone"];
    assert_eq!((m["outcome"].as_str(), m["current"].as_str(), m["confirmed_before"].as_str(), m["n"].as_u64(), m["held_out"].as_bool(), m["ratchet_k_next"].as_u64()),
        (Some("confirmed"), Some(tg.as_str()), Some(tb.as_str()), Some(6), Some(true), Some(4)), "{v}");
    assert_eq!(h.s()["reserve_used"], true);
    assert!(h.err(&["sealed", "milestone"]).contains("used by an earlier milestone"));
    assert!(h.err(&["sealed", "milestone", "--force"]).contains("is the confirmed one"), "current = the last confirmed tuple");
    // a regression (good confirmed, bad current) halves ratchet_k and raises the alert
    let v = h.ok(&["sealed", "milestone", "--force", "--repeats", "2", "--current", &tb, "--baseline", &tg]);
    assert_eq!((v["milestone"]["outcome"].as_str(), v["milestone"]["ratchet_k_next"].as_u64()), (Some("regression"), Some(2)), "{v}");
    assert!(h.alerts().contains("milestone_regression"));
    // observe-only money (2026-10-23): a $0.01 value no longer cuts the check short; the spend is recorded
    let mut s = h.s();
    s["reserve_used"] = json!(false);
    fs::write(h.rh.join("sealed/S.json"), s.to_string()).unwrap();
    let v = h.ok(&["sealed", "milestone", "--usd", "0.01", "--current", &tg, "--baseline", &tb]);
    assert_ne!(v["milestone"]["outcome"].as_str(), Some("partial"), "{v}");
    assert!(v["milestone"]["spent_musd"].as_u64().unwrap() > 0, "the check ran (the old reservation rule ran nothing) and its spend is recorded: {v}");
    records_clean(&h);
}

/// S1-CORE's integration needs on the real pack (root/sealed/packs/s1, integration only): the Root's draw generates
/// and validates its Rust E templates and the 4 canaries in the Root's own sealed check environment, so every Rust
/// template's reference scores 1000 there (cargo and rustc from the INIT toolchain on PATH, the SDK for linking);
/// every generator gets this install's GUID (the leak canary carries it). The test asserts counts and bytes only.
#[test]
fn m7_real_pack_rust_and_canaries() {
    let pack = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sealed/packs/s1");
    let sysroot = Command::new("rustc").args(["--print", "sysroot"]).output().ok().filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()));
    let (Some(tc), true) = (sysroot, pack.join("pack.json").is_file()) else {
        eprintln!("SKIP: rustc or the s1 pack is not available");
        return;
    };
    let Some(h) = home("rp") else { return };
    h.store();
    let good = h.body("good", "good");
    let (tg, t_g) = h.tuple(&good);
    h.install_tc(chain(&[(&tg, &t_g)], &tg), &tc);
    let p = h.dir.join("pack");
    fs::create_dir_all(p.join("E")).unwrap();
    let cp = |from: &Path, to: &Path| assert!(Command::new("/bin/cp").arg("-R").arg(from).arg(to).status().unwrap().success());
    for d in ["lib", "runner", "LICENSES", "canary"] {
        cp(&pack.join(d), &p.join(d));
    }
    let mut n = 0;
    for e in fs::read_dir(pack.join("E")).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('_') || name.starts_with("e-rs-") {
            cp(&e.path(), &p.join("E").join(&name));
            n += usize::from(name.starts_with("e-rs-"));
        }
    }
    let mut pj: Value = serde_json::from_str(&fs::read_to_string(pack.join("pack.json")).unwrap()).unwrap();
    pj["version"] = json!("m7-test");
    pj["composition"] = json!({"E": {"g4": n, "reserve": 0}});
    fs::write(p.join("pack.json"), pj.to_string()).unwrap();
    let d = h.ok(&["sealed", "draw", "--pack-dir", p.to_str().unwrap()]);
    let dr = &d["sealed_draw"];
    assert_eq!((dr["counts"]["E"]["g4"].as_u64(), dr["counts"]["canary"].as_u64(), dr["skipped_templates"].as_u64()),
        (Some(n as u64), Some(4), Some(0)), "every Rust template and canary validates under the Root's check env: {d}");
    assert!(n >= 1 && guid_everywhere(&h), "INIT_S1_GUID reaches every generator, the leak canary's included");
}
