# INIT Agent: the whole picture

*Status: v0.2, 2026-10-19, after reconciliation rounds 1–2. Area: overview.*

This document is a one-file map of the whole system. It holds no new decisions. Every statement points to the doc that owns the detail. Read it first, then go deeper only where you need to.

---

## 1. In one screen

**What INIT is:** a coding agent whose own source code is the thing it is allowed to change.

**What it starts as:**
- one loop;
- five tools (`read`, `write`, `edit`, `bash`, `ext`);
- one provider adapter (DeepSeek first);
- an append-only log;
- a prompt designed to keep provider caching working.

The whole body is about 20–25k tokens of Rust, small enough for the agent to read in one context.

**How it grows:**

| Tier | What changes | How |
|---|---|---|
| T0 | Skills and notes (data) | Reloaded |
| T1 | WebAssembly extensions | Hot-swapped |
| T2 | The Rust core | The kernel gates the change, then hands the live conversation to the new binary |

**What stops it from breaking itself:** a small kernel it can read but not write. The kernel:
- supervises and rolls back;
- holds the API keys and the spend cap;
- runs the evaluation gate on tasks the agent cannot see;
- records the true lineage.

That is the whole of governance. Inside the sandbox, the model decides everything else.

**How it stays current:**
- When models change, only the provider changes.
- Every scaffold carries a `Compensates-for:` tag. On a model upgrade, the agent tries to delete each tagged scaffold, and the gate decides.

---

## 2. The picture

```
  owner ──(chat, /panic)──▶ ┌──────────────────── KERNEL (Rust, maintainer-owned, read-only to agent) ────────────────────┐
                            │  supervise ─ spawns/hands off/rolls back the body · crash backoff · safe mode · /panic     │
                            │  proxy ───── API keys · spend caps · persistent upstream conns · raw traces · cache stats   │──▶ DeepSeek / Anthropic / OpenAI
                            │  gate ────── G0 compiler · G1 self-map · G2 boot probe + prefix hash · G3 can-still-evolve │
                            │              G4 tasks + hidden holdout + canaries → non-inferior? cheaper? smaller?         │
                            │  selfmap ─── generates self.json from source, binary and WIT (the agent's verified mirror) │
                            │  serve ───── initctl server (the agent's syscalls)                                         │
                            │  vault/ (hidden): holdout · canaries · traces · lineage · keys                             │
                            └───────────────▲──────────────────────────────▲─────────────────────────────────────────────┘
                                            │ run/kernel.sock (initctl)    │ run/proxy.sock (model calls, no keys)
                            ┌───────────────┴──────────────── SANDBOX ─────┴─────────────────────────────────────────────┐
                            │  BODY (Rust crate, agent-owned, #![forbid(unsafe_code)])                                   │
                            │   loop · provider · context/cache · log · tools: read write edit bash ext · ext host       │
                            │   ext/*.wasm  (T1 components, Wasmtime)        skills/ SELF.md  (T0 data)                  │
                            │  state/  sessions/*.jsonl (the conversation, survives every restart) · tasks/              │
                            │  work/   task workspaces · worktrees for self-edits                                        │
                            └────────────────────────────────────────────────────────────────────────────────────────────┘
```

Details: [spec §3](../INIT%20Agent%20spec.md), [CONVENTIONS §2–3](CONVENTIONS.md), [kernel/supervise.md](kernel/supervise.md), [kernel/sandbox.md](kernel/sandbox.md).

---

## 3. Life of a task

1. The owner sends a message. It reaches the body through the user channel ([kernel/supervise.md](kernel/supervise.md)) and is appended to the session log ([body/log-context.md](body/log-context.md)).
2. The body builds the context. The prefix (genesis prompt, `SELF.md`, tool schemas) is identical byte for byte, so the provider's cache hits. Volatile facts and self-signals go at the tail ([body/log-context.md](body/log-context.md), [awareness.md](awareness.md)).
3. The model streams through the kernel proxy. The proxy injects the key, counts cost and records the trace ([body/provider.md](body/provider.md), [kernel/proxy.md](kernel/proxy.md)).
4. Tool calls run in parallel on std threads, and their results are appended in order ([body/loop.md](body/loop.md), [body/tools.md](body/tools.md)).
5. If the agent lacks a capability, it changes itself at the smallest tier that closes the gap (§4).
6. When the owner accepts the result, the task is recorded with a deterministic check and joins the regression pool ([kernel/gate.md](kernel/gate.md)).

---

## 4. Life of a self-edit

| | T0: data | T1: extension | T2: core |
|---|---|---|---|
| **Edits** | skills, `SELF.md`, prompt text | a Wasm component in `ext/` | loop, provider, context, tools, host, boot, dependencies |
| **Applied by** | `initctl reload` at the turn boundary | build `wasm32-wasip2`, `initctl reload`, `InstancePre` swap | worktree, commit with trailers, `initctl promote` |
| **Checked by** | G0–G1 to keep | G0–G3 to keep | G0–G3 to run provisionally, G4 to become LKG |
| **Cache** | a prefix change costs one miss, batched to a boundary | none, because the `ext` tool schema never changes | the prefix hash must match unless declared |
| **If it fails** | revert the file | the old instance keeps serving | **the old body never stopped**, so a failed swap is a non-event |

The overlapped handoff runs in this order:
1. The kernel builds and probes the candidate in a staging sandbox.
2. The live body pauses at a turn boundary.
3. The kernel atomically swaps the binaries.
4. The new body takes over the same log and writes a `boot` event, then the old body exits with code 75.
5. G4 runs asynchronously. The candidate becomes LKG only if G4 passes; otherwise the kernel rolls back at the next idle point.

