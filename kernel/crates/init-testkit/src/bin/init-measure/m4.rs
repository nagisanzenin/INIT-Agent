//! M-4: the T2 handoff (testing.md §3.11 M-4), two ways, both at $0.
//!
//! 1. `m4` (body level, socket mode, the real `init-body`): this tool plays the kernel's side of
//!    the overlapped handoff (supervise.md §3.6.4) on one session: `pause` A → `paused` → spawn B
//!    (`INIT_BOOT_REASON=promote`, a new `INIT_SHA`, the same binary: the comment-only
//!    `m4-nop.patch` changes no code) → B's `state/run/ready` → `exit` A (75) → A gone → one owner
//!    line answered by B. Repeated `handoffs` times on fresh processes (A→B→C…), with
//!    `init-mockllm --simulate-cache`. Asserts: the `prefix_sha256` of every `turn` request is the
//!    same, every `boot` event and `ready` file reports the same `prefix_hash`, and the simulated
//!    `cache_read` of the first request after each boot covers the head. Negative control: a
//!    second chain edits `body/SELF.md` (part of the head, `Prefix-change: yes`) during the pause;
//!    the hash must differ. Timings per handoff: pause → paused, B spawn → ready, exit → A gone,
//!    the whole window (pause sent → A gone), and B's first reply.
//! 2. `m4-a3` (kernel level): runs WP15's A3 acceptance test (the real `init-kernel` under
//!    `init-fakeroot`, `initctl promote` → G0–G3 → overlapped handoff) unchanged, snapshots the
//!    kernel's job records from its scratch home while it runs (the test removes the home when it
//!    passes), and reports each stage's `wall_ms`, the handoff stage, and the promote job's total.

use crate::common;
use crate::rig::{self, Body, Home, Mock};
use init_testkit::util;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct Opts {
    pub handoffs: usize,
    pub body: PathBuf,
    pub mock: PathBuf,
    pub keep: bool,
}

