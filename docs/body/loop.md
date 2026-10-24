# Agent loop
Status: v0.3 + build sync 2026-10-20 · Area: body
Depends on: CONVENTIONS.md, body/provider.md, body/tools.md, body/boot.md, body/log-context.md, kernel/supervise.md, ext/host.md, awareness.md   Defines: the loop algorithm, the thread and channel layout, steering and follow-up semantics, abort semantics, parallel tool execution and result ordering, the length-stop rule, the loop's use of the turn boundary (pause, reload, notices, self-signals), the log events the loop causes, and the `ModelClient` seam used by tests

---

## 1. Purpose

The loop is the body's heart. It does this repeatedly:
1. take owner input;
2. ask the model for the next step;
3. run the tool calls the model asks for, in parallel, with the results in order;
4. feed the results back.

It stops when the model stops calling tools. Everything durable goes through the append-only event log (body/log-context.md). Any body (this one, a restarted one, a newly promoted one) can therefore pick up the same conversation exactly where it was.

The loop is hand-rolled, like every one of the eleven audited production harnesses (research_notes/production_harness_internals.md §1). It runs `while tool_calls or pending_steer` (spec §4).

## 2. Responsibilities / non-goals

**Responsibilities**
- The run, turn and batch algorithm, with no artificial turn cap (§3.10).
- Threads and channels: the input, output and heartbeat threads, the tool workers, and the shared `Abort` and `Status` (§3.1).
- Mapping the kernel's stdio frames to loop actions, and producing the body's frames (§4.1).
- Steering (delivered at the next turn boundary) and follow-ups (delivered when the agent would otherwise stop) (§3.4).
- Abort: cancel a blocking model read, kill running tool process groups, keep partial output (§3.5).
- Parallel tool execution with `std::thread::scope`, results in call order (§3.7).
- The length stop and the other stop-reason rules (§3.6).
- *When* the loop calls log-context's boundary steps, pause/resume, the extension host's reload, and the self-signal hook (§3.8).

**Non-goals**
- Wire formats, retries, timeouts: provider.md.
- Tool behavior: tools.md.
- The event schema, the projection, the boundary steps themselves, durability points D1–D3, compaction, the head hash, epochs: log-context.md.
- The stdio protocol, the pause/handoff protocol, the owner channel: kernel/supervise.md §3.6.4, §3.10, §4.5. This doc **consumes** them.
- Wasmtime, reload internals: ext/host.md.
- Boot, rehydration, `ready`, the heartbeat writer: boot.md.
- What a self-signal says: awareness.md.
- Growth patterns: loop detection, a verification checklist, time-budget awareness, speculative tool execution (spec §4).

## 3. Design

### 3.1 Threads, channels and shared state

```
                      stdin: kernel frames (INIT_UI=stdio-jsonl/1)            stdout: body frames
                                │                                                   ▲
                       ┌────────▼────────┐  Abort::trigger() on interrupt  ┌────────┴────────┐
                       │  input thread   │──────────────┐ ext.begin_reload │  output thread  │
                       └────────┬────────┘              │ on reload       └────────▲────────┘
                ctl: mpsc::channel<Control>             │          out: mpsc::sync_channel<Out>(1024)
                                │                       ▼                           │
 ┌──────────────────────────────▼───────────────────────────────────────────────────┴────────────┐
 │ main thread = LOOP  (owns Session: ctx (log writer + projection), ModelClient, Tools, ExtHost) │
 │   serve() ─▶ run() ─▶ boundary() ─▶ model turn (blocking read on proxy socket) ─▶ batch      │
 │                                     ▲ shutdown(Both) by Abort ends the read                   │
 │   execute_batch(): std::thread::scope ── tool-0 … tool-15 workers (≤16)                       │
 │                       results: mpsc::channel<(usize, ToolResult)>  (workers → loop)           │
 │                       bash call → its own reader + waiter threads (tools.md §3.5)             │
 │                       ext call  → host's own worker thread (ext/host.md §3.10)                │
 └───────────────────────────────────────────────────────────────────────────────────────────────┘
 heartbeat thread (boot.md §3.12): reads Arc<Status> every 5 s, writes state/run/heartbeat (also while paused)
 Arc<Abort>: flag · in-flight UnixStream clone · live bash process groups · Condvar
 Arc<Status>: phase · busy · run/turn counters · last_progress · leaf id · live tool pgids
```

| Thread | Lifetime | Blocks on | Sends to |
|---|---|---|---|
| main (loop) | process | `ctl.recv()` when idle; proxy socket read; batch `results` rx | `out`; the log (single writer) |
| input | process | `stdin.read_line` | `ctl`. It also calls `Abort::trigger` itself for `interrupt`, and `ext.begin_reload(…)` for `reload`: `ReloadKind::Request` for `kind:"request"`, `ReloadKind::Kept(components)` for `kind:"kept"` (§3.8.2) |
| output | process | `out.recv()` | stdout, one frame per line, flushed |
| heartbeat | process | `thread::sleep(5 s)` | file write |
| tool workers | one batch (`thread::scope`) | the tool | `results` |

- **Why the input thread triggers abort itself.** The loop thread may be blocked in a socket read or inside `thread::scope`. `Abort::trigger` touches only `Mutex`-guarded handles and atomics.
- **Why it starts reloads itself.** host.md's `begin_reload` returns at once and compiles off the hot path. The swap still happens only at a turn boundary.

Channels (std only):
- `ctl` is unbounded (`mpsc::channel`): kernel frames are tiny and must never be dropped.
- `out` is bounded (`mpsc::sync_channel(1024)`):
  - `display` text and thinking deltas use `try_send` and are dropped when the queue is full; the drop count goes into the next `turn_end` text;
  - every other frame uses a blocking `send`.
  - A stalled kernel reader therefore never stalls the model stream, while protocol frames are never lost.
- Batch `results` is unbounded and per batch.

```rust
use std::sync::{Arc, Mutex, Condvar, atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering}};

pub struct Abort {                                   // provider.md §3.11 uses the stream part
    flag: AtomicBool,
    stream: Mutex<Option<std::os::unix::net::UnixStream>>,
    groups: Mutex<std::collections::BTreeSet<i32>>,  // live bash process-group ids (pgid = leader pid)
    wake: (Mutex<()>, Condvar),
    kill_group: fn(i32),                             // SIGKILLs one group; boot passes tools::kill_group, so this type does not depend on tools
}
impl Abort {
    pub fn new(kill_group: fn(i32)) -> Self;
    pub fn trigger(&self) {                          // idempotent, any thread
        self.flag.store(true, Ordering::SeqCst);
        if let Some(s) = self.stream.lock().unwrap().take() { let _ = s.shutdown(std::net::Shutdown::Both); }
        for &pg in self.groups.lock().unwrap().iter() { (self.kill_group)(pg); }  // SIGKILL, tools.md §3.5.4
        let _g = self.wake.0.lock().unwrap(); self.wake.1.notify_all();
    }
    pub fn reset(&self);                             // start of each run; clears flag and stale handles
    pub fn is_set(&self) -> bool;
    pub fn register_group(&self, pg: i32) -> bool;   // false ⇒ already aborted: caller kills the group itself
    pub fn unregister_group(&self, pg: i32);
    pub fn flag(&self) -> &AtomicBool;               // passed to ext calls as CallCtx.abort (host.md §3.10; tools.md §3.6)
}

#[repr(u8)] pub enum Phase { Booting = 0, Idle = 1, Model = 2, Tools = 3, Paused = 4, Stopping = 5 }
pub struct Status {
    pub phase: AtomicU8, pub run: AtomicU64, pub turn: AtomicU64,
    pub last_progress_ms: AtomicU64,                 // ms since boot of the last stream byte / tool event / log append
    pub leaf: Mutex<String>,
}
impl Status { pub fn busy(&self) -> bool { matches!(self.phase(), Phase::Model | Phase::Tools) } }
```

