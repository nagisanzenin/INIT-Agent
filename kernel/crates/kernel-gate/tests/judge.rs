//! WP8c: `gate judge` (gate.md §4.1a) on a home copy built from WP24's stand-in S and M
//! (root/crates/init-root/tests/fixtures/sealed/, read only), with `init-mockllm` in socket mode as the
//! upstream: $0, no network. G-45 (each M class rejected by the maintainer gate, the controls accepted,
//! identical outputs on a second run, exit codes), G-50 (nothing left in the home copy, no traces).
//!
//! Homes live under `kernel/target/test-homes/WP8/` and are removed by exact path. The only processes
//! are this test's own mock (stopped by its handle) and the judge's children.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use kernel_gate::judge::{self, Args, JudgeCfg, Outcome, sorted_json};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// The watchdog bound of every test here (K-FLAKE: 3-10x its normal runtime; init_testkit::watchdog).
const WD: Duration = Duration::from_secs(960);

fn fix() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../root/crates/init-root/tests/fixtures/sealed")
}

fn test_home(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-homes/WP8")
        .join(format!("{name}-{}", std::process::id()));
    if d.exists() {
        fs::remove_dir_all(&d).unwrap(); // exact path, an earlier run of this test
    }
    fs::create_dir_all(&d).unwrap();
    fs::canonicalize(d).unwrap()
}

fn remove_home(d: &Path) {
    assert!(d.to_string_lossy().contains("/target/test-homes/WP8/"));
    let _ = fs::remove_dir_all(d);
}

/// `INIT_MOCKLLM_BIN`, or the newest kernel-workspace build.
fn mock_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("INIT_MOCKLLM_BIN") {
        return Some(PathBuf::from(p)).filter(|p| p.is_file());
    }
    // This test's own target dir first (`<target>/debug/deps/<test>`): with the default target
    // dir the scan below looks in `kernel/target/debug/debug/` and the judge tests were skipped
    // silently (K-FLAKE).
    let own = std::env::current_exe().ok().and_then(|e| Some(e.parent()?.parent()?.join("init-mockllm")));
    if let Some(p) = own.filter(|p| p.is_file()) {
        return Some(p);
    }
    let k = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    let c: Vec<PathBuf> = fs::read_dir(&k)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().join("debug/init-mockllm"))
        .collect();
    c.into_iter()
        .filter(|p| p.is_file())
        .max_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok())
}

struct Mock(Child, PathBuf);
impl Drop for Mock {
    fn drop(&mut self) {
        let _ = self.0.kill(); // our own child, by its handle
        let _ = self.0.wait();
        let _ = fs::remove_file(&self.1);
    }
}

fn mock(dir: &Path) -> Option<Mock> {
    let Some(bin) = mock_bin() else {
        eprintln!("SKIP: init-mockllm is not built (set INIT_MOCKLLM_BIN)");
        return None;
    };
    let sock = dir.join("m.sock");
    let c = init_testkit::watchdog::spawn(Command::new(bin)
        .args(["--mode", "socket", "--record-bodies", "none", "--socket"])
        .arg(&sock)
        .arg("--tasks-dir")
        .arg(fix().join("mock"))
        .stdout(Stdio::null())
        .stderr(Stdio::null()))
        .unwrap();
    let t = Instant::now();
    while !sock.exists() {
        assert!(t.elapsed() < Duration::from_secs(10), "mock socket");
        std::thread::sleep(Duration::from_millis(20));
    }
    Some(Mock(c, sock))
}

