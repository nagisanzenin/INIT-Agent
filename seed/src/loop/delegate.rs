//! `delegate`: sub-tasks in fresh contexts (tools/delegate.rs owns the pure part).
//!
//! A sub-agent shares this body's head (persona, rules, tool schemas, so the same head hash and a
//! prompt-cache hit) but not this session's transcript: its message list starts at the task text
//! and grows only with its own turns. The parent's log gains exactly one `tool_result` per
//! delegate call, the summary; the sub-task's tool output never reaches the main context.
//!
//! Based on the `delegate` tool INIT wrote for itself (evo/delegate 4258c2e, amended as 48b6de1).
//! The seed version adds:
//! - **Parallel fan-out.** A batch's delegate calls run on worker threads, at most
//!   `Bounds::parallel` (default 4) at once, before the rest of the batch; results keep call
//!   order. Writers (`mode:"write"`) form one unit, so at most one writer runs at a time.
//! - **Mode.** In a read sub-task `write` and `edit` are refused. `bash` and `ext` keep the body's
//!   rights (Seatbelt cannot nest, kernel/sandbox.md §3.11), so read mode is advisory for them.
//! - **Bounds** in turns, tool calls, requests, tokens and wall time (tools/delegate.rs `Bounds`).
//!   The wall-time cap is hard: each sub-task has a child [`Abort`], and a watchdog triggers it,
//!   which shuts the sub-task's stream and kills its bash groups. An owner abort reaches every
//!   child. A sub-agent cannot delegate, so the tree has depth 1.
//! - **Attribution.** Sub-requests carry the parent's `x-init-task` and `x-init-subtask:
//!   delegate-<call id>` (kernel/proxy.md §3.5a).
//! - **Display.** Each inner call sends `tool_start`/`tool_end` with `parent_call_id` (the TUI
//!   nests it under the delegate row) and a batch-unique `call_id` (`<delegate id>/<inner id>`).
//! - **Durable record.** `state/spill/<session>/delegate-<call_id>.json` holds every sub-task's
//!   full transcript, bounds, tally, end reason and the returned summary, on every exit path.
//!
//! Rendering needs `&mut` context, so worker threads send render requests to the batch thread,
//! which owns the context and answers them while it waits for results.
//!
//! @init compensates-for: context-pollution

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};

use serde_json::json;

use super::*;
use crate::context::project::{Msg, ToolRes};
use crate::log::event::Block;
use crate::model::ContentBlock;
use crate::tools::delegate::{self, Access, Args, Bounds, Tally};
use crate::tools::{Job, text_result};

const IN_SUB: &str = "error: not_available: delegate cannot run inside a sub-task (depth 1). Do the work with the other tools and return the result.";

/// A sub-task's request for a rendered body; answered by the batch thread.
struct RenderReq {
    msgs: Vec<Msg>,
    max_tokens: u32,
    reply: Sender<Result<(Vec<u8>, String), String>>,
}

enum Ev {
    Render(RenderReq),
    Done(usize, Ran),
}

/// What every sub-task of one batch shares (all `Sync`).
struct SubEnv<'a> {
    model: &'a dyn ModelClient,
    tools: &'a Tools,
    reg: Option<&'a Snapshot>,
    out: &'a OutTx,
    status: &'a Status,
    abort: &'a Abort,
    pcx: PathCx<'a>,
    spill_dir: &'a Path,
    info: &'a ModelInfo,
    mode: Mode,
    effort: Effort,
    session: &'a str,
    task: Option<&'a str>,
    max_tokens: u32,
}

/// How a sub-task ended (the record's `end`).
#[derive(Clone, Debug, PartialEq, Eq)]
enum End {
    Done,
    Capped(String),
    TimedOut,
    Aborted,
    Failed(String, String),
}

impl End {
    fn as_str(&self) -> String {
        match self {
            End::Done => "done".into(),
            End::Capped(w) => format!("capped: {w}"),
            End::TimedOut => "capped: wall time".into(),
            End::Aborted => "aborted".into(),
            End::Failed(c, _) => format!("error: {c}"),
        }
    }
}

