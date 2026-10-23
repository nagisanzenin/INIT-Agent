//! EV-03 (`finish.*` on scripted runs; red-then-green is not friction), EV-04 (fatal attribution,
//! s_c and divergence pairs on a synthetic pool), EV-05 (incremental mining = a full rescan, and
//! its speed).

use kernel_evo::classify::PathCtx;
use kernel_evo::mine::{self, Cluster, MineCfg};
use kernel_evo::session;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn home(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/EVO-LITE").join(format!("{name}-{}", std::process::id()));
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap();
    }
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn remove(d: &Path) {
    assert!(d.to_string_lossy().contains("/target/test-homes/EVO-LITE/"));
    let _ = std::fs::remove_dir_all(d);
}

/// A seed-format log from `(kind, a, b)` steps: ("owner", text), ("bash", cmd, output),
/// ("write", path, ""), ("final", text).
fn log(sid: &str, steps: &[(&str, &str, &str)]) -> String {
    let mut out = vec![json!({"type": "session", "session_id": sid, "id": "0000000000000000", "parent_id": null, "ts": "2026-10-20T10:00:00.000Z"}).to_string()];
    let mut n = 1u32;
    let mut ev = |v: Value| {
        let mut v = v;
        v["id"] = json!(format!("{n:016x}"));
        v["ts"] = json!(format!("2026-10-20T10:{:02}:{:02}.000Z", n / 60, n % 60));
        n += 1;
        out.push(v.to_string());
    };
    for (i, (k, a, b)) in steps.iter().enumerate() {
        let cid = format!("c{i}");
        match *k {
            "owner" => ev(json!({"type": "message", "role": "user", "source": "owner", "content": [{"type": "text", "text": a}]})),
            "final" => ev(json!({"type": "message", "role": "assistant", "content": [{"type": "text", "text": a}]})),
            "bash" => {
                ev(json!({"type": "message", "role": "assistant", "content": [{"type": "tool_use", "call_id": cid, "name": "bash", "input": {"command": a}}]}));
                let code = b.rsplit("[exit_code=").next().and_then(|r| r.split(' ').next()).and_then(|c| c.parse::<i64>().ok());
                ev(json!({"type": "tool_result", "call_id": cid, "name": "bash", "exit_code": code, "is_error": code != Some(0), "output": [{"type": "text", "text": b}]}));
            }
            "write" => {
                ev(json!({"type": "message", "role": "assistant", "content": [{"type": "tool_use", "call_id": cid, "name": "write", "input": {"path": a, "content": "x"}}]}));
                ev(json!({"type": "tool_result", "call_id": cid, "name": "write", "exit_code": null, "is_error": false, "output": [{"type": "text", "text": "wrote 1 line"}]}));
            }
            _ => unreachable!(),
        }
    }
    out.join("\n") + "\n"
}

const RED: &str = "F\nFAILED (failures=1)\n[exit_code=1 wall_time=0.1s]";
const GREEN: &str = "OK\n[exit_code=0 wall_time=0.1s]";

fn signals(text: &str) -> (Vec<String>, usize) {
    let p = session::parse(text.as_bytes(), "x", &PathCtx::default(), None);
    (p.instances.iter().map(|i| i.signal.clone()).collect(), p.expected.len())
}

#[test]
fn ev03_finish_signals() {
    let untested = log("s1", &[("owner", "fix it", ""), ("write", "calc.py", ""), ("final", "done", "")]);
    assert_eq!(signals(&untested).0, ["finish.untested"]);
    let failing = log("s2", &[("owner", "fix it", ""), ("write", "calc.py", ""), ("bash", "python3 -m unittest -q", RED), ("final", "done", "")]);
    let (s, _) = signals(&failing);
    assert!(s.contains(&"finish.tests_failing".to_string()) && s.contains(&"bash.exit:python3".to_string()), "{s:?}");
    // Red, edit, green: ordinary test-driven work, no friction at all.
    let tdd = log("s3", &[("owner", "fix it", ""), ("bash", "python3 -m unittest -q", RED), ("write", "calc.py", ""), ("bash", "python3 -m unittest -q", GREEN), ("final", "done", "")]);
    let (s, expected) = signals(&tdd);
    assert!(s.is_empty() && expected == 1, "{s:?}");
}

fn run_record(task: &str, run: &str, score: u64) -> String {
    json!({"v": 3, "gate_id": "j-000007", "run_id": run, "arm": "p", "task": task, "set": "pool", "labels": ["py"], "score_permille": score, "outcome": "done", "cost_ref_musd": 10, "turns": 3}).to_string()
}