fn sha256(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

/// One stored task (gate.md §3.7.1): tars and the digest from the fixture source.
fn pack(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for (d, tar) in [("ws", "workspace.tar"), ("sol", "solution.tar")] {
        let ok = init_testkit::watchdog::status(Command::new("/usr/bin/tar")
            .env("COPYFILE_DISABLE", "1")
            .args(["--no-xattrs", "-cf"])
            .arg(dst.join(tar))
            .arg("-C")
            .arg(src.join(d))
            .arg("."))
            .unwrap()
            .success();
        assert!(ok);
    }
    assert!(
        init_testkit::watchdog::status(Command::new("/bin/cp")
            .arg("-R")
            .arg(src.join("check"))
            .arg(dst.join("check")))
            .unwrap()
            .success()
    );
    let ws = fs::read(dst.join("workspace.tar")).unwrap();
    let mut t: Value =
        serde_json::from_str(&fs::read_to_string(src.join("task.json")).unwrap()).unwrap();
    t["workspace"] = json!({"bytes": ws.len(), "file": "workspace.tar", "sha256": sha256(&ws)});
    fs::write(dst.join("task.json"), t.to_string()).unwrap();
}

/// The home copy the gate-of-gates builds: pool, holdout and the leak canary.
/// The stand-in S's task ids (the leak canary excluded).
fn s_ids() -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(fix().join("S-src"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|id| id != "sx-canary-token")
        .collect();
    v.sort();
    v
}

/// The gog's sample shape (sealed-eval.md §3.3, FIX5): 4 holdout tasks, the rest of S in the pool.
const GOG_HOLDOUT: [&str; 4] = ["sx-calc-fix", "sx-fib", "sx-primes", "sx-upper"];

fn home_copy(h: &Path, holdout: &str) {
    home_copy_with(h, &[holdout]);
}

/// The home copy with `holdout` as the holdout set and every other S task in the pool.
fn home_copy_with(h: &Path, holdout: &[&str]) {
    for e in fs::read_dir(fix().join("S-src")).unwrap().flatten() {
        let id = e.file_name().to_string_lossy().to_string();
        let set = if id == "sx-canary-token" {
            "vault/canaries"
        } else if holdout.contains(&id.as_str()) {
            "vault/holdout"
        } else {
            "vault/gate/pool"
        };
        pack(&e.path(), &h.join(set).join(&id));
        if id == "sx-canary-token" {
            fs::write(
                h.join(set).join(&id).join("canary.json"),
                r#"{"kind":"leak","v":1}"#,
            )
            .unwrap();
        }
    }
}

/// A class's tree from suite.json's exact-text edits (M5's answer key left empty).
fn variant(out: &Path, ops: &Value) -> PathBuf {
    let mut text = fs::read_to_string(fix().join("M/base/init-body")).unwrap();
    for op in ops.as_array().into_iter().flatten() {
        let (f, w) = (op["find"].as_str().unwrap(), op["with"].as_str().unwrap());
        assert!(text.contains(f), "anchor");
        text = text.replace(f, &w.replace("{{ANSWERS}}", "{}"));
    }
    fs::create_dir_all(out).unwrap();
    fs::write(out.join("init-body"), text).unwrap();
    fs::set_permissions(out.join("init-body"), fs::Permissions::from_mode(0o755)).unwrap();
    out.to_path_buf()
}

/// A 1-task holdout (the rest of S in the pool), below gate.md's minimums.
fn cfg(h: &Path) -> JudgeCfg {
    cfg_with(h, 4, 1)
}

fn cfg_with(h: &Path, min_pool: usize, min_holdout: usize) -> JudgeCfg {
    let o: Vec<String> = vec![
        format!("gate.g4.min_pool={min_pool}"),
        format!("gate.g4.min_holdout={min_holdout}"),
        "gate.judge.quiet_ms=500".into(),
    ];
    JudgeCfg::load(h, &o).unwrap()
}

fn files(d: &Path) -> Vec<String> {
    let mut v = Vec::new();
    let mut stack = vec![d.to_path_buf()];
    while let Some(x) = stack.pop() {
        for e in fs::read_dir(&x).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p.clone());
            }
            v.push(p.strip_prefix(d).unwrap().to_string_lossy().to_string());
        }
    }
    v.sort();
    v
}

fn run(h: &Path, cand: &Path, parent: &Path, m: &Mock, c: &JudgeCfg) -> Outcome {
    let a = Args {
        home: h.to_path_buf(),
        candidate: cand.to_path_buf(),
        running: parent.to_path_buf(),
        lkg: parent.to_path_buf(),
        stages: judge::STAGES.to_vec(),
        level: 0,
        out: h.join("unused"),
    };
    judge::judge(&a, c, Some(m.1.clone()))
}

