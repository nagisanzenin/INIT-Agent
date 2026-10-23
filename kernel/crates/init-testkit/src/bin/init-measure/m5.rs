//! M-5: body memory over a synthetic 1,000-step session (testing.md §3.11 M-5; body/loop.md item
//! 21), socket mode: the real `init-body` and `init-mockllm`, no kernel.
//!
//! Script (`gen-steps`): step i is one `bash` call `printf '%<bytes>s\n' x`; every `read_every`-th
//! step instead reads a 5,000-line fixture (spill and continuation); after the last step the
//! model ends the run. Compaction requests (purpose `compaction`) get a valid summary. The model
//! is `mock-frontier-1` (131,072-token window), so compaction runs several times.
//!
//! The sampler reads, every `sample_ms`, the body's physical footprint (`proc_pid_rusage`
//! `ri_phys_footprint`, macOS) and its `ps -o rss=`, and maps each sample to the step count, i.e.
//! the number of `turn` requests in the mock's log at that moment.
//!
//! Pass (testing.md M-5, provisional owner decision 2026-10-20): the baseline is the first context
//! epoch (the first `mask` or `compaction` event in the session log, written when the context
//! first reaches `T_trigger`); its value is the last sample before that event. From there to the
//! last step, the least-squares slope of the metric over steps is ≤ 2 KiB/step, and its max is
//! ≤ 1.25 × the baseline value. The metric is `phys_footprint` on macOS (ps RSS counts pages
//! libmalloc freed but left resident, FIX8) and RSS elsewhere. The old rule (ps RSS, baseline at
//! step 100) stays selectable (`--metric rss --baseline 100`) and every run reports all four
//! combinations under `compare`.

use crate::common;
use crate::rig::{self, Body, Home, Mock};
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const SUMMARY: &str = "<summary>\n## Intent\n- [I1] Run the M-5 steps the owner asked for\n## Constraints and preferences\n- none\n## Progress\n### Done\n- [P1] ran the printf steps so far\n### In progress\n- [P2] the remaining steps\n### Blocked\n- none\n## Decisions\n- none\n## Open errors\n- none\n## Next steps\n- [N1] continue the steps\n## Critical details\n- [K1] the fixture is m5/fixture.txt\nRetired:\n</summary>";

pub const FIXTURE: &str = "m5/fixture.txt";

/// The `*.mock.jsonl` script (body/provider.md §3.12 format). `seed` makes the reasoning text of
/// each step vary deterministically (the replies are otherwise identical).
pub fn gen_steps(
    steps: u64,
    output_bytes: u64,
    read_every: u64,
    seed: u64,
    compactions: u64,
) -> String {
    let mut out = String::new();
    let meta = json!({"v": 1, "kind": "meta", "provider": "deepseek", "model": "mock-frontier-1",
                      "name": format!("m5-{steps}-{output_bytes}-{read_every}-seed{seed}"), "chunk": 64});
    out.push_str(&meta.to_string());
    out.push('\n');
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    for i in 1..=steps {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let call = if read_every > 0 && i % read_every == 0 {
            json!({"name": "read", "input": {"path": FIXTURE}})
        } else {
            json!({"name": "bash", "input": {"command": format!("printf '%{output_bytes}s\\n' x")}})
        };
        let t = json!({"v": 1, "kind": "turn",
                       "reply": {"think": format!("step {i} ({:04x})", x & 0xffff), "calls": [call], "stop": "tool_use"}});
        out.push_str(&t.to_string());
        out.push('\n');
    }
    let fin = json!({"v": 1, "kind": "turn", "reply": {"text": format!("done: {steps} steps"), "stop": "end_turn"}});
    out.push_str(&fin.to_string());
    out.push('\n');
    for _ in 0..compactions {
        let c = json!({"v": 1, "kind": "turn", "purpose": "compaction", "reply": {"text": SUMMARY, "stop": "end_turn"}});
        out.push_str(&c.to_string());
        out.push('\n');
    }
    out
}

/// The fixture: 5,000 numbered lines.
pub fn fixture_text(lines: u64) -> String {
    (1..=lines)
        .map(|i| {
            format!(
                "{i:05} the quick brown fox jumps over the lazy dog, line {i} of the M-5 fixture\n"
            )
        })
        .collect()
}

/// The sampled memory number (testing.md M-5 step 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    /// macOS `phys_footprint` (the default there).
    Footprint,
    /// `ps -o rss=` (the old rule; the default off macOS).
    Rss,
}

