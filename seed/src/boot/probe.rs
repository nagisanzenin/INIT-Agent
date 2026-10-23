//! The probe round trip (body/boot.md §3.9), probe mode only: the real head and the projected
//! conversation of the staged log, plus one harness instruction, sent with `x-init-purpose: probe`;
//! one real tool round trip under the probe policy; at most 3 model requests; a 60 s watchdog.
//!
//! It drives the same machinery as a loop turn: `ContextHooks::boundary_append`, `next_request`
//! and `on_response` on the loop's context, `ModelClient::stream`, and `Tools::prepare`/`run`
//! (with `catch_unwind`). Only the batch policy differs: exactly `bash {"command":"true"}` runs;
//! every other call gets `POLICY_TEXT`, so the probe cannot change the staging root.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::BootError;
use crate::context::CallStart;
use crate::r#loop::{BoundaryInput, ContextHooks, Timing};
use crate::model::{
    Abort, AssistantMessage, Effort, ModelClient, Purpose, Request, StopReason, StreamEvent,
    ToolCall, ToolResult, Usage,
};
use crate::tools::{Mode, PathCx, Prepared, ToolCx, Tools, error_result};

/// The instruction, fixed here and quoted by awareness.md §3.6 (R2).
pub const PROBE_TEXT: &str = "[init:probe] Gate probe: call the bash tool once with the command \"true\", then reply with the single word ready.";
/// The one nudge when request 1 had no tool call.
pub const NUDGE_TEXT: &str = "[init:probe] Call the bash tool with the command \"true\".";
/// The result of any call other than exactly `bash {"command":"true"}`.
pub const POLICY_TEXT: &str = "error: probe_policy: not executed in probe mode";
pub const WATCHDOG: Duration = Duration::from_secs(60);
pub const MAX_REQUESTS: u32 = 3;

/// The `ready.probe` object (§4.2).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProbeReport {
    pub ok: bool,
    pub requests: u32,
    pub tool_round_trip: bool,
    pub model_called_tool: bool,
    pub model_ms: u64,
    pub tool_ms: u64,
    pub usage: Usage,
    pub request_id: Option<String>,
    pub head_hash: String,
}

impl ProbeReport {
    pub fn to_value(&self) -> Value {
        json!({
            "head_hash": self.head_hash,
            "model_called_tool": self.model_called_tool,
            "model_ms": self.model_ms,
            "ok": self.ok,
            "request_id": self.request_id,
            "requests": self.requests,
            "tool_ms": self.tool_ms,
            "tool_round_trip": self.tool_round_trip,
            "usage": {"cache_read": self.usage.cache_read, "cache_write": self.usage.cache_write,
                      "input": self.usage.input, "output": self.usage.output,
                      "reasoning": self.usage.reasoning},
        })
    }
}

pub struct ProbeCx<'a> {
    pub tools: &'a Tools,
    pub cwd: &'a Path,
    pub home: &'a Path,
    pub user_home: Option<&'a Path>,
    pub effort: Effort,
    pub watchdog: Duration,
}

/// `bash {"command":"true"}` and nothing else.
pub fn is_probe_call(c: &ToolCall) -> bool {
    c.name == "bash"
        && c.args_error().is_none()
        && c.input.len() == 1
        && c.input.get("command").and_then(Value::as_str) == Some("true")
}

/// Calls `Abort::trigger` when `limit` passes; dropped (cancelled) when the probe ends.
struct Watchdog {
    cancel: Option<mpsc::Sender<()>>,
}

impl Watchdog {
    fn start(abort: Arc<Abort>, limit: Duration) -> Watchdog {
        let (tx, rx) = mpsc::channel::<()>();
        let _ = std::thread::Builder::new()
            .name("probe-watchdog".into())
            .spawn(move || {
                if let Err(mpsc::RecvTimeoutError::Timeout) = rx.recv_timeout(limit) {
                    abort.trigger();
                }
            });
        Watchdog { cancel: Some(tx) }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        if let Some(tx) = self.cancel.take() {
            let _ = tx.send(());
        }
    }
}