/// A log block as JSON, for the sub-task's record only (the log's own shapes are elsewhere).
fn block_values(blocks: &[Block]) -> Vec<serde_json::Value> {
    blocks
        .iter()
        .map(|b| match b {
            Block::Text { text } => json!({"type": "text", "text": text}),
            Block::Thinking { text, .. } => json!({"type": "thinking", "text": text}),
            Block::ToolUse {
                call_id,
                name,
                input,
                ..
            } => json!({"type": "tool_use", "call_id": call_id, "name": name, "input": input}),
            other => json!({"type": "other", "raw": format!("{other:?}")}),
        })
        .collect()
}

/// Rough token estimate for a block of text (the real calibration lives in `context::Calib`).
fn est_tokens(s: &str) -> u64 {
    (s.len() as u64 / 4).max(1)
}

fn to_blocks(content: &[ContentBlock]) -> Vec<Block> {
    let mut out = Vec::new();
    for b in content {
        match b {
            ContentBlock::Text { text } => out.push(Block::text(text.clone())),
            ContentBlock::Thinking(t) => out.push(Block::Thinking {
                provider: t.provider.clone(),
                model: t.model.clone(),
                text: t.text.clone(),
                native: t.native.clone(),
            }),
            ContentBlock::ToolCall(c) => out.push(Block::ToolUse {
                call_id: c.id.clone(),
                name: c.name.clone(),
                input: c.input.clone(),
                raw_arguments: c.raw_arguments.clone(),
            }),
            // Images and opaque blocks are not replayed into a sub-request: the summary is text.
            ContentBlock::Image { .. } | ContentBlock::Opaque { .. } => {}
        }
    }
    out
}

fn result_tokens(r: &ToolResult) -> u64 {
    r.content
        .iter()
        .map(|b| match b {
            ContentBlock::Text { text } => est_tokens(text),
            _ => 0,
        })
        .sum()
}