impl Metric {
    pub fn parse(s: &str) -> Result<Metric, String> {
        match s {
            "footprint" => Ok(Metric::Footprint),
            "rss" => Ok(Metric::Rss),
            _ => Err(format!("--metric is footprint or rss, not {s:?}")),
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Metric::Footprint => "footprint",
            Metric::Rss => "rss",
        }
    }
    pub fn default_for_os() -> Metric {
        if cfg!(target_os = "macos") {
            Metric::Footprint
        } else {
            Metric::Rss
        }
    }
}

/// Where the pass rule's window starts (testing.md M-5 "Pass").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Baseline {
    /// The first context epoch read from the session log (the default).
    FirstEpoch,
    /// A fixed step (the old rule: 100).
    Step(u64),
}

impl Baseline {
    pub fn parse(s: &str) -> Result<Baseline, String> {
        match s {
            "epoch" => Ok(Baseline::FirstEpoch),
            n => n
                .parse()
                .map(Baseline::Step)
                .map_err(|_| format!("--baseline is epoch or a step number, not {s:?}")),
        }
    }
    pub fn label(self) -> String {
        match self {
            Baseline::FirstEpoch => "epoch".into(),
            Baseline::Step(n) => n.to_string(),
        }
    }
    /// Where the window starts: at a fixed step, or at the context's first full state (the last
    /// sample before the first epoch event).
    pub fn resolve(self, epoch: Option<&FirstEpoch>) -> Option<Start> {
        match self {
            Baseline::Step(n) => Some(Start::AtOrAfter(n)),
            Baseline::FirstEpoch => epoch.map(|e| Start::AtOrBefore(e.after_step)),
        }
    }
}

/// The baseline sample: the window runs from it to the last step, and its value is the base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    /// The first sample with at least this step count (a fixed step: the old rule's step 100).
    AtOrAfter(u64),
    /// The last sample with at most this step count. For the first epoch this is the memory when
    /// the context first holds `T_trigger` tokens, taken before the epoch frees and rebuilds the
    /// view: right after it, the allocator returns the old view's pages at its own pace, so a
    /// sample there swings by up to 2 MiB from run to run (M5 lane trials).
    AtOrBefore(u64),
}

