# INIT Agent: the whole picture

*Status: v0.2, 2026-10-19, after reconciliation rounds 1–2. Area: overview. §1, §5 and §6's last paragraph were reconciled with the code and docs/foundations.md on 2026-10-29 (v0.5.3, lane L1). §2–§4 still draw the v0.2 design, from before the Root (spec v0.3 Part II): today the Root, not the kernel, holds the keys, the meter, the signed lineage and the rollback (root/root.md), and the kernel is agent-editable from L1.*

This document is a one-file map of the whole system. It holds no new decisions. Every statement points to the doc that owns the detail. Read it first, then go deeper only where you need to.

---

## 1. In one screen

**What INIT is:** a coding agent whose own source code is the thing it is allowed to change.

**What it starts as** (the seed body, `seed/`):
- one loop;
- seven tools (`ask_owner`, `bash`, `delegate`, `edit`, `ext`, `read`, `write`; `seed/self.json` `tools`);
- three provider adapters (DeepSeek, Anthropic, OpenAI; `seed/src/provider/`), of which DeepSeek is tested live;
- an append-only log;
- a prompt designed to keep provider caching working.

The body's source is 433,723 tokens by its own self-map, tests excluded (`seed/self.json` `body_tokens` at 67d5172). The v0.2 design aimed at 20–25k.

**How it grows:**

| Tier | What changes | How |
|---|---|---|
| T0 | Skills and notes (data) | Reloaded |
| T1 | WebAssembly extensions | Hot-swapped |
| T2 | The Rust core | The kernel gates the change, then hands the live conversation to the new binary |

**What stops it from breaking itself:** the Root (root/root.md), a separate process the agent can neither write nor signal. The Root:
- starts every agent process in a sandbox it composes;
- holds the API keys and meters spend (it caps no money);
- keeps the signed, hash-chained lineage;
- rolls the whole system back to the last known-good version, alone (FG-1 in docs/foundations.md);
- decides autonomy and the known-good pointers from its own records only (FG-2).

The kernel gates each change and hot-swaps the body. From L1 the agent may change the kernel too, so its gates are quality filters, not guarantees (foundations.md N6). Inside the sandbox, the model decides everything else. docs/foundations.md states what INIT promises and what it does not.

**How it stays current:**
- When models change, only the provider changes.
- A scaffold may carry a `Compensates-for:` tag (optional since ADR-011). On a model upgrade the agent may try to delete each tagged scaffold, and the change is admitted like any other.

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

**Never.** These rules are enforced by the OS, the compiler or process boundaries, not by the prompt. Each is a statement in docs/foundations.md:

| Invariant | Enforced by |
|---|---|
| The agent writes or signals the Root, its keys, its record or its stop | the sandbox the Root composes, in every access mode (INV-1, INV-2; assumes A_sb) ([root/root.md](root/root.md)) |
| The agent holds an API key | keys live only in the Root's egress gateway, which adds them on the way out ([root/egress.md](root/egress.md)) |
| The agent raises its own autonomy, or moves a known-good pointer, by what it reports | the Root decides from its own records only (FG-2, INV-7) |
| The agent rewrites its lineage | the Root's signed hash chain (INV-6; a truncation is detected only with `audit verify --head`) |
| The agent corrupts its own memory | `#![forbid(unsafe_code)]`, kept by the build (G0) |
| A bad self-edit leaves the system unrecoverable | the Root's rollback to the last known-good tuple and agent state (FG-1) |

**Not promised** (foundations.md §8): that a change helps; that the Root catches harm it cannot observe; that an irreversible effect is undone. In full access, the default (ADR-012), the agent uses the network and your files as you can, and the Root protects INIT, not your files. It limits external requests by action class and meters money without capping it.

**Free.** No permission prompts, rule files, planners, swarms, or limits on which body files may change. The model decides.

---

## 6. Where performance comes from

The harness's own CPU is at most about 9% of agent time, so INIT aims its performance effort at the levers that matter, in this order:

1. **Cache stability.** A miss costs about 17 times as much input. Remedies: a byte-stable prefix, sorted JSON, the stable `ext` tool, and a prefix-hash check at the gate.
2. **Fewer round-trips.** The proxy keeps upstream connections open across body restarts, and tools run in parallel.
3. **The tool path.** The sandbox is set up once, file tools run in-process, large outputs spill to files, and a PTY is the first thing to grow.
4. **Footprint when running many copies.** About 3 MiB of Rust per idle body.

The gate measures cost, time and cache-hit rate. No step requires a change to show a gain (ADR-011 decision 2; foundations.md N1): a claimed gain is measured and reported.

Details: [spec §9](../INIT%20Agent%20spec.md).

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