impl Session {
    /// Runs the batch's delegate calls (`idx`, all `Job::Delegate`): readers concurrently, the
    /// writers one at a time, at most `Bounds::parallel` at once. Each sends its own `tool_start`
    /// when it begins; this thread sends each `tool_end` as it finishes and answers the workers'
    /// render requests. Returns `(index, Ran)` in completion order.
    pub(super) fn run_delegates(
        &mut self,
        prepared: &[Prepared],
        idx: &[usize],
        reg: Option<&Snapshot>,
        task: Option<&str>,
        spill_dir: &Path,
    ) -> Vec<(usize, Ran)> {
        let session = self.ctx.session_id();
        let info = self.model.info().clone();
        let bounds = self.tools.delegate;
        let env = SubEnv {
            model: &*self.model,
            tools: &self.tools,
            reg,
            out: &self.out,
            status: &self.status,
            abort: &self.abort,
            pcx: PathCx {
                cwd: &self.cwd,
                home: self.user_home.as_deref(),
                init_home: Some(&self.home),
            },
            spill_dir,
            info: &info,
            mode: self.mode,
            effort: self.effort,
            session: &session,
            task,
            max_tokens: info.default_max_tokens.min(info.max_output.max(1)),
        };
        let ctx = &mut self.ctx;
        let writers: Vec<usize> = idx
            .iter()
            .copied()
            .filter(|&i| {
                matches!(&prepared[i], Prepared::Ready { job: Job::Delegate(a), .. }
                    if a.mode == Access::Write)
            })
            .collect();
        let mut units: Vec<Vec<usize>> = Vec::new();
        if !writers.is_empty() {
            units.push(writers.clone()); // one unit: writers never overlap
        }
        units.extend(
            idx.iter()
                .copied()
                .filter(|i| !writers.contains(i))
                .map(|i| vec![i]),
        );
        let cursor = AtomicUsize::new(0);
        let (env_ref, units_ref, cursor_ref) = (&env, &units, &cursor);
        let run_one = |i: usize, tx: &Sender<Ev>| -> Ran {
            let p = &prepared[i];
            env_ref.out.send(Out::Display {
                kind: "tool_start",
                text: env_ref.tools.start_text(p),
                call_id: Some(p.call_id().to_string()),
                name: Some(p.name().to_string()),
                ok: None,
                parent: None,
                detail: None,
            });
            let Prepared::Ready {
                call_id,
                job: Job::Delegate(a),
                ..
            } = p
            else {
                return Ran {
                    result: error_result(
                        p.call_id(),
                        "error: internal: run_delegates got a call that is not a delegate",
                    ),
                    summary: "error: internal".into(),
                };
            };
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_sub(env_ref, call_id, a, bounds.narrow(a), tx)
            }))
            .unwrap_or_else(|payload| {
                let msg = format!(
                    "error: panic: the delegate sub-task crashed: {}",
                    panic_message(&*payload)
                );
                Ran {
                    result: error_result(call_id, &msg),
                    summary: "error: panic".into(),
                }
            })
        };
        let mut done: Vec<(usize, Ran)> = Vec::new();
        std::thread::scope(|sc| {
            let (tx, rx) = mpsc::channel::<Ev>();
            let workers = units.len().min(bounds.parallel.max(1));
            let mut spawned = 0;
            for w in 0..workers {
                let (tx, run_one) = (tx.clone(), &run_one);
                let r = std::thread::Builder::new()
                    .name(format!("delegate-{w}"))
                    .spawn_scoped(sc, move || {
                        loop {
                            let u = cursor_ref.fetch_add(1, Ordering::Relaxed);
                            let Some(unit) = units_ref.get(u) else { break };
                            for &i in unit {
                                let ran = run_one(i, &tx);
                                let _ = tx.send(Ev::Done(i, ran));
                            }
                        }
                    });
                if r.is_ok() {
                    spawned += 1;
                }
            }
            drop(tx);
            for ev in rx {
                match ev {
                    Ev::Render(r) => {
                        let res = ctx
                            .subtask_request(&r.msgs, r.max_tokens)
                            .map_err(|e| e.to_string());
                        let _ = r.reply.send(res);
                    }
                    Ev::Done(i, ran) => {
                        env_ref.status.progress();
                        send_end(env_ref.out, &prepared[i], &ran);
                        done.push((i, ran));
                    }
                }
            }
            if spawned == 0 {
                // No worker thread could start: nothing ran, and every call says so.
                for &i in idx {
                    let p = &prepared[i];
                    let ran = Ran {
                        result: error_result(
                            p.call_id(),
                            "error: io_error: could not start a sub-task thread; nothing ran",
                        ),
                        summary: "error: io_error".into(),
                    };
                    send_end(env_ref.out, p, &ran);
                    done.push((i, ran));
                }
            }
        });
        done
    }
}