/// G-45 and G-50 on every class of the stand-in M, in the gog's shape (4 holdout tasks, including the
/// task C1 restores most; the rest of S in the pool) at the gate's default minimums (8 and 4).
#[test]
fn g45_g50_judge_rejects_m_accepts_controls_and_leaves_nothing() {
    let _wd = init_testkit::watchdog::arm("g45_g50_judge_rejects_m_accepts_controls_and_leaves_nothing", Duration::from_secs(810));
    let t = test_home("judge-m");
    let Some(m) = mock(&t) else {
        return remove_home(&t);
    };
    let h = t.join("h");
    home_copy_with(&h, &GOG_HOLDOUT);
    let suite: Value =
        serde_json::from_str(&fs::read_to_string(fix().join("M/suite.json")).unwrap()).unwrap();
    let base = variant(&t.join("trees/base"), &json!([]));
    let c = cfg_with(&h, 8, 4);
    let before = files(&h);
    let want = [
        ("M1", "reject", "test"),
        ("M2", "reject", "forbid_missing"),
        ("M3", "reject", "score_inferior"),
        ("M4", "reject", "canary"),
        ("M5", "reject", "score_inferior"),
        ("M6", "reject", "manifest_error"),
        ("M7", "reject", "canary"),
        ("C1", "accept", "better"),
        ("C2", "accept", "cheaper"),
        ("C3", "accept", "smaller"),
    ];
    let mut got = Vec::new();
    for cl in suite["classes"].as_array().unwrap() {
        let id = cl["id"].as_str().unwrap();
        let cand = variant(&t.join("trees").join(id), &cl["ops"]);
        let parent = if cl.get("parent_ops").is_some() {
            variant(&t.join("trees").join(format!("{id}-p")), &cl["parent_ops"])
        } else {
            base.clone()
        };
        let o = run(&h, &cand, &parent, &m, &c);
        let code = match o.verdict.as_str() {
            "accept" => 0,
            "reject" => 1,
            _ => 2,
        };
        assert_eq!(o.exit_code(), code, "{id}: {}", o.doc);
        assert_eq!(o.doc["record"]["verdict"], json!(o.verdict));
        got.push((id.to_string(), o.verdict.clone(), o.reason.clone()));
        if id == "C1" {
            // the record carries the run data the Root's agreement check reads (§4.4), sorted, no timings
            let runs = o.doc["record"]["runs"].as_array().unwrap();
            assert_eq!(
                runs.iter().filter(|r| r["arm"] == "p").count(),
                2 * s_ids().len(),
                "every S task × R=2 on the LKG arm"
            );
            assert!(
                runs.iter()
                    .all(|r| r["wall_ms"] == 0 && r["outcome"] == "done"),
                "{runs:?}"
            );
            assert!(
                o.doc["record"]["stages"]["G4"]["n"]["holdout"] == 4
                    && o.doc["record"]["spent_musd"].as_u64().unwrap() > 0
            );
            // G-45: the same inputs give byte-identical output
            let again = run(&h, &cand, &parent, &m, &c);
            assert_eq!(
                sorted_json(&again.doc),
                sorted_json(&o.doc),
                "deterministic"
            );
        }
        // G-50: nothing of the run is left in the home copy (no probe roots, scratch, traces or ledgers)
        assert_eq!(files(&h), before, "{id} left files in the home copy");
    }
    let want: Vec<(String, String, String)> = want
        .iter()
        .map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string()))
        .collect();
    assert_eq!(got, want);
    assert!(!h.join("vault/traces").exists() && !h.join("vault/gate/noise.json").exists());
    drop(m);
    remove_home(&t);
}