fn add_usage(a: &mut Usage, b: &Usage) {
    a.input += b.input;
    a.cache_read += b.cache_read;
    a.cache_write += b.cache_write;
    a.output += b.output;
    a.reasoning += b.reasoning;
}

fn probe_err(msg: impl Into<String>) -> BootError {
    BootError::internal("probe", msg)
}

/// Runs one tool call through the normal pipeline (validation already done by `prepare`).
fn run_tool(cx: &ProbeCx, abort: &Abort, sid: &str, model: &dyn ModelClient, p: &Prepared) -> ToolResult {
    let info = model.info();
    let spill_dir = cx.home.join("state").join("spill").join(sid);
    let tcx = ToolCx {
        paths: PathCx {
            cwd: cx.cwd,
            home: cx.user_home,
            init_home: Some(cx.home),
        },
        spill_dir: &spill_dir,
        call_id: p.call_id(),
        abort,
        images: info.images,
        model_id: info.id,
        mode: Mode::Probe,
        reg: None,
    };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cx.tools.run(p, &tcx)))
        .map(|r| r.result)
        .unwrap_or_else(|_| error_result(p.call_id(), "error: panic: the bash tool crashed"))
}

/// One model request: drains the stream; `Err` on a `Failed` terminal event.
fn turn(
    ctx: &mut dyn ContextHooks,
    model: &dyn ModelClient,
    abort: &Abort,
    effort: Effort,
    rep: &mut ProbeReport,
) -> Result<(AssistantMessage, Timing), BootError> {
    let info = model.info().clone();
    let r = ctx
        .next_request(&info, info.default_max_tokens)
        .map_err(|e| probe_err(format!("request render failed: {e}")))?;
    if rep.requests == 0 {
        rep.head_hash = r.head_hash.clone();
    }
    rep.requests += 1;
    let sid = ctx.session_id();
    let req = Request {
        body: &r.body,
        head_hash: &r.head_hash,
        messages: &r.messages,
        max_tokens: info.default_max_tokens,
        effort,
        purpose: Purpose::Probe,
        session: &sid,
        task: r.task_id.as_deref(),
    };
    let t0 = Instant::now();
    let mut outcome = None;
    for ev in model.stream(&req, abort) {
        match ev {
            StreamEvent::Started {
                request_id,
                proxy_req_id,
                ..
            } => {
                if rep.request_id.is_none() {
                    rep.request_id = proxy_req_id.or(request_id);
                }
            }
            StreamEvent::Done(m) => {
                outcome = Some(Ok(m));
                break;
            }
            StreamEvent::Failed(e) => {
                outcome = Some(Err(e));
                break;
            }
            _ => {}
        }
    }
    abort.clear_stream();
    let wall = t0.elapsed().as_millis() as u64;
    rep.model_ms += wall;
    let n = rep.requests;
    match outcome {
        Some(Ok(m)) if m.stop == StopReason::Aborted || abort.is_set() => Err(probe_err(format!(
            "watchdog: request {n} was aborted after {wall} ms"
        ))),
        Some(Ok(m)) => {
            add_usage(&mut rep.usage, &m.usage);
            Ok((
                m,
                Timing {
                    wall_ms: Some(wall),
                    ttft_ms: None,
                },
            ))
        }
        Some(Err(e)) if abort.is_set() => Err(probe_err(format!(
            "watchdog: request {n} was aborted after {wall} ms ({e})"
        ))),
        Some(Err(e)) => Err(probe_err(format!("request {n} failed: {e}"))),
        None => Err(probe_err(format!(
            "request {n}: the stream ended without a terminal event"
        ))),
    }
}

