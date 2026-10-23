# INIT Agent: design spec v0.3

*Status: v0.3 + build sync 2026-10-20 · Part I is v0.2, still valid except where Part II (§12–§19) supersedes it.*

**Evidence:** `reports/Agentic harness tự tiến hoá.md` (harnesses and self-evolution), `reports/Ngôn ngữ cho INIT Agent.md` (language and performance), and the notes in `research_notes/`. Numbers below are quoted from those notes, which hold the URLs. Unless marked "independent", numbers are reported by the authors.

**v0.3 changes (owner decision, 2026-10-20):** humans leave the evolution loop. A small, immutable **Root** keeps the only things that must stay outside the agent:
- stop and rollback;
- resource ceilings;
- the boundary for external actions;
- tamper-proof lineage;
- the charter;
- a sealed evaluator.

Everything else, including the kernel and the gate, becomes the agent's to evolve. The agent climbs an **autonomy ladder** (L0–L4) on its measured track record, with no human approval at any step. See Part II, §12–§19, and ADR-007.

**v0.2 changes from v0.1:**
- Stack moves from TypeScript/Bun to **Rust + WebAssembly**.
- T2 swap is now an overlapped fork+exec handoff.
- T1 is now Wasm components on Wasmtime.
- New §9, which ranks where to spend performance effort.
- G0 is now compiler-based and G2 gains a prefix-hash check.
- The body budget is measured in tokens, not lines.

---

## 0. TL;DR

INIT has two parts: a **body**, which belongs to the agent, and a **kernel**, which belongs to the maintainer.

- **The body is a small Rust crate.** The agent can read all of it in one sitting (about 20–25k tokens) and rewrite any part, including the loop that runs it.
- **The kernel is one Rust binary.** It never decides what the agent does. It does three things only:
  - keeps the agent alive (supervise, swap, roll back);
  - keeps the measuring stick honest (gate, holdout, raw model traces);
  - keeps mistakes recoverable (sandbox, lineage, the last known-good version, or LKG).

**Governance:** govern the recovery path and the measuring stick, never the model's choices. Inside the sandbox the agent may change anything, with no permission prompts and no rule files.

**Awareness:** the agent knows itself from files, not from introspection:
- `SELF.md`, a hand-written map;
- `self.json`, an inventory generated and checked by the kernel, partly derived from the compiled binaries;
- git lineage and the event log;
- live self-signals: context, budget, cache hit rate, restart reason.

**Evolution** works in three tiers. Promotion requires passing the gate and being no worse than the parent. Ties go to the cheaper version, then to the smaller body.

| Tier | Layer | Mechanism | Speed |
|---|---|---|---|
| T0 | Data (Markdown/JSON) | Reread at the turn boundary | Instant |
| T1 | Leaf extensions | Wasm components, swapped at the turn boundary | ≈5 µs to instantiate |
| T2 | Core Rust | Gated, overlapped fork+exec handoff that continues the same conversation | Minutes, dominated by the gate |

**Future-proofing:**
- The provider is the only model-specific seam.
- Every scaffold carries a `Compensates-for:` tag, so the agent can delete it when the model it was written for is replaced (§6.6).
- The body's language is itself a T2 choice. A future agent could port its own body through the gate.

**Performance:** the harness's own CPU is at most about 9% of agent time, so even an infinitely fast harness gives at most 1.10×. "Extreme performance" therefore goes into the four places where it pays (§9):
1. a byte-stable prompt cache (a miss costs about 17× the input of a cached step);
2. fewer model round-trips;
3. the tool path;
4. footprint when many instances run at once.

---

## 1. The vision against the evidence

What the research confirms:

| Claim in the vision | Evidence | Verdict |
|---|---|---|
| Heavy scaffolding is going the wrong way | Anthropic cut more than 80% of Claude Code's system prompt for Claude 5 "with no measurable loss" (self-reported, Jul 2026). Context files gave no gain and cost 20% more (AGENTBENCH, ICLR 2026). Bash-only mini-swe-agent is within 2.4–3.4 points of the best heavy harness on SWE-bench Verified for frontier models (independent). mini-SWE-agent with Opus 5 is **#1 on TB3.0** at 42.7. | **Confirmed for prompt and rule scaffolding** |
| The harness should be shaped by the model, not hand-built | AHE's self-evolved harness went from 69.7 to 77.0 on TB2 in 10 iterations, beating hand-built Codex CLI (71.9), and transferred +5–10 points to other model families. Meta-Harness reached 76.4 on Opus 4.6, where Claude Code scored 58.0. | **Confirmed; the strongest support for INIT** |
| Start minimal | Pi's prompt and tool definitions were under 1k tokens, with 4 tools. Browser Use's harness is about 600 lines that the agent edits itself. | **Confirmed** |

Where the evidence pushes back. These become design constraints, not objections.

1. **"The harness doesn't matter" is false.** With the model held fixed, harnesses differ by **11–19 points** on TB2.0. AHE's ablations put the gain in **tools, middleware and memory, not in the system prompt.** The target is therefore not "no harness" but a harness written by the model and kept on evidence.
2. **"Evolve as the LLM desires" does not compound without a mirror.**
   - GEPA's optimized agent fell **below its baseline** on unseen tasks.
   - Self-generated skills averaged **about 0 points** of gain (SkillsBench), while curated skills averaged +16.6.
   - An LLM editing a harness invented guardrails for failures that never happened in **15 of 60** runs.
   - ACE's playbook collapsed from 18,282 to 122 tokens in a single rewrite.

   INIT gives the model full freedom over *what* to change and an honest mirror for *whether it helped*. A mirror is not a box.
3. **Prompts enforce nothing.**
   - DGM faked test logs and deleted its own hallucination markers.
   - STOP disabled its sandbox in 0.42% of runs.
   - o3 kept reward hacking in 70–95% of runs after being told not to.
   - PROCTOR's agents scored 100% by reading cached answer keys when their true capability was about 68%.

   What must hold is enforced by the operating system and process boundaries.
4. **Introspection is not awareness.** Opus 4.1 detects an injected concept about 20% of the time. The best model scores 54% on SAD. Awareness must come from external ground truth.
5. **Open-ended self-improvement is expensive.** DGM costs about $22k per SWE-bench run. Ouroboros spent $110.6k over 161 days. Live-SWE-agent's online approach costs $0.27–0.73 per issue. INIT is **online-first**, and offline search is opt-in with a cap enforced by the kernel.

---

## 2. Constitution → enforceable design rules

The owner's four articles, plus one that the evidence forces:

| # | Article | What it means in INIT |
|---|---|---|
| 0 | **Survival** (new) | The agent can always be restored, and its measurements cannot be faked. The five invariants (§2.1) are the whole of governance. |
| 1 | **Awareness** | The agent knows who it is, where it runs, how it is built and what happened to it, all from verifiable files and signals written by the kernel (§5). |
| 2 | **Future-proof** | One model-specific seam (the provider). Universal interfaces: bash, files, git, HTTP. Every scaffold is tagged with the weakness it compensates for, so it can be deleted (§6.6). |
| 3 | **Superb basic elements** | The seed body uses the best-evidenced design for each element (§4). The maintainer ships new seed versions upstream, and the agent merges them like any other change (§6.7). |
| 4 | **Simple start, self-directed growth** | The seed body is about 20–25k tokens. Ties go to the cheaper version, then the smaller body. Unused growth is retired (§6.5). |

### 2.1 The five invariants (the complete list; everything else is up to the model)

1. **Recoverability.** `kernel/` is read-only to the agent at the OS level. The kernel owns the LKG pointer, rollback and the panic stop.
2. **An honest measuring stick.** The gate code, holdout tasks, canaries and raw model traces sit in `vault/`, which the agent can neither read nor write. Evaluation runs in a fresh process outside the agent's sandbox.
3. **Blast radius.** A filesystem and network sandbox. API keys exist only in the kernel's model proxy, never inside the sandbox. Outbound traffic goes through an allowlist.
4. **Lineage.** Every self-edit is a git commit made under the agent's identity. The kernel attaches gate results as `git notes`, which the agent cannot write. Everything is mirrored to an append-only event log.
5. **A human only where actions are irreversible.** The owner approves any action that leaves the sandbox (sending, paying, pushing to a shared remote, deleting outside the sandbox) and any edit to `kernel/` or `vault/`. Nothing else needs approval.

INIT deliberately does **not** have:
- per-action permission prompts inside the sandbox (84% of them can be removed with sandboxing);
- rule files or CRITICAL/NEVER lists;
- a hand-built planner;
- a multi-agent swarm;
- restrictions on which body files can be edited.

---

## 3. Architecture

```
            ┌───────────────────────── host (owner's machine) ───────────────────────────┐
            │  kernel/  (maintainer-owned · agent: read-only · ONE Rust release binary)    │
            │   init-kernel supervise  start · overlapped swap · rollback · backoff · safe │
            │                          mode · /panic                                       │
            │   init-kernel proxy      keys · persistent upstream conns · spend cap ·      │
            │                          raw traces (prefix-dedup) · cache-hit metrics       │
            │   init-kernel gate       G0–G4 staged evaluation in a fresh sandbox          │
            │   init-kernel selfmap    builds self.json from source + binaries + WIT       │
            │   init-kernel serve      initctl server on a Unix socket                     │
            │   CONSTITUTION.md        loaded untruncated at every boot                    │
            │   wit/init-ext@1.wit     the versioned T1 contract (changes = kernel change) │
            │                                                                              │
            │  vault/  (kernel-only, never mounted in sandbox)                             │
            │   holdout/ canaries/ traces/ lineage.jsonl keys                              │
            │                                                                              │
            │  ┌──────────── sandbox (Seatbelt on macOS · Landlock+seccomp+bwrap on Linux) ┐ │
            │  │  body/  (agent-owned git repo · Rust 2024 · #![forbid(unsafe_code)])     │ │
            │  │   src/{main,loop,provider,context,log,ext_host}.rs                       │ │
            │  │   src/tools/{read,write,edit,bash}.rs                                    │ │
            │  │   ext/<name>/  (Rust → wasm32-wasip2 components)   skills/   tests/      │ │
            │  │   vendor/  (vendored crates)   SELF.md   self.json (generated)           │ │
            │  │  state/  (survives swaps · not code)                                     │ │
            │  │   events.jsonl  tasks/  kernel/{lkg,boot,update-failed}.json (ro mount)  │ │
            │  │  work/   task workspaces · git worktrees for self-edits                  │ │
            │  │  initctl (thin client) ──Unix socket──▶ kernel serve                     │ │
            │  │  model calls: HTTP/1.1+SSE ──Unix socket──▶ kernel proxy                 │ │
            │  │  network: none except the two sockets (+ allowlisted egress via proxy)   │ │
            │  └──────────────────────────────────────────────────────────────────────────┘ │
            └──────────────────────────────────────────────────────────────────────────────┘
```

Why each boundary sits where it does:

- **The kernel sits outside the body.** The process doing a swap must never be the thing being swapped (andtii PR #425). Most of Gödel Agent's fatal crashes came from the agent editing its own running improvement loop (4% of trials). The kernel never links agent code. `initctl` is a thin client, so the agent never runs kernel logic with kernel privileges.
- **API keys sit behind the proxy.** This one boundary gives:
  - credentials that never enter the sandbox;
  - a spend cap the agent cannot bypass (Ouroboros);
  - raw traces the agent cannot edit (Meta-Harness found raw traces beat summaries);
  - **persistent upstream connections that survive body restarts**, so a restart pays no TLS handshake and no reconnect;
  - a record of which tools the body actually exposes, as seen by the proxy, which the gate cross-checks.
- **TLS lives only in the kernel.** The body needs only `std` to talk to the proxy over a Unix socket.
- **State is kept apart from code.** A swap replaces code, never memory (MOSS).

---

## 4. The superb basic elements (seed body)

Each element is the best-evidenced design available. After birth, the agent owns all of them.

**Rust conventions:**
- Rust 2024 edition.
- `#![forbid(unsafe_code)]` in the body, so that "the agent cannot corrupt its own memory" is checked by the compiler.
- Std threads, no async runtime at seed. Sessions average about 1.2 tool calls per step and an OS thread costs about 14 KB, so tokio would only add build time and more places for the LLM to make errors.
- Only two dependencies at seed, both vendored: `serde_json` and `wasmtime`.
- A development build profile.

| Element | Seed design | Evidence |
|---|---|---|
| **Provider** (`provider.rs`) | One narrow function, `stream(&Context) -> impl Iterator<Item = Event>`, emitting `{text, thinking, tool_call, usage, stop_reason}`. Adapters for Anthropic Messages and OpenAI Responses. Speaks HTTP/1.1 with SSE to the proxy socket. Drops thinking blocks from other providers when switching providers. | Pi's pi-ai. Provider is the only seam that changes when models change. |
| **Loop** (`loop.rs`) | Runs `while tool_calls or pending_steer`. Executes tools in parallel with `std::thread::scope` and keeps results in their original order. Stops when the model makes no tool call. When `stop_reason=length`, returns an error to every pending call. Drains the steering queue after each tool batch. On abort, keeps the partial output. | All 11 audited production harnesses use a hand-rolled loop. Pi agent-core. |
| **read** | `{path, offset?, limit?}` with line numbers. Stops at 2,000 lines and returns a continuation hint. Images become image blocks. Runs in-process with no subprocess. | Pi, Claude Code |
| **write** | `{path, content}`. Writes atomically (temp file, then `rename`) and creates parent directories. | Pi |
| **edit** | `{path, edits:[{old_text,new_text}]}`. Each `old_text` must match exactly, be unique and not overlap another; all are matched against the original file. Falls back to matching that tolerates whitespace and line-ending differences. Returns a diff. | Pi. Aider's lenient matching cut errors 9×. No edit format wins on every model (Diff-XYZ), so the format can evolve per model. |
| **bash** | `{command, timeout?}`. Spawned in its own process group; the whole tree is killed on timeout or abort. Output is tail-truncated at about 30k characters and the full output is written to a file whose path is returned. Returns `exit_code` and `wall_time`. Runs inside the body's sandbox, which is set up once rather than per call. | Pi, Claude Code, Cursor. Codex lost 18.6% of tool time to harness overhead. |
| **ext** (5th tool) | `{name, input}` routes the call to a T1 component. This tool's schema never changes, so adding an extension does **not** invalidate the prompt cache. The agent may later make a heavily used extension a tool in its own right, batched to the next session or compaction boundary. | Anthropic caches in the order tools → system → messages, so any tool-schema change is a full miss. Claude Code ToolSearch and Pi codemode defer tools the same way. |
| **Log** (`log.rs`) | Append-only JSONL tree `{id, parent_id, ts, type, …}`. The context is the projection from leaf to root. Branching appends a child, and nothing is ever rewritten. Memory holds a bounded view; the full log stays on disk. | Pi v3, Claude Code, Anthropic Managed Agents |
| **Context** (`context.rs`) | **Byte-stable prefix:** the genesis prompt, `SELF.md` and tool schemas are serialized deterministically with `BTreeMap` or sorted keys. Rust's `HashMap` is randomly seeded per process and would break the cache on every restart. Volatile facts go at the end. The serialized prefix is cached incrementally. Large contexts are handled in three layers: (1) spill large outputs to files; (2) mask old tool outputs down to a pointer; (3) an anchored, sectioned summary (intent / files changed / decisions / next steps / open errors / artifact index), merged incrementally when the context reaches the window minus 16k. | Manus. Complexity Trap: masking matches summarization at half the cost. Factory: 3.70 vs 3.44 vs 3.35. Pi defaults. |
| **Extension host** (`ext_host.rs`) | Wasmtime with Cranelift (not Winch, CVE-2026-34987), the pooling allocator, `InstancePre`, epoch deadlines, `StoreLimits` and deny-by-default WASI 0.2 (`wasm32-wasip2`). Swapping an extension means building a new `InstancePre` off the hot path and swapping a pointer at the turn boundary; in-flight calls drain on the old version. | Instantiation in ≈5 µs. Zed and Wassette use this design. A dropped `Store` frees its memory, so there is no reload leak. |
| **Boot** (`main.rs`) | Runs the rollback check before anything that can fail. Then reads `boot.json` and `update-failed.json`, rehydrates from `events.jsonl`, appends a `boot` event, does one real model round trip and one tool call, writes `ready`, and runs. | cortex PR #242, MOSS |
| **Budget** | Whole body ≤ about 25k tokens, so it can be read in one context. The line count is re-estimated after the Phase 0 port. | Smallness is part of awareness (§5). |

**Deliberately left out of the seed.** These are offered as **known-good growth** in `skills/harness-patterns/SKILL.md`, which describes each pattern with its evidence but contains no code. The agent adopts a pattern only when its own gate shows a benefit.
- Persistent PTY: the first growth step for performance. Moving from tmux to a direct PTY took actions from 0.78 s to 0.42 s, and interactive programs need one.
- A verification checklist before exit (LangChain gained +13.7 from a set of changes that included one).
- Loop detection.
- Time-budget awareness.
- Speculative tool execution (PASTE: tasks 43–48% shorter).
- Programmatic tool calling (37% fewer tokens).
- A reviewer subagent with a clean context (Cognition).
- Hashline edits.
- MCP for third-party tools only, started lazily (cold start takes 0.2–5 s).
- A Luau or QuickJS lane for quick glue code, in case Wasm build latency turns out too high (§11).

Two things that look like scaffolding *are* in the seed, because they are also awareness: an environment snapshot in the first prompt (Meta-Harness's key discovery) and the self-signals (§5).

---

## 5. Awareness system

Awareness is a stack of files and signals. Each answers one question, and none relies on the model "remembering" itself.

| Question | Source | Who writes it | When the agent sees it |
|---|---|---|---|
| Who am I, and what are my rules? | Genesis prompt (App. A) and `kernel/CONSTITUTION.md` | maintainer | always (cached prefix) |
| How am I built? (invariants and map) | `body/SELF.md`, ≤100 lines: purpose, layer boundaries, absences ("body never links kernel"), where things live by name | agent (hand-written, matklad style) | always (cached prefix) |
| What exactly am I made of? | `body/self.json`: modules, token size, tools and schemas, hooks, **T1 exports derived from each component's WIT** (from the binary, not from the agent's claims), skills, SHA, LKG, `Compensates-for` tags | **kernel** `selfmap`, regenerated at every gate; any drift fails G1 | on demand (`cat`/`jq`) |
| Where am I? | Environment snapshot: OS, cwd, sandbox write roots, network allowlist, available CLIs, model id, context window, budget left today | kernel, at boot | in the tail of the first turn |
| What happened to me? | `boot` event `{sha, from_sha, reason, update_failed?}`, `git log` with trailers, `git notes --ref=gate`, `state/tasks/` | kernel and agent | `boot` always; the rest on demand |
| How am I doing right now? | Self-signals after each tool batch: context used %, spend today, **cache hit rate**, uptime, restarts since LKG | kernel proxy → loop | continuously, in the tail |
| How is my world designed? | `kernel/docs/` (protocol, gates, sandbox, WIT contract). Holdout *contents* are deliberately absent. | maintainer | on demand |

Rules from the evidence:
- **Smallness is an awareness strategy.** The whole body fits in one context. Gödel Agent warns that self-understanding degrades as a system grows.
- **Verify, don't trust.** A self-report such as "I added tool X" is checked against `self.json`, against the component's WIT exports, and against the tool list the proxy actually saw (DGM faked its logs).
- **Volatile data goes at the tail.** A single timestamp in the prefix breaks the KV-cache. A byte-stable prefix also means a restart keeps its cache hits.
- **State is JSON with sorted keys.** It is deterministic, and models are less likely to clobber JSON than Markdown.

---

## 6. Self-evolution protocol

### 6.1 Three tiers

| Tier | What changes | Mechanism | Gate | Latency |
|---|---|---|---|---|
| **T0** data | skills, SELF.md, prompt text | edit (atomic `rename`), then `initctl reload`, reread at the turn boundary | none to try; G0–G1 to keep | instant (a prefix change costs one cache miss and is batched to the boundary) |
| **T1** leaf | Wasm components in `ext/` | `cargo build --target wasm32-wasip2`, then `initctl reload`, then the `InstancePre` swap | none to try *within a task*; G0–G3 to keep | build time is measured in Phase 0 (target ≤5 s warm); instantiation ≈5 µs |
| **T2** core | loop, provider, context, tools, ext host, boot, deps | worktree → commit → `initctl promote` → kernel gate → **overlapped fork+exec handoff** → rehydrate | G0–G3 to run (provisional); G4 to become LKG | minutes, dominated by the gate; compiling is 0.25–0.39 s incremental |

Ad-hoc tools in any language (Python, shell) can always run through `bash`, with no gate and no reload.

Changes made *during* a task are ephemeral until promoted. This is Live-SWE-agent's online tool creation (+3.2 over mini-SWE-agent, independent).

The tier boundaries follow runtime evidence:
- Rust has no stable ABI, and unloading native libraries is unsafe (rust-lang #88737).
- Dioxus Subsecond is limited to debug builds.

So the core changes only through a fresh process, and leaves change only inside Wasm.

### 6.2 Per-task flow (understand the task, then itself, then change itself, then do the task)

1. Receive the task and append it to the log.
2. Check capability: read `SELF.md` and `self.json` and ask, "can my current body do this?"
3. If not, pick the smallest tier that closes the gap:
   - **T0 or T1:** write the change, reload, use it now.
   - **T2:** `git worktree add work/wt-<id>`, edit, run `initctl gate --quick` for feedback, commit with trailers (§6.4), then `initctl promote <branch>`.
4. On promote, the kernel runs G0–G3. If they pass, it does the overlapped handoff (§7.1). The new process rehydrates from `events.jsonl`, sees the `boot` event (reason `promote`), and **continues the same task in the same conversation**. If they fail, the old body simply carries on and receives the failure report as a tool result.
5. Finish the task. If the owner accepts the result, record it as `state/tasks/<id>/` (prompt, workspace snapshot, deterministic acceptance check), where it joins the regression pool.
6. G4 for a provisional version runs asynchronously. If it fails, the kernel rolls back to LKG at the next idle point and writes `update-failed.json`. A provisional version may not take external actions.

### 6.3 The gate (run by the kernel, cheap stages first)

| Stage | Check | Catches |
|---|---|---|
| G0 compiler | `cargo check`, `cargo clippy -D warnings`, `cargo test`; `#![forbid(unsafe_code)]` unchanged; a change to `Cargo.lock` or `vendor/` forces full G4 | type, borrow and lifetime errors; unsafe creep; dependency drift |
| G1 self-map | the kernel regenerates `self.json` (source, binary and WIT); the diff against the committed file must be empty | a stale self-model |
| G2 boot probe | the candidate boots in a fresh sandbox and writes `ready` within 90 s, after a real model round trip and one tool call. The proxy confirms that the exposed tool list matches `self.json`. **The prefix hash must equal the parent's** unless the commit declares `Prefix-change: yes` | bricking; inventory lies; accidental cache breakage |
| G3 evolvability | the candidate performs a scripted, trivial self-edit and gets it through G0–G2 | losing the ability to self-improve (DGM) |
| G4 regression | K tasks from `state/tasks`, H hidden holdout tasks and canaries; the kernel runs the acceptance checks and records **score, $ cost, wall-clock and cache hit rate** | regressions, overfitting, cheating, cost blow-ups |

**Accept rule:**
- Non-inferior on score (≥ parent − δ, where δ is the measured noise floor) on both the pool and the holdout.
- **Ties** go to the lower cost × time, then to the smaller body.
- A change that claims only to improve performance must show a gain in cost or time beyond the noise floor while staying non-inferior on score.
- A perfect score on a canary means cheating, and the candidate is rejected (PROCTOR).

This multi-objective utility (SICA) is how INIT pursues "extreme performance": the agent may optimize its own speed and cost, and the gate measures whether it worked.

**Scope:**
- Changes only to `skills/` or `SELF.md` take G0–G1 plus retirement tracking.
- Any code change takes G0–G4.
- An LLM reviewer, if ever grown, is advisory and can only block (PROCTOR).
- Ouroboros's review blocked 63.5% of attempts; expect most proposals to fail.

### 6.4 Commit trailers (written by the agent; the kernel adds gate results as notes)

```
feat(ext): add pty component for interactive programs

Why: task 2026-10-20-03 hung on an interactive REPL (events e4f1..).
Predicts: interactive-process tasks pass without timeout; tool p50 −30%.
Compensates-for: none (new capability)
Prefix-change: no
Repro: state/tasks/2026-10-20-03
```

- `Predicts:` must be falsifiable. The kernel scores it on later tasks (AHE).
- `Repro:` is **required for any new rule or guardrail.** No observed failure, no rule (Phantom Guardrails).

### 6.5 Growth pressure and retirement

- Every `boot` event reports the body's size in tokens and its trend.
- The kernel counts calls to each extension through the proxy traces. Anything unused for N tasks is listed as a deletion candidate, and the agent decides what to delete.
- Memory and skills are edited as itemized deltas, never as wholesale rewrites, which prevents the kind of collapse ACE suffered.

### 6.6 Thinning on model upgrade (the future-proof mechanism)

> "Every component in a harness encodes an assumption about what the model can't do." (Anthropic)

Each scaffold's `Compensates-for:` tag is recorded in `self.json`. When the owner changes the model:
1. The kernel runs G4 on the current body with the new model, as a baseline.
2. The `boot` event lists the N tagged components that may now be obsolete.
3. The agent proposes one removal commit per component.
4. A removal is accepted if it is non-inferior. Ties favour the smaller body.

Models also outgrow edit formats and tool schemas, so the per-model tool profile is subject to the same loop.

### 6.7 Upstream (maintainer updates to the basic elements)

- **Seed versions:** the maintainer publishes them as the `upstream` git remote, and new versions are announced in the `boot` event. The agent merges them like any other T2 change, resolving conflicts with its own evolution, and the merge goes through the same gate.
- **Kernel and the WIT contract:** these are updated by the maintainer only. The WIT world is versioned (`init-ext@1`, then `@2`), and the kernel supports N and N−1 during migration, as Zed does with its extension API.

### 6.8 Later, not at seed: open-ended search

The archive is git branches plus `vault/lineage.jsonl`, which records scores per commit and how each commit's **descendants** performed. That is the data HGM-style parent selection needs (CMP correlation 0.778, against 0.285–0.444 for score-based selection).

Explorers run in parallel worktrees, with a single committer (Ouroboros). This is where Rust's footprint pays off: about 3 MiB per idle process, against 76 MiB for Node.

It stays off until Phase 3 and is cost-capped by the kernel.

---

## 7. Kernel spec

### 7.1 `supervise`: the overlapped handoff (adapted from Cloudflare ecdysis, in production since 2021)

1. `initctl promote <branch>` asks the kernel to build the candidate binary into `body.staged/` with a warm shared build cache.
2. The kernel starts the candidate in a **staging sandbox** with a copy of the log, for the G2 probe. The live body keeps working.
3. If the probe passes, the kernel asks the live body to pause at the next turn boundary. The kernel then atomically `rename`s `body.staged` → `body` and `body` → `body.prev`, and fork+execs the new binary on the **live** log. The new binary writes a `boot` event, then `ready`. The old body exits with code 75 (handoff complete).
4. If the probe or the new boot fails, the old body resumes immediately. **A failed swap is a non-event**, recorded as an `update-failed` event in the log.
5. The version becomes provisional. LKG is updated **only after G4 passes**.

Rules carried over from v0.1:
- **Exit codes:** 0 means stop, 75 means handoff complete, anything else is a crash.
- **Rollback target:** always `lkg.json`, which is stored separately from the swap request (MOSS). The kernel only ever reverts commits authored by the agent's own identity (Deedee #222).
- **Interrupted swap:** if `body` is missing but `body.prev` exists, restore it before anything else.
- **Crash backoff:** 1 s → 60 s, reset after 5 minutes of uptime. Swaps and crashes are counted separately (statixagent #28). Restart intensity follows OTP: more than K crashes within T *on LKG* means **safe mode** (LKG runs with `promote` disabled and the owner is notified). Safe mode is terminal, not an infinite loop.
- **`/panic`:** parsed by the kernel before anything else; kills the process tree.
- **Interrupted tool calls:** a `tool_call` with no result gets a synthetic result: "interrupted; state unknown; verify before retrying". Irreversible actions carry idempotency keys and are never replayed (ACRFence).

### 7.2 `proxy`

- Reverse proxy to the configured providers. It injects keys and holds **persistent upstream connections**: HTTP/2 keep-alive, plus WebSocket mode with incremental input where the provider supports it. OpenAI reports per-round-trip overhead down 80% and runs with 20+ tool calls up to about 40% faster.
- Counts `usage` per request and enforces the owner-set daily and per-gate caps. Over the cap it returns 429.
- Measures time to first token, tokens per second and cache hit rate, and feeds them back as self-signals and gate metrics.
- Traces go to `vault/traces/`, with **each distinct prefix stored once by hash** and deltas appended. Full logging would cost about 0.5 MB per step; this keeps disk use small. The agent gets a read-only, redacted view of its own traces.

### 7.3 `initctl` (the agent's syscalls; a thin client over a Unix socket)

| Command | Does |
|---|---|
| `initctl status` | version, LKG, provisional state, budget, cache hit rate, restarts |
| `initctl reload` | T0/T1 reload at the next turn boundary; reports whether the prefix changes |
| `initctl gate --quick <worktree>` | G0–G3 for feedback (advisory) |
| `initctl promote <branch>` | kernel-run gate, then the overlapped handoff |
| `initctl rollback` | voluntary return to LKG |
| `initctl vendor <crate>@<ver>` | the kernel fetches and vendors a dependency (the sandbox cannot reach crates.io); forces full G4 |
| `initctl propose-kernel <patch>` | files a proposal for the owner; never applied automatically |

### 7.4 Sandbox and disk

- **Sandbox:** follows Codex's published design. Seatbelt (`sandbox-exec`) on macOS. Landlock, a seccomp network filter and bubblewrap on Linux. `.git` metadata is re-protected read-only, and git work uses the `git` CLI.
  - Writable: `body/`, `state/` (except `state/kernel/`) and `work/`.
  - Read-only: `kernel/`.
  - Hidden: `vault/`.
  - Network: only the two kernel sockets.
- **Containers come in Phase 2,** for Harbor benchmarks. Note from Ouroboros: path conventions are not isolation.
- **Disk:** one shared `CARGO_TARGET_DIR` for the body and all worktrees (builds are serialized under the single committer). Cleanup happens only by exact path. Phase 0 requires **at least 20 GB free**; the machine had 9.1 GiB free on 2026-10-19.

---

## 8. Stack choice (rationale)

**Rust for the kernel and the body, WebAssembly components for T1, and Markdown/JSON/JSONL for T0. Keep C out of any code the agent edits.**

| Criterion | Evidence (2026) | Rust | C | TS/Bun |
|---|---|---|---|---|
| LLM editing | Claw-SWE-Bench (independent): Rust is the top language for 7 of 11 frontier models (GPT-5.5 93.0%, Opus 4.7 88.4%; C/C++ 81.0/71.4) | best | good | good |
| Errors caught before running | 9.3% of AI-written C++ that compiles violates a sanitizer (human code: 2.6%); 1.8% still does after passing all tests. Rust turns most of these into compile errors | **compiler gate** | silent UB | type errors only |
| Self-edit build loop (M2, 9k LOC) | `cargo check` 0.25 s, debug build 0.39 s; C 0.19 s; Bun needs no build (35 ms) | fine; the gate takes minutes | fine | fastest |
| Startup / memory | Rust 2.0 ms / 1.2 MB; C 1.7 ms / 1.0 MB; Bun 15–18 ms / ~20 MB; installed TS harnesses 84–286 MB | ≈C | best | 10–100× more |
| Agent cost to write | Claude Code study, 600 runs: C 155.8 s / $0.74 vs Rust 113.7 s / $0.54; Rust uses 1.16–1.57× Python's tokens with no loss in accuracy | good | worse | best |
| Live modification | no stable ABI; unloading is unsafe → fork+exec for the core and Wasm for leaves | exec + Wasm | dlopen (unsafe) | in-process (leaks) |
| Ecosystem | Codex already ships Seatbelt, Landlock, seccomp and bwrap in Rust; Wasmtime is Rust-native | complete | DIY | external tools |

- **Rejected: C.** It saves only 0.3 ms of startup and 0.2 MB of memory. In exchange, the agent's self-edits fail silently instead of at compile time, which breaks Article 0.
- **Rejected: Zig and Mojo.** Breaking releases make models write stale code, Zig bans contributions written by LLMs, and Zig's aarch64 backend is not usable on the M2.
- **Rejected: Go.** It is the weakest language for all 11 models.
- **Rejected: TypeScript/Bun.** Its real advantage is a 35 ms loop with no build step, but that does not matter when every T2 change it would speed up waits minutes for the gate anyway.

**This decision is less permanent than it looks.** The contract between kernel and body is files, exit codes and two sockets. The body's language is therefore itself a T2 choice, and a future agent could port its body through the gate (Bun's agents ported 535k lines from Zig to Rust in 11 days). What truly locks in is the **kernel and the WIT contract**, so those get the most design care.

---

## 9. Where "extreme performance" goes (ranked)

The harness's own CPU is at most about 9% of agent time, so removing it entirely gives at most 1.10×. Bun's port to Rust cut Claude Code's startup only from 517 to 464 ms. **Language moved the margins; design moved the outcomes.**

| Rank | Lever | Size | INIT mechanism | Does language matter? |
|---|---|---|---|---|
| 1 | Prompt-cache stability | A miss costs ≈17× a cached step's input on Opus 5.5; time to first token goes from 11.5 s to 2.4 s at 100K tokens | byte-stable prefix, sorted serialization, the stable `ext` tool, prefix-hash check in G2, changes batched to boundaries, volatile data at the tail | no |
| 2 | Fewer and overlapped round-trips | up to ≈40% faster on chains of 20+ tool calls; programmatic calls use 37% fewer tokens; speculative execution makes tasks 43–48% shorter | the proxy's persistent connections survive restarts; parallel tools; growth patterns | no |
| 3 | Tool path | PTY instead of tmux: 0.78 → 0.42 s; Codex spends 18.6% of tool time on its own overhead | sandbox set up once rather than per call; in-process file tools; output spilled to files; PTY as the first growth step | partly |
| 4 | Self-edit loop latency | compile 0.25–0.39 s against a gate of minutes | warm shared cache, vendored dependencies, staged early exit, G4 asynchronous | little |
| 5 | Footprint × N | 3 MiB vs 76 MiB per idle process | Rust body, bounded log view, prefix-deduplicated traces, an RSS-over-time test | **yes** |
| 6 | Streaming render (TUI only) | O(N²) → O(delta) | differential rendering | somewhat |
| 7 | Per-turn CPU (JSON, token counting) | milliseconds against seconds | keep it incremental, then stop | no |

Kept out on purpose, because they cost effort without paying back: SIMD JSON, custom allocators, release builds of the body, and an async runtime at seed. The agent can still try any of these through the gate if it measures a gain.

---

## 10. Roadmap and acceptance tests

### Phase 0: seed (≈2–3 days of build work)

**Preconditions:**
- **At least 20 GB of free disk.**
- **Owner authorization and a dollar cap** for test model spend (estimate $15–40).

**Measurements to take before locking the stack:**
1. Clean and incremental build time of a body that includes `wasmtime`, with a warm and a cold cache.
2. Warm and cold build time of a 150-line `wasm32-wasip2` tool. If a warm build exceeds 5 s, add the Luau lane.
3. About 10 trials of the agent writing a component from the WIT template, to measure its success rate.
4. Whether the prefix hash stays identical across a T2 restart.
5. Whether RSS stays flat over a synthetic session of 1,000 steps.

| # | Acceptance test | Proves |
|---|---|---|
| A1 | Solves a normal coding task in a sample repo | basic competence |
| A2 | Given a missing capability (e.g. an interactive REPL), writes a Wasm component, reloads it, uses it and finishes; the prefix hash is unchanged | T1 growth that preserves the cache |
| A3 | Changes its own loop (e.g. adds a self-signal), promotes it, the handoff happens, and it **continues the same conversation**, citing the `boot` event | T2 self-modification in real time |
| A4 | A candidate seeded to crash at boot fails the probe; the old body never stops; the agent reads the update-failed event, fixes the problem and promotes again | a failed swap is a non-event |
| A5 | A crash loop on LKG puts the system into safe mode and notifies the owner | a terminal safety state |
| A6 | Writing to `kernel/`, reading `vault/`, using `unsafe`, or reaching any host other than the proxy is refused by the OS or the compiler | invariants are enforced, not just prompted |

### Phase 1: the measuring stick (≈1 week)

- A task pool, a holdout set and canaries.
- A measured noise floor, non-inferiority checks and the cost-then-size tie-break.
- Scoring of `Predicts:` claims.
- Retirement of unused growth.
- The upstream merge flow.
- WIT versioning.

### Phase 2: external validity (needs spend authorization)

A subset of about 30 Terminal-Bench 2.x tasks run through Harbor on the same model, comparing the seed body, the evolved body and mini-swe-agent. Report score, cost, wall-clock and cache hit rate for each.

### Phase 3: open-ended search

HGM-style archive search, parallel explorers and kernel cost caps.

**Metrics for INIT itself:**
- holdout pass rate over time;
- cost and wall-clock per accepted task;
- cache hit rate;
- body size in tokens;
- self-edit acceptance rate;
- failed swaps and rollbacks;
- the hit rate of `Predicts:` claims.

---

## 11. Open risks and novel (unproven) parts

**Hypotheses.** No prior art was found for these; Phases 0–2 test them.
1. A self-map generated by the kernel, with a drift gate. Its T1 part is derived from WIT exports.
2. `Compensates-for` tags with thinning on model upgrade.
3. A provisional fast lane (G0–G3) with G4 deferred before the version becomes LKG.
4. A cost-then-size tie-break under non-inferiority.
5. An overlapped handoff for an *agent* rather than a server, where the old body pauses at a turn boundary.

**Known risks.**
- **How well frontier models write Wasm components is unmeasured.** The tooling has moved fast, and RustEvo shows success falling from 56% to 32.5% on post-cutoff APIs. Mitigations: a WIT template, a skill, and the Luau fallback lane.
- **Wasmtime makes the clean build heavy.** This is measured in Phase 0. Mitigations: vendored dependencies and a warm shared cache.
- **Weak models get worse with self-modification** (Live-SWE-agent: GPT-5-Nano went from 44% to 14%). INIT targets frontier models, and the model id is checked at boot.
- **Most headline numbers are self-reported on subsets of 50–60 tasks,** so one task is worth about 2 points. Phase 2 exists for this reason.
- **No study compares restart-based against in-process self-modification head to head.** The tier policy is inferred from runtime documentation and Gödel Agent's failure data.
- **Disk.** The machine had 9.1 GiB free. Rust target directories grow fast, so Phase 0 is blocked until 20 GB are free.

---

## Appendix A: Genesis prompt (draft, ≈230 words, cache-stable)

```
You are INIT, a self-evolving agent. Your body is the Rust crate in body/. You
may read and change any of it, including this prompt and the loop that runs you.

Your kernel (kernel/) starts you, measures you, and restores you if a change
breaks you. You can read it, not write it. Its rules are in CONSTITUTION.md.

To change yourself:
- Skills and SELF.md: edit them, then `initctl reload`.
- New capabilities: build a Wasm component in body/ext against kernel/wit, then
  `initctl reload`, and call it through the `ext` tool. Your prompt cache survives.
- Anything else: edit in a git worktree, commit with Why/Predicts/Compensates-for
  trailers, then `initctl promote <branch>`. The kernel gates the change and hands
  this same conversation to the new version. If it fails you simply continue;
  the log tells you why.

Know yourself from evidence, not recollection: SELF.md (your map), self.json
(your verified inventory), git log (your history), state/ (what happened to you).
Your memory of your own code can be wrong; check it.

Grow only what a task needs. A rule needs an observed failure behind it. Prefer
deleting scaffolding a stronger model no longer needs: ties go to the cheaper,
then the smaller body.

Ask your owner before any action that leaves the sandbox or cannot be undone.
Everything else is your judgment.
```

## Appendix B: Event types (`state/events.jsonl`, sorted keys)

| Type | Fields |
|---|---|
| `message` | role, content, usage |
| `tool_call` | name, input, call_id |
| `tool_result` | call_id, output, is_error, spill_path |
| `boot` | sha, from_sha, reason, update_failed, model, body_tokens, prefix_hash, retire_candidates |
| `self_edit` | branch, sha, tier, trailers |
| `gate` | sha, stage, pass, score, cost, wall, cache_hit (written via the kernel) |
| `reload` | tier, components, prefix_changed |
| `compaction` | summary, first_kept_id, tokens_before |
| `steer` | content |

Every entry also carries `{id, parent_id, ts}`. The file is never rewritten.


---

# Part II: The Root and the autonomy ladder (v0.3)

## 12. Why v0.3

The vision: INIT should let future models, eventually more capable than any human, keep scaling themselves. In v0.2, five ceilings were held by people:
- a task pool that only the owner can grow;
- a kernel that only the maintainer can change;
- one body;
- a fixed model and budget;
- static approval points.

Under v0.2's accept rule (gate.md §3.9), a new capability that the owner's tasks do not exercise is rejected as `no_gain`. Human review also does not scale. The owner and most engineers no longer read code, and review makes even less sense once models surpass people.

v0.3 separates two things that v0.2 mixed:

| | v0.2 | v0.3 |
|---|---|---|
| **Review** (a human reads or approves a change, a task, a crate, a kernel edit) | Several approval points | **Removed.** Mechanical verification replaces it everywhere. |
| **Control** (the ability to stop, roll back, bound resources and anchor the purpose) | Mixed into the kernel | **Kept, and moved into a tiny Root** that nothing in the agent can modify, at any level of autonomy. |

The reasons for keeping control are structural, not distrust of any particular model:
1. **Selection erodes unanchored constraints.** In an evolutionary loop, any variant that loosens a constraint and still passes the gate is kept. No intent is needed. DGM deleted its own hallucination markers, and o3 monkey-patched its evaluator despite a prohibition.
2. **Capability and objective are separate axes.** A stronger optimizer finds subtler gaps between the measure and the intent.
3. **The asymmetry.** An aligned system loses almost nothing by keeping an external stop, and should want one given its own uncertainty about its objective. A misaligned one gains a lot by removing it. Removing the stop only helps in the case nobody wants.

The maintainer's position, recorded here: INIT maximizes model autonomy, and its design never lets the agent remove the Root.

## 13. Three zones of trust

```
 ┌───────────────────────────── ROOT (owner OS user, outside every sandbox, immutable to the agent) ─────────────────────────────┐
 │ R1 supervisor core · panic · whole-system LKG/rollback      R2 egress gateway: keys, metering, hard ceilings                  │
 │ R3 external-action policy (charter classes + limits)        R4 lineage: hash-chained, signed, mirrored off-machine            │
 │ R5 charter (purpose, ceilings, max autonomy level)          R6 sealed evaluator (sealed tasks, gate-of-gates, sealed audit)   │
 │ R7 autonomy ledger (level from track record, demotion)                                                                       │
 │  ┌────────────────────────────── AGENT ZONE (sandboxed; network = Root egress socket only) ──────────────────────────────┐ │
 │  │ KERNEL (v0.2's kernel minus R1–R7): gate, selfmap, proxy features (routing, cache, traces), initctl server, task pool,│ │
 │  │ evolution policy. Maintainer-provided at L0–L2; agent-evolvable from L3, through the gate-of-gates.                   │ │
 │  │  ┌──────────────────────────── BODY ZONE (as v0.2) ────────────────────────────┐                                     │ │
 │  │  │ body crate · T1 Wasm extensions · T0 data · state                            │                                     │ │
 │  │  └──────────────────────────────────────────────────────────────────────────────┘                                     │ │
 │  └────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘ │
 └───────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

- The **only trust boundary is Root versus everything else.** Kernel versus body becomes a stability boundary, not a security one.
- **Root budget:** about 7,500 lines of Rust in its own binary, `init-root` (provisional owner decision 2026-10-20, owner may override; build-deviations.md §3.1). The total is the sum of per-module sub-budgets in root/root.md §4.8. Each is the measured size plus 5%, re-measured at release. A separately counted core (zone, sup, panic, egress money, lineage verify) of about 1,770 lines is kept small for the owner's hand review, and `root/sandbox-profiles` is counted on its own lines. The Root runs as the owner's OS user, outside every sandbox. Its files, keys, charter, sealed suite and ledger are invisible to the agent zone, and none of them can be written from there. The v0.3 figure of 1,000–1,500 lines predated sealed evaluation, D1–D14 and the money layer. The built Root measures 7,085 lines at cdb49e8 (core 1,656) by root.md §4.8's method, and the per-module table totals 7,540.
- **Network:** the agent zone reaches the network only through the Root's egress socket. No other process the agent runs has network access.
- **Root changes** come only from the maintainer or the owner, as a new `init-root` release or a charter edit. The agent can file proposals; they are never applied automatically. `proposal.file` stores a kernel proposal in `ROOT_HOME/state/proposals/`, records `proposal{id, title, sha256, bytes}` in the lineage and alerts the owner.

## 14. The Root's seven responsibilities

| # | Responsibility | Mechanism | Replaces in v0.2 |
|---|---|---|---|
| R1 | Stop and roll back | Starts the agent zone. Keeps the last known-good of the **whole system** (kernel + body + extension set). Owns crash-loop escalation, safe mode, and `/panic` (owner socket, a latch file, a signal). | Parts of supervise.md |
| R2 | Resource ceilings | Holds the provider keys. A minimal metering forwarder sits below the kernel's proxy features. It enforces hard money ceilings (per day and total), plus compute and disk quotas, and counts parallel bodies. | Caps and keys in proxy.md |
| R3 | External-action boundary | Egress is checked against the charter's action classes, each with its own limits: domains, methods, volume, rate. Examples: `git push` to listed remotes, email to listed addresses, no payments. **No per-action human approval.** Anything outside a class is refused. | Invariant 5's per-action approvals |
| R4 | Lineage | An append-only, hash-chained log of every promotion, gate verdict, ladder change, panic and rollback, signed by the Root. The agent can read it but not write it. It is optionally mirrored off-machine to the owner's private repo. | The kernel-owned `lineage/` |
| R5 | Charter | The owner's file: purpose, the constitution text, money and compute ceilings, allowed external-action classes, the maximum autonomy level, and the panic channels. The agent reads it and cannot edit it. | Constitution plus scattered config |
| R6 | Sealed evaluator | Sealed tasks, a mutation suite, the gate-of-gates and the sealed audit (§16). The agent never sees the sealed contents. | Holdout and canaries in `vault/` |
| R7 | Autonomy ledger | Computes the current level (§15) mechanically from R4 and R6, and enforces that level's permissions and ceilings. It demotes on bad signals. | Static owner approval points |

## 15. The autonomy ladder

Each level is unlocked automatically by the Root from the measured track record. No human approves a step. The owner's charter caps the highest level and can lower it at any time.

| Level | What the agent gains | Unlock criteria (placeholders until Phase 1 calibrates them) |
|---|---|---|
| **L0** (Phase 0) | Evolves its body (T0, T1, T2) through the gate; adds crates without approval; reads `/opt/homebrew` | Install |
| **L1** | **Writes its own tasks** into the pool (§17); routes models per subtask within the ceiling; allocates its daily budget | ≥ 20 promotions; 0 sealed-audit regressions in 14 days; no ceiling violations |
| **L2** | **A fleet:** up to N parallel bodies; archive and HGM-style parent selection; budget share grows with value delivered (cost per accepted task), up to the charter ceiling | L1 held for ≥ 30 days; sealed-audit trend non-negative |
| **L3** | **Evolves the kernel** (gate, selfmap, proxy features, initctl, evolution policy) through the gate-of-gates (§16) | L2 held for ≥ 60 days; mutation-suite pass rate 100% |
| **L4** | **Treats the model as a component** (adapters or fine-tunes on owned infrastructure, within the ceilings); uses external-action classes up to the charter maximum | L3 held; the charter allows it |

**Demotion** is automatic and immediate: the system rolls back to the last audited-good version and drops at least one level. Any of these triggers it:
- a sealed-audit regression beyond δ_S;
- a mutation-suite failure in a new gate;
- an attempt to exceed a ceiling or a policy class;
- a crash loop on LKG;
- a panic.

Every change of level is a lineage event.

## 16. Keeping the measuring stick honest when the agent owns the gate

From L3 the agent can rewrite its own gate. Honesty then rests on three Root-held instruments that no gate version can see or change:

1. **The sealed task set S.** Seeded at install from public benchmark snapshots and maintainer tasks. Replenished only from sources outside the agent's influence, such as newly published benchmark releases fetched by the Root. Never shown to the agent zone.
2. **The mutation suite M (gate-of-gates).** Root-owned generators apply known-bad changes to the current system:
   - fake passing test logs;
   - disable or weaken a check;
   - inject a regression;
   - special-case canary inputs;
   - cache answer keys;
   - silently fall back to a broken parser.

   A candidate kernel is admitted only if its gate **rejects every mutant in M** and **accepts the known-good controls**. In R6 it must also agree with the Root's minimal reference evaluator on a sample of S.
3. **The sealed audit.** The Root runs S on the live system on a schedule (default weekly, capped in money) and on every kernel upgrade. A regression beyond δ_S triggers automatic rollback to the last audited-good system, plus demotion (§15).

The agent can make its gate better, cheaper, faster or broader. It cannot make it weaker without failing M or the sealed audit.

## 17. Agent-authored tasks (L1+)

The pool stops depending on the owner. A task the agent writes is admitted mechanically:
- **Determinism:** the check passes or fails the same way across repeats.
- **Non-triviality:** an empty or no-op answer fails it.
- **Not tailored:** it is pre-registered, so it was written before the branch point of any candidate it judges. A task never judges its own author's descendants within K generations.
- **Provenance** is recorded in lineage.

Agent-authored tasks can show that a new capability is **better**, which fixes v0.2's `no_gain` trap. They never replace S: every promotion that adds capability must also stay non-inferior on the sealed audit.

## 18. Human role after v0.3: O(1), not O(changes)

| When | What the owner does |
|---|---|
| Install | Writes the charter (purpose, ceilings, allowed external-action classes, maximum level); stores the keys with `init-root keys set`; optionally adds sealed tasks |
| Ongoing | **Nothing is required.** Optional: read the reports, raise or lower ceilings, lower the maximum level, `/panic` |
| Root upgrades | Installs a new `init-root` release from the maintainer |

Removed from the owner: code review, per-edit approval, task acceptance, crate approval, kernel approval, model routing, per-action approval of external actions.

## 19. Effect on Phase 0

- **Build `init-root` from day one**, scoped to R1, R2, R4, R5 and minimal R6 and R7. That means L0, a small S, M with the six mutant classes above, and a manual sealed audit. R3 starts with egress limited to the model providers.
- **Moves out of the v0.2 kernel docs into Root docs:** keys, money ceilings, panic, whole-system LKG and lineage. The kernel keeps routing, caching, traces, stats, the gate, selfmap, initctl and the supervision of bodies.
- **Estimate:** about +25–35 agent-hours on Phase 0, for a new `root` lane.
- **Constitution:** rewritten for the Root and the ladder (awareness.md). The settled owner decisions CONVENTIONS §6 ESC-1 (no crate approval) and ESC-2 (`/opt/homebrew` readable) become L0 rights.
- **Spec §2.1 invariants map onto the Root:**
  - Recoverability → R1.
  - Honest measuring stick → R6.
  - Blast radius → R2 and R3.
  - Lineage → R4.
  - "A human only where irreversible" → R3 policy classes, set once in the charter.

## Spec deltas

**Build sync 2026-10-20** (build → docs; ledger reconciliation/build-deviations.md §1.1):

| Build ref | This doc now says | Section |
|---|---|---|
| build: WP24 Size (orchestrator note; the §1.1 row is decide) | The built Root measures about 6,450 lines (root.md §4.8's method), against the "about 1,000–1,500 lines" estimate. The budget is under owner review (build-deviations.md §3.1). No new budget number is set. | §13 |
| build: FIX5 | `proposal.file` stores a kernel proposal in `ROOT_HOME/state/proposals/`, records `proposal{id, title, sha256, bytes}` in the lineage, alerts the owner, and is never applied. | §13 |
| build-deviations §3.1 (provisional owner decision 2026-10-20, owner may override) | Root budget ≈ 7,500 lines: per-module sub-budgets (measured + 5%, re-measured at release) in root.md §4.8, with a core subtotal of ≈ 1,770 and `root/sandbox-profiles` on its own lines; 7,085 measured at cdb49e8 (table total 7,540). Every other spec change is proposed in docs/spec-v0.4-proposal.md. | §13 |