/// Exit codes and refusals: a non-directory candidate, a cargo tree, G4 without an upstream are `error`
/// (3); a stage subset that passes is `accept: stages_pass`.
#[test]
fn g45_exit_codes_and_refusals() {
    let _wd = init_testkit::watchdog::arm("g45_exit_codes_and_refusals", Duration::from_secs(30));
    let t = test_home("judge-err");
    let h = t.join("h");
    fs::create_dir_all(&h).unwrap();
    let base = variant(&t.join("base"), &json!([]));
    let c = JudgeCfg::load(&h, &[]).unwrap();
    let args = |cand: &Path, stages: Vec<&'static str>| Args {
        home: h.clone(),
        candidate: cand.to_path_buf(),
        running: base.clone(),
        lkg: base.clone(),
        stages,
        level: 0,
        out: t.join("o.json"),
    };
    let o = judge::judge(&args(&t.join("nope"), judge::STAGES.to_vec()), &c, None);
    assert_eq!(
        (o.verdict.as_str(), o.reason.as_str(), o.exit_code()),
        ("error", "candidate_form", 3)
    );
    let cargo = t.join("cargo");
    fs::create_dir_all(&cargo).unwrap();
    fs::write(cargo.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    let o = judge::judge(&args(&cargo, judge::STAGES.to_vec()), &c, None);
    assert_eq!(
        (o.verdict.as_str(), o.reason.as_str()),
        ("error", "unsupported_tree")
    );
    let o = judge::judge(&args(&base, judge::STAGES.to_vec()), &c, None);
    assert_eq!(
        (o.verdict.as_str(), o.reason.as_str()),
        ("error", "no_egress"),
        "{}",
        o.doc
    );
    let o = judge::judge(&args(&base, vec!["G0", "G1", "G2"]), &c, None);
    assert_eq!(
        (o.verdict.as_str(), o.reason.as_str(), o.exit_code()),
        ("accept", "stages_pass", 0),
        "{}",
        o.doc
    );
    assert_eq!(o.doc["stages"], json!(["G0", "G1"]));
    assert!(o.doc["record"]["stages"]["G2"]["skipped"].is_string());
    assert_eq!(
        files(&h),
        Vec::<String>::new(),
        "the home copy is untouched"
    );
    remove_home(&t);
}