Details: [evolution.md](evolution.md), [kernel/supervise.md](kernel/supervise.md), [kernel/gate.md](kernel/gate.md), [ext/host.md](ext/host.md), [ADR-001](adr/001-overlapped-handoff.md).

---

## 5. What can never happen, and what is free

**Never.** These rules are enforced by the OS, the compiler or process boundaries, not by the prompt:

| Invariant | Enforced by |
|---|---|
| The agent writes `kernel/` or reads `vault/` | Seatbelt or Landlock profile ([kernel/sandbox.md](kernel/sandbox.md)) |
| The agent holds an API key or exceeds the spend cap | Keys live only in the proxy, which also enforces the caps ([kernel/proxy.md](kernel/proxy.md)) |
| The agent fakes its gate results or rewrites its lineage | The kernel runs the gate and owns the canonical history and notes ([kernel/gate.md](kernel/gate.md), [kernel/supervise.md](kernel/supervise.md)) |
| The agent corrupts its own memory | `#![forbid(unsafe_code)]`, checked in G0 |
| A bad self-edit bricks the agent | Overlapped handoff, LKG rollback, crash backoff, then safe mode |
| An irreversible external action happens without the owner | Network limited to the proxy socket, plus owner approval for external effects |

**Free.** No permission prompts, rule files, planners, swarms, or limits on which body files may change. The model decides.

---

## 6. Where performance comes from

The harness's own CPU is at most about 9% of agent time, so INIT aims its performance effort at the levers that matter, in this order:

1. **Cache stability.** A miss costs about 17 times as much input. Remedies: a byte-stable prefix, sorted JSON, the stable `ext` tool, and a prefix-hash check at the gate.
2. **Fewer round-trips.** The proxy keeps upstream connections open across body restarts, and tools run in parallel.
3. **The tool path.** The sandbox is set up once, file tools run in-process, large outputs spill to files, and a PTY is the first thing to grow.
4. **Footprint when running many copies.** About 3 MiB of Rust per idle body.

The gate measures cost, time and cache-hit rate. A change that only claims a performance gain is accepted only if it shows one.

Details: [spec §9](../INIT%20Agent%20spec.md), [reports/Ngôn ngữ cho INIT Agent.md](../reports/Ng%C3%B4n%20ng%E1%BB%AF%20cho%20INIT%20Agent.md).

---

## 7. Doc map: which doc answers what

| Question | Doc |
|---|---|
| Layouts, contracts, global decisions | [CONVENTIONS.md](CONVENTIONS.md) |
| How the body is started, handed off, rolled back; user channel; git authority | [kernel/supervise.md](kernel/supervise.md) |
| The agent's syscalls | [kernel/initctl.md](kernel/initctl.md) |
| Exact sandbox profiles | [kernel/sandbox.md](kernel/sandbox.md) |
| Model proxy, keys, caps, stats, traces | [kernel/proxy.md](kernel/proxy.md) |
| Gate stages, statistics, accept rule | [kernel/gate.md](kernel/gate.md) |
| `self.json` and how it is verified | [kernel/selfmap.md](kernel/selfmap.md) |
| Owner configuration | [kernel/config.md](kernel/config.md) |
| Provider adapters (DeepSeek first), mock model | [body/provider.md](body/provider.md) |
| The agent loop | [body/loop.md](body/loop.md) |
| The five tools, exact schemas | [body/tools.md](body/tools.md) |
| Body boot, probe and manifest modes | [body/boot.md](body/boot.md) |
| Event log, context, cache, compaction | [body/log-context.md](body/log-context.md) |
| Genesis prompt, constitution text, `SELF.md`, environment snapshot, self-signals | [awareness.md](awareness.md) |
| WIT contract `init:ext@1.0.0` | [ext/wit.md](ext/wit.md) |
| Wasmtime host | [ext/host.md](ext/host.md) |
| Writing an extension (template, skill) | [ext/authoring.md](ext/authoring.md) |
| End-to-end evolution flows, trailers, thinning, upstream | [evolution.md](evolution.md) |
| Tests, A1–A6 scenarios, mock model, Phase 0 measurements | [testing.md](testing.md) |
| Build milestones, parallel work breakdown, critical path | [roadmap.md](roadmap.md) |
| Why: handoff, proxy, threads, Wasm, Rust, gate policy | [adr/](adr/) |
| Cross-doc decisions (who accepted or rejected what, and why) | [reconciliation/](reconciliation/) |
| What Phase 0 must verify, by compiling or measuring | [phase0-verification.md](phase0-verification.md) |
| Pending owner decisions | [CONVENTIONS.md §6](CONVENTIONS.md) |

---

## 8. From docs to a running agent

1. **Docs complete:** every doc in §7 exists, a cross-doc consistency pass is done, and open questions are closed or listed.
2. **Phase 0 build:** parallel agents, one per crate or area ([roadmap.md](roadmap.md)). It needs **at least 20 GB of free disk**.
3. **Flow test:** a DeepSeek end-to-end run with the owner's key and a small cap. Tests A2–A6 also run at $0 against the mock model.
4. **Phase 1:** the measuring stick (task pool, holdout, canaries, statistics).
5. **Phase 2:** Terminal-Bench subset (seed vs evolved vs mini-swe-agent, same model).
6. **Phase 3:** open-ended archive search.

**What the owner provides:** disk space, a DeepSeek API key with a dollar cap, and decisions on any item listed under "Open questions" across the docs.