#[derive(Clone, Debug, Default)]
struct Hand {
    pause_ms: f64,
    spawn_to_ready_ms: f64,
    exit_ms: f64,
    window_ms: f64,
    first_reply_ms: f64,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

struct Chain {
    hands: Vec<Hand>,
    turn_prefixes: Vec<String>,
    boot_prefixes: Vec<String>,
    ready_prefixes: Vec<String>,
    first_after_boot_cache: Vec<u64>,
    head_tokens: Option<u64>,
    expect_fail: usize,
    protocol_errors: usize,
}

const SELF_EDIT: &str = "\nM-4 negative control: this sentence changes the head.\n";

fn chain(o: &Opts, tag: &str, handoffs: usize, negative: bool) -> Result<Chain, String> {
    let home = Home::new(tag, o.keep)?;
    let script: String = (0..=handoffs)
        .map(|j| json!({"v": 1, "kind": "turn", "reply": {"text": format!("ok {j}"), "stop": "end_turn"}}).to_string() + "\n")
        .collect();
    let mock = Mock::start(&o.mock, &home, &script, &["--simulate-cache"])?;
    let mut sha = rig::fake_sha();
    let mut bid = util::random_hex(8);
    home.boot_json(&bid, "cold", &sha, json!({}))?;
    let mut a = Body::spawn(
        &o.body,
        &home,
        rig::body_env(&home, "cold", &bid, &sha),
        "0",
    )?;
    let mut ready_prefixes = vec![
        home.wait_ready(&bid, &mut a, Duration::from_secs(60))?["prefix_hash"]
            .as_str()
            .unwrap_or("")
            .to_string(),
    ];
    a.input(1, "Say ok.")?;
    a.wait_run(Duration::from_secs(30))?;
    let mut hands = Vec::new();
    for j in 1..=handoffs {
        let tok = util::random_hex(8);
        let t_pause = Instant::now();
        a.send(json!({"t": "pause", "token": tok, "reason": "handoff"}))?;
        let (t_paused, _) =
            a.wait_frame("paused", Duration::from_secs(10), |f| f["t"] == "paused")?;
        if negative {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(home.path.join("body/SELF.md"))
                .map_err(|e| e.to_string())?;
            f.write_all(SELF_EDIT.as_bytes())
                .map_err(|e| e.to_string())?;
        }
        let from = sha.clone();
        sha = rig::fake_sha();
        bid = util::random_hex(8);
        home.boot_json(
            &bid,
            "promote",
            &sha,
            json!({"from_sha": from, "branch": "evo/m4",
            "provisional": {"depth": 1, "g4": "queued"}}),
        )?;
        let t_spawn = Instant::now();
        let mut b = Body::spawn(
            &o.body,
            &home,
            rig::body_env(&home, "promote", &bid, &sha),
            &j.to_string(),
        )?;
        let rb = home.wait_ready(&bid, &mut b, Duration::from_secs(60))?;
        let t_ready = Instant::now();
        ready_prefixes.push(rb["prefix_hash"].as_str().unwrap_or("").to_string());
        a.send(json!({"t": "exit", "token": tok, "code": 75}))?;
        let code = a.wait_exit(Duration::from_secs(10))?;
        let t_gone = Instant::now();
        if code != Some(75) {
            return Err(format!("handoff {j}: A exited {code:?}, not 75"));
        }
        let t_in = Instant::now();
        b.input(j as u64 + 1, "Say ok again.")?;
        let t_idle = b.wait_run(Duration::from_secs(30))?;
        hands.push(Hand {
            pause_ms: ms(t_paused.duration_since(t_pause)),
            spawn_to_ready_ms: ms(t_ready.duration_since(t_spawn)),
            exit_ms: ms(t_gone.duration_since(t_ready)),
            window_ms: ms(t_gone.duration_since(t_pause)),
            first_reply_ms: ms(t_idle.duration_since(t_in)),
        });
        a = b;
    }
    let _ = a.stop();
    let log = mock.log();
    let turns: Vec<&Value> = log.iter().filter(|l| l["purpose"] == "turn").collect();
    let turn_prefixes = turns
        .iter()
        .map(|l| l["prefix_sha256"].as_str().unwrap_or("").to_string())
        .collect();
    let first_after_boot_cache = turns
        .iter()
        .skip(1)
        .map(|l| l["cache_read"].as_u64().unwrap_or(0))
        .collect();
    let boot_prefixes = home
        .session_log()
        .iter()
        .filter(|e| e["type"] == "boot")
        .map(|e| e["prefix_hash"].as_str().unwrap_or("").to_string())
        .collect();
    // The head: DeepSeek's prefix is messages[0] + tools (provider.md); ≈ bytes / 4 tokens.
    let head_tokens = mock.bodies().first().map(|b| {
        let bytes = serde_json::to_string(&b["messages"][0])
            .map(|s| s.len())
            .unwrap_or(0)
            + serde_json::to_string(&b["tools"])
                .map(|s| s.len())
                .unwrap_or(0);
        bytes.div_ceil(4) as u64
    });
    Ok(Chain {
        hands,
        turn_prefixes,
        boot_prefixes,
        ready_prefixes,
        first_after_boot_cache,
        head_tokens,
        expect_fail: log.iter().filter(|l| l["expect_ok"] == false).count(),
        protocol_errors: log
            .iter()
            .filter(|l| !l["protocol_error"].is_null())
            .count(),
    })
}

fn all_same(v: &[String]) -> bool {
    !v.is_empty() && v.iter().all(|x| !x.is_empty() && x == &v[0])
}

pub fn run(o: &Opts) -> Result<common::Outcome, String> {
    common::disk_guard()?;
    let pos = chain(o, "m4", o.handoffs, false)?;
    let neg = chain(o, "m4neg", 1, true)?;
    let col = |f: fn(&Hand) -> f64| pos.hands.iter().map(f).collect::<Vec<f64>>();
    let runs = vec![
        common::summary("m4.body.pause_to_paused", "ms", &col(|h| h.pause_ms)),
        common::summary(
            "m4.body.candidate_spawn_to_ready",
            "ms",
            &col(|h| h.spawn_to_ready_ms),
        ),
        common::summary("m4.body.exit_to_gone", "ms", &col(|h| h.exit_ms)),
        common::summary("m4.body.window_pause_to_swap", "ms", &col(|h| h.window_ms)),
        common::summary(
            "m4.body.first_reply_after_handoff",
            "ms",
            &col(|h| h.first_reply_ms),
        ),
    ];
    let unit = 64u64; // init-mockllm's default --cache-unit-tokens
    let head_floor = pos.head_tokens.map(|h| h / unit * unit);
    let cache_ok = head_floor.is_some_and(|f| pos.first_after_boot_cache.iter().all(|c| *c >= f));
    let prefix_ok = all_same(&pos.turn_prefixes) && pos.turn_prefixes.len() == o.handoffs + 1;
    let boot_ok = all_same(&pos.boot_prefixes)
        && pos.boot_prefixes.len() == o.handoffs + 1
        && all_same(&pos.ready_prefixes);
    let neg_differs = neg.turn_prefixes.len() == 2
        && neg.turn_prefixes[0] != neg.turn_prefixes[1]
        && neg.boot_prefixes.len() == 2
        && neg.boot_prefixes[0] != neg.boot_prefixes[1];
    let pass = prefix_ok && boot_ok && cache_ok && neg_differs && pos.protocol_errors == 0;
    let mut extra = Map::new();
    extra.insert(
        "result".into(),
        json!({
            "handoffs": o.handoffs,
            "prefix_sha256_identical": prefix_ok,
            "turn_prefix_sha256": pos.turn_prefixes,
            "boot_prefix_hash_identical": boot_ok,
            "boot_prefix_hash": pos.boot_prefixes,
            "ready_prefix_hash": pos.ready_prefixes,
            "first_request_after_boot_cache_read": pos.first_after_boot_cache,
            "head_tokens_est": pos.head_tokens,
            "cache_covers_head": cache_ok,
            "negative_control": {
                "edit": "append one sentence to body/SELF.md during the pause (Prefix-change: yes)",
                "turn_prefix_sha256": neg.turn_prefixes,
                "boot_prefix_hash": neg.boot_prefixes,
                "differs": neg_differs,
                "pause_ms": neg.hands.first().map(|h| common::r3(h.pause_ms)),
            },
            "mock_expect_failures": pos.expect_fail + neg.expect_fail,
            "mock_protocol_errors": pos.protocol_errors + neg.protocol_errors,
            "pass": pass,
            "pass_rule": "testing.md M-4: 5/5 identical prefix (turn requests, boot events, ready), cache covers the head, negative control differs",
            "not_covered": "the kernel proxy's last_head_hash (no proxy in socket mode); see m4-a3 for the kernel path",
        }),
    );
    extra.insert("body".into(), json!(crate::builds::rel(&o.body)));
    Ok((runs, extra, pass))
}

// ---- m4-a3: the kernel's promote pipeline and handoff, from WP15's A3 ---------------------------

const A3_TEST: &str = "a3_t2_self_edit_promote_handoff_same_conversation";

fn copy_tree_files(from: &Path, to: &Path, ext: &[&str]) {
    let Ok(rd) = std::fs::read_dir(from) else {
        return;
    };
    let _ = std::fs::create_dir_all(to);
    for e in rd.flatten() {
        let p = e.path();
        if p.extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| ext.contains(&x))
        {
            // Keep the last complete copy: JSON files are replaced atomically by the kernel, so a
            // read either sees the old or the new record.
            if let Ok(b) = std::fs::read(&p) {
                let _ = std::fs::write(to.join(e.file_name()), b);
            }
        }
    }
}