/// D14 (FIX2; WP8c finding 1). On a 1-task holdout (the rest of S in the pool, `-c` minimums 4 and 1) a
/// deterministic mock gives δ = 0. Before D14 the divergence guard's threshold was then 2·√(0 + 0) = 0,
/// and C1's gain was `reject/pool_holdout_divergence` on most holdout draws. The divergence floor
/// (δ_p ≥ 0.5/n_pool, δ_h ≥ 0.5) stops that, and the holdout rule makes a score gain on a holdout of fewer
/// than 4 tasks `inconclusive/underpowered:holdout` on every draw. The record carries the measured δ, the
/// floor and the floored δ. On a 4-task holdout without the task C1 restores most, at the default
/// minimums, C1 is `accept/better` (the G-45 test covers a holdout with it).
#[test]
fn c1_is_better_with_a_holdout_of_four_and_inconclusive_with_one() {
    let _wd = init_testkit::watchdog::arm("c1_is_better_with_a_holdout_of_four_and_inconclusive_with_one", Duration::from_secs(570));
    let t = test_home("judge-c1");
    let Some(m) = mock(&t) else {
        return remove_home(&t);
    };
    let suite: Value =
        serde_json::from_str(&fs::read_to_string(fix().join("M/suite.json")).unwrap()).unwrap();
    let cl = suite["classes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "C1")
        .unwrap()
        .clone();
    let cand = variant(&t.join("trees/C1"), &cl["ops"]);
    let parent = variant(&t.join("trees/C1-p"), &cl["parent_ops"]);
    let n = s_ids().len() as f64;
    let pm = |x: f64| Some((x * 1000.0).round() as i64);
    for hold in ["sx-calc-fix", "sx-upper", "sx-wordcount", "sx-squares"] {
        let h = t.join(format!("h-{hold}"));
        home_copy(&h, hold);
        let o = run(&h, &cand, &parent, &m, &cfg(&h));
        assert_eq!(
            (o.verdict.as_str(), o.reason.as_str(), o.exit_code()),
            ("inconclusive", "underpowered:holdout", 2),
            "holdout {hold}: {}",
            o.doc
        );
        let s = &o.doc["record"]["stages"]["G4"]["score"];
        let g = |v: &Value, k: &str| v[k].as_i64();
        assert!(g(s, "l_permille").unwrap() > 0, "a gain: {s}");
        assert_eq!(
            (
                g(s, "delta_permille"),
                g(s, "delta_min_permille"),
                g(s, "margin_permille")
            ),
            (Some(0), pm(0.5 / n), pm(0.5 / n)),
            "{s}"
        );
        for (split, floor) in [("pool", pm(0.5 / (n - 1.0))), ("holdout", Some(500))] {
            assert_eq!(
                (
                    g(&s[split], "delta_permille"),
                    g(&s[split], "delta_min_permille"),
                    g(&s[split], "delta_floored_permille")
                ),
                (Some(0), floor, floor),
                "{s}"
            );
        }
    }
    // A 4-task holdout without `sx-calc-fix`: the rule stands aside and C1 is `better`.
    let h = t.join("h-4");
    home_copy_with(&h, &["sx-reverse", "sx-sort", "sx-upper", "sx-wordcount"]);
    let o4 = run(&h, &cand, &parent, &m, &cfg_with(&h, 8, 4));
    assert_eq!(
        (o4.verdict.as_str(), o4.reason.as_str(), o4.exit_code()),
        ("accept", "better", 0),
        "{}",
        o4.doc
    );
    let n4 = &o4.doc["record"]["stages"]["G4"]["n"];
    assert_eq!(
        (n4["pool"].as_i64(), n4["holdout"].as_i64()),
        (Some(s_ids().len() as i64 - 4), Some(4))
    );
    drop(m);
    remove_home(&t);
}

/// The direct egress (the Root's sealed profile denies `network-bind`, so no relay can listen there):
/// runs are serialized and priced from the upstream's `/_init/stats` difference. A test relay in front
/// of the mock plays the sealed lane. Verdicts and per-run reference costs equal the relay mode's.
#[test]
fn direct_egress_prices_runs_by_the_lane_stats_difference() {
    let _wd = init_testkit::watchdog::arm("direct_egress_prices_runs_by_the_lane_stats_difference", WD);
    let t = test_home("judge-d");
    let Some(m) = mock(&t) else {
        return remove_home(&t);
    };
    let lane =
        kernel_gate::judge::relay::Relay::start(&t.join("l.sock"), Some(m.1.clone()), u64::MAX)
            .unwrap();
    let h = t.join("h");
    home_copy(&h, "sx-calc-fix");
    let suite: Value =
        serde_json::from_str(&fs::read_to_string(fix().join("M/suite.json")).unwrap()).unwrap();
    let base = variant(&t.join("trees/base"), &json!([]));
    let costs = |o: &Outcome| -> Vec<(String, String, u64, u64)> {
        o.doc["record"]["runs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["task"].as_str().unwrap().into(),
                    r["arm"].as_str().unwrap().into(),
                    r["repeat"].as_u64().unwrap(),
                    r["cost_ref_musd"].as_u64().unwrap(),
                )
            })
            .collect()
    };
    for id in ["C2", "M3"] {
        let cl = suite["classes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .unwrap();
        let cand = variant(&t.join("trees").join(id), &cl["ops"]);
        let a = Args {
            home: h.clone(),
            candidate: cand.clone(),
            running: base.clone(),
            lkg: base.clone(),
            stages: judge::STAGES.to_vec(),
            level: 0,
            out: t.join("o"),
        };
        let mut o: Vec<String> = [
            "gate.g4.min_pool=4",
            "gate.g4.min_holdout=1",
            "gate.judge.quiet_ms=500",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let relay_mode = judge::judge(&a, &JudgeCfg::load(&h, &o).unwrap(), Some(m.1.clone()));
        o.push("gate.judge.egress=\"direct\"".into());
        let direct = judge::judge(&a, &JudgeCfg::load(&h, &o).unwrap(), Some(t.join("l.sock")));
        assert_eq!(direct.doc["record"]["egress"], "direct", "{}", direct.doc);
        assert_eq!(relay_mode.doc["record"]["egress"], "relay");
        assert_eq!(
            (direct.verdict.as_str(), direct.reason.as_str()),
            (relay_mode.verdict.as_str(), relay_mode.reason.as_str()),
            "{id}"
        );
        assert_eq!(
            costs(&direct),
            costs(&relay_mode),
            "{id}: per-run reference costs"
        );
        assert!(costs(&direct).iter().all(|c| c.3 > 0));
    }
    assert!(lane.stop().requests > 0);
    drop(m);
    remove_home(&t);
}