The `unwrap()` calls on `lock()` can only panic on a poisoned mutex, which needs a panic while the lock is held. No code inside these critical sections can panic.

### 3.2 The model seam

The loop never touches HTTP. It depends on a narrow trait. The real implementation is provider.md's `stream()`; tests supply a scripted one.

```rust
// src/model.rs (shared with provider.md)
pub trait ModelClient: Send + Sync {
    fn info(&self) -> &ModelInfo;
    fn stream<'a>(&'a self, req: &'a Request<'a>, abort: &'a Abort)
        -> Box<dyn Iterator<Item = StreamEvent> + 'a>;
}
```

The `Request` here is byte-based: it carries the wire body that log-context already rendered (§4.2).

### 3.3 Algorithm

The boundary order is log-context.md §3.13's, adopted as-is:

| Step | What happens |
|---|---|
| 1 | tool results |
| 2 | steers |
| 3 | reload |
| 4 | notices |
| — | pause check (a pause found here runs before steps 5–8) |
| 5 | rebuild check (mask, compaction) |
| 6 | signal |
| 7 | sync (D2) |
| 8 | render and send |

log-context's boundary is split into two calls, `boundary_append` (steps 1–4) and `next_request` (steps 5–8), so that the pause check can sit between step 4 and step 5 (log-context §4.3):

```rust
pub enum RunEnd { Idle, Aborted, Refused, Failed, Stopping, Exit(u8) }   // Exit(code): the process must exit now (75 handoff, 0 stop, 1 fatal log error)
pub struct BoundaryInput {                          // what the loop hands to log-context steps 1–4
    pub results: Vec<(String, ToolResult)>,         // (tool name, result), call order (§3.7); empty at the start of a run
    pub owner: Vec<OwnerInput>,                     // owner text that starts a run, or a follow-up
    pub steers: Vec<OwnerInput>,
    pub commands: Vec<OwnerInput>,                  // /compact /new /branch /label (log-context §4.5)
    pub reload: Vec<ReloadIn>,                      // one per tier (T0, T1), §3.8.2
    pub notices: Vec<Notice>,
    pub harness_text: Option<String>,               // e.g. the cut-off note (§3.6)
    pub errors: Vec<ErrorEv>,                       // run-ending errors and provider warnings (§3.9)
}

impl Session {
    /// Called by boot.md after `ready` is written. Returns the process exit code (0 stop or EOF, 75 handoff,
    /// 1 fatal log error); the loop never calls `process::exit`, and boot flushes `out` and exits with the code.
    pub fn serve(mut self) -> u8 {
        if self.ctx.has_pending_work() && self.mode == Mode::Normal {
            self.run(BoundaryInput::default());           // continue after handoff / crash / promote (spec §6.2 step 4)
        }
        loop {
            self.set_phase(Phase::Idle);                   // emits state{busy:false} on change
            match self.ctl.recv() {
                Ok(Control::Input(i)) => match self.classify(i) {
                    Owner(i) => { self.run(BoundaryInput { owner: vec![i], ..Default::default() }); }
                    Command(c) => self.idle_boundary(BoundaryInput { commands: vec![c], ..Default::default() }),
                    Duplicate(seq) => self.out.send(Out::Ack { seq }),           // already in the log
                },
                Ok(Control::Notice(n)) => self.idle_boundary(BoundaryInput { notices: vec![n], ..Default::default() }),
                Ok(Control::Reload(job)) => self.idle_boundary(BoundaryInput { reload: Some(self.commit_reload(job)), ..Default::default() }),
                Ok(Control::Pause(p)) => if let Some(code) = self.pause(p) { return self.exit(code); },
                Ok(Control::Interrupt) => {}               // nothing running
                Ok(Control::Stop(_)) => return self.exit(0),
                Ok(other) => self.protocol_warning(other),  // resume/exit while not paused
                Err(_) => return self.exit(0),              // stdin EOF: the kernel is gone
            }
            if self.stopping { return self.exit(0); }
        }
    }

    fn run(&mut self, first: BoundaryInput) -> RunEnd {
        self.abort.reset();
        self.status.run.fetch_add(1, Ordering::Relaxed);
        let mut g = Guards::default();                     // length strikes, overflow retried, max_tokens boost
        let mut input = first;
        loop {                                             // one iteration = one turn
            let req = match self.boundary(&mut input, &mut g) {          // §3.8: steps 1–8
                Ok(req) => req,
                Err(end) => return end,
            };
            self.set_phase(Phase::Model);
            let msg = match self.model_turn(&req) {        // forwards deltas to `out` (§3.6)
                TurnOutcome::Done(m) => m,
                TurnOutcome::Failed(e) if e.is_context_overflow() && !g.overflow_retried => {
                    g.overflow_retried = true; self.ctx.request_emergency_compaction(); continue;   // log-context §3.12.6
                }
                TurnOutcome::Failed(e) => { self.log_error("provider", &e); return RunEnd::Failed; }
            };
            g.overflow_retried = false;
            let (calls, prefilled) = self.tools.prepare_all(&msg);   // §3.6 validation; Ready calls only get markers
            self.ctx.on_response(&msg, &calls.started())?;  // message + tool_call markers, then sync D1
            input = BoundaryInput::default();
            match &msg.stop {
                StopReason::Aborted => { input.results = self.aborted_results(&calls, prefilled); return self.end_aborted(input); }
                StopReason::Refusal { .. } => { input.results = refused_results(&calls, prefilled); return self.end_refused(input); }
                StopReason::MaxTokens | StopReason::ContextWindow => {
                    g.length_strikes += 1;                 // §3.6
                    if calls.is_empty() { input.harness_text = Some(cut_off_note(req.max_tokens)); }
                    else { input.results = length_cutoff_results(&calls, prefilled, req.max_tokens); }
                    if g.length_strikes >= 3 { self.log_error_guard("length", &msg); return self.end_failed(input); }
                    g.boost_next = true;                    // next request only: max_tokens × 2, capped at max_output
                    continue;
                }
                _ => { g.length_strikes = 0; }
            }
            if calls.is_empty() {
                if self.queue.has_steer() { continue; }    // the next boundary appends them
                if let Some(f) = self.queue.pop_follow_up() { input.owner.push(f); continue; }
                return RunEnd::Idle;
            }
            self.set_phase(Phase::Tools);
            input.results = self.execute_batch(&calls, prefilled);   // §3.7: call order
            self.status.turn.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The turn boundary: log-context §3.13 with the loop's pause check between steps 4 and 5.
    fn boundary(&mut self, input: &mut BoundaryInput, g: &mut Guards) -> Result<Request, RunEnd> {
        while let Ok(c) = self.ctl.try_recv() { self.enqueue(c); }   // inputs → steer/follow-up/command queues; notices; flags
        input.steers.extend(self.queue.drain_steer());
        input.commands.extend(self.queue.drain_commands());
        input.notices.extend(self.queue.drain_notices());
        if let Some(job) = self.pending_reload.take() { input.reload = Some(self.commit_reload(job)); }
        else { self.ext.commit_reload(Duration::ZERO); }          // host.md §3.9: pick up late loads for free
        self.ctx.boundary_append(std::mem::take(input))?;          // steps 1–4 (+ sync), then send acks/notice_acks
        self.send_acks();
        if self.abort.is_set() { return Err(self.end_aborted_at_boundary()); }
        if let Some(p) = self.pending_pause.take() {
            if let Some(code) = self.pause(p) { self.exit_now(code); }   // handoff completed: exit 75
        }
        if self.stopping { return Err(RunEnd::Stopping); }
        self.ctx.next_request(self.model.info(), g.max_tokens(self.model.info()))   // steps 5–8 (signal, sync D2, render)
            .map_err(|e| { self.log_error("context", &e); RunEnd::Failed })
    }
}
```