#[test]
fn ev04_attribution_and_divergence() {
    let h = home("ev04");
    let job = h.join("vault/gate/jobs/j-000007");
    std::fs::create_dir_all(job.join("sessions")).unwrap();
    // t0: one failing run (fatal git error) and one passing run → a divergence pair.
    // t1: failing; its git error is recovered later (not fatal). t2: failing, fatal.
    let lines = [run_record("t0", "p-t0-r1", 0), run_record("t0", "p-t0-r2", 1000), run_record("t1", "p-t1-r1", 0), run_record("t2", "p-t2-r1", 500)];
    std::fs::write(job.join("runs.jsonl"), lines.join("\n") + "\n").unwrap();
    let git_err = "fatal: bad revision\n[exit_code=128 wall_time=0.01s]";
    let ok = "abc\n[exit_code=0 wall_time=0.01s]";
    std::fs::write(job.join("sessions/p-t0-r1.jsonl"), log("r0", &[("owner", "task", ""), ("bash", "git log x", git_err), ("final", "gave up", "")])).unwrap();
    std::fs::write(job.join("sessions/p-t1-r1.jsonl"), log("r1", &[("owner", "task", ""), ("bash", "git log x", git_err), ("bash", "git log x", ok), ("final", "done", "")])).unwrap();
    std::fs::write(job.join("sessions/p-t2-r1.jsonl"), log("r2", &[("owner", "task", ""), ("bash", "git log x", git_err), ("final", "gave up", "")])).unwrap();
    let cfg = MineCfg { live_sessions: false, traces: false, ..MineCfg::for_home(&h) };
    let out = mine::mine(&h, &cfg).unwrap();
    assert_eq!(out.failing_runs, 3);
    let c = out.clusters.iter().find(|c| c.signature == "bash.exit:git").unwrap_or_else(|| panic!("{:#?}", out.clusters));
    // Hand counts: 3 instances in 3 runs; fatal in t0 and t2 → s_c = 2/3; t0 is the divergence pair.
    assert_eq!((c.counts.instances, c.counts.fatal_runs, c.counts.divergence_pairs, c.s_c_permille), (3, 2, 1, 666), "{c:#?}");
    assert_eq!(c.fatal_tasks, ["t0", "t2"]);
    assert_eq!(c.prio, 3 + 2 * 2 + 3, "3·div + 2·fatal tasks + Σ weight");
    assert!(c.eligible && c.refs[0].starts_with("run:"), "attested refs first: {:?}", c.refs);
    let tf = out.clusters.iter().find(|c| c.signature == "task.fail:py").unwrap();
    assert_eq!(tf.counts.instances, 3);
    remove(&h);
}

fn strip(v: Vec<Cluster>) -> Vec<Cluster> {
    let mut v: Vec<Cluster> = v.into_iter().map(|mut c| {
        c.id = String::new();
        c
    }).collect();
    v.sort_by(|a, b| a.signature.cmp(&b.signature));
    v
}

#[test]
fn ev05_incremental_equals_full_rescan() {
    let inc = home("ev05i");
    let full = home("ev05f");
    let sessions = |h: &Path| h.join("state/sessions");
    std::fs::create_dir_all(sessions(&inc)).unwrap();
    std::fs::create_dir_all(sessions(&full)).unwrap();
    // ~10 MB of logs: 400 sessions, each a run with paths, git errors, retries and greens.
    let pad = "y".repeat(600);
    let one = |i: usize| {
        let out = format!("{pad}\n[exit_code=0 wall_time=0.01s]");
        let mut steps: Vec<(&str, String, String)> = vec![("owner", "go".into(), String::new())];
        for k in 0..20 {
            match k % 4 {
                0 => steps.push(("bash", format!("cd work/p{i} && ls"), "bash: cd: work/p: No such file or directory\n[exit_code=1 wall_time=0.01s]".into())),
                1 => steps.push(("bash", "git log x".into(), "fatal: bad\n[exit_code=128 wall_time=0.01s]".into())),
                _ => steps.push(("bash", format!("cat f{k}"), out.clone())),
            }
        }
        steps.push(("final", "done".into(), String::new()));
        let s: Vec<(&str, &str, &str)> = steps.iter().map(|(a, b, c)| (*a, b.as_str(), c.as_str())).collect();
        log(&format!("20261020-10{:04}-abcdef", i), &s)
    };
    let cfg = |h: &Path| MineCfg { traces: false, pool: false, ..MineCfg::for_home(h) };
    let mut bytes = 0usize;
    for i in 0..200 {
        let t = one(i);
        bytes += t.len();
        std::fs::write(sessions(&inc).join(format!("s{i:04}.jsonl")), &t).unwrap();
        std::fs::write(sessions(&full).join(format!("s{i:04}.jsonl")), &t).unwrap();
    }
    mine::mine(&inc, &cfg(&inc)).unwrap();
    // More sessions, and one existing log grows.
    for i in 200..400 {
        let t = one(i);
        bytes += t.len();
        std::fs::write(sessions(&inc).join(format!("s{i:04}.jsonl")), &t).unwrap();
        std::fs::write(sessions(&full).join(format!("s{i:04}.jsonl")), &t).unwrap();
    }
    let grow = one(9999);
    for h in [&inc, &full] {
        use std::io::Write;
        std::fs::OpenOptions::new().append(true).open(sessions(h).join("s0007.jsonl")).unwrap().write_all(grow.as_bytes()).unwrap();
    }
    let t = std::time::Instant::now();
    let second = mine::mine(&inc, &cfg(&inc)).unwrap();
    let inc_ms = t.elapsed().as_millis();
    assert_eq!(second.parsed_sources, 201, "only new or grown logs are re-read");
    let t = std::time::Instant::now();
    let f = mine::mine(&full, &cfg(&full)).unwrap();
    let full_ms = t.elapsed().as_millis().max(1);
    assert_eq!(strip(mine::read_index(&inc)), strip(mine::read_index(&full)));
    assert_eq!(f.parsed_sources, 400);
    // EV-05's bound: < 2 s per hour of logs; an hour of S-REAL-like activity is ≈ 5 MB.
    let mb = bytes as f64 / 1e6;
    let per_hour_s = full_ms as f64 / 1000.0 / (mb / 5.0);
    eprintln!("EV-05: {mb:.1} MB, full {full_ms} ms, incremental {inc_ms} ms, {per_hour_s:.2} s per 5 MB");
    assert!(per_hour_s < 2.0, "{per_hour_s:.2} s per hour of logs");
    remove(&inc);
    remove(&full);
}