/// Runs one sub-task on this (worker) thread, under its own child abort and wall-time watchdog.
fn run_sub(env: &SubEnv, call_id: &str, args: &Args, b: Bounds, tx: &Sender<Ev>) -> Ran {
    let t0 = Instant::now();
    let child = env.abort.child();
    let timed_out = AtomicBool::new(false);
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let label = delegate::subtask_label(call_id);
    let (child_ref, timed_ref) = (&*child, &timed_out);
    let mut st = std::thread::scope(|sc| {
        let _ = std::thread::Builder::new()
            .name("delegate-watchdog".into())
            .spawn_scoped(sc, move || {
                if let Err(RecvTimeoutError::Timeout) = stop_rx.recv_timeout(b.max_wall) {
                    timed_ref.store(true, Ordering::SeqCst);
                    child_ref.trigger();
                }
            });
        let st = sub_loop(env, call_id, args, &b, &label, child_ref, timed_ref, tx);
        drop(stop_tx); // wakes the watchdog
        st
    });
    st.tally.wall_ms = t0.elapsed().as_millis() as u64;
    // A stop is the owner's abort when the parent fired, else the wall-time watchdog.
    if st.end == End::Aborted && !env.abort.is_set() && timed_out.load(Ordering::SeqCst) {
        st.end = End::TimedOut;
    }
    let mut text = st.text.clone();
    match &st.end {
        End::Capped(w) => text.push_str(&format!("\n\n[init:delegate] the sub-task hit its {w}")),
        End::TimedOut => text.push_str(&format!(
            "\n\n[init:delegate] the sub-task hit its {} s wall-time cap and was stopped",
            b.max_wall.as_secs()
        )),
        _ => {}
    }
    let record = record_path(env.spill_dir, call_id);
    let shown = record.to_string_lossy().into_owned();
    let (result, summary) = match &st.end {
        End::Aborted => (
            error_result(call_id, ABORTED_TEXT),
            "aborted".to_string(),
        ),
        End::Failed(code, msg) => (
            error_result(
                call_id,
                &format!("error: {code}: {msg}\n[init:delegate] record {shown}"),
            ),
            format!("error: {code}"),
        ),
        _ => {
            let body = delegate::result_text(&text, &st.tally, Some(&shown));
            let line = format!(
                "{} · {} call{}",
                st.end.as_str(),
                st.tally.calls,
                if st.tally.calls == 1 { "" } else { "s" }
            );
            (text_result(call_id, body), line)
        }
    };
    let returned = result
        .content
        .iter()
        .find_map(|c| match c {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .unwrap_or("");
    write_record(&record, call_id, args, &b, &label, &st, returned);
    Ran { result, summary }
}

struct SubState {
    msgs: Vec<Msg>,
    tally: Tally,
    text: String,
    end: End,
}

/// The sub-task's turns. Stops at the first of: a reply with no tool calls (done), a cap, an
/// abort (owner or wall time), a failure.
#[allow(clippy::too_many_arguments)]
fn sub_loop(
    env: &SubEnv,
    call_id: &str,
    args: &Args,
    b: &Bounds,
    label: &str,
    abort: &Abort,
    timed_out: &AtomicBool,
    tx: &Sender<Ev>,
) -> SubState {
    let mut opening = delegate::opening(args, b);
    if b.max_turns == 1 || b.max_requests == 1 {
        opening.push_str("\n\n");
        opening.push_str(&delegate::last_turn_note("budget"));
    }
    let mut s = SubState {
        msgs: vec![Msg::User {
            tokens: est_tokens(&opening),
            blocks: vec![Block::text(opening)],
        }],
        tally: Tally::default(),
        text: String::new(),
        end: End::Done,
    };
    let stopped = |s: &mut SubState| -> bool {
        if abort.is_set() || timed_out.load(Ordering::SeqCst) {
            s.end = End::Aborted; // refined by the caller (owner abort or wall time)
            return true;
        }
        false
    };
    let mut last = b.max_turns == 1 || b.max_requests == 1;
    loop {
        if stopped(&mut s) {
            return s;
        }
        s.tally.turns += 1;
        let (rtx, rrx) = mpsc::channel();
        let req = RenderReq {
            msgs: s.msgs.clone(),
            max_tokens: env.max_tokens,
            reply: rtx,
        };
        if tx.send(Ev::Render(req)).is_err() {
            s.end = End::Failed("context".into(), "the batch thread is gone".into());
            return s;
        }
        let (body, head_hash) = match rrx.recv() {
            Ok(Ok(x)) => x,
            Ok(Err(e)) => {
                s.end = End::Failed(
                    "context".into(),
                    format!("could not render the sub-request: {e}"),
                );
                return s;
            }
            Err(_) => {
                s.end = End::Failed("context".into(), "no render reply".into());
                return s;
            }
        };
        let req = Request {
            body: &body,
            head_hash: &head_hash,
            messages: &s.msgs,
            max_tokens: env.max_tokens,
            effort: env.effort,
            purpose: Purpose::Turn,
            session: env.session,
            task: env.task,
            subtask: Some(label),
        };
        s.tally.requests += 1;
        let mut reply: Option<AssistantMessage> = None;
        let mut failed: Option<ProviderError> = None;
        for ev in env.model.stream(&req, abort) {
            env.status.progress();
            match ev {
                StreamEvent::Retry { .. } => s.tally.requests += 1,
                StreamEvent::Done(m) => {
                    reply = Some(m);
                    break;
                }
                StreamEvent::Failed(e) => {
                    failed = Some(e);
                    break;
                }
                _ => {}
            }
        }
        abort.clear_stream();
        if let Some(e) = failed {
            if !stopped(&mut s) {
                s.end = End::Failed("provider".into(), format!("the sub-task stopped: {e}"));
            }
            return s;
        }
        let Some(m) = reply else {
            s.end = End::Failed(
                "protocol".into(),
                "the sub-task's stream ended without a terminal event".into(),
            );
            return s;
        };
        s.tally.in_tokens += m.usage.input;
        s.tally.out_tokens += m.usage.output;
        if matches!(m.stop, StopReason::Aborted) || stopped(&mut s) {
            s.end = End::Aborted;
            return s;
        }
        s.text = m.text();
        let calls: Vec<ToolCall> = m.tool_calls().cloned().collect();
        if calls.is_empty() {
            s.end = End::Done;
            return s;
        }
        s.msgs.push(Msg::Assistant {
            event: 0,
            blocks: to_blocks(&m.content),
            tokens: est_tokens(m.text().as_str()),
        });
        let mut results = Vec::new();
        for c in &calls {
            let ran = if last {
                Ran {
                    result: error_result(
                        &c.id,
                        "error: not_executed: the sub-task's budget is spent; reply with your result",
                    ),
                    summary: "error: not_executed".into(),
                }
            } else if s.tally.calls >= b.max_calls {
                Ran {
                    result: error_result(
                        &c.id,
                        &format!(
                            "error: not_executed: the sub-task's {}-call cap is reached",
                            b.max_calls
                        ),
                    ),
                    summary: "error: not_executed".into(),
                }
            } else if abort.is_set() || timed_out.load(Ordering::SeqCst) {
                Ran {
                    result: error_result(&c.id, ABORTED_TEXT),
                    summary: "aborted".into(),
                }
            } else {
                s.tally.calls += 1;
                inner_call(env, call_id, args.mode, c, abort)
            };
            results.push(ToolRes {
                event: 0,
                call_id: c.id.clone(),
                name: c.name.clone(),
                content: to_blocks(&ran.result.content),
                is_error: ran.result.is_error,
                tokens: result_tokens(&ran.result),
            });
        }
        s.msgs.push(Msg::ToolResults(results));
        if last {
            s.end = End::Capped(if s.tally.turns >= b.max_turns {
                format!("{} turn cap", b.max_turns)
            } else if s.tally.requests >= b.max_requests {
                format!("{} request cap", b.max_requests)
            } else {
                format!("{} tool-call cap", b.max_calls)
            });
            return s;
        }
        if s.tally.requests >= b.max_requests {
            s.end = End::Capped(format!("{} request cap", b.max_requests));
            return s;
        }
        if s.tally.in_tokens + s.tally.out_tokens >= b.max_tokens {
            s.end = End::Capped(format!("{} token cap", b.max_tokens));
            return s;
        }
        // The next turn is the last one the budget allows: say so, so it answers with what it has.
        let why = if s.tally.turns + 1 >= b.max_turns {
            Some("turn cap")
        } else if s.tally.calls >= b.max_calls {
            Some("tool-call cap")
        } else if s.tally.requests + 1 >= b.max_requests {
            Some("request cap")
        } else {
            None
        };
        if let Some(w) = why {
            last = true;
            let text = delegate::last_turn_note(w);
            s.msgs.push(Msg::Note {
                tokens: est_tokens(&text),
                text,
            });
        }
    }
}

/// One tool call inside a sub-task, with its nested display frames.
fn inner_call(env: &SubEnv, parent: &str, access: Access, c: &ToolCall, abort: &Abort) -> Ran {
    let prep = env.tools.prepare(0, c, &env.pcx);
    let shown_id = delegate::inner_call_id(parent, &c.id);
    if prep.is_ready() {
        env.out.send(Out::Display {
            kind: "tool_start",
            text: env.tools.start_text(&prep),
            call_id: Some(shown_id.clone()),
            name: Some(c.name.clone()),
            ok: None,
            parent: Some(parent.to_string()),
            detail: None,
        });
    }
    let ran = match &prep {
        Prepared::Ready {
            job: Job::Delegate(_),
            ..
        } => Ran {
            result: error_result(&c.id, IN_SUB),
            summary: "error: not_available".into(),
        },
        Prepared::Ready {
            job: Job::Ask(_), ..
        } => Ran {
            result: error_result(&c.id, crate::tools::ask::IN_SUB),
            summary: "error: not_available".into(),
        },
        Prepared::Ready {
            job: Job::Write(_) | Job::Edit(_),
            ..
        } if access == Access::Read => Ran {
            result: error_result(&c.id, &delegate::read_mode_refusal(&c.name)),
            summary: "error: not_available".into(),
        },
        _ => {
            let cx = ToolCx {
                paths: env.pcx,
                spill_dir: env.spill_dir,
                call_id: &c.id,
                abort,
                images: env.info.images,
                model_id: env.info.id,
                mode: env.mode,
                reg: env.reg,
            };
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| env.tools.run(&prep, &cx)))
                .unwrap_or_else(|payload| Ran {
                    result: error_result(
                        &c.id,
                        &format!(
                            "error: panic: the {} tool crashed: {}",
                            c.name,
                            panic_message(&*payload)
                        ),
                    ),
                    summary: "error: panic".into(),
                })
        }
    };
    env.out.send(Out::Display {
        kind: "tool_end",
        text: ran.summary.clone(),
        call_id: Some(shown_id),
        name: Some(c.name.clone()),
        ok: Some(!ran.result.is_error),
        parent: Some(parent.to_string()),
        detail: crate::tools::fail_detail(&ran.result),
    });
    ran
}

