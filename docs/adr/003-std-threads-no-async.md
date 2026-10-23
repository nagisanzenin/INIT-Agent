# ADR-003: Std threads, no async runtime, in the body

```
Status: draft v0.3 (reconciliation round 2) · 2026-10-19 · Area: body · Decision: proposed (records CONVENTIONS §5 and spec v0.2 §4; final after Phase 0 M-5)
Depends on: spec v0.2 §4, §9; CONVENTIONS.md §5; reports/Ngôn ngữ cho INIT Agent.md; body/provider.md; body/loop.md; body/tools.md; ext/host.md
Defines:    the body's concurrency model (OS threads, std channels, blocking I/O with timeouts, OS-level cancellation);
            the conditions that would reopen it
```

## Context

The body has to do a few things at once:
- stream one model response from the kernel proxy (HTTP/1.1 + SSE over a Unix socket) while staying interruptible;
- run the tool calls of one model reply in parallel, keeping results in call order;
- wait on child processes with timeouts, and kill whole process trees on timeout or abort;
- run extension calls that can block in host functions (ext/host.md);
- read kernel frames from stdin and write frames to stdout while the loop is busy;
- write a heartbeat every 5 s.

**Forces:**

| Force | Evidence | Consequence for the choice |
|---|---|---|
| Harness CPU is not the bottleneck | Harness CPU is at most about 9% of agent time, so a free harness gives at most 1.10× (spec §9). An async runtime at seed is listed as "kept out on purpose" (spec §9) | Throughput of the runtime is irrelevant |
| Concurrency is tiny | About 1.2 tool calls per step (TraceLab, via reports/Ngôn ngữ cho INIT Agent.md). One model stream at a time per process. Phase 3 parallel explorers are separate processes | Tens of threads at most, never thousands |
| Cost of a thread | About 14 KB resident per OS thread, against about 0.3 KB per idle tokio task (same report). Default Rust thread stacks are 2 MiB of virtual memory | 16 tool workers cost about 224 KB resident: negligible |
| The body must stay small and fully readable | Body budget about 20–25k tokens (spec §0, §4). Smallness is part of awareness (spec §5) | Every dependency and abstraction layer is paid for in tokens the agent must read |
| The model edits this code with no human reviewer | Rust is the strongest language for LLM editing (spec §8), but async Rust adds `Send`/`'static` bounds across `.await`, `Pin`, cancellation-safety rules and executor choice. These are exactly the places LLM edits go wrong, and the compiler errors are long | Fewer concepts means fewer failed self-edits at G0 |
| Dependencies are fixed | Body dependencies are `serde`, `serde_json`, `wasmtime` and `wasmtime-wasi`, all vendored (CONVENTIONS §5). A `Cargo.lock` or `vendor/` change forces full G4 (spec §6.3) | tokio, smol, mio or rayon as a *direct* dependency would each break the rule (tokio and rayon already arrive transitively through `wasmtime-wasi` and Wasmtime's `parallel-compilation`, as internals; see Consequences) |
| Build time | A release binary whose only dependency was tokio took 8.1 s to clean-build on the M2 (reports/Ngôn ngữ cho INIT Agent.md). Leaving tokio out removes the largest clean-build cost measured | The self-edit loop stays fast (spec §9 rank 4) |
| Precedent | Bun's Zig-to-Rust port, done by agents, shipped with no async Rust (DevClass, via the same report) | A large agent-maintained Rust codebase works without async |
| No `unsafe` in the body | `#![forbid(unsafe_code)]` (spec §4) | No raw `poll`/`kqueue`/`epoll`, no `libc::killpg`; everything must go through safe std or safe dependencies |

The open question is cancellation: how to stop a blocking read, a running child process tree, or an extension, without `select!` and without `unsafe`.

## Decision

The body uses **OS threads from `std`, `std::sync::mpsc` channels, blocking I/O with timeouts, and OS-level cancellation**. There is **no async code in the body** (CONVENTIONS §5 v0.2): no futures, no `async fn`, and no runtime that body code starts or drives. The tokio runtime inside `wasmtime-wasi` and Wasmtime's rayon compile pool are internals of allowed dependencies (Consequences). (The kernel's proxy may use tokio; the body never links kernel code.)

**Primitives** (all stable std, verified against doc.rust-lang.org on 2026-10-19):

