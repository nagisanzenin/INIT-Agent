//! Signals from one body session log (`state/sessions/<sid>.jsonl`, body/log-context.md §3.3), or
//! from a retained pool-run log (`vault/gate/jobs/<job>/sessions/<run_id>.jsonl`): the tool-result
//! extractors of [`crate::classify`] plus the run-level ones of evolution-v2 §3.4.2 (`finish.*`,
//! `owner.*`, `retry.same_call`, `loop.repeat_*`) and the red-then-green exclusion.
//!
//! The log is body-written (agent zone): it is parsed as data, never executed; unparsable lines
//! and unknown event types are skipped.

use crate::classify::{self, Hit, PathCtx, ToolEvent};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// One signal instance (evolution-v2 §3.4.2: `{signal, ref, task, session, label?, family?, ts,
/// detail}`), plus what the clustering needs.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Instance {
    pub signal: String,
    pub weight: u32,
    /// The best ref: `trace:<id>` / `run:<job>/<run>` (attested) or `event:<sid>#<id>`.
    #[serde(rename = "ref")]
    pub reference: String,
    /// Every ref seen for this instance (attested first after merging).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
    pub session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    /// `<job>/<run_id>` for instances inside a pool run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    pub ts: String,
    pub detail: String,
    /// The call failed (silent hits are `false`).
    pub failure: bool,
    /// Fatal attribution (§3.4.3): inside a failing pool run, and no later call with the same
    /// command or path succeeded in that run.
    #[serde(default)]
    pub fatal: bool,
    /// Identity across sources: `<session>/<call_id>` for tool results, `<session>/<event id>`
    /// otherwise.
    pub uid: String,
}

/// The result of parsing one log.
#[derive(Clone, Debug, Default)]
pub struct Parsed {
    pub session: String,
    pub instances: Vec<Instance>,
    /// Failing test commands later followed by the same command passing (excluded; uid list).
    pub expected: Vec<String>,
}

/// Run context for a log that belongs to a pool run.
#[derive(Clone, Debug, Default)]
pub struct RunCtx {
    /// `<job>/<run_id>`.
    pub run: String,
    pub task: String,
    pub label: Option<String>,
    pub family: Option<String>,
    /// The run failed (score < 1000): its failure instances may be fatal.
    pub failing: bool,
}

struct Call {
    name: String,
    input: Value,
}

struct Res {
    uid: String,
    event: String,
    ts: String,
    name: String,
    input: Value,
    text: String,
    exit: Option<i64>,
    failed: bool,
    hit: Option<Hit>,
    test: Option<String>,
    run_idx: usize,
}

