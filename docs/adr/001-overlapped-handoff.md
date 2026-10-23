# ADR-001: Overlapped handoff for T2 body swaps

```
Status: draft v0.1 (decision: accepted) · 2026-10-19 · Area: kernel
Depends on: spec v0.2 (§6.1, §7.1, §11 hypothesis 5), kernel/supervise.md, body/log-context.md, body/loop.md
Defines:    the decision to swap the core body by an overlapped, probe-first, pause-at-step-boundary
            handoff; the alternatives rejected and why
```

---

## 1. Purpose and context

A T2 change replaces the body's compiled core: loop, provider, context, tools, extension host, boot. The agent makes such changes *in the middle of a task*, and spec A3 requires that, after the swap, "it continues the same conversation, citing the boot event". A4 requires that a candidate which crashes at boot changes nothing: "the old body never stops".

Constraints from the evidence (research notes B1, B2; live_modification_isolation §1):

- Rust has no stable ABI, and unloading native code is unsafe (rust-lang #88737, #91979). Subsecond hot-patching is debug-only, tip-crate-only and cannot change structs. The core can change only through a fresh process.
- The process doing a swap must never be the thing being swapped (andtii PR #425). Gödel Agent's fatal crashes clustered when it edited its own running improvement loop.
- A failed swap must be a non-event, and the rollback target must come from a record separate from the swap request (MOSS).
- Rollback logic must run before anything that can fail (cortex #242). Swaps and crashes must be counted separately (statixagent #28).
- State lives in an append-only session log with exactly one writer (body/log-context.md §3.4.1). A swap replaces code, never memory.

## 2. Responsibilities / non-goals

This ADR records *which* swap mechanism INIT uses and why. The exact protocol (frames, timeouts, renames, recovery table) lives in kernel/supervise.md §3.6. The gate stages are in kernel/gate.md.

## 3. Design: the decision

**Swap the body with an overlapped handoff, in this order:**

1. **Probe first, off to the side.** The candidate binary is built, staged as `bin/init-body.staged`, and booted in a separate probe sandbox on a *copy* of the log (G2). The live body keeps working throughout.
2. **Pause, don't kill.** When the gate passes, the kernel asks the live body A to pause at its next **step boundary** (after a tool batch, before the next model request). A flushes the log, releases its `flock`, reports `paused{log_len, leaf}`, and waits.
3. **Fence, then start B.** The kernel proves A released the lock by taking it itself, then spawns B on the **live** log and releases the fence. B takes the lock, rehydrates, appends its `boot` event and writes `ready`.
4. **Commit only after B is ready and settled** (3 s). The commit is the `rename(2)` that installs B's inode as `bin/init-body`, plus moving lineage `main`. A is then told to exit with code 75.
5. **Abort costs nothing.** If B fails at any point before commit, the kernel kills B and tells A to resume. A re-locks, branches back over anything B appended, records `update_failed`, and continues the same conversation.
6. **Provisional, then LKG.** A committed version is provisional. A crash within 120 s returns to the previous version; a crash after that returns to LKG. LKG moves only after G4 passes.

This is Cloudflare ecdysis's pattern (parent keeps serving until the child calls `ready()`; "Since the parent never stopped listening, no connections are dropped"), adapted to an agent. The "connection" is the conversation, and the hand-over point is a step boundary instead of a listening socket.

### 3.1 Alternatives considered

| # | Alternative | Why rejected |
|---|---|---|
| A | **Stop-then-start** (andtii #425: body exits 75; supervisor renames `staged → current`; starts it; rolls back if no `ready` in 90 s) | The old body is already dead when the new one fails, so a failed swap costs a cold restart of the old version, and the conversation stalls. A4 ("the old body never stops") is violated by construction. |
| B | **Bare `execve` of the new binary by the body itself** | Once `exec` succeeds the old image is gone, so there is no automatic fallback (live_modification_isolation §1). And the body would be swapping itself, the exact failure mode the kernel exists to prevent. |
| C | **In-process hot reload** (dylib + `libloading`, `abi_stable`, Subsecond) | No stable ABI; unloading is unsafe (TLS destructors after `dlclose`); `abi_stable` cannot unload; Subsecond is compiled out of release and patches only the tip crate. Fine for leaves through Wasm (T1), not for the core. |
| D | **Blue-green with two live bodies** (both serve; switch traffic) | The log has one writer. Two bodies answering one conversation would diverge and double the model spend. Shadow traffic would need a replay harness the gate already provides more cheaply (G2–G4). |
| E | **Hand over in-memory state** (memfd / systemd fdstore / shellflip) | The state is already on disk in the log. Serializing in-memory state couples consecutive versions to a shared state format, which is exactly the ABI problem of C. It also contradicts "a swap replaces code, never memory". |
| F | **Container/image swap** (MOSS) | Heavier (image build, container start) with no benefit on one host. Phase 2 containers are for benchmark isolation, not for swaps. |
| G | **Overlapped, but rename before spawn** (spec v0.2 §7.1 step 3 as written) | Leaves a window where `bin/init-body` points at an unproven binary, and makes abort a rename-back. Spawning from `.staged` and renaming only at commit means `bin/init-body` is always a binary that has run successfully. |
| H | **Pause at the end of the user turn** instead of a step boundary | A turn can last the whole task. The agent would not see its own improvement until the task ended, which defeats "change itself, then do the task" (spec §6.2) and A3. |

### 3.2 Consequences

**Positive**

- A failed swap is a non-event, as the spec requires: no restart, no crash count, the same conversation continues with one `update_failed` event.
- Boot failures are caught by the probe before the live body is even paused, and again by the live `ready` check before commit.
- Every intermediate state is recoverable after a kernel crash. Before commit, nothing changed; during commit, each step is idempotent; after commit, the new version runs provisionally (supervise.md §3.9).
- The model's prompt cache survives: the prefix is byte-stable across restarts, and the proxy's upstream connections live in the kernel.

**Negative / costs**

- The body must implement a cooperative pause/resume protocol and a branch-back on resume. Mitigation: timeouts on every wait (pause 1800 s, fence 5 s, ready 90 s, exit 10 s), and the kernel kills by sandbox instance, never by cooperation.
- The swap waits for the current step to finish. A step that runs a long tool delays the swap by that long.
- For a few seconds two body processes exist: memory doubles briefly, and B's boot self-check costs a model round trip.
- Background processes started by A die at the swap. The new body must restart what it needs, using the log as its memory.
- More kernel code than stop-then-start: about 10 handoff phases and a recovery table. That is acceptable, because this is the one piece of the kernel the agent cannot repair for itself.

**Neutral**

- Rollbacks reuse the same protocol (target LKG, no probe). A provisional crash, where A no longer exists, degrades to stop-then-start by necessity.
- The mechanism does not depend on Rust. A future body written in another language only has to speak the stdio frames, the lock and the ready file.

## 4. Interfaces

None defined here; see kernel/supervise.md §4.2 (exit codes), §4.4 (ready/heartbeat), §4.5 (`pause`, `paused`, `resume`, `exit` frames).

## 5. Failure modes and recovery

kernel/supervise.md §3.6.4 (abort path) and §5 (F3–F10, F13).

## 6. Performance notes (spec §9)

Rank 4 (self-edit loop): the handoff adds one step of waiting plus B's boot (dominated by its self-check round trip, about 1–5 s) plus 3 s settle; the gate (minutes) dominates. Rank 1 (cache) and rank 2 (round trips) are preserved as described above.

## 7. Invariants and security (spec §2.1)

Invariant 1 (recoverability): the swapping process (kernel) is never the swapped one; `bin/init-body` is never missing; the rollback target is `lkg.json`, separate from the swap request. Invariant 4 (lineage): lineage `main` moves only at commit.

## 8. Test plan

A3 (handoff continues the conversation: supervise.md SV-5, SV-14), A4 (crashing candidate: SV-6, SV-7, G2 probe failure), kernel-crash at every phase (SV-13).

## 9. Open questions

1. Spec §11 lists "an overlapped handoff for an agent" as unproven hypothesis 5. Phase 0 measures: handoff latency from `pause` to the first model request of B, the rate of aborted swaps, and whether the prefix hash stays equal across the swap (Phase 0 measurement 4).

## 10. Spec deltas

- §7.1 step 3: spawn from `.staged` and rename at commit (alternative G), instead of rename-then-spawn.
- §7.1 "pause at the next turn boundary" is read as the next **step** boundary (alternative H).
- Adds the 120 s probation window (evolution.md F10).

## 11. References

All checked 2026-10-19.

- Cloudflare, "ecdysis: graceful restarts for Rust services" — https://blog.cloudflare.com/ecdysis-rust-graceful-restarts
- andtii/agentic PR #425 — https://github.com/andtii/agentic/pull/425
- MOSS, arXiv 2605.22794 — https://arxiv.org/html/2605.22794v1
- Gödel Agent, arXiv 2410.04444 — https://arxiv.org/html/2410.04444v4
- rust-lang/rust #88737 — https://github.com/rust-lang/rust/issues/88737 ; #91979 — https://github.com/rust-lang/rust/issues/91979
- subsecond 0.7.10 docs — https://docs.rs/subsecond/0.7.10/src/subsecond/lib.rs.html
- `abi_stable` — https://docs.rs/abi_stable
- statixagent #28 — https://github.com/eliau2005/statixagent/issues/28 ; cortex PR #242 — https://github.com/dinglebear-ai/cortex/pull/242
- Erlang/OTP code loading (two versions; failed `on_load` keeps current) — https://www.erlang.org/doc/system/code_loading.html
- research_notes: awareness_runtime_principles.md (B1, B2); live_modification_isolation.md (§1)