/// §3.9 steps 1–5. `Ok` means every request ended `Done` (write `ready`); `Err` exits 70.
pub fn run(
    ctx: &mut dyn ContextHooks,
    model: &dyn ModelClient,
    abort: &Arc<Abort>,
    cx: &ProbeCx,
) -> Result<ProbeReport, BootError> {
    let _wd = Watchdog::start(abort.clone(), cx.watchdog);
    let mut rep = ProbeReport::default();
    let mut input = BoundaryInput {
        harness_text: Some(PROBE_TEXT.to_string()),
        ..Default::default()
    };
    let mut nudged = false;
    let mut results_pending = false;
    loop {
        ctx.boundary_append(std::mem::take(&mut input))
            .map_err(|e| BootError::log("log", format!("log: {e}")))?;
        if abort.is_set() {
            return Err(probe_err("watchdog fired before the request"));
        }
        let (msg, timing) = turn(ctx, model, abort, cx.effort, &mut rep)?;
        if results_pending {
            rep.tool_round_trip = true; // the request that carried the results came back Done
            results_pending = false;
        }
        let calls: Vec<ToolCall> = msg.tool_calls().cloned().collect();
        let sid = ctx.session_id();
        let pcx = PathCx {
            cwd: cx.cwd,
            home: cx.user_home,
            init_home: Some(cx.home),
        };
        let prepared: Vec<Option<Prepared>> = calls
            .iter()
            .enumerate()
            .map(|(i, c)| is_probe_call(c).then(|| cx.tools.prepare(i, c, &pcx)))
            .collect();
        let normal = !matches!(msg.stop, StopReason::MaxTokens | StopReason::ContextWindow);
        let started: Vec<CallStart> = prepared
            .iter()
            .flatten()
            .filter(|p| normal && p.is_ready())
            .map(|p| CallStart {
                call_id: p.call_id().to_string(),
                name: p.name().to_string(),
                idempotency_key: None,
            })
            .collect();
        ctx.on_response(&msg, &started, timing)
            .map_err(|e| BootError::log("log", format!("log: {e}")))?;
        if calls.is_empty() {
            if !nudged && !rep.model_called_tool && rep.requests < MAX_REQUESTS {
                // Request 1 had no tool call: one nudge (§3.9 step 4).
                nudged = true;
                input.harness_text = Some(NUDGE_TEXT.to_string());
                continue;
            }
            break;
        }
        rep.model_called_tool = true;
        let mut results = Vec::with_capacity(calls.len());
        for (c, p) in calls.iter().zip(prepared) {
            let r = match p {
                Some(p) if normal => {
                    let t = Instant::now();
                    let r = run_tool(cx, abort, &sid, model, &p);
                    rep.tool_ms += t.elapsed().as_millis() as u64;
                    r
                }
                Some(p) => error_result(p.call_id(), "error: length_cutoff: not executed"),
                None => error_result(&c.id, POLICY_TEXT),
            };
            results.push((c.name.clone(), r));
        }
        input.results = results;
        if rep.requests >= MAX_REQUESTS {
            // No further request: still close the round, so the staged log stays well-formed for G3.
            ctx.boundary_append(std::mem::take(&mut input))
                .map_err(|e| BootError::log("log", format!("log: {e}")))?;
            break;
        }
        results_pending = true;
    }
    if !rep.model_called_tool {
        // The model ignored the instruction: `bash true` runs directly (not logged), so the tool
        // path is still exercised; the gate reports `probe_no_round_trip` (§5).
        let call = ToolCall {
            id: "probe-direct".into(),
            name: "bash".into(),
            input: json!({"command": "true"}).as_object().cloned().unwrap_or_default(),
            raw_arguments: None,
        };
        let pcx = PathCx {
            cwd: cx.cwd,
            home: cx.user_home,
            init_home: Some(cx.home),
        };
        let p = cx.tools.prepare(0, &call, &pcx);
        let t = Instant::now();
        let _ = run_tool(cx, abort, &ctx.session_id(), model, &p);
        rep.tool_ms += t.elapsed().as_millis() as u64;
    }
    if abort.is_set() {
        return Err(probe_err("watchdog fired"));
    }
    rep.ok = true;
    Ok(rep)
}