fn text_of(output: &Value) -> String {
    match output {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// `sha256(name + canonical input)`, 16 hex: the `retry.same_call` identity.
fn call_hash(name: &str, input: &Value) -> String {
    let mut h = Sha256::new();
    h.update(name.as_bytes());
    h.update([0]);
    h.update(input.to_string().as_bytes());
    h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Parses one session log. `fallback_session` names it when the header is missing.
pub fn parse(bytes: &[u8], fallback_session: &str, cx: &PathCtx, run: Option<&RunCtx>) -> Parsed {
    let text = String::from_utf8_lossy(bytes);
    let mut session = fallback_session.to_string();
    let mut calls: HashMap<String, Call> = HashMap::new();
    let mut results: Vec<Res> = Vec::new();
    let mut extra: Vec<Instance> = Vec::new();
    // Runs: an owner message starts one; a final assistant message (no tool_use) ends it.
    let mut run_idx = 0usize;
    let mut last_final: Option<(u64, String)> = None;
    let mut writes_in_run: Vec<usize> = Vec::new();
    let mut tests_in_run: Vec<(usize, bool)> = Vec::new();
    let mut steps_since_error: Option<(usize, String)> = None;
    let mut seen_calls: Vec<String> = Vec::new();
    let mut repeat: HashMap<String, u32> = HashMap::new();

    let mk = |signal: &str, reference: String, uid: String, ts: &str, detail: &str, session: &str| Instance {
        signal: signal.to_string(),
        weight: classify::weight(signal),
        reference,
        refs: vec![],
        session: session.to_string(),
        task: run.map(|r| r.task.clone()),
        label: run.and_then(|r| r.label.clone()).or_else(|| run.is_none().then(|| "live".to_string())),
        family: run.and_then(|r| r.family.clone()),
        run: run.map(|r| r.run.clone()),
        ts: ts.to_string(),
        detail: classify::cap(detail, 200),
        failure: true,
        fatal: false,
        uid,
    };

    for line in text.split('\n') {
        let Ok(e) = serde_json::from_str::<Value>(line) else { continue };
        let ty = e.get("type").and_then(Value::as_str).unwrap_or("");
        let id = e.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        let ts = e.get("ts").and_then(Value::as_str).unwrap_or("").to_string();
        match ty {
            "session" => {
                if let Some(s) = e.get("session_id").and_then(Value::as_str) {
                    session = s.to_string();
                }
            }
            "message" => {
                let role = e.get("role").and_then(Value::as_str).unwrap_or("");
                let source = e.get("source").and_then(Value::as_str).unwrap_or("");
                if role == "user" && source == "owner" {
                    if let Some((at, _)) = &last_final
                        && let Some(now) = crate::fsx::parse_rfc3339_ms(&ts)
                        && now.saturating_sub(*at) <= 300_000
                        && run.is_none()
                    {
                        extra.push(mk("owner.followup", format!("event:{session}#{id}"), format!("{session}/{id}"), &ts, "owner message within 300 s of a final answer", &session));
                    }
                    run_idx += 1;
                    writes_in_run.clear();
                    tests_in_run.clear();
                    repeat.clear();
                    last_final = None;
                } else if role == "assistant" {
                    let blocks = e.get("content").and_then(Value::as_array).cloned().unwrap_or_default();
                    let mut any_tool = false;
                    for b in &blocks {
                        if b.get("type").and_then(Value::as_str) == Some("tool_use") {
                            any_tool = true;
                            let cid = b.get("call_id").and_then(Value::as_str).unwrap_or("").to_string();
                            let name = b.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                            let input = b.get("input").cloned().unwrap_or(Value::Null);
                            calls.insert(cid, Call { name, input });
                        }
                    }
                    if !any_tool {
                        // A final answer: finish.* over the run so far.
                        let last_write = writes_in_run.last().copied();
                        let last_test = tests_in_run.last().copied();
                        if let Some((_, ok)) = last_test
                            && !ok
                        {
                            extra.push(mk("finish.tests_failing", format!("event:{session}#{id}"), format!("{session}/{id}"), &ts, "final answer while the last test command failed", &session));
                        } else if let Some(w) = last_write
                            && !tests_in_run.iter().any(|(i, _)| *i > w)
                        {
                            extra.push(mk("finish.untested", format!("event:{session}#{id}"), format!("{session}/{id}"), &ts, "final answer after a write with no test run afterwards", &session));
                        }
                        last_final = crate::fsx::parse_rfc3339_ms(&ts).map(|t| (t, id.clone()));
                        writes_in_run.clear();
                        tests_in_run.clear();
                    }
                }
            }
            "steer" => {
                extra.push(mk("owner.steer", format!("event:{session}#{id}"), format!("{session}/{id}"), &ts, "owner steered a busy run", &session));
            }
            "tool_result" => {
                let cid = e.get("call_id").and_then(Value::as_str).unwrap_or("").to_string();
                let name = e.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                let (cname, input) = match calls.get(&cid) {
                    Some(c) => (c.name.clone(), c.input.clone()),
                    None => (name.clone(), Value::Null),
                };
                let name = if name.is_empty() { cname } else { name };
                let out = text_of(e.get("output").unwrap_or(&Value::Null));
                let exit = e.get("exit_code").and_then(Value::as_i64);
                let is_error = e.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                let tev = ToolEvent { tool: &name, input: &input, text: &out, exit_code: exit, is_error };
                let hit = classify::classify(&tev, cx);
                let failed = is_error || out.trim_start().starts_with("error:") || (name == "bash" && exit.or_else(|| classify::exit_from_text(&out)).is_some_and(|c| c != 0));
                let idx = results.len();
                let test = (name == "bash").then(|| input.get("command").and_then(Value::as_str).and_then(classify::test_key)).flatten();
                if matches!(name.as_str(), "write" | "edit") && !failed {
                    writes_in_run.push(idx);
                    // A change between a failure and the same call again is not a blind retry.
                    steps_since_error = None;
                }
                if let Some(t) = &test {
                    let _ = t;
                    tests_in_run.push((idx, !failed));
                }
                // retry.same_call: the same call within 3 steps after an error.
                let h = call_hash(&name, &input);
                if let Some((at, prev)) = &steps_since_error
                    && idx - at <= 3
                    && *prev == h
                    && idx != *at
                {
                    extra.push(mk("retry.same_call", format!("event:{session}#{id}"), format!("{session}/{id}/retry"), &ts, &format!("{name} repeated after an error"), &session));
                }
                if failed {
                    steps_since_error = Some((idx, h.clone()));
                }
                seen_calls.push(h);
                // loop.repeat_*: the same read or command ≥ 3 times in a run with no write between.
                let rk = match name.as_str() {
                    "read" => input.get("path").and_then(Value::as_str).map(|p| format!("loop.repeat_read|{p}")),
                    "bash" => input.get("command").and_then(Value::as_str).map(|c| format!("loop.repeat_cmd|{}", c.trim())),
                    "write" | "edit" => {
                        repeat.clear();
                        None
                    }
                    _ => None,
                };
                if let Some(rk) = rk {
                    let n = repeat.entry(rk.clone()).or_insert(0);
                    *n += 1;
                    if *n == 3 {
                        let sig = rk.split('|').next().unwrap_or("loop.repeat_cmd");
                        extra.push(mk(sig, format!("event:{session}#{id}"), format!("{session}/{id}/loop"), &ts, &classify::cap(rk.split('|').nth(1).unwrap_or(""), 160), &session));
                    }
                }
                results.push(Res { uid: format!("{session}/{cid}"), event: id, ts, name, input, text: out, exit, failed, hit, test, run_idx });
            }
            _ => {}
        }
    }

    let mut parsed = Parsed { session: session.clone(), ..Default::default() };
    for (i, r) in results.iter().enumerate() {
        let Some(h) = &r.hit else { continue };
        // Red-then-green: a failing test command followed by the same test command passing.
        if h.signal.starts_with("bash.exit:")
            && let Some(t) = &r.test
            && results[i + 1..].iter().any(|l| l.test.as_deref() == Some(t.as_str()) && !l.failed)
        {
            parsed.expected.push(r.uid.clone());
            continue;
        }
        let later_ok = results[i + 1..].iter().any(|l| !l.failed && l.run_idx == r.run_idx && !h.key.is_empty() && same_key(l, &h.key));
        let mut inst = mk(&h.signal, format!("event:{session}#{}", r.event), r.uid.clone(), &r.ts, &h.detail, &session);
        inst.weight = h.weight;
        inst.failure = h.failure;
        inst.fatal = run.is_some_and(|c| c.failing) && h.failure && !later_ok;
        if let Some(rc) = run {
            inst.refs = vec![format!("run:{}", rc.run)];
        }
        parsed.instances.push(inst);
        let _ = (&r.name, &r.input, &r.text, r.exit);
    }
    parsed.instances.extend(extra);
    parsed
}

fn same_key(r: &Res, key: &str) -> bool {
    match r.name.as_str() {
        "bash" => r.input.get("command").and_then(Value::as_str).is_some_and(|c| c.trim() == key || c.contains(key)),
        _ => r.input.get("path").and_then(Value::as_str) == Some(key) || r.input.get("input").and_then(|i| i.get("path")).and_then(Value::as_str) == Some(key),
    }
}