- The sketches in this section and in §3.7 are simplified. `prepare_all` and `prefilled` stand for the results of `Tools::prepare`, called once per call (`Prepared::Done` carries the pre-filled result). The built signatures of the seams are in §4.2.
- A run *starts* with a boundary that appends the owner's message (log-context's "owner message" boundary). Each later turn's boundary appends the previous batch's results.
- **This boundary is supervise's "step boundary"** (kernel/supervise.md §3.6.4 phase 1). At that point no provider request is in flight, no tool of the batch is running, and every event is appended and synced. A pending pause is checked at every boundary, which comes after each tool batch, and at once while idle (`serve()`). `state{busy}` is emitted by `set_phase` on every busy↔idle change, and `interrupt` is handled in §3.5. R1 confirmation of supervise Q1.
- **No epoch starts inside an open tool round** (log-context X7). Step 1 closes the round by appending every result before step 5 can mask or compact. Aborted and length-stopped replies get their synthetic results in step 1 too, and the D1 and D2 sync points are exactly log-context's.
- `Guards` are not turn caps. Each fires only when the run cannot make progress (§3.10).
- **Task id (`x-init-task`).** A *task* is the run started by one owner input (evolution.md §3.1). Its id is that input's `id`, the 16-hex `input_id` the kernel put on the `input` frame; the body chooses it, and the kernel assigns none (supervise §9 item 5). **Every request of the run carries it** as `Request.task` (provider.md §3.3), turn and compaction requests alike. The rule, stated on the log so that a restarted or newly promoted body sends the same id with no extra state: `Request.task` = the `input_id` of the **latest owner `message`** (`role:"user"`, `source:"owner"`) on the current path.
  - A plain input that starts a run, and a delivered follow-up (§3.4), are appended as owner messages, so each starts a new task.
  - Steers are `steer` events and commands are `command` events, so they stay in the current task. Harness messages (`[init:cutoff]`, `[init:probe]`) do not count.
  - After a handoff, crash or promote, the continuing run (`serve()` with pending work) keeps the task of the owner message it continues.
  - With no owner message on the path (a fresh `/new` session), `Request.task` is `None` and the proxy books the request as `_none` under the same cap (kernel/proxy.md §3.4). In a probe the header is ignored, because gate scopes are their own attribution.
  - `next_request` fills the field when it builds a request (log-context §4.3; cross-doc §9), and log-context records the value as `message.request.task_id`. The agent files the finished task under `state/tasks/<task_id>/`, and the owner accepts it with `/accept <task_id>`.

### 3.4 Steering, follow-ups and commands

kernel/supervise.md §4.5 delivers all owner text as one frame type, `input{id, seq, text, ts, mode}`, where `mode` is `"steer"` (the default) or `"follow_up"`. The loop classifies each input once:
1. **Duplicate.** `input_id` is already in the log (supervise redelivers unacked inputs after a restart): answer `ack{seq}` and do nothing else.
2. **Command.** The text starts with `/compact`, `/new`, `/branch` or `/label` (log-context.md §4.5): it goes to the command queue and is handled by log-context at the next boundary. `/panic` and the other kernel commands never reach the body.
3. **Follow-up.** The input carries `"mode":"follow_up"`. The owner gets it by typing `/later <text>` in `init-kernel attach` (supervise §3.10, §4.6), and the gate's `send_input` may set it. A missing or unknown `mode` is treated as `steer`.
4. **Plain.** Anything else.

| Input arrives while… | Plain input | Follow-up | Log event |
|---|---|---|---|
| idle | starts a run at once | starts a run at once | `message` (role user, `input_id`) |
| a run is in progress | **steer**: queued, delivered at the next turn boundary. That is after the current tool batch's results and before the next model request; if the current reply has no tool calls, right after it, and the run continues | queued; delivered only when the run would otherwise end (no tool calls, steer queue empty), **one at a time**, each starting another turn | `steer` / `message` |

Rules:
- Steering never interrupts a running model stream or tool batch (pi-agent-core; Claude Code: a queued message is read "as soon as those calls finish, within the same turn").
- In the context, a steer is user text in the same user-side group as the tool results that precede it (log-context §3.13 order: results, then steers, then notes). Each adapter renders that legally (provider.md §3.8).
- `ack{seq}` is sent only after the boundary that appended the input has synced the log (supervise: "ack = input durably in the log"). An input still sitting in the steer queue at a crash is not lost: it is unacked, so supervise redelivers it.
- At a pause (§3.8.1), steers, follow-ups and notices already queued are appended at that boundary *before* the ack. Inputs that arrive during a pause are held by the kernel, not by the body (supervise §3.6.4 step 1).
- At abort (§3.5), the queued items are appended at the same final boundary, but no model request follows; the run ends idle. What the owner typed is in the context of the next run. A failed or refused run ends the same way.
- At an abort, failed or pause boundary, queued follow-ups are appended as owner messages (each one starts a new task, §3.3). A steer still queued when the body goes idle starts a run, as an owner message.
- **Retract.** `retract{seq}` asks to take input `seq` back. The input thread answers at once (never at a boundary, which can be an hour away behind a long bash) with `retracted{seq, ok}`, deciding under the lock of a shared gate (`Status.gate`: `retracted` and `consumed` seq sets, each bounded to the newest 1024). `ok:true`: the input was not consumed, and it will never be appended, whether it is queued now or arrives later. It is dropped silently with no `ack`, because the kernel already marked it done. `ok:false`: the input was already committed to an append, so it is acked normally. The loop's single choke point before `ctx.boundary_append` filters retracted owner inputs, steers, commands and delivered follow-ups, and marks the rest consumed, under the same lock, so the two decisions are atomic. If everything new in a boundary that would start or continue a run on owner text alone was retracted, no request is sent and the run ends idle. A follow-up stays retractable until it is delivered. A redelivered input that was already in the log before a restart is a duplicate. It is acked even if it was retracted, so a late `ack` after `ok:true` means it was consumed after all.

### 3.5 Abort

**Sources:**
- `interrupt` from the kernel (the owner pressed Ctrl-C in `init-kernel attach`);
- `stop` whose grace expires;
- stdin EOF (an interrupt, then exit 0 within 10 s, supervise §4.5).

`/panic` is different. The kernel sweeps the whole sandbox instance (supervise §3.8), and the body does not react.

**What `Abort::trigger` does, by phase:**

| Phase when the interrupt lands | Effect | Partial output kept |
|---|---|---|
| Model streaming | `shutdown(Both)` on the in-flight proxy connection. The blocked read returns `Ok(0)` (verified on macOS, provider.md §3.11), and the transport returns `Done(partial, Aborted)` | an assistant message with `stop_reason:"aborted"`: completed text, DeepSeek reasoning so far, **complete** tool calls. Incomplete tool calls are dropped. An Anthropic thinking block without its signature keeps its text for the log with `native: None`, so it is never replayed |
| Retry backoff | `Condvar` wakes `abort.sleep` | nothing to keep; `error{code:"aborted_in_retry"}` |
| Tool batch | every registered bash process group gets SIGKILL (`kill -s KILL -- -PGID`, tools.md §3.5.4); calls not yet started return `aborted` without running; `read`/`write`/`edit` (in-process, bounded, < 1 s) finish normally; a running `ext` call sees the flag through `CallCtx.abort` (host.md §3.10): the host sets the call's `cancel`, the guest traps at the next 10 ms epoch tick and its `process.run`/`read` stop, and the call returns `Aborted` within `ABORT_GRACE` (200 ms), or is abandoned like a timeout if it is stuck in a host call | every result, in call order: completed ones as they are, killed bash calls with partial output and `[aborted after N s; process group killed]`, aborted ext calls as `error: aborted: …` with the host's log tail |
| Boundary, pause wait, idle | flag only | — |