impl Start {
    fn index(self, samples: &[Sample], m: Metric) -> Option<usize> {
        match self {
            Start::AtOrAfter(n) => samples
                .iter()
                .position(|s| s.step >= n && s.value(m).is_some()),
            Start::AtOrBefore(n) => samples
                .iter()
                .rposition(|s| s.step <= n && s.value(m).is_some()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub t_ms: u64,
    pub step: u64,
    pub rss_kib: u64,
    pub fp_kib: Option<u64>,
}

impl Sample {
    fn value(&self, m: Metric) -> Option<u64> {
        match m {
            Metric::Footprint => self.fp_kib,
            Metric::Rss => Some(self.rss_kib),
        }
    }
}

/// The first context epoch of a session: the first `mask` or `compaction` event, which the body
/// writes when the context first reaches `T_trigger` (log-context.md §3.11–§3.12).
#[derive(Clone, Debug, PartialEq)]
pub struct FirstEpoch {
    pub kind: String,
    pub event_id: String,
    /// Assistant messages (= `turn` responses = steps) logged before the epoch event: request
    /// `after_step` was the first one sent with the context at `T_trigger`, and the epoch runs
    /// before request `after_step + 1`.
    pub after_step: u64,
}

/// Reads the first epoch from the session log's events, in file order.
pub fn first_epoch(events: &[Value]) -> Option<FirstEpoch> {
    let mut steps = 0u64;
    for e in events {
        match e["type"].as_str() {
            Some("message") if e["role"] == "assistant" => steps += 1,
            Some(k @ ("mask" | "compaction")) => {
                return Some(FirstEpoch {
                    kind: k.to_string(),
                    event_id: e["id"].as_str().unwrap_or("").to_string(),
                    after_step: steps,
                });
            }
            _ => {}
        }
    }
    None
}

fn value_at(samples: &[Sample], m: Metric, step: u64) -> Option<u64> {
    samples
        .iter()
        .find(|s| s.step >= step)
        .and_then(|s| s.value(m))
}

/// The pass rule and the reported numbers (testing.md M-5 "Pass") for one metric, with the
/// window starting at the baseline sample (`None`: no epoch was seen, which fails).
pub fn analyze(samples: &[Sample], steps: u64, metric: Metric, start: Option<Start>) -> Value {
    let idx = start.and_then(|st| st.index(samples, metric));
    let window: Vec<(u64, u64)> = idx
        .map(|i| &samples[i..])
        .unwrap_or(&[])
        .iter()
        .filter(|s| s.step <= steps)
        .filter_map(|s| s.value(metric).map(|v| (s.step, v)))
        .collect();
    let pts: Vec<(f64, f64)> = window.iter().map(|&(s, v)| (s as f64, v as f64)).collect();
    let slope = common::ols_slope(&pts);
    let base_step = idx.map(|i| samples[i].step);
    let base = idx.and_then(|i| samples[i].value(metric));
    let max_after = window.iter().map(|w| w.1).max();
    let peak = samples.iter().filter_map(|s| s.value(metric)).max();
    let ratio = match (max_after, base) {
        (Some(m), Some(b)) if b > 0 => Some(m as f64 / b as f64),
        _ => None,
    };
    let slope_ok = slope.is_some_and(|s| s <= 2.0);
    let ratio_ok = ratio.is_some_and(|r| r <= 1.25);
    let reached = samples.iter().map(|s| s.step).max().unwrap_or(0);
    // Diagnostic only (not the pass rule): the slope over the second half of the session, when the
    // context has been through compaction at least once on a 131k window.
    let half: Vec<(f64, f64)> = samples
        .iter()
        .filter(|s| s.step >= steps / 2 && s.step <= steps)
        .filter_map(|s| s.value(metric).map(|v| (s.step as f64, v as f64)))
        .collect();
    json!({
        "metric": metric.as_str(),
        "base_step": base_step,
        "diag_slope_kib_per_step_second_half": common::ols_slope(&half).map(common::r3),
        "kib_at": {
            "0": samples.first().and_then(|s| s.value(metric)),
            "100": value_at(samples, metric, 100),
            "500": value_at(samples, metric, 500),
            "1000": value_at(samples, metric, 1000),
        },
        "peak_kib": peak,
        "max_kib_from_base": max_after,
        "kib_at_base": base,
        "max_over_base": ratio.map(common::r3),
        "slope_kib_per_step": slope.map(common::r3),
        "window_samples": window.len(),
        "steps_reached": reached,
        "thresholds": {"slope_kib_per_step_max": 2.0, "max_over_base_max": 1.25},
        "slope_ok": slope_ok,
        "ratio_ok": ratio_ok,
        "pass": slope_ok && ratio_ok && reached >= steps,
    })
}

/// The four metric × baseline combinations of one run, in short form (the old rule is `rss@100`).
pub fn compare(samples: &[Sample], steps: u64, epoch: Option<&FirstEpoch>) -> Value {
    let mut m = Map::new();
    for metric in [Metric::Footprint, Metric::Rss] {
        for b in [Baseline::FirstEpoch, Baseline::Step(100)] {
            let a = analyze(samples, steps, metric, b.resolve(epoch));
            m.insert(
                format!("{}@{}", metric.as_str(), b.label()),
                json!({"slope_kib_per_step": a["slope_kib_per_step"], "max_over_base": a["max_over_base"],
                       "kib_at_base": a["kib_at_base"], "peak_kib": a["peak_kib"], "pass": a["pass"]}),
            );
        }
    }
    Value::Object(m)
}

/// Re-applies the rule to a stored `m5` result (`samples` as `[t, step, rss]` from before this
/// change, or `[t, step, footprint|null, rss]`). With `Baseline::FirstEpoch` the file must carry
/// `first_epoch` (older files do not; pass the step instead).
pub fn reanalyze(doc: &Value, metric: Metric, baseline: Baseline) -> Result<Value, String> {
    let rows = doc["samples"].as_array().ok_or("no samples in the file")?;
    let mut samples = Vec::with_capacity(rows.len());
    for r in rows {
        let a = r.as_array().ok_or("a sample is not an array")?;
        let n = |i: usize| a.get(i).and_then(Value::as_u64);
        let s = match a.len() {
            3 => Sample {
                t_ms: n(0).unwrap_or(0),
                step: n(1).unwrap_or(0),
                rss_kib: n(2).unwrap_or(0),
                fp_kib: None,
            },
            4 => Sample {
                t_ms: n(0).unwrap_or(0),
                step: n(1).unwrap_or(0),
                fp_kib: n(2),
                rss_kib: n(3).unwrap_or(0),
            },
            k => return Err(format!("a sample has {k} columns")),
        };
        samples.push(s);
    }
    let steps = doc["session"]["steps"].as_u64().unwrap_or(1000);
    let epoch = doc.get("first_epoch").and_then(|e| {
        Some(FirstEpoch {
            kind: e["kind"].as_str()?.to_string(),
            event_id: e["event_id"].as_str().unwrap_or("").to_string(),
            after_step: e["after_step"].as_u64()?,
        })
    });
    let base = baseline
        .resolve(epoch.as_ref())
        .ok_or("the file has no first_epoch; pass --baseline <step>")?;
    let mut a = analyze(&samples, steps, metric, Some(base));
    a["baseline"] = json!(baseline.label());
    Ok(a)
}

pub struct Opts {
    pub steps: u64,
    pub output_bytes: u64,
    pub read_every: u64,
    pub seed: u64,
    pub sample_ms: u64,
    pub body: PathBuf,
    pub mock: PathBuf,
    pub keep: bool,
    pub limit_s: u64,
    pub metric: Metric,
    pub baseline: Baseline,
}

fn sample(pid: u32, t_ms: u64, step: u64) -> Option<Sample> {
    let fp_kib = common::footprint_kib(pid);
    let rss_kib = common::rss_kib(pid)?;
    Some(Sample {
        t_ms,
        step,
        rss_kib,
        fp_kib,
    })
}

pub fn run(o: &Opts) -> Result<common::Outcome, String> {
    common::disk_guard()?;
    let home = Home::new("m5", o.keep)?;
    std::fs::create_dir_all(home.path.join("work/m5")).map_err(|e| e.to_string())?;
    std::fs::write(home.path.join("work").join(FIXTURE), fixture_text(5000))
        .map_err(|e| e.to_string())?;
    let script = gen_steps(o.steps, o.output_bytes, o.read_every, o.seed, 200);
    let mock = Mock::start(&o.mock, &home, &script, &["--record-bodies", "none"])?;
    let bid = "5555000011112222";
    let sha = rig::fake_sha();
    home.boot_json(bid, "cold", &sha, json!({}))?;
    let mut body = Body::spawn(
        &o.body,
        &home,
        rig::body_env(&home, "cold", bid, &sha),
        "m5",
    )?;
    let t_spawn = Instant::now();
    home.wait_ready(bid, &mut body, Duration::from_secs(60))?;
    let ready_ms = t_spawn.elapsed().as_millis() as u64;
    let pid = body.pid();
    if o.metric == Metric::Footprint && common::footprint_kib(pid).is_none() {
        return Err(
            "--metric footprint: proc_pid_rusage gave no phys_footprint (not macOS?)".into(),
        );
    }
    let mut samples = Vec::new();
    let t0 = Instant::now();
    let check_start = (common::footprint_kib(pid), common::footprint_tool_kib(pid));
    samples.push(sample(pid, 0, 0).ok_or("ps failed")?);
    body.input(
        1,
        "Run the M-5 steps: one tool call per step until the script ends.",
    )?;
    let mut busy_seen = false;
    let mut last_report = Instant::now();
    let limit = Duration::from_secs(o.limit_s);
    let idle_at = loop {
        std::thread::sleep(Duration::from_millis(o.sample_ms));
        let step = mock.count("turn") as u64;
        let got = sample(pid, t0.elapsed().as_millis() as u64, step);
        if let Some(s) = got {
            samples.push(s);
        }
        if last_report.elapsed() > Duration::from_secs(30) {
            eprintln!(
                "m5: step {step}/{} after {:.0} s, footprint {:?} KiB, rss {:?} KiB",
                o.steps,
                t0.elapsed().as_secs_f64(),
                got.and_then(|s| s.fp_kib),
                got.map(|s| s.rss_kib)
            );
            last_report = Instant::now();
        }
        if !busy_seen
            && body
                .poll_frame(|f| f["t"] == "state" && f["busy"] == true)
                .is_some()
        {
            busy_seen = true;
        }
        if busy_seen
            && step > o.steps
            && let Some((at, _)) = body.poll_frame(|f| f["t"] == "state" && f["busy"] == false)
        {
            break at;
        }
        if let Some(c) = body.try_exit() {
            return Err(format!(
                "body exited {c:?} at step {step}; stderr:\n{}",
                body.stderr_tail()
            ));
        }
        if t0.elapsed() > limit {
            return Err(format!(
                "m5: not idle after {limit:?} (step {step}); stderr:\n{}",
                body.stderr_tail()
            ));
        }
        if step <= o.steps && samples.len() > 30 && got.is_none() {
            return Err("m5: ps stopped answering".into());
        }
    };
    let run_s = idle_at.duration_since(t0).as_secs_f64();
    // One more sample at idle (after the run's end).
    if let Some(s) = sample(
        pid,
        t0.elapsed().as_millis() as u64,
        mock.count("turn") as u64,
    ) {
        samples.push(s);
    }
    let check_end = (common::footprint_kib(pid), common::footprint_tool_kib(pid));
    let log = mock.log();
    let compactions = log.iter().filter(|l| l["purpose"] == "compaction").count();
    let protocol_errors = log
        .iter()
        .filter(|l| !l["protocol_error"].is_null())
        .count();
    let exhausted = log.iter().filter(|l| l["responder"] == "give_up").count();
    let events = home.session_log();
    let kinds = |k: &str| events.iter().filter(|e| e["type"] == k).count();
    let errors: Vec<Value> = events
        .iter()
        .filter(|e| e["type"] == "error")
        .take(5)
        .cloned()
        .collect();
    let session_bytes =
        std::fs::metadata(home.path.join(format!("state/sessions/{}.jsonl", rig::SID)))
            .map(|m| m.len())
            .ok();
    let stop = body.stop();
    let epoch = first_epoch(&events);
    let start = o.baseline.resolve(epoch.as_ref());
    let mut a = analyze(&samples, o.steps, o.metric, start);
    a["baseline"] = json!(o.baseline.label());
    let pass = a["pass"] == true;
    let from = start
        .and_then(|st| st.index(&samples, o.metric))
        .unwrap_or(samples.len());
    let mib: Vec<f64> = samples[from..]
        .iter()
        .filter_map(|s| s.value(o.metric))
        .map(|v| v as f64 / 1024.0)
        .collect();
    let kind = format!(
        "m5.socket.{}_from_{}",
        o.metric.as_str(),
        match o.baseline {
            Baseline::FirstEpoch => "first_epoch".to_string(),
            Baseline::Step(n) => format!("step_{n}"),
        }
    );
    let mut run = common::summary(&kind, "MiB", &[]);
    run["n"] = json!(mib.len());
    run["median"] = json!(common::median(&mib).map(common::r3));
    run["min"] = json!(mib.iter().copied().reduce(f64::min).map(common::r3));
    run["max"] = json!(mib.iter().copied().reduce(f64::max).map(common::r3));
    run["values"] = json!([]);
    let mut extra = Map::new();
    extra.insert("compare".into(), compare(&samples, o.steps, epoch.as_ref()));
    extra.insert(
        "first_epoch".into(),
        match &epoch {
            Some(e) => json!({"kind": e.kind, "event_id": e.event_id, "after_step": e.after_step}),
            None => Value::Null,
        },
    );
    extra.insert(
        "footprint_crosscheck_kib".into(),
        json!({"start": {"syscall": check_start.0, "footprint_tool": check_start.1},
               "end": {"syscall": check_end.0, "footprint_tool": check_end.1}}),
    );
    extra.insert("result".into(), a);
    extra.insert(
        "session".into(),
        json!({
            "steps": o.steps, "output_bytes": o.output_bytes, "read_every": o.read_every, "seed": o.seed,
            "sample_ms": o.sample_ms, "samples": samples.len(), "run_s": common::r3(run_s), "ready_ms": ready_ms,
            "mock_turn_requests": log.iter().filter(|l| l["purpose"] == "turn").count(),
            "mock_compaction_requests": compactions, "mock_protocol_errors": protocol_errors,
            "mock_exhausted": exhausted,
            "events": {"compaction": kinds("compaction"), "mask": kinds("mask"), "tool_result": kinds("tool_result"),
                       "error": kinds("error"), "total": events.len()},
            "first_errors": errors, "session_log_bytes": session_bytes,
            "stop_exit": format!("{stop:?}"),
            "body": crate::builds::rel(&o.body), "mock": crate::builds::rel(&o.mock),
        }),
    );
    extra.insert(
        "samples".into(),
        Value::Array(
            samples
                .iter()
                .map(|s| json!([s.t_ms, s.step, s.fp_kib, s.rss_kib]))
                .collect(),
        ),
    );
    extra.insert(
        "samples_format".into(),
        json!(
            "[t_ms since input, step (turn requests seen), phys_footprint_kib or null, ps_rss_kib]"
        ),
    );
    drop(body);
    drop(mock);
    Ok((vec![run], extra, pass))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gen_steps_shape() {
        let s = gen_steps(10, 2048, 5, 7, 2);
        let lines: Vec<Value> = s
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 1 + 10 + 1 + 2);
        assert_eq!(lines[0]["kind"], "meta");
        assert_eq!(lines[1]["reply"]["calls"][0]["name"], "bash");
        assert_eq!(
            lines[1]["reply"]["calls"][0]["input"]["command"],
            "printf '%2048s\\n' x"
        );
        assert_eq!(lines[5]["reply"]["calls"][0]["name"], "read");
        assert_eq!(lines[10]["reply"]["calls"][0]["name"], "read");
        assert_eq!(lines[11]["reply"]["stop"], "end_turn");
        assert_eq!(lines[12]["purpose"], "compaction");
        assert_eq!(s, gen_steps(10, 2048, 5, 7, 2), "deterministic");
        assert_ne!(s, gen_steps(10, 2048, 5, 8, 2), "seeded");
        // The mock parses it.
        init_testkit::mockllm::Script::parse(&s, std::path::Path::new(".")).unwrap();
    }

    #[test]
    fn fixture_has_5000_lines() {
        assert_eq!(fixture_text(5000).lines().count(), 5000);
    }

    fn line(n: u64, f: impl Fn(u64) -> u64) -> Vec<Sample> {
        (0..=n)
            .step_by(10)
            .map(|s| Sample {
                t_ms: s,
                step: s,
                rss_kib: f(s),
                fp_kib: Some(f(s)),
            })
            .collect()
    }

    fn rss100(samples: &[Sample]) -> Value {
        analyze(samples, 1000, Metric::Rss, Some(Start::AtOrAfter(100)))
    }

    #[test]
    fn flat_rss_passes() {
        let a = rss100(&line(1000, |s| 40_000 + (s % 30)));
        assert_eq!(a["pass"], true, "{a}");
        assert!(a["slope_kib_per_step"].as_f64().unwrap().abs() < 0.1);
    }

    #[test]
    fn a_leak_fails_on_slope() {
        // 3 KiB per step: 2.7 MiB over 900 steps, under the 1.25 ratio but over the slope.
        let a = rss100(&line(1000, |s| 40_000 + 3 * s));
        assert_eq!(a["slope_ok"], false, "{a}");
        assert_eq!(a["pass"], false);
    }

    #[test]
    fn a_spike_fails_on_ratio() {
        let a = rss100(&line(1000, |s| if s == 500 { 60_000 } else { 40_000 }));
        assert_eq!(a["ratio_ok"], false, "{a}");
        assert_eq!(a["pass"], false);
    }

    #[test]
    fn an_unfinished_run_fails() {
        let a = rss100(&line(600, |_| 40_000));
        assert_eq!(a["pass"], false, "{a}");
    }

    fn ev(ty: &str, role: Option<&str>) -> Value {
        let mut v = json!({"type": ty, "id": format!("{:016x}", ty.len())});
        if let Some(r) = role {
            v["role"] = r.into();
        }
        v
    }

    #[test]
    fn first_epoch_counts_assistant_messages_before_the_first_mask_or_compaction() {
        let mut evs = vec![
            ev("session", None),
            ev("boot", None),
            ev("message", Some("user")),
        ];
        for _ in 0..3 {
            evs.extend([
                ev("message", Some("assistant")),
                ev("tool_call", None),
                ev("tool_result", None),
                ev("signal", None),
            ]);
        }
        assert_eq!(first_epoch(&evs), None, "no epoch yet");
        evs.push(ev("mask", None));
        evs.push(ev("message", Some("assistant")));
        evs.push(ev("compaction", None));
        let e = first_epoch(&evs).unwrap();
        assert_eq!((e.kind.as_str(), e.after_step), ("mask", 3));
        // A compaction first is an epoch too.
        let e = first_epoch(&[ev("message", Some("assistant")), ev("compaction", None)]).unwrap();
        assert_eq!((e.kind.as_str(), e.after_step), ("compaction", 1));
    }

    /// FIX8's shape: the context fills until the first epoch (memory rises legitimately), then
    /// stays flat; the footprint is flat but RSS keeps the allocator's freed pages. The new rule
    /// (footprint from the epoch) passes, the old one (RSS from step 100) fails, and a real leak
    /// after the epoch fails both.
    #[test]
    fn the_epoch_baseline_skips_the_fill_and_footprint_ignores_retained_pages() {
        let fill = |s: u64| 10_000 + 120 * s.min(155);
        let samples: Vec<Sample> = (0..=1000)
            .step_by(5)
            .map(|s| Sample {
                t_ms: s,
                step: s,
                fp_kib: Some(fill(s) + (s % 7)),
                rss_kib: fill(s) + 8 * s, // retained free pages: never falls
            })
            .collect();
        let epoch = FirstEpoch {
            kind: "mask".into(),
            event_id: String::new(),
            after_step: 155,
        };
        let at_epoch = Baseline::FirstEpoch.resolve(Some(&epoch));
        assert_eq!(at_epoch, Some(Start::AtOrBefore(155)));
        let new = analyze(&samples, 1000, Metric::Footprint, at_epoch);
        assert_eq!(
            new["base_step"], 155,
            "the last sample before the epoch: {new}"
        );
        assert_eq!(new["pass"], true, "{new}");
        let fp100 = analyze(
            &samples,
            1000,
            Metric::Footprint,
            Some(Start::AtOrAfter(100)),
        );
        assert_eq!(
            fp100["ratio_ok"], false,
            "the fill phase fails the ratio: {fp100}"
        );
        assert_eq!(rss100(&samples)["pass"], false);
        let c = compare(&samples, 1000, Some(&epoch));
        assert_eq!(c["footprint@epoch"]["pass"], true, "{c}");
        assert_eq!(c["rss@100"]["pass"], false, "{c}");
        assert_eq!(c["rss@epoch"]["pass"], false, "{c}");
        // A leak of 3 KiB/step after the epoch fails the new rule.
        let leak: Vec<Sample> = samples
            .iter()
            .map(|s| Sample {
                fp_kib: s.fp_kib.map(|f| f + 3 * s.step.saturating_sub(156)),
                ..*s
            })
            .collect();
        let a = analyze(&leak, 1000, Metric::Footprint, at_epoch);
        assert_eq!(a["slope_ok"], false, "{a}");
        // No epoch at all fails.
        assert_eq!(
            analyze(&samples, 1000, Metric::Footprint, None)["pass"],
            false
        );
    }

    #[test]
    fn reanalyze_reads_old_and_new_sample_files() {
        let old = json!({"session": {"steps": 1000},
                         "samples": (0..=1000).step_by(10).map(|s| json!([s, s, 40_000])).collect::<Vec<_>>()});
        let a = reanalyze(&old, Metric::Rss, Baseline::Step(100)).unwrap();
        assert_eq!(a["pass"], true, "{a}");
        assert!(reanalyze(&old, Metric::Rss, Baseline::FirstEpoch).is_err());
        assert_eq!(
            reanalyze(&old, Metric::Footprint, Baseline::Step(100)).unwrap()["pass"],
            false,
            "old files have no footprint column"
        );
        let new = json!({"session": {"steps": 1000},
                         "first_epoch": {"kind": "mask", "event_id": "x", "after_step": 155},
                         "samples": (0..=1000).step_by(10).map(|s| json!([s, s, 12_000, 30_000 + 9 * s])).collect::<Vec<_>>()});
        let a = reanalyze(&new, Metric::Footprint, Baseline::FirstEpoch).unwrap();
        assert_eq!(
            (a["pass"].clone(), a["base_step"].clone()),
            (json!(true), json!(150)),
            "samples every 10 steps: the last one at or before step 155"
        );
        assert_eq!(
            reanalyze(&new, Metric::Rss, Baseline::FirstEpoch).unwrap()["pass"],
            false
        );
    }

    #[test]
    fn metric_and_baseline_parse() {
        assert_eq!(Metric::parse("footprint").unwrap(), Metric::Footprint);
        assert_eq!(Metric::parse("rss").unwrap(), Metric::Rss);
        assert!(Metric::parse("vsz").is_err());
        assert_eq!(Baseline::parse("epoch").unwrap(), Baseline::FirstEpoch);
        assert_eq!(Baseline::parse("100").unwrap(), Baseline::Step(100));
        assert!(Baseline::parse("soon").is_err());
    }
}