fn snapshot(scratch: &Path, snap: &Path) {
    let h = scratch.join("h");
    copy_tree_files(&h.join("state/kernel/jobs"), &snap.join("jobs"), &["json"]);
    copy_tree_files(
        &h.join("state/sessions"),
        &snap.join("sessions"),
        &["jsonl"],
    );
    copy_tree_files(&h.join("state/kernel"), &snap.join("kernel"), &["json"]);
    copy_tree_files(&scratch.join("m"), &snap.join("mock"), &["jsonl"]);
}

fn a3_dirs(t15: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(t15)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("a3-"))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn ts_ms(v: &Value) -> Option<i64> {
    v.as_str().and_then(util::parse_rfc3339)
}

/// Reads a snapshot: the promote job (`cmd` promote) and the G4 job, their stages and totals.
pub fn parse_snapshot(snap: &Path) -> Value {
    let mut jobs: Vec<Value> = std::fs::read_dir(snap.join("jobs"))
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    serde_json::from_slice::<Value>(&std::fs::read(e.path()).ok()?).ok()
                })
                .collect()
        })
        .unwrap_or_default();
    jobs.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let job_view = |j: &Value| {
        let total = match (ts_ms(&j["created_at"]), ts_ms(&j["finished_at"])) {
            (Some(a), Some(b)) => Some(b - a),
            _ => None,
        };
        let stages: Map<String, Value> = j["stages"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|s| {
                        (
                            s["stage"].as_str().unwrap_or("?").to_string(),
                            json!({"wall_ms": s["wall_ms"], "pass": s["pass"]}),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        json!({"id": j["id"], "cmd": j["cmd"], "state": j["state"], "created_at": j["created_at"],
               "finished_at": j["finished_at"], "total_ms": total, "stages": stages})
    };
    let views: Vec<Value> = jobs.iter().map(job_view).collect();
    let events: Vec<Value> = std::fs::read_dir(snap.join("sessions"))
        .map(|rd| {
            rd.flatten()
                .flat_map(|e| util::read_jsonl(&e.path()))
                .collect()
        })
        .unwrap_or_default();
    let boots: Vec<Value> = events
        .iter()
        .filter(|e| e["type"] == "boot")
        .map(|e| json!({"reason": e["reason"], "ts": e["ts"], "sha": e["sha"], "prefix_hash": e["prefix_hash"]}))
        .collect();
    let mock = util::read_jsonl(&snap.join("mock/mock.log.jsonl"));
    let turn_prefixes: Vec<Value> = mock
        .iter()
        .filter(|l| l["purpose"] == "turn" && l["responder"] == "script")
        .map(|l| l["prefix_sha256"].clone())
        .collect();
    json!({"jobs": views, "boots": boots, "turn_prefix_sha256": turn_prefixes})
}

pub fn run_a3(runs: usize, keep: bool) -> Result<(Vec<Value>, Map<String, Value>), String> {
    let r = common::repo();
    let t15 = r.join("target/t15");
    let base = common::scratch_base().join("m4-a3");
    common::remove_exact(&base);
    let mut per_run = Vec::new();
    for i in 0..runs {
        common::disk_guard()?;
        let snap = base.join(format!("run{i}"));
        std::fs::create_dir_all(&snap).map_err(|e| e.to_string())?;
        let before: Vec<PathBuf> = a3_dirs(&t15);
        let log = std::fs::File::create(snap.join("cargo-test.log")).map_err(|e| e.to_string())?;
        let t0 = Instant::now();
        let mut child = Command::new("cargo")
            .args([
                "test",
                "--offline",
                "-p",
                "init-testkit",
                "--test",
                "acceptance",
                "--",
                "--exact",
                A3_TEST,
                "--nocapture",
            ])
            .current_dir(r.join("kernel"))
            .env("CARGO_TARGET_DIR", r.join("kernel/target/k1"))
            .env("CARGO_INCREMENTAL", "0")
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|e| e.to_string())?)
            .stderr(log)
            .spawn()
            .map_err(|e| format!("spawn cargo test: {e}"))?;
        let status = loop {
            for d in a3_dirs(&t15) {
                if !before.contains(&d) {
                    snapshot(&d, &snap);
                }
            }
            if let Some(st) = child.try_wait().map_err(|e| e.to_string())? {
                break st;
            }
            if t0.elapsed() > Duration::from_secs(900) {
                let _ = child.kill(); // our own child
                let _ = child.wait();
                return Err("m4-a3: cargo test ran past 15 min".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let text = std::fs::read_to_string(snap.join("cargo-test.log")).unwrap_or_default();
        let passed = status.success() && text.contains("test result: ok. 1 passed");
        let mut v = parse_snapshot(&snap);
        v["test_passed"] = json!(passed);
        v["test_wall_s"] = json!(common::r3(t0.elapsed().as_secs_f64()));
        eprintln!(
            "m4-a3 run {i}: passed {passed}, {:.1} s",
            t0.elapsed().as_secs_f64()
        );
        per_run.push(v);
        if !keep {
            common::remove_exact(&snap);
        }
    }
    let mut cols: std::collections::BTreeMap<String, Vec<f64>> = Default::default();
    for v in &per_run {
        for j in v["jobs"].as_array().into_iter().flatten() {
            let cmd = j["cmd"].as_str().unwrap_or("?");
            if let Some(t) = j["total_ms"].as_f64() {
                cols.entry(format!("m4.kernel.{cmd}.total"))
                    .or_default()
                    .push(t);
            }
            for (st, s) in j["stages"].as_object().into_iter().flatten() {
                if let Some(w) = s["wall_ms"].as_f64() {
                    cols.entry(format!("m4.kernel.{cmd}.stage.{st}"))
                        .or_default()
                        .push(w);
                }
            }
        }
    }
    let runs_v: Vec<Value> = cols
        .iter()
        .map(|(k, xs)| common::summary(k, "ms", xs))
        .collect();
    let mut extra = Map::new();
    extra.insert("a3_runs".into(), Value::Array(per_run));
    if !keep {
        common::remove_exact(&base);
    }
    Ok((runs_v, extra))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_same_needs_nonempty_equal_values() {
        assert!(all_same(&["a".into(), "a".into()]));
        assert!(!all_same(&["a".into(), "b".into()]));
        assert!(!all_same(&[]));
        assert!(!all_same(&["".into(), "".into()]));
    }

    #[test]
    fn parse_snapshot_reads_jobs_and_boots() {
        let d = common::scratch_base().join(format!("unit-a3-{}", std::process::id()));
        common::remove_exact(&d);
        std::fs::create_dir_all(d.join("jobs")).unwrap();
        std::fs::create_dir_all(d.join("sessions")).unwrap();
        std::fs::write(
            d.join("jobs/j-000001.json"),
            json!({"id": "j-000001", "cmd": "promote", "state": "succeeded",
                   "created_at": "2026-10-20T00:00:00.000Z", "finished_at": "2026-10-20T00:00:12.500Z",
                   "stages": [{"stage": "G0", "pass": true, "wall_ms": 4000}, {"stage": "handoff", "pass": true, "wall_ms": 900}]})
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            d.join("sessions/s.jsonl"),
            "{\"type\":\"boot\",\"reason\":\"cold\",\"ts\":\"x\"}\n",
        )
        .unwrap();
        let v = parse_snapshot(&d);
        assert_eq!(v["jobs"][0]["total_ms"], 12500);
        assert_eq!(v["jobs"][0]["stages"]["handoff"]["wall_ms"], 900);
        assert_eq!(v["boots"][0]["reason"], "cold");
        common::remove_exact(&d);
    }
}