| Need | Primitive | Since |
|---|---|---|
| Parallel tool calls, results in order | `std::thread::scope` + `Builder::spawn_scoped`, workers send `(index, result)` over `mpsc::channel`; `catch_unwind` per call | 1.63.0 |
| Waiting with a deadline | `mpsc::Receiver::recv_timeout`; `Condvar::wait_timeout` for interruptible backoff sleeps | 1.12.0 |
| Stream idle timeouts | `UnixStream::set_read_timeout` / `set_write_timeout` (a timed-out read returns `WouldBlock` or `TimedOut`) | 1.10.0 |
| Cancel a blocking socket read | another thread calls `shutdown(Shutdown::Both)` on a `try_clone` of the stream; the blocked `read` returns `Ok(0)` | 1.10.0 |
| One process group per command | `CommandExt::process_group(0)` | 1.64.0 |
| Kill a process tree | spawn `/bin/kill -s KILL -- -<pgid>` (tools.md §3.5.4) | — |
| Merged stdout+stderr with one reader thread | `std::io::pipe()` | 1.87.0 |
| Single writer on the session log | `File::try_lock` / `unlock` | 1.89.0 |
| Bounded output queue | `mpsc::sync_channel(1024)` + `try_send` for droppable display deltas | 1.0.0 |
| Stop running Wasm | Wasmtime epoch interruption on a ticker thread (ext/host.md §3.11) | wasmtime |

**Thread inventory** of a running body:
- **Long-lived:** main (the loop), input, output, heartbeat, plus the extension host's ticker and loader.
- **Per batch:** up to 16 tool workers.
- **Per running bash call:** 2 (reader and waiter).
- **Per running ext call:** 1 host worker, plus 3 (two readers, waiter) or 4 (with a stdin writer) during an extension's `process.run` (tools.md §3.5.7).
- **Dependency internals, lazily:** `wasmtime-wasi`'s tokio worker threads on the first async-shimmed WASI call, and Wasmtime's rayon pool while compiling.

**Verified locally** on macOS 14.6, rustc 1.97.1, 2026-10-19:
- `shutdown` on a cloned `UnixStream` returned `Ok(0)` to a reader blocked in another thread after 200 ms;
- `process_group(0)` plus `/bin/kill -s KILL -- -PGID` emptied a 3-member group;
- one `kill` spawn costs p50 1.44 ms;
- `std::io::pipe` merged stdout and stderr in order;
- `File::try_lock` on a second handle returned `WouldBlock`.

## Alternatives considered