**After an abort:**
1. Each complete tool call of an aborted reply gets an error result: `error: aborted: not executed (owner abort)`. Every `tool_use` needs a matching result before the conversation can continue on any API.
2. The final boundary appends the results, queued steers and notices (steps 1–4), then syncs.
3. The loop emits `display{kind:"info", text:"aborted"}`, goes idle, and sends `state{busy:false}`. There is no automatic continuation, except through interrupt-and-steer.

**Interrupt-and-steer.** `interrupt{then:"continue"}` means the owner typed a note and wants it handled now. The kernel sends the note's `input` frame first. The input thread calls `Abort::trigger_continue()`, which records the wish and then triggers the same abort. A plain `trigger()`, a second Ctrl-C, the `stop` grace or EOF clears the wish, and so does `reset()`. When the stream abort, the abort in a retry wait, or the boundary abort check after a batch finds the wish *and* a steer to answer (one queued now, or one appended by this boundary or the last), the run continues. That boundary appends the aborted results (built exactly as in steps 1–2) and then the steer, calls `Abort::reset()`, and emits `display{kind:"info", text:"interrupted · continuing with your note"}` in place of `aborted`. The next model request follows. Follow-ups stay queued for after the run. Without a steer, or when its steer was retracted, the interrupt is a plain stop as above. An interrupt while paused stays a no-op, because the reset on resume clears the wish. An older body ignores `then` and simply stops, which is the documented safe fallback.

The abort is visible in the log through `stop_reason:"aborted"` and the `aborted` results. No separate event type is needed.

**Races:**
- A bash call that registers its group *after* the flag is set sees `register_group == false` and kills its own group at once.
- A model connection that registers after the flag gets `Err(Aborted)`.
- The flag is cleared only by `Abort::reset()`: at the start of the next run, on resume, or at the boundary where interrupt-and-steer continues the run.

### 3.6 Model turn, validation and stop reasons

**`model_turn`** drains the `StreamEvent` iterator:

| Event | Loop action |
|---|---|
| `Started{head_hash, …}` | `last_progress` = now; remember `head_hash` for log-context's head-hash comparison (`error{code:"head_hash_mismatch"}`) |
| `TextDelta` / `ThinkingDelta` | `out.try_send(display{kind:"text"\|"thinking"})` |
| `ToolCallReady` | nothing (never executed early) |
| `Usage` | kept for `turn_end` |
| `Warning{code, detail}` | `error{code}` event at the next append (for example `thinking_dropped`) |
| `Retry` | `display{kind:"info", text:"retrying (attempt n, <reason>) in <s> s"}`; the attach client clears the partial text |
| `Done(msg)` | return `Done(msg)` |
| `Failed(e)` | return `Failed(e)` |

**Per-call validation before the batch** (`Tools::prepare`, called for each call in call order; failures get a pre-filled result and **no** `tool_call` marker, because they never execute):
- Unknown tool name:
  ```
  error: unknown_tool: no tool named "grep". Available tools: bash, edit, ext, read, write.
  ```
- Arguments not a JSON object (`ToolCall::args_error()`):
  ```
  error: invalid_json: the arguments for "edit" are not valid JSON (<serde message>). Re-issue the call with valid JSON.
  ```
- Typed parsing fails: `error: invalid_input: <field>: <problem>` (tools.md §4.6).

**Stop-reason rules:**
- `EndTurn` / `StopSequence` / `ToolUse`: normal. Tool calls, if any, run.
- **Length stop** (`MaxTokens`, `ContextWindow`): the tool calls in the message are **not run**, complete or not. A cut-off reply may hold a call whose arguments were truncated mid-JSON but still parse (for example a `write` whose `content` was cut). Each call gets:
  ```
  error: length_cutoff: not executed. Your reply reached the output limit (max_tokens=65536) before it ended, so this call may be incomplete. Re-issue it; split large content (write a file in parts, or use edit).
  ```
  The run continues (pi-agent-core does the same).
  - The **next request only** doubles `max_tokens`, capped at `ModelInfo.max_output` (DeepSeek: 393,216). This is a request parameter, outside the head hash and outside any cached prefix.
  - A length stop with **no** tool calls also continues, so the model can finish its cut-off answer. The boundary appends harness text, `[init:cutoff] Your previous reply was cut off at the output limit (max_tokens=65536). Continue from where it stopped.`, as a user-side item. Without it the next request would end on an assistant turn, which current Anthropic models reject as a prefill (400). The text uses the `[init:…]` harness tag (awareness.md §3.1) and is logged as a `message` with `source:"harness"` (log-context §3.3, §4.2).
  - After 3 consecutive length stops the run ends with `error{code:"length_guard"}`, because the model is not converging. This guard is a scaffold, tagged `Compensates-for: models that loop on oversized single replies`.
- `Refusal`: no tools run. Complete calls get `error: refused: not executed (the provider stopped the reply: <category>)`. Then `error{code:"refusal"}`, `display{kind:"error"}`, and the run ends. The agent does not retry by itself; the refusal is the provider's decision.
- `PauseTurn`: re-send with no tool results. This happens only with server tools, which INIT does not use.
- `Aborted`: §3.5.

**Provider failures** (provider.md has already applied its retry policy):
- `ContextOverflow`: log-context's emergency compaction (`KEEP/2`, §3.12.6), then one more try in the same turn. A second overflow ends the run (`error{code:"context_overflow"}`).
- `Budget`: `error{code:"budget"}`, then `display{kind:"error", text:<the proxy's message>}`, and the run ends. This is how the kernel's budget governs (§3.10).
- Anything else: `error{code:"provider", kind, message, attempts, request_id}`, and the run ends. The owner can say "continue".

### 3.7 Parallel tool execution

**Plan** (on the loop thread, in call order):
1. `prepare` (§3.6), called for each call, gives `Ready` calls and pre-filled results.
2. `on_response` has already appended one `tool_call` marker for every `Ready` call and synced (D1). After a power loss, a tool that ran is never reported as "not started" (log-context §3.4.3).
3. Group the `Ready` calls into **units**:
   - all `write`/`edit` calls whose resolved path is the same form one unit, run sequentially in call order. Two concurrent edits of one file would both read the original, and the last rename would win (a lost update);
   - every other call is its own unit.
   - Path identity is lexical (tools.md §3.1). A symlink alias can defeat it; that case is accepted.
4. `ext.snapshot()` once per batch (host.md §4.1). If `ext.reload_pending()` and the batch has an `ext` call, call `ext.commit_reload(60 s)` first, so an `ext` call never sees a half-loaded registry (host.md §3.9).
5. Workers = `min(units, 16)`. The cap bounds threads per batch; real sessions average about 1.2 tool calls per step (reports/Ngôn ngữ cho INIT Agent.md, TraceLab).

**Execute:**

```rust
fn execute_batch(&mut self, calls: &Prepared, mut slots: Vec<Option<ToolResult>>) -> Vec<ToolResult> {
    let units: Vec<Vec<usize>> = plan_units(calls);                // Ready calls only
    let reg = self.ext.snapshot();
    let cursor = std::sync::atomic::AtomicUsize::new(0);
    let (tx, rx) = std::sync::mpsc::channel::<(usize, ToolResult)>();
    let (tools, abort, base) = (&self.tools, &*self.abort, &self.tool_cx);  // shared borrows for workers
    std::thread::scope(|sc| {                                       // stable since 1.63.0
        let n = units.len().min(MAX_PARALLEL);                       // MAX_PARALLEL = 16
        let mut spawned = 0;
        for w in 0..n {
            let (tx, units, cursor, reg) = (tx.clone(), &units, &cursor, &reg);
            let r = std::thread::Builder::new().name(format!("tool-{w}")).spawn_scoped(sc, move || loop {
                let u = cursor.fetch_add(1, Ordering::Relaxed);
                let Some(unit) = units.get(u) else { break };
                for &i in unit {
                    let r = if abort.is_set() { not_started_aborted(calls.call(i)) } else {
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tools.run(calls.ready(i), &base.for_call(i, reg))))
                            .unwrap_or_else(|p| panic_result(calls.call(i), p))   // "error: panic: …" (is_error)
                    };
                    let _ = tx.send((i, r));
                }
            });
            if r.is_ok() { spawned += 1; }
        }
        if spawned == 0 { /* run every unit inline on this thread (degraded, sequential) */ }
        drop(tx);
        for (i, r) in rx {                                           // ends when every worker has exited
            self.out.send(Out::ToolEnd { call: calls.call(i), ok: !r.is_error, summary: r.summary() });
            slots[i] = Some(r);
        }
    });
    slots.into_iter().map(Option::unwrap).collect()                 // call order; every slot filled
}
```