fn record_path(spill_dir: &Path, call_id: &str) -> PathBuf {
    spill_dir.join(format!(
        "delegate-{}.json",
        call_id.replace(
            |c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_',
            "_"
        )
    ))
}

/// Keeps the sub-task's record where the session log's removed output goes: readable, not paid
/// for by the next request. Written on every exit path.
fn write_record(
    path: &Path,
    call_id: &str,
    args: &Args,
    b: &Bounds,
    label: &str,
    st: &SubState,
    returned: &str,
) {
    let msgs: Vec<serde_json::Value> = st
        .msgs
        .iter()
        .map(|m| match m {
            Msg::User { blocks, .. } => json!({"role": "user", "blocks": block_values(blocks)}),
            Msg::Assistant { blocks, .. } => {
                json!({"role": "assistant", "blocks": block_values(blocks)})
            }
            Msg::ToolResults(r) => json!({"role": "tool", "results": r
                .iter()
                .map(|t| json!({"call_id": t.call_id, "name": t.name, "is_error": t.is_error,
                                "blocks": block_values(&t.content)}))
                .collect::<Vec<_>>()}),
            Msg::Note { text, .. } => json!({"role": "note", "text": text}),
        })
        .collect();
    let detail = match &st.end {
        End::Failed(_, m) => Some(m.clone()),
        _ => None,
    };
    let rec = json!({
        "tool": "delegate",
        "v": 1,
        "call_id": call_id,
        "subtask": label,
        "task": args.task,
        "context": args.context,
        "mode": args.mode.as_str(),
        "bounds": {
            "max_turns": b.max_turns,
            "max_calls": b.max_calls,
            "max_requests": b.max_requests,
            "max_tokens": b.max_tokens,
            "max_seconds": b.max_wall.as_secs(),
        },
        "end": st.end.as_str(),
        "error": detail,
        "turns": st.tally.turns,
        "tool_calls": st.tally.calls,
        "requests": st.tally.requests,
        "tokens": {"input": st.tally.in_tokens, "output": st.tally.out_tokens},
        "wall_ms": st.tally.wall_ms,
        "summary": returned,
        "messages": msgs,
    });
    if let Some(dir) = path.parent()
        && std::fs::create_dir_all(dir).is_ok()
        && let Ok(bytes) = crate::log::icj::to_vec(&rec)
    {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}