| Alternative | Why not |
|---|---|
| **tokio** (multi-thread runtime) with an async HTTP stack | Adds a large dependency tree, against the fixed dependency rule; clean-build cost (8.1 s for tokio alone); and async complexity in code the model must edit. Its real advantages are `select!`, cancellation by dropping a future, and scaling to thousands of tasks, and the body needs none of them. Cancellation-by-drop also does not reach the cases that matter here (a child process tree, a Wasm call blocked in the host), which need OS-level action anyway |
| **smol / async-io / futures-lite** (small executors) | Fewer dependencies than tokio, but still new crates, and all of async Rust's conceptual load with less documentation for models to draw on |
| **A mio-based event loop** (one thread, readiness-driven) | A dependency (mio), or `unsafe` `poll`/`kqueue` calls; neither is allowed. A hand-written state machine per stream and per child is harder to read and edit than straight-line blocking code |
| **rayon** for parallel tools | It is in the dependency graph already (Wasmtime's compile pool), but using it directly would add a direct dependency and share the compile pool with tool calls. Its work-stealing pool helps CPU-bound work, while tool calls mostly wait on processes and files. `thread::scope` gives the same ordered fan-out in a dozen lines |
| **Non-blocking sockets plus a polling loop** (`set_nonblocking` + sleep) | Busy-waiting trades latency against wasted CPU, and still needs a thread per child-process wait, because std has no non-blocking wait for children |
| **`std::process::Child::kill`** for tree cancellation | Signals only the leader pid. `bash -c 'make test'` would leave `make`'s children running |
| **A safe dependency for `killpg`** (`rustix::process::kill_process_group`) | Microsecond kills, and `rustix` is already vendored under wasmtime. But it would be a new direct dependency, against the fixed list, and the kill path is cold. Kept as a growth option (tools.md §3.5.4) |

## Consequences

**Positive**
- **Simple, linear code.** The loop, the provider and the tools read top to bottom: the request is sent, the stream is read, the tools are run. Panic backtraces and debugger stacks show real call chains, not executor frames.
- **Within the dependency rule.** The body crate needs nothing beyond the fixed dependency list.
- **Cancellation works on everything, not only on cooperative code.** An owner interrupt shuts the socket down, kills process groups, and (with host.md) traps Wasm. None of these need the cancelled code to check a token.
- **Isolated failures.** A tool panic is caught per call, and a stuck host call is abandoned by host.md's watchdog without blocking the batch.
- **Footprint is predictable** and stays far below the 76 MiB-per-process figure of Node harnesses (spec §6.8).

**Negative, accepted**
- **Dependencies run their own thread pools.** `wasmtime-wasi`, one of the fixed dependencies, has an unconditional `tokio` dependency and builds a multi-thread runtime lazily: every async-implemented WASI host call (filesystem, streams, poll) runs through its `in_tokio` shim, which does `RUNTIME.block_on(…)` (ext/host.md §3.2, verified in v48.0.6; spec delta 10.3). `allow_blocking_current_thread(true)` removes only the extra `spawn_blocking` hop, so filesystem calls run on the calling thread inside that `block_on`; it does **not** avoid the runtime. Separately, Wasmtime's `parallel-compilation` feature brings `rayon`, a CPU thread pool used only while Cranelift compiles a component (reload, boot load). Both are internals of the extension host: no body code is `async`, and nothing in the loop, provider or tools awaits on the runtime or submits work to the pool. CONVENTIONS §5 (v0.2) words the rule exactly this way: "no async code in the body", with the tokio runtime and the rayon pool accepted as internals of allowed dependencies. This ADR's decision is about the body's own concurrency model.
- **One thread per blocking operation.** About 14 KB resident each. The 16-worker cap and per-call helpers bound the count; Phase 0 M-5 measures RSS over 1,000 steps.
- **No `select!`.** Waiting on "child exited OR timeout OR abort" takes a helper thread plus `recv_timeout`, and abort acts out-of-band, by killing the process group. Where a plain flag is the only signal (an extension's `process.run` under an owner abort), the wait is cut into 10 ms slices that read the flag (tools.md §3.5.7).
- **Timeouts are per-syscall (idle), not total.** A total deadline needs a watchdog thread, as in the probe's 60 s and host.md's call watchdog.
- **A blocked file read cannot be cancelled.** File tools are bounded (caps in tools.md §4.5) and local, so they finish in milliseconds. NFS-like hangs are out of scope.
- **The tree-kill path spawns a process** (about 1.4 ms), acceptable because it runs only on timeout or abort.
- **One model stream per process.** That is fine for one agent loop. Speculative or programmatic tool calling with many concurrent model calls in one process (growth patterns, spec §4) would need either more threads (still fine at tens) or a revisit.

## Conditions to reopen

The body language and its runtime are themselves T2 choices: the agent may change them through the gate if it measures a gain (spec §8, §9). Concrete triggers:
1. A growth step needs **many** concurrent streams in one process, for example a PTY multiplexer with dozens of interactive sessions, or tens of concurrent model calls. Thread count or RSS (M-5) then exceeds the footprint budget.
2. Phase 0 or later measurements show harness overhead from threads above 1% of turn time. This is not expected: thread spawns cost about 20–50 µs per batch.
3. The fixed dependency list is relaxed by the owner (CONVENTIONS §5), and an async stack shows a measured gain in cost × time at the gate without a score loss.

## References (checked 2026-10-19)

- Rust std docs: `thread::scope`, `thread::Builder::spawn_scoped`, `sync::mpsc`, `Condvar`, `os::unix::net::UnixStream`, `os::unix::process::CommandExt::process_group`, `io::pipe`, `fs::File::try_lock`: https://doc.rust-lang.org/stable/std/
- reports/Ngôn ngữ cho INIT Agent.md (≈14 KB per OS thread; ≈0.3 KB per idle tokio task; 8.1 s tokio-only clean release build on the M2; ≈1.2 tool calls per step, TraceLab; Bun's port with no async Rust)
- spec v0.2 §4 (Rust conventions: std threads, no async runtime at seed), §8, §9 (rank table; "kept out on purpose")
- body/provider.md §3.6, §3.11; body/loop.md §3.1, §3.5, §3.7; body/tools.md §3.5; ext/host.md §3.2 (tokio via `in_tokio` → `RUNTIME.block_on`, `allow_blocking_current_thread`, `parallel-compilation`/rayon; verified there against wasmtime-wasi 48.0.6), §3.10–§3.13
- CONVENTIONS.md v0.2 §5 ("no async code in the body"; dependency internals accepted)
- Local verification, macOS 14.6 (Darwin 23.6.0), rustc 1.97.1, 2026-10-19 (see Decision)