Tool-start frames (`display{kind:"tool_start"}`) are sent from the loop thread for every `Ready` call before the scope opens. The tools run concurrently, so "started" means "dispatched".

Properties:
- **Order.** Results return in call order and are appended at the next boundary, step 1 (log-context §4.3). A process crash in the middle of a batch leaves the markers without results. Boot's repair then writes `INTERRUPTED_TEXT` for them (log-context §3.6 step 9). The model is told "state unknown; verify before retrying".
- **Wall time.** A batch takes about as long as its slowest unit, not the sum.
- **Panic isolation.** `catch_unwind` (safe code) turns a tool panic into an error result (`error: panic: the <tool> tool crashed: <message>`, with no source location; the location is in the panic hook's stderr line), and the worker keeps going. Workers never panic out, so `thread::scope` never re-panics (it panics only if an auto-joined thread panicked). This requires `panic = "unwind"`, the Cargo default for the dev profile the body uses (boot.md §7).
- **Thread budget.** At most 16 workers, plus 2 helper threads per running bash call (tools.md §3.5), plus 1 host worker per running ext call (host.md §3.10) and 3–4 more while that extension runs `process.run` (tools.md §3.5.7). Each OS thread costs about 14 KB resident (reports/Ngôn ngữ cho INIT Agent.md); default stacks are 2 MiB of virtual memory.
- **Borrowing.** The workers capture only shared references. The loop thread keeps exclusive use of the log and `out` inside the scope, so Rust's split borrows compile without `Arc` or locks around the tools.

### 3.8 The loop's use of the turn boundary

The **turn boundary** is the point just before a model request, plus the idle state. Nothing is executing, every tool call has a result, and the log is consistent. It is the only place where the following happen.

#### 3.8.1 Pause and handoff (kernel/supervise.md §3.6.4, §4.5)

1. `pause{token, reason}` (reason `handoff`, `rollback` or, since v0.3, `swap`) arriving during a run is held in `pending_pause`. It takes effect at the next boundary, after steps 1–4 (so finished results, steers and notices are in the log). If the body is idle, it takes effect at once.
2. `pause(p)`:
   1. `(log_len, leaf) = ctx.pause()`: sync, `File::unlock` (log-context §4.3).
   2. `phase = Paused` (this emits `state{busy:false}` if the body was busy).
   3. Send `paused{token, log_len, leaf}`.
   4. Read only `ctl` until `resume`, `exit` or `stop`. The heartbeat keeps running with `busy:false` (boot.md §3.12). An `interrupt` arriving now is cleared on resume (a no-op).
3. `exit{token, code}` (75 after the kernel committed the handoff): `ext.shutdown()` (host.md: kills extension process sessions), then `serve()` returns the code, and boot flushes `out` and exits with it, within supervise's 10 s grace. The loop itself never calls `process::exit`. Nothing more is written to the log.
   - `stop{reason, grace_ms}` while paused (v0.3: the Root's `stop` or `prepare_swap`, kernel/supervise.md §3.8.3): the log is already synced and unlocked, so the body does the same with code 0 at once (`ext.shutdown()`, then `serve()` returns 0). It writes nothing more to the log and does not re-lock it.
4. `resume{token, update_failed}`: `ctx.resume(log_len, leaf, update_failed)` does the following (log-context §3.4.1):
   - re-locks the log, waiting up to 10 s;
   - rescans from `log_len`;
   - if `update_failed` is not null, appends `update_failed` with `parent_id = leaf`. That branch-back leaves the failed body's events on an abandoned branch.

   The loop then sets the phase back (a resume in the middle of a run emits `state{busy:true}`) and continues exactly where it paused: the run continues, or the body stays idle. The model reads the failure at its next request (A4).

#### 3.8.2 Reload (kernel `reload{job, kind, components}`)

The kernel sends `reload` with `kind:"request"` for `initctl reload`, and with `kind:"kept"` plus `components` after it published kept T1 components (supervise §3.6.8, §4.5).

1. The input thread calls `ext.begin_reload(…)` at once (host.md §3.9: compile off the hot path): `ReloadKind::Request` for `kind:"request"` (or a missing `kind`), `ReloadKind::Kept(components)` for `kind:"kept"`, so the kept artifacts win over older trials (host.md §3.9 rule 3).
2. At the boundary, or at once if idle, the loop calls `commit_reload(job)`:
   - `ext.commit_reload(10 s)` returns the `ReloadReport`, or `None` if the load is still running. In that case the report is picked up later with `commit_reload(Duration::ZERO)` at a following boundary, or polled every 100 ms while the body is idle. A report taken by the 60 s wait before a batch with an `ext` call (§3.7) is logged at the next boundary;
   - the T0 head rebuild (log-context §3.8; awareness.md) returns `head_hash_before` and `head_hash_after`. It runs only when boot supplies a `HeadSource`; otherwise the boundary has no T0 tier;
   - step 3 of the boundary appends one `reload` event per tier (tier, components, `prefix_changed = before ≠ after`, text).
3. Send `reload_done{job, ok, prefix_changed, components, error}` (supervise §4.5). It waits for the T1 report. The kernel uses it to answer `initctl reload` (spec §7.3: "reports whether the prefix changes").
4. In safe mode: no extension host and no reload. The reply is `reload_done{job, ok:false, error:"safe mode"}` (supervise refuses `reload` in safe mode anyway).

#### 3.8.3 Notices

`notice{id, kind, data}` (gate results, `update_failed`, `lkg`, `promoted`, …):
- If the body is idle, the notice is appended at once through a boundary with only step 4 and a sync. If busy, at the next boundary, step 4.
- `notice_ack{id}` follows the sync. Duplicates (same `id` already logged) are acked without appending.
- log-context maps kinds to event types and decides projection (§3.13). Notices never start a run on their own.

#### 3.8.4 Self-signals

Step 6 of every boundary that follows a tool batch appends one `signal` event (awareness.md §3.8, log-context §3.13). The loop supplies `&Status`. No signal follows a final answer.

### 3.9 Events the loop causes in the log

log-context.md owns every type and field (§3.3, §4.2). The loop triggers these:

| When | `type` | Through |
|---|---|---|
| owner input that starts a run, or a delivered follow-up | `message` (role user, `source:"owner"`, `input_id`) | `boundary_append` |
| steer delivered at a boundary | `steer` | `boundary_append` |
| model reply complete, or an aborted partial | `message` (role assistant, blocks, `provider`, `model`, `stop_reason`, `usage`, `request`) | `on_response` |
| a `Ready` call is about to execute | `tool_call` marker `{call_id, name, message_id}` | `on_response` (D1) |
| results of a batch, in call order | `tool_result` (+ `spill_path`, `exit_code`, `wall_ms`) | `boundary_append` |
| reload committed | `reload` | `boundary_append` |
| kernel notice | `gate` / `update_failed` / `thinning_done` / `notice` | `boundary_append` |
| after each tool batch | `signal` | `next_request` (step 6) |
| resume after a failed handoff | `update_failed` (branch-back) | `ctx.resume` |
| a run ends on a provider error, budget, refusal or guard; a provider warning; a render failure | `error` (`code`: `provider`, `budget`, `refusal`, `length_guard`, `context_overflow`, `aborted_in_retry`, `thinking_dropped`, `head_hash_mismatch`, `render`). `where` is `provider` for the provider-side codes, `loop` for `length_guard` and `context` for `render` | `boundary_append` |
| the cut-off note | `message` (role user, `source:"harness"`) | `boundary_append` |

Not caused by the loop: `boot` (boot.md and log-context rehydration), `compaction` and `mask` (log-context), `self_edit` and `gate` from the agent's own `initctl` (evolution.md).

### 3.10 No turn cap

The loop counts turns (for the heartbeat and self-signals) but never stops because of a count. A run ends only when one of these happens:
- the model stops calling tools and no steer or follow-up is queued;
- the owner interrupts or stops the body;
- the provider refuses;
- a provider error survives the retry policy. That includes the kernel proxy's **budget** 429, which is how spend is governed (spec §7.2; invariant 3, enforced outside the body);
- a no-progress guard fires: 3 consecutive length stops, or a second context overflow in one turn.

## 4. Interfaces

### 4.1 Frames consumed and produced (kernel/supervise.md §4.5, `INIT_UI=stdio-jsonl/1`)

One JSON object per line, `"v":1`, key `t`. Unknown `t` values and unknown fields are ignored. The input thread parses kernel frames into `Control`:

| Kernel → body | Loop behavior |
|---|---|
| `input{id, seq, text, ts, mode}` | §3.4 (`mode`: `steer` default, `follow_up`). `ack{seq}` after the appending boundary syncs. `id` is also the task id of the run it starts (§3.3) |
| `interrupt{then?}` | the input thread calls `Abort::trigger()`, or `Abort::trigger_continue()` for `then:"continue"` (interrupt-and-steer; any other `then` is plain), then `Control::Interrupt` (§3.5) |
| `retract{seq}` | answered by the input thread at once with `retracted{seq, ok}` (§3.4); never reaches the loop |
| `pause{token, reason}` | §3.8.1 |
| `resume{token, update_failed}` | §3.8.1, only while paused; otherwise a protocol warning on stderr |
| `exit{token, code}` | §3.8.1, only while paused |
| `stop{reason, grace_ms}` | set `stopping`. If busy, the current turn may run until its next boundary for up to `grace_ms` (default 30,000), then an interrupt. Then sync, `ext.shutdown()`, exit 0. While paused: §3.8.1 (exit 0 at once, no log write) |
| `reload{job, kind, components}` | §3.8.2 |
| `notice{id, kind, data}` | §3.8.3 |
| EOF on stdin | interrupt, then sync, `ext.shutdown()`, exit 0 within 10 s |

| Body → kernel | When |
|---|---|
| `display{kind:"text"\|"thinking", text}` | stream deltas (dropped if `out` is full) |
| `display{kind:"tool_start", call_id, name, text}` | before the batch; `text` is a one-line summary: the bash command (≤ 200 chars), `read <path>`, `edit <path> (n edits)`, `ext <name>` |
| `display{kind:"tool_end", call_id, name, ok, text}` | as each call finishes; `text` like `exit 0, 96 lines`, `112 lines`, `2 replacements`, `error: no_match` |
| `display{kind:"turn_end", text}` | after each model reply; `text` = usage summary + dropped-delta count |
| `display{kind:"info"\|"error", text}` | retries, abort, run-ending errors, safe-mode refusals |
| `ack{seq}` · `notice_ack{id}` | after the appending boundary's sync |
| `retracted{seq, ok}` | at once, in reply to `retract{seq}` (§3.4) |
| `state{busy}` | on every busy↔idle change. This is the kernel's "next idle point" (supervise §3.6.7) |
| `paused{token, log_len, leaf}` | §3.8.1 |
| `reload_done{job, ok, prefix_changed, components, error}` | §3.8.2 |

stderr carries only diagnostics: boot errors, warnings, panic lines (boot.md §4.5). The kernel writes it to `state/kernel/logs/`, never to the owner.

### 4.2 Interfaces the loop uses from other docs

```rust
// src/model.rs (provider.md §3.3, adapted): byte-based, because log-context already renders the wire body
pub struct Request<'a> {
    pub body: &'a [u8],                    // sent unchanged, with the API's path and headers
    pub head_hash: &'a str,
    pub messages: &'a [Msg],               // the projection the body was rendered from (adapters, scripted test models)
    pub max_tokens: u32, pub effort: Effort, pub purpose: Purpose,
    pub session: &'a str,                  // x-init-session
    pub task: Option<&'a str>,             // x-init-task (§3.3)
}
// body/log-context.md §4.3 (src/loop/hooks.rs; LogContext wraps log-context's Context)
pub struct Timing { pub wall_ms: Option<u64>, pub ttft_ms: Option<u64> }       // logged in message.request
trait ContextHooks {
    fn on_response(&mut self, msg: &AssistantMessage, started: &[CallStart], t: Timing) -> io::Result<()>;   // message + markers, sync D1
    fn boundary_append(&mut self, input: BoundaryInput) -> io::Result<BoundaryOut>;   // steps 1–4, sync; BoundaryOut = duplicates, command outcomes, new session, prefix_changed per reload tier
    fn next_request(&mut self, m: &ModelInfo, max_tokens: u32) -> Result<RequestOut, ContextError>;   // steps 5–8; RequestOut = body, head_hash, task_id, messages, …; fills the task (§3.3)
    fn pause(&mut self) -> io::Result<(u64, String)>;                                           // sync, unlock → (log_len, leaf)
    fn resume(&mut self, log_len: u64, leaf: &str, failure: Option<serde_json::Value>) -> io::Result<()>;
    fn has_pending_work(&self) -> bool;          // leaf = owner message, steer, or complete result set
    fn request_emergency_compaction(&mut self);  // §3.12.6
    fn leaf(&self) -> String;
    fn has_input(&self, id: &str) -> bool;       // duplicate classification (§3.4)
    fn session_id(&self) -> String;              // changes on /new
}
// body/tools.md §3.1 (src/tools/mod.rs)
pub enum Prepared {
    Ready { idx: usize, call_id: String, name: String, job: Job },
    Done  { idx: usize, call_id: String, name: String, result: ToolResult },   // validation failed: pre-filled result
}
pub struct Ran { pub result: ToolResult, pub summary: String }                // summary = the tool_end text (§4.1)
// Tools::prepare(&self, idx: usize, c: &ToolCall, cx: &PathCx) -> Prepared;  Tools::run(&self, p: &Prepared, cx: &ToolCx) -> Ran
pub struct PathCx<'a> { pub cwd: &'a Path, pub home: Option<&'a Path>, pub init_home: Option<&'a Path> }
pub struct ToolCx<'a> { pub paths: PathCx<'a>, pub spill_dir: &'a Path, pub call_id: &'a str, pub abort: &'a Abort,
                        pub images: bool, pub model_id: &'a str, pub mode: Mode, pub reg: Option<&'a Snapshot> }
// ext/host.md §4.1, as the trait the loop and tools use (src/tools/ext.rs); the extension host implements it
pub enum ReloadKind { Boot, Request, SessionStart, Kept(Vec<String>) }
pub struct CallCtx<'a> { pub call_id: &'a str, pub workspace: &'a Path, pub abort: &'a AtomicBool }   // abort = Abort::flag()
pub type Snapshot = Arc<dyn Any + Send + Sync>;   // host.md's Arc<Registry>, type-erased: no Wasmtime types reach the loop or tools
pub trait ExtHostApi: Send + Sync {
    fn begin_reload(&self, kind: ReloadKind);  fn commit_reload(&self, max_wait: Duration) -> Option<ReloadReport>;
    fn reload_pending(&self) -> bool;          fn reset_trials(&self);
    fn snapshot(&self) -> Snapshot;            fn call(&self, reg: &Snapshot, name: &str, input: &Value, cx: &CallCtx) -> ExtOutcome;
    fn shutdown(&self);
}
```

`reset_trials()` is called when a new session starts: a cold boot with a newly created session, or `/new` (host.md §3.9 item 5).

## 5. Failure modes and recovery

| Failure | Effect | Recovery |
|---|---|---|
| Body crash mid-stream | the partial reply is lost (never logged) | restart; the leaf is still pending, so auto-continue repeats the request, which is cheap because it hits the cache |
| Body crash mid-batch | markers without results | boot repair: `INTERRUPTED_TEXT` results (log-context §3.6); auto-continue; the model verifies before redoing side effects |
| Power loss mid-batch | the same, since the markers were synced at D1 | same |
| A tool hangs past its timeout | bash kills its group (tools.md); ext uses its epoch deadline and watchdog (host.md) | result with `timeout` |
| A tool panics | `catch_unwind` | `error: panic: …` result; the loop goes on |
| Worker spawn fails (EAGAIN) | fewer workers | units run on the remaining workers, or inline if none spawned |
| The kernel stops reading stdout | `out` fills | deltas are dropped and counted; protocol frames block the sender; the kernel watchdog sees the heartbeat (supervise §3.5) |
| stdin EOF (the kernel died) | interrupt, exit 0 | supervise's cold start sweeps leftovers and restarts the body |
| A pause is never answered with resume or exit | the body stays paused | supervise's `pause_timeout_s` (1,800 s) decides. The body never resumes on its own, because that could create two writers |
| `resume` while the new body still holds the lock | `ctx.resume` fails after 10 s | exit 1 (crash). supervise sweeps the new body *before* `resume` (§3.6.4 abort path step 1), so this indicates a kernel bug |
| Provider budget reached | the run ends | the owner raises the cap or waits; no retry loop burns calls |
| Repeated length stops | the guard ends the run after 3 | the owner can steer ("write it in parts") |

## 6. Performance notes (spec §9)

- **Rank 1 (cache).**
  - The loop only appends: steers, notes, harness text and results go at the tail.
  - Validation errors and length cut-offs become tool results, never edits of the assistant message.
  - The `max_tokens` boost after a length stop is a request parameter, outside the head hash and outside the cached prefix.
- **Rank 2 (round-trips).**
  - Parallel tools: a batch costs as long as its slowest unit.
  - Steers ride the next request instead of forcing another.
  - No model call on normal boots (boot.md §3.10).
- **Rank 3 (tool path).**
  - Workers are spawned per batch, about 20–50 µs each, which is noise against even a 1 ms `read`. A persistent pool is a growth option if a measurement shows a gain.
  - D1 adds one `sync_data` per tool batch, about 10–30 ms on Apple SSDs (log-context §3.4.3, measured in Phase 0 T-LOG-9). It is accepted for the "never misreport a tool that ran" guarantee.
- **Rank 5 (footprint).**
  - Idle body: 4 threads plus the extension host's ticker and loader.
  - `out` is bounded at 1,024 frames.
  - Batch threads exit after every batch, and tool outputs held in memory are capped (tools.md).
  - Measured by testing.md M-5.
- **Rank 7 (CPU).** The loop's own work per turn is O(new events). Request building cost is log-context's.

## 7. Invariants and security (spec §2.1)

- **Invariant 4 (lineage).**
  - Every model reply becomes one `message` event, synced (D1) before any of its tool calls runs.
  - Every tool call gets exactly one `tool_result`, in call order.
  - The log is append-only, and nothing is written to it while paused.
- **Invariant 1 (recoverability).**
  - A pause releases the lock only after a sync, so the next body sees a complete log.
  - A failed swap is resumed by branch-back, never retried by the body.
- **Invariant 3 (blast radius).**
  - Interrupts and timeouts kill whole process groups.
  - The budget is enforced by the proxy, and the loop has no path around a `Budget` error.
- **Invariant 5.** The loop takes no external action itself.
- **Single writer.** Only the loop thread writes the log. Workers return results over a channel, so no lock around the writer is needed.

## 8. Test plan

All tests use a scripted `ModelClient` or `init-mockllm` (provider.md §3.12; testing.md §3.4). Spend: $0.

1. A turn with no tools: events `message(user)`, `message(assistant, end_turn)`; frames `state{busy:true}` … `state{busy:false}`.
2. A batch of 3 bash calls sleeping 0.3 s, 0.1 s and 0.2 s: the results are in call order; batch wall time < 0.45 s; all three `tool_call` markers come before any result, and the D1 sync happens before the first spawn (checked with a fault point that crashes right after `on_response`).
3. Two `edit` calls on the same file in one batch: both applied, in order (one unit). A `read` of another file in the same batch runs concurrently.
4. A tool that panics: `error: panic: …` result; the next turn proceeds; no process abort.
5. A steer during a 1 s bash: appended after its result, before the next request (mock `expect.last_role:"user"`). `ack{seq}` comes only after the boundary sync.
6. A steer while the final reply streams: the run continues for one more turn.
7. Two follow-ups (`mode:"follow_up"`): delivered one per run end, in order.
8. **Interrupt mid-stream** (mock `stall` fault): a partial assistant message with `stop_reason:"aborted"`; complete calls get `aborted` results; incomplete calls are dropped; handled in < 100 ms; the next input starts a run normally.
9. **Interrupt during `bash sleep 100`** plus a concurrent `read`: the bash group is dead within 200 ms (`pgrep -g`); the `read` result is intact; both results are in call order.
10. A length stop with 2 calls: two `length_cutoff` results; the next request's `max_tokens` is doubled; after 3 strikes, `error{code:"length_guard"}` and idle. A text-only length stop gets the `[init:cutoff]` note, and the next request does not end on an assistant turn.
11. Unknown tool and invalid JSON: pre-filled error results, no markers; the model gets the next turn.
12. **Pause while idle:** `paused{token, log_len, leaf}` within 50 ms; another process can `try_lock` the log; heartbeats continue with `busy:false`. Then `resume` with `update_failed`: an `update_failed` event whose `parent_id` = `leaf`; the run continues. Then pause and `exit{code:75}`: the process exits 75 and `ext.shutdown()` ran.
13. **Pause during a batch:** `paused` comes only after the batch's results and queued steers are appended.
14. Reload at a boundary: a `reload` event; `prefix_changed` matches the head-hash comparison; `reload_done`. Reload while idle: immediate. `reload{kind:"kept", components:["pty"]}` reaches the host as `begin_reload(Kept(["pty"]))` (fake host records it); `kind:"request"` and a missing `kind` as `Request`.
15. Notice while idle and while busy: one event each, `notice_ack` after the sync; a duplicate `id` is acked without a second event.
16. A duplicate `input` after a simulated restart: acked, not appended twice.
17. Budget error: no retry; `error{code:"budget"}`; idle.
18. Context overflow once: emergency compaction, then the turn succeeds. Twice: `error{code:"context_overflow"}`.
19. Crash repair: `kill -9` during a batch with one finished and one running call; restart; both get `INTERRUPTED_TEXT`; auto-continue (A3/A4 building block).
20. `stop{grace_ms:2000}` during a 10 s bash: the call is interrupted at 2 s; exit 0. stdin EOF: tool groups killed, exit 0 within 10 s.
21. Footprint: Phase 0 measurement 5 is testing.md's M-5 (slope ≤ 2 KiB/step, max ≤ 1.25 × RSS at step 100; testing Q13 adopted).
22. **Task id:** an input `id:"9c41e07a2b5d13f0"` starts a run of 3 turns with one steer in the middle: all 3 mock recordings carry `x-init-task: 9c41e07a2b5d13f0`, and so does a compaction request inside the run. A delivered follow-up switches the header to the follow-up's id. After `kill -9` and restart with pending work, the continuing request carries the same id as before the crash. A fresh `/new` session's first request after a `/compact` with no owner message sends no header.
23. **`mode`:** `follow_up` while idle starts a run at once; while busy it waits for the run end (item 7); `mode:"steer"`, a missing `mode` and `mode:"bogus"` all steer.
24. **Interrupt during an ext call** (host test component that spins): the result is `aborted` within 250 ms of the interrupt, the other calls of the batch keep call order, and the next input starts a run normally.

Scenario IDs from testing.md (roadmap.md Q2):
- A1 and E2E-2/E2E-4: the real DeepSeek flow (items 1–2 live).
- A3 and A3-real: continue after a handoff (items 12, 19, 22, and boot.md).
- A4 and F-crash-boot (a, b): a failed swap is a non-event (item 12, resume path).
- A5: safe mode has no auto-continue (boot.md).
- F-hang (c): item 8 with the mock `stall` fault.
- F-kill-kernel: item 20 (stdin EOF) and the "no lost valid events" invariant (items 2, 19).
- M-5: item 21.

## 9. Open questions

Resolved in R1/R2 and removed: Q-L1 (a) `input.mode` with `/later` (supervise §4.5, §4.6), (b) `promote` returns before `pause` (initctl §4.1), (c) cwd = `$INIT_HOME/work` (supervise §3.4); Q-L2 (boundary split, harness messages, `error` codes: log-context §3.3, §4.2, §4.3); Q-L3 (`CallCtx.abort`, host.md §3.10); Q-L4 (settle and staleness without A's heartbeat, supervise §3.5; `setsid` escapees swept by supervise §3.8.2).

**Cross-doc**
- **Q-L7 → body/log-context.md.** `next_request` fills `Request.task` by §3.3's rule (the `input_id` of the latest owner `message` on the current path) for every request it builds, including compaction requests, and records it as `message.request.task_id` (its §4.2 field already exists). The rule is a pure function of the path, so it needs no new state and survives restarts. Until log-context states it, the loop sets `req.task` itself on turn requests from the input that started the run, and compaction requests go out as `_none`.

**Owner decisions** (not in CONVENTIONS §6; the stated default ships unless the owner objects; reconciliation/body-core.md R2 asks the main session to list them)
- **Q-L5.** *Settled 2026-10-19, keep as proposed (CONVENTIONS §6).* While a run is in progress, a plain owner message acts as a **steer** (Claude Code's behavior) rather than a follow-up; `/later` queues a follow-up. OK?
- **Q-L6.** *Settled 2026-10-19, keep as proposed (CONVENTIONS §6).* A length stop with **no** tool calls auto-continues, up to 3 strikes. OK, or should the run end?

## 10. Spec deltas

1. **Spec §4 Loop: "stops when the model makes no tool call".** Refined: the run also continues for a queued steer or follow-up (Pi semantics), and for a length stop, up to 3 strikes.
2. **Spec §4: "when stop_reason=length, returns an error to every pending call".** Kept, plus a doubled `max_tokens` for the next request, a cut-off note for text-only stops, and the 3-strike guard.
3. **Same-path `write`/`edit` calls are serialized within a batch.** The spec says only "in parallel". Why: lost updates.
4. **Spec §4: "drains the steering queue after each tool batch".** Kept, inside log-context's fixed boundary order, which also places reloads, notices, the pause check and signals.
5. **No turn cap**, as the brief requires: the kernel budget governs. Only no-progress guards end a run on their own.

**Build sync 2026-10-20** (build → docs; ledger reconciliation/build-deviations.md §1.14):

| Build ref | This doc now says | Section |
|---|---|---|
| build: WP10 D1 | `Abort::new` takes a `kill_group: fn(i32)` hook (boot passes `tools::kill_group`), so `Abort` does not depend on `tools` | §3.1 |
| build: WP10 D6 | an aborted Anthropic thinking block without a signature keeps its text with `native: None` | §3.5 |
| build: WP11 D1 | `model::Request` is byte-based: it carries the wire body that log-context already rendered, and the provider sends it unchanged | §3.2, §4.2 |
| build: WP11 D2 | `ContextHooks`: `boundary_append` returns `BoundaryOut`, `on_response` takes `Timing`, `next_request` returns `RequestOut`, and `has_input` and `session_id` are added; `BoundaryInput.results` is `(name, ToolResult)`, `reload` is one `ReloadIn` per tier, and `errors` is added | §3.3, §4.2 |
| build: WP11 D3 | `serve()` returns the exit code (`u8`) and `RunEnd` gains `Exit(u8)`; the loop never calls `process::exit`, and boot flushes `out` and exits | §3.3, §3.8.1 |
| build: WP11 D5, D6 | the `Prepared` (with `idx`, `call_id`, `name`), `Ran`, `PathCx` and `ToolCx` shapes; `Tools::prepare` is called once per call | §3.3, §3.6, §3.7, §4.2 |
| build: WP11 D7 | `ExtHostApi` is a trait with a type-erased `Snapshot` | §4.2 |
| build: WP11 D12 | a pause emits `state{busy:false}`, a resume in the middle of a run emits `state{busy:true}`, and an interrupt that arrived while paused is cleared on resume | §3.8.1 |
| build: WP11 D13 | queued follow-ups are appended as owner messages at abort, failed and pause boundaries; a steer left at idle starts a run | §3.4 |
| build: WP11 D14 | T0 runs only when boot supplies a `HeadSource`; `reload_done` waits for the T1 report (polled every 100 ms while idle); a report taken by the 60 s wait before an `ext` batch is logged at the next boundary | §3.8.2 |
| build: WP11 D15 | the `error` `where` values: `provider`, `loop` (`length_guard`), `context` (`render`) | §3.9 |
| build: WP11 D16 | a panic result carries no source location; the location is in the panic hook's stderr line | §3.7 |

## 11. References (checked 2026-10-19)

- Rust std `thread::scope` (stable since 1.63.0; panics if an auto-joined thread panicked): https://doc.rust-lang.org/stable/std/thread/fn.scope.html
- Rust std `thread::Builder::spawn_scoped`: https://doc.rust-lang.org/stable/std/thread/struct.Builder.html
- Rust std `mpsc` (`channel`, `sync_channel`, `try_send`, `Receiver::recv_timeout` since 1.12.0): https://doc.rust-lang.org/stable/std/sync/mpsc/index.html
- Rust std `panic::catch_unwind`: https://doc.rust-lang.org/stable/std/panic/fn.catch_unwind.html
- Rust std `File::lock`/`try_lock`/`unlock` (stable since 1.89.0): https://doc.rust-lang.org/stable/std/fs/struct.File.html
- Rust std `UnixStream::shutdown`: https://doc.rust-lang.org/stable/std/os/unix/net/struct.UnixStream.html
- Wasmtime `Store::epoch_deadline_callback` (the abort path host.md §3.11 adopted; verified there for v48.0.6): https://docs.rs/wasmtime/latest/wasmtime/struct.Store.html
- Pi agent-core loop (`while hasMoreToolCalls || pending`, steer/follow-up, parallel with ordered results, length → error per call): https://github.com/earendil-works/pi/blob/main/packages/agent/src/agent-loop.ts and README (via research_notes/minimal_harnesses.md Q1)
- Claude Code: queued messages are read after the current tool calls finish; Esc cancels the running tool: https://code.claude.com/docs/en/how-claude-code-works (via research_notes/production_harness_internals.md §1)
- reports/Ngôn ngữ cho INIT Agent.md (≈1.2 tool calls per step, TraceLab; ≈14 KB per OS thread)
- kernel/supervise.md §3.6.4, §3.10, §4.5; body/log-context.md §3.4, §3.6, §3.13, §4.3; ext/host.md §3.9, §3.10, §4.1; testing.md §3.4, M-5 (sibling docs, read 2026-10-19)
- Local verification, macOS 14.6, rustc 1.97.1, 2026-10-19: shutdown unblocks a read; `File::try_lock` on a second handle returns `WouldBlock`; process-group kill.
