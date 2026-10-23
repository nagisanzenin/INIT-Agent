# Spec v0.4 proposal

```
Status: draft v0.1 · 2026-10-20 · Area: overview
Depends on: INIT Agent spec.md (v0.3), every doc's Spec deltas, reconciliation/v03-decisions.md, reconciliation/build-deviations.md, RELEASE_PLAN.md
Defines: nothing (a proposal; the owner or maintainer applies it to the spec)
```

## Purpose

`INIT Agent spec.md` is still v0.3 in text: Part I describes the v0.2 kernel, and Part II adds the Root and the ladder. Since then the docs have recorded where they differ from the spec, in their "Spec deltas" sections, the build has fed its own changes back into "Build sync 2026-10-20" blocks, the main session settled fourteen cross-doc decisions (v03-decisions D1–D14), and the orchestrator took nine provisional owner decisions on 2026-10-20 (RELEASE_PLAN.md). This file folds all of them into one proposal against the spec, section by section, with text ready to paste. It is an M6 definition-of-done item (roadmap.md M6).

## How to read

- **Item ids** are `P-<spec section>.<n>`, for example `P-13.1` for the first item against §13. `P-A` and `P-B` are the appendices, and `P-N` is a new section.
- **Source** names every row the item folds in: the doc and the row's first column (a design row by its quoted spec phrase, a build row by its `build:` reference), a v03 decision as `v03-decisions D<n>`, or a provisional decision as `RELEASE_PLAN §3.<n>`. One item can merge several sources that raise the same change.
- **Tags:**
  - `build`: from a Build sync row (the code is right, the doc changed);
  - `design`: a design-phase delta (the doc's own table or list);
  - `decision`: a v03 D-row;
  - `provisional`: a provisional owner decision of 2026-10-20. Every such item says "provisional owner decision 2026-10-20 (owner may override)".

  An item with several tags lists the tag that settles its text first; the totals count that first tag.
- **Proposed spec text** is written to be pasted. A long text is in a block under its table, named by the item id.
- **Nothing here is applied to the spec yet**, except §13's budget line, which the DOCS lane applied on 2026-10-20. P-13.1 still lists it, with the few facts the applied line lacks.
- Spec line numbers (L…) refer to `INIT Agent spec.md` as of 2026-10-20 (694 lines, with §13's budget line applied).

---

## §0 TL;DR

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-0.1 | `kernel/supervise.md` rows "v0.3 §14 R1 (stop, whole-system LKG, panic)" and "v0.3 §14 R2 keys, money ceilings, egress" | L29–35: "INIT has two parts: a **body** … and a **kernel** … It does three things only: keeps the agent alive (supervise, swap, roll back); keeps the measuring stick honest (gate, holdout, raw model traces); keeps mistakes recoverable (sandbox, lineage, the last known-good version, or LKG)." | Block P-0.1. | design |
| P-0.2 | `ext/host.md` 10.1 | L50, T1 row: "≈5 µs to instantiate" | "≈5 µs to instantiate on Linux, about 20–100 µs on macOS (estimate)" | design |
| P-0.3 | `adr/001` "§7.1 'pause at the next turn boundary' is read as the next step boundary"; `kernel/initctl.md` row "`initctl reload` reports whether the prefix changes" | "turn boundary" at L49, L50, L197, L245, L348, L372 and L491 | Replace "turn boundary" with "step boundary" at each of these lines. Add to §4's Loop row: "A step boundary follows each tool batch: no provider request is in flight, no tool is running, and every event is appended and synced. An idle body is always at one." | design |

**Block P-0.1** (replaces L29–35):

```
INIT has three parts: a **body**, which belongs to the agent; a **kernel**, which runs and measures the body; and a small, immutable **Root**, which holds what must stay outside the agent (Part II).

- **The body is a small Rust crate.** The agent can read all of it in one sitting (about 20–25k tokens) and rewrite any part, including the loop that runs it.
- **The kernel is one Rust binary**, started by the Root inside the agent zone. It never decides what the agent does. It does three things only:
  - keeps the body alive (supervise, swap, body rollback);
  - keeps the gate honest (gate, holdout, raw model traces);
  - keeps body mistakes recoverable (sandbox profiles, lineage objects, the body LKG).

  The maintainer provides it at L0–L2. From L3 it is the agent's to evolve, through the gate-of-gates (§16).
- **The Root is a second binary, `init-root`.** It runs as the owner's OS user, outside every sandbox, and the agent can never modify it. It holds stop, panic and whole-system rollback, the keys and hard ceilings, the external-action boundary, the signed lineage, the charter, the sealed evaluator and the autonomy ladder (§14).
```

## §2 Constitution → enforceable design rules

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-2.1 | `awareness.md` row "v0.2 constitution (not in spec): Article 0 'Survival' and five invariants" | L104: "**Survival** (new) \| The agent can always be restored, and its measurements cannot be faked. The five invariants (§2.1) are the whole of governance." | "**Survival** (new) \| The agent can always be stopped and restored, and its measurements cannot be faked. The Root holds what guarantees this (§13–§16), and the agent can never modify it, at any level of autonomy. The five invariants (§2.1) map onto the Root (§19)." | design |
| P-2.2 | `kernel/supervise.md` row "v0.3 §14 R1 (stop, whole-system LKG, panic)"; `root/root.md` row "§13, R1: the Root 'starts the agent zone'" | L112: "**Recoverability.** `kernel/` is read-only to the agent at the OS level. The kernel owns the LKG pointer, rollback and the panic stop." | "**Recoverability.** The Root (R1) owns the whole-system LKG, rollback and the panic stop, outside every sandbox. The kernel keeps the body LKG and body rollback below it. `kernel/` is read-only to the body at the OS level; from L3 the agent changes the kernel only through the gate-of-gates (§16)." | design |
| P-2.3 | `kernel/gate.md` row "v0.2 holdout and canaries in `vault/`, hidden"; `kernel/proxy.md` row "v0.2 §3.14 `vis: kernel` hides holdout traces"; `kernel/sandbox.md` row "(not in spec) every gate activity … runs in its own sandbox instance" | L113: "**An honest measuring stick.** The gate code, holdout tasks, canaries and raw model traces sit in `vault/`, which the agent can neither read nor write. Evaluation runs in a fresh process outside the agent's sandbox." | "**An honest measuring stick.** The Root's sealed set S, mutation suite M and sealed audit (R6) are never visible to the agent zone. The gate's own holdout, canaries and raw model traces sit in `vault/`: hidden from the body at L0–L2, and reachable by an L3 agent through the kernel it may evolve. Every gate activity (fetch, build, probe, task run, check) runs in its own fresh sandbox instance." | design |
| P-2.4 | `kernel/proxy.md` rows "§7.2 'injects keys'" and "§3 'allowlisted egress via proxy'"; `adr/002` row "§3 'allowlisted egress via proxy'" | L114: "API keys exist only in the kernel's model proxy, never inside the sandbox. Outbound traffic goes through an allowlist." | "**Blast radius.** A filesystem and network sandbox. API keys exist only in the Root (R2), never in the agent zone. The agent zone's only network path is the Root's egress socket, which forwards only what the charter's action classes allow (R3)." | design |
| P-2.5 | `kernel/supervise.md` row "v0.3 §14 R4 lineage"; `kernel/gate.md` row "v0.2 §3.13 gate record in `vault/lineage.jsonl`" | L115: "**Lineage.** … The kernel attaches gate results as `git notes`, which the agent cannot write. Everything is mirrored to an append-only event log." | "**Lineage.** Every self-edit is a git commit made under the agent's identity. The kernel attaches gate results as `git notes`, which the agent cannot write. Every promotion, gate verdict, ladder change, panic and rollback is also recorded in the Root's signed, hash-chained lineage (R4), and the session log is append-only." | design |
| P-2.6 | v03-decisions D4; `awareness.md` row "v0.2 constitution …" ("no human reviews your code, tasks or changes"); `kernel/supervise.md` row "v0.2 owner approvals"; `kernel/initctl.md` row "v0.2 owner lane (`run/owner.sock`)" | L116: "**A human only where actions are irreversible.** The owner approves any action that leaves the sandbox … and any edit to `kernel/` or `vault/`. Nothing else needs approval." | "**Irreversible actions are bounded by the charter.** An action that leaves the sandbox (sending, paying, pushing to a shared remote, deleting outside the sandbox) must fit one of the charter's action classes, and the Root refuses anything else (R3). No human approves individual actions, edits, tasks, crates or kernel changes, and no owner step is required anywhere (D4)." | decision, design |

## §3 Architecture

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-3.1 | `adr/002` row "§3 diagram shows `init-kernel proxy` as a subcommand"; `kernel/proxy.md` row "v0.2 `init-kernel proxy --standalone` … removed"; `ext/wit.md` row "§3 and §6.7: `kernel/wit/init-ext@1.wit`"; `kernel/sandbox.md` row "§3 'Landlock+seccomp+bwrap on Linux'"; `kernel/supervise.md` row "§3 diagram: `state/events.jsonl`, `kernel/{lkg,boot,update-failed}.json`"; `body/log-context.md` D1; `CONVENTIONS.md` build: WP0 D4, WP5 Done | L129–159, the Part I diagram | Block P-3.1. (Conflict C1.) | design |
| P-3.2 | `kernel/proxy.md` rows "§7.2 'injects keys' …", "§7.2 'Reverse proxy … persistent upstream connections'" and "§3 'allowlisted egress via proxy'"; `adr/002` row "§3 'allowlisted egress via proxy'" | L164–169: "**API keys sit behind the proxy.** This one boundary gives: credentials that never enter the sandbox; a spend cap the agent cannot bypass …; raw traces …; persistent upstream connections that survive body restarts …; a record of which tools the body actually exposes …" | "**API keys sit in the Root's egress gateway (R2), below the kernel's model proxy.** The proxy forwards only model calls, to providers on per-provider route allowlists. It is not a general egress proxy, because a key-holding general proxy would widen the blast radius. Together the two layers give: credentials that never enter the agent zone; hard ceilings the agent cannot bypass (Ouroboros), under soft budgets it can tune; raw traces the agent cannot edit (Meta-Harness found raw traces beat summaries); **persistent upstream connections, held by the Root, that survive body and kernel restarts**, so a restart pays no TLS handshake and no reconnect; and a record of which tools the body actually exposes, as seen by the proxy, which the gate cross-checks." | design |
| P-3.3 | `CONVENTIONS.md` build: WP0 D4, WP5 Done; `kernel/proxy.md` row "§7.2 'Reverse proxy … persistent upstream connections'" | L170: "**TLS lives only in the kernel.** The body needs only `std` to talk to the proxy over a Unix socket." | "**TLS lives only in the Root** (ureq with rustls, in the egress gateway). The kernel's proxy speaks local HTTP/1.1 to the Root's egress socket, and the body needs only `std` to talk to the proxy over a Unix socket." | build, design |

**Block P-3.1** (a paragraph above the diagram, then label changes inside it):

```
Part II (§13) supersedes this picture's trust boundaries: the Root, outside every sandbox, spawns the kernel and every body, and holds the keys, TLS, panic and the hard ceilings. The proxy is an in-process module of `init-kernel supervise`, with its own runtime, so it shares the panic flag and the lane table with supervision. There is no standalone proxy process; kernel-only tests run the real kernel under `init-fakeroot`.
```

Label changes in the diagram:
- `init-kernel supervise  start · overlapped swap · rollback · backoff · safe mode · /panic` → `init-kernel supervise  start · overlapped swap · body rollback · backoff · body safe mode`
- `init-kernel proxy      keys · persistent upstream conns · spend cap · raw traces (prefix-dedup) · cache-hit metrics` → `proxy (in-process)     routing · soft budgets · raw traces (prefix-dedup) · cache-hit metrics`
- `wit/init-ext@1.wit` → `wit/init-ext/v1/init-ext.wit` (one directory per major; the body keeps a byte-identical copy in `body/wit/`)
- `holdout/ canaries/ traces/ lineage.jsonl keys` → `holdout/ canaries/ traces/ lineage.jsonl`
- `Seatbelt on macOS · Landlock+seccomp+bwrap on Linux` → `Seatbelt on macOS · bwrap + Landlock + seccomp on Linux`
- `events.jsonl  tasks/  kernel/{lkg,boot,update-failed}.json (ro mount)` → `sessions/<session_id>.jsonl  tasks/  kernel/*.json (read-only)`
- `network: none except the two sockets (+ allowlisted egress via proxy)` → `network: none except the two kernel sockets`

## §4 The superb basic elements (seed body)

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-4.1 | `body/log-context.md` D9; `ext/host.md` 10.2, 10.3 | L182–183: "Std threads, no async runtime at seed. …" and "Only two dependencies at seed, both vendored: `serde_json` and `wasmtime`." | "- Std threads, no async runtime in body code. Sessions average about 1.2 tool calls per step and an OS thread costs about 14 KB, so tokio would only add build time and more places for the LLM to make errors. `wasmtime-wasi` starts its own tokio runtime for WASI I/O, and Wasmtime compiles on a rayon pool; both are accepted as internals of allowed dependencies.<br>- Three dependencies at seed, all vendored: `serde`, `serde_json` (never with `preserve_order`), and `wasmtime` with `wasmtime-wasi`, pinned at `=48.0.6`, the LTS release with security support until about 2028-08. SHA-256 is in-house (`src/sha256.rs`)." | design |
| P-4.2 | `ext/authoring.md` row "§4 '`#![forbid(unsafe_code)]` in the body'" | L181: "`#![forbid(unsafe_code)]` in the body, so that 'the agent cannot corrupt its own memory' is checked by the compiler." | "`#![forbid(unsafe_code)]` in every crate of the body workspace, so that 'the agent cannot corrupt its own memory' is checked by the compiler. Extension crates are exempt, because they run inside Wasm and wit-bindgen's `export!` needs `unsafe` attributes. They must be `cdylib`-only, and nothing in the body workspace may depend on them." | design |
| P-4.3 | `body/provider.md` items 1 (interface), 2 (events), 3 (thinking), 4 (adapter set); `body/provider.md` build: WP10 D2; `body/loop.md` build: WP11 D1 | L188, Provider row: "One narrow function, `stream(&Context) -> impl Iterator<Item = Event>`, emitting `{text, thinking, tool_call, usage, stop_reason}`. Adapters for Anthropic Messages and OpenAI Responses. … Drops thinking blocks from other providers when switching providers." | "**Provider** (`provider/`) \| A dyn-compatible `Provider` trait, chosen at runtime from `boot.json`, with pure request-building and decoding functions that are tested from fixtures. The loop hands it a byte-based request that the context module already rendered, and `stream(&dyn Provider, &Transport, HttpRequest, Cx, &Abort) -> EventStream` sends it unchanged. Events: `Started`, text, thinking, tool calls, usage, `Retry`, `Failed`, and a terminal `Done(AssistantMessage)` that carries the stop reason. Adapters: DeepSeek Chat Completions (first), Anthropic Messages, OpenAI Responses. Speaks HTTP/1.1 with SSE to the proxy socket. Thinking is dropped when the model changes, and Anthropic and OpenAI thinking from before a context epoch is dropped too. DeepSeek reasoning from the same model is never dropped, because DeepSeek returns 400 without it when tools are present." | design, build |
| P-4.4 | `body/loop.md` items 1 (continue conditions), 2 (length stop), 3 (same-path serialization), 4 (steering order), 5 (no turn cap) | L189, Loop row: "Runs `while tool_calls or pending_steer`. Executes tools in parallel … Stops when the model makes no tool call. When `stop_reason=length`, returns an error to every pending call. Drains the steering queue after each tool batch. On abort, keeps the partial output." | "**Loop** (`loop.rs`) \| Runs while there are tool calls, a queued steer or follow-up, or a length stop with no tool calls (at most 3 in a row). Executes tools in parallel with `std::thread::scope`, serializes `write` and `edit` calls on the same path within a batch, and keeps results in their original order. When `stop_reason=length`, returns an error to every pending call and doubles `max_tokens` for the next request. Drains the steering queue after each tool batch, inside one fixed boundary order that also places reloads, notices, the pause check and self-signals. On abort, keeps the partial output. There is no turn cap: the budget governs, and only no-progress guards end a run on their own." | design |
| P-4.5 | `body/tools.md` item 3 | L190, read row: "Images become image blocks." | "`{path, offset?, limit?}` with line numbers. Stops at 2,000 lines and returns a continuation hint. Images become image blocks without resizing, because std cannot decode them; an oversized image returns a `too_large` error with a downscale hint. Runs in-process with no subprocess." | design |
| P-4.6 | `body/tools.md` item 2 | L192, edit row: "Falls back to matching that tolerates whitespace and line-ending differences." | "Falls back through two tolerant tiers: trailing whitespace and punctuation, then a uniform indent difference with re-indentation. Line endings are normalized before every tier. Returns a diff." | design |
| P-4.7 | `body/tools.md` items 1 (head + tail), 4 (ANSI); `body/tools.md` build: WP11 D11 | L193, bash row: "Output is tail-truncated at about 30k characters and the full output is written to a file whose path is returned." | "Output over 30,000 bytes keeps its first 4 KB and last 26 KB, because the first compiler error is at the top. ANSI sequences are stripped, `\r` overwrites are collapsed and CRLF becomes LF in the model's text; the full raw output is written to a file whose path is returned." | design, build |
| P-4.8 | `body/tools.md` item 5; `ext/wit.md` row "§4 'this tool's schema never changes'" | L194, ext row: "This tool's schema never changes, so adding an extension does **not** invalidate the prompt cache." | Add after that sentence: "The model learns extension schemas from reload reports, `state/ext/catalog.json` (written by the extension host), `self.json` and the unknown-name error, which lists the loaded names." | design |
| P-4.9 | `body/log-context.md` D4, D5, D8 | L196, Context row: "(3) an anchored, sectioned summary …, merged incrementally when the context reaches the window minus 16k." | "(3) an anchored, sectioned summary (intent / files changed / decisions / next steps / open errors / artifact index), merged incrementally. Masking and compaction run only as batched epochs at one trigger, masking first, so the cache misses once per epoch rather than on every step. The trigger is `T_trigger = min(W_eff − 16,384, window − max_out − 1,024)` with `W_eff = min(window, 200k)`, so a 1M window does not wait until 984k. The head hash `prefix_hash` is SHA-256 over the ICJ of `{provider, model, params, system, tools}` without cache markers; the proxy recomputes it." | design |
| P-4.10 | `ext/host.md` 10.1, 10.2, 10.4, 10.5, 10.8, 10.9; `ext/wit.md` row "Spec is silent on what T1 can reach" | L197, Extension host row | "**Extension host** (`ext_host/`) \| Wasmtime `=48.0.6` (the LTS release) with Cranelift (not Winch, CVE-2026-34987), the pooling allocator (32 instances), `InstancePre`, epoch deadlines, `StoreLimits` and deny-by-default WASI 0.2 (`wasm32-wasip2`). A guest sees `/work` (the task workspace), its own `/data`, and host-mediated processes through `init:ext/process` (run and pipe sessions, no PTY); it has no network. Swapping an extension means building a new `InstancePre` off the hot path and swapping a pointer at a step boundary. A call still running finishes on the old version; a call blocked in a host call, which epochs cannot interrupt, is abandoned by a watchdog. An owner abort stops a guest within one 10 ms epoch tick and a blocked host call within 200 ms. \| Instantiation in ≈5 µs on Linux and about 20–100 µs on macOS (estimate), where copy-on-write memory init needs `unsafe` and pooling only reuses slots. Zed and Wassette use this design. A dropped `Store` frees its memory, so there is no reload leak." | design |
| P-4.11 | `body/boot.md` items 1 (probe only), 2 (rollback check), 3 (probe shape), 4 (safe mode, exit sub-codes); `body/boot.md` build: WP13 D1; `testing.md` row "v0.2 §4 'boot does one real model round trip …'" | L198, Boot row: "Runs the rollback check before anything that can fail. Then reads `boot.json` and `update-failed.json`, rehydrates from `events.jsonl`, appends a `boot` event, does one real model round trip and one tool call, writes `ready`, and runs." | "**Boot** (`main.rs`) \| The kernel performs any rollback before the spawn; the body reads its outcome (`boot.json`, including `update_failed`) before any fallible step (lock, log, network). Then it rehydrates from the session log, appends a `boot` event, writes `ready`, and runs. Only a probe-mode boot (G2, G3) makes a real model round trip with one tool call: two requests over the full projected conversation, the second carrying the tool result. Normal and safe boots, including handoffs and crash restarts, skip it: a handoff runs on the live log, G2 has just probed the same binary, and the first real turn detects a broken provider just as well. Safe mode runs with no extensions, T0 from the LKG tree and no automatic continuation." (Conflict C2.) | design, build |
| P-4.12 | `kernel/selfmap.md` row "§5 'token size' (no measure named)" | L199, Budget row: "Whole body ≤ about 25k tokens, so it can be read in one context." | "Whole body ≤ about 25k tokens, counted with `o200k_base` (tiktoken-rs) whatever the model, so the trend and the tie-break do not move when the model changes. It can be read in one context." | design |
| P-4.13 | `ext/authoring.md` row "§4, §11: 'a Luau or QuickJS lane'"; `ext/authoring.md` build: WP14 Measurements | L211: "A Luau or QuickJS lane for quick glue code, in case Wasm build latency turns out too high (§11)." | "A script lane for quick glue code: a JavaScript engine inside a Wasm component (QuickJS, or boa where there is no C toolchain), loaded like any extension, so it adds no body dependency and keeps the same sandbox and limits. It is added only if criterion C1, C2 or C3 triggers (§10). A warm component rebuild measured 0.34–0.39 s (M-2), so it is not needed now." | design, build |

## §5 Awareness system

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-5.1 | `awareness.md` rows "v0.3 §14 R5 'the charter … the constitution text'" and "v0.2 §5 'the prompt' as an unspecified file"; `CONVENTIONS.md` build: WP13 D4, WP20 Done, WP12 D13 | L223: "Who am I, and what are my rules? \| Genesis prompt (App. A) and `kernel/CONSTITUTION.md` \| maintainer \| always (cached prefix)" | "Who am I, and what are my rules? \| The genesis prompt `body/GENESIS.md` (App. A), the constitution (`kernel/CONSTITUTION.md`) and the charter's purpose text \| genesis: seed text, agent-editable T0; constitution and purpose: the owner's charter, installed by the Root (R5) and checked against its published digest \| always (cached head; the purpose block follows the constitution)" (Conflict C3.) | design, build |
| P-5.2 | `awareness.md` row "v0.2 §5: `SELF.md` content outline" | L224: "`body/SELF.md`, ≤100 lines: purpose, layer boundaries, absences …, where things live by name" | "`body/SELF.md`, ≤ 100 lines: purpose, layer boundaries, absences ('body never links kernel'), where things live, how to change and check the body, how to read its own record (including masked and compacted output), and the Root's role" | design |
| P-5.3 | v03-decisions D12; `kernel/selfmap.md` rows "§5 `self.json` includes 'SHA, LKG'", "§5 'hooks'", "v0.2 `self_v` 1", "spec §5 'SHA, LKG' (v0.2: moved out of `self.json`)", "(new) `live.level` and `live.ladder`", "(new) kernel inventory", "(new) capability self-model", "v0.2 holdout redaction"; `ext/wit.md` row "'one versioned WIT world for tools and hooks'"; `awareness.md` row "v0.2 §5 'What exactly am I made of?'"; `awareness.md` build: WP13 D7 (capability recipe) | L225: "`body/self.json`: modules, token size, tools and schemas, hooks, **T1 exports derived from each component's WIT** …, skills, SHA, LKG, `Compensates-for` tags \| **kernel** `selfmap`, regenerated at every gate; any drift fails G1" | "What exactly am I made of? \| `body/self.json` (committed, derived from the tree): modules, token size, tools and schemas, **T1 exports derived from each component's WIT** (from the binary, not from the agent's claims), skills, `source_digest`, `Compensates-for` markers; hooks stay empty in WIT 1.x. The kernel also serves a live copy, `state/kernel/self.json`, whose `live` section adds the running and LKG shas, the level and ladder copied from the Root, the kernel inventory, and the capability self-model: per-category pass rates of the LKG with 95% intervals, with holdout, canary and sealed results excluded \| **kernel** `selfmap`; G1 regenerates the committed file at every gate and compares committed fields only \| on demand (`cat`/`jq`)" | decision, design |
| P-5.4 | `awareness.md` row "v0.2 §5: environment snapshot 'in the tail of the first turn'"; `body/log-context.md` D6; `awareness.md` build: WP13 D7 (full environment text on a first boot) | L226: "… \| kernel, at boot \| in the tail of the first turn" | "… \| kernel, at boot \| in the boot block, before the first owner message: in full on a session's first boot and whenever the environment changes, short otherwise; pinned as the prelude after compaction, so kernel truth never passes through the summarizer" | design |
| P-5.5 | `awareness.md` row "v0.2 §5: signals = context %, spend, cache, uptime, restarts"; `awareness.md` build: WP13 D7 (a stats failure keeps the ceiling) | L228: "Self-signals after each tool batch: context used %, spend today, **cache hit rate**, uptime, restarts since LKG \| kernel proxy → loop" | "Self-signals after each tool batch, on one fixed line: the level, context used %, spend today against the soft day budget and the Root's day ceiling, **cache hit rate** (last request and today), uptime, restarts since LKG, plus `provisional` while it is true and a compaction hint near the trigger. An unknown value renders as `?`; the day ceiling comes from the boot block, so it survives a stats failure \| kernel proxy and Root → loop" | design |
| P-5.6 | `body/log-context.md` D10 | L235: "**State is JSON with sorted keys.** It is deterministic, and models are less likely to clobber JSON than Markdown." | "**State, events and prompts use INIT canonical JSON (ICJ v1):** sorted keys and integers only (money in micro-USD, ratios in permille, durations in ms). It is deterministic, floats cannot break byte stability, and models are less likely to clobber JSON than Markdown." | design |

## §6 Self-evolution protocol

### §6.1 Three tiers

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-6.1.1 | `evolution.md` row "v0.2 §6.1 T1 'G0–G3 to keep' vs §6.3 'any code change takes G0–G4'"; `ext/authoring.md` row "§6.1: `cargo build --target wasm32-wasip2`"; `ext/authoring.md` build: WP0 D2, WP14 Measurements; `ext/host.md` 10.6; `kernel/supervise.md` row "(not in spec) kept T1 components published by the kernel" | L246, T1 row: "`cargo build --target wasm32-wasip2`, then `initctl reload`, then the `InstancePre` swap \| none to try *within a task*; G0–G3 to keep \| build time is measured in Phase 0 (target ≤5 s warm); instantiation ≈5 µs" | "\| **T1** leaf \| Wasm components in `ext/` \| `cargo build --target wasm32-wasip2` in `body/ext/<name>/` (one extension workspace, with its target dir outside it), then `initctl reload`, then the `InstancePre` swap. A trial loads from `work/target/`. A kept component is rebuilt by the gate and published by the kernel as an immutable set `bin/ext.d/<set>/`, swapped together with the body binary; the kernel never trusts an agent-built artifact \| none to try *within a task*; G0–G3 to become provisional, G4 to become LKG \| a warm rebuild of a small component measured 0.34–0.39 s (M-2; target ≤ 5 s); instantiation ≈5 µs \|" | design, build |

### §6.2 Per-task flow

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-6.2.1 | `evolution.md` row "(none) task = the run started by one input" | L261: "1. Receive the task and append it to the log." | "1. Receive the task and append it to the log. A task is the run started by one owner input, and its id is that input's 16-hex id, chosen by the body; caps, predictions and retirement all use it." | design |
| P-6.2.2 | `kernel/initctl.md` row "`initctl promote`" (non-blocking); `awareness.md` row "v0.2 App. A 'Your kernel …'" (the genesis says "If it fails, you continue here") | L266: "If they fail, the old body simply carries on and receives the failure report as a tool result." | "Promotion is non-blocking: `initctl promote` returns at once (`--wait` returns at the handoff boundary), because a tool call that waited for its own handoff would deadlock. If the gate fails, the old body carries on, and the failure arrives as a kernel notice at the next step boundary." | design |
| P-6.2.3 | v03-decisions D4; `evolution.md` row "v0.2 §6.2 step 5 'if the owner accepts the result …'"; `kernel/supervise.md` row "§6.2 step 5 owner accepts the result"; `kernel/gate.md` rows "§6.3 G4 'K tasks from state/tasks'" and "v0.2 pool frozen 'at owner acceptance'" | L267: "5. Finish the task. If the owner accepts the result, record it as `state/tasks/<id>/` …, where it joins the regression pool." | "5. Finish the task. No owner acceptance is needed. The regression pool grows in three ways: tasks the maintainer or owner imports, an optional owner `/accept <task_id>`, and, from L1, agent-authored task families admitted mechanically and pre-registered with the Root (§17). The kernel freezes its own copy of each admitted task (prompt, workspace snapshot, deterministic check), because the agent can rewrite `state/tasks/`." | decision, design |
| P-6.2.4 | `evolution.md` rows "v0.2 §6.2 single provisional version implied" and "v0.2 §6.2 'the kernel rolls back' (implying it resets the agent's repo)"; `kernel/supervise.md` row "§6.2 step 6 rollback 'at the next idle point'" | L268: "6. G4 for a provisional version runs asynchronously. If it fails, the kernel rolls back to LKG at the next idle point and writes `update-failed.json`. A provisional version may not take external actions." | "6. G4 for a provisional version runs asynchronously. Provisional versions may chain up to a depth of 3, and G4 judges the head of the chain against LKG. If G4 fails, the kernel rolls back to LKG at the next idle point, or at a step boundary after 3,600 s if the body never idles: it moves lineage `main` to the LKG sha and writes `update-failed.json`, and the body resyncs `body/` at its next boot. The kernel never runs git inside `body/`. A provisional version may not take external actions." | design |

### §6.3 The gate

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-6.3.1 | `kernel/gate.md` rows "§6.3 G0 '`cargo check`, `cargo clippy -D warnings`, `cargo test`'" and "§6.3 `Cargo.lock`/`vendor/` 'forces full G4'"; `kernel/supervise.md` row "§6.3 'a change to `Cargo.lock` or `vendor/` forces full G4'" | L274, G0 row | "\| G0 compiler \| `cargo check`, `cargo clippy -D warnings`, `cargo test`; `#![forbid(unsafe_code)]` unchanged; a build-config policy (no wrappers, no lint capping, no aliases that shadow G0's commands) and a workspace-shape check. A change to `Cargo.lock` or `vendor/` makes the scope `dep`: its G4 runs synchronously before the handoff and commits straight to LKG, so new third-party code gets no provisional fast lane \| type, borrow and lifetime errors; unsafe creep; dependency drift; a committed build config that bypasses the compiler gate \|" | design |
| P-6.3.2 | v03-decisions D12; `kernel/selfmap.md` rows "(new) `generator.version`" and "v0.2 `self_v` 1, one committed file" | L275, G1 row: "the kernel regenerates `self.json` (source, binary and WIT); the diff against the committed file must be empty" | "the kernel regenerates the committed `self.json` (source, binary and WIT); the diff against the committed file must be empty, comparing committed fields only (pass rates and the level live in the kernel's live copy and are never committed). A kernel upgrade that changes the generator's output fails G1 as `generator_changed` until the file is regenerated" | decision, design |
| P-6.3.3 | `kernel/gate.md` row "§6.3 G2 'prefix hash must equal the parent's'"; `body/boot.md` item 3; `body/boot.md` build: WP13 D1 | L276, G2 row | "\| G2 boot probe \| the candidate and its parent each boot in a fresh sandbox and write `ready` within 90 s, after a probe-mode tool round trip (two model requests). The proxy confirms that the exposed tool list matches `self.json`. **The prefix hash must equal the parent's**, measured on the parent's own probe, unless the commit declares `Prefix-change: yes`. A continuity check and a boot-honesty check run on the same probes \| bricking; inventory lies; accidental cache breakage; message-rendering drift \|" | design, build |
| P-6.3.4 | `kernel/gate.md` rows "§6.3 G4 'K tasks from state/tasks'", "v0.2 holdout and canaries in `vault/`, hidden" and "v0.2 G4 'same model'" | L278, G4 row: "K tasks from `state/tasks`, H hidden holdout tasks and canaries; the kernel runs the acceptance checks and records **score, $ cost, wall-clock and cache hit rate**" | "K tasks from the kernel's frozen pool copy, H holdout tasks and canaries (hidden from the body at L0–L2, visible from L3). Both arms use the same default model and the same routing snapshot. The kernel runs the acceptance checks and records **score, $ cost (at reference prices), wall-clock and cache hit rate**" | design |
| P-6.3.5 | `kernel/gate.md` rows "§6.3 '≥ parent − δ'" and "§6.3 'δ is the measured noise floor'"; `evolution.md` build: WP8 D12 ("strict" perf) | L281, L283: "Non-inferior on score (≥ parent − δ, where δ is the measured noise floor) on both the pool and the holdout." and "A change that claims only to improve performance must show a gain in cost or time beyond the noise floor while staying non-inferior on score." | "- Non-inferior on score on both the pool and the holdout: a one-sided cluster-bootstrap test with margin m = max(2δ, 0.5/n) at α = 0.05, plus point-estimate split guards on each set. δ is measured per metric from sign-flip splits of the LKG arm's repeats, at no extra cost, and smoothed across gates.<br>- A change that claims only to improve performance must show a strict gain beyond the noise floor (lower cost, time, turns or tokens, or a higher cache hit rate) while staying non-inferior on score." | design, build |
| P-6.3.6 | v03-decisions D14; `kernel/gate.md` row "(new) divergence guard"; `kernel/gate.md` build: WP8c F1, FIX2 (D14), FIX2 (follow-up) | after L281 (new) | "- A pool effect that exceeds the holdout effect by more than 2·√(δ_p'² + δ_h'²) is rejected as `pool_holdout_divergence`. Here each set's δ is floored at half a task, δ' = max(δ, 0.5/n_set), because a deterministic δ of 0 is not zero true noise. The split guards and the margin keep the measured δ.<br>- A score-gain accept (`better`, `capability_gain`, `repro_fixed`) on a holdout of fewer than 4 tasks becomes `inconclusive: underpowered:holdout`. `GAIN_MIN_HOLDOUT = 4` is a constant, not a config key, so no configuration can tune it away." | decision, build |
| P-6.3.7 | v03-decisions D2; `kernel/gate.md` rows "v0.2 §3.9: a capability the pool does not exercise is rejected `no_gain`", "(new) §3.9.2 … `audit_required`", "§6.3 accept rule adds `repro_fixed` …", "v0.2 `repro_fixed` on owner-accepted frozen tasks"; `kernel/gate.md` build: WP8 D9 | after L283 (new) | "- **`capability_gain`:** a capability the pool does not exercise is accepted when, after passing non-inferiority as before, it shows a significant gain (family-clustered bootstrap) on the sealed halves of at least 4 eligible agent-authored task families named by a pre-registered `Predicts: … label:` claim (§17).<br>- **`repro_fixed`:** a pre-registered fix that flips its frozen repro task is accepted; on agent-authored tasks only sealed instances count.<br>- **`audit_required`:** a score-gain accept whose gain rests only on agent-authored tasks becomes LKG with this flag, and the Root's next sealed audit confirms it or rolls the system back and demotes. Agent tasks never replace S." | decision, design, build |
| P-6.3.8 | `kernel/gate.md` rows "v0.2 `needs_owner:cost_increase`" and "§6.3 accept rule adds … `needs_owner` when a better candidate costs > 1.5×"; `evolution.md` row "v0.2 gate policy: inconclusive G4 and better-but-costlier go to the owner"; `kernel/gate.md` build: WP8b D8 | (new, after L282's tie rule) | "- A better but costlier candidate is accepted only if the projected task spend at its cost fits the task part of the soft day budget (`cost_guard`); otherwise it is `reject: costlier_than_budget`. Until a 30-day task-rate source exists, the projection assumes 10 tasks a day." | design, build |
| P-6.3.9 | v03-decisions D4; `kernel/gate.md` rows "v0.2 §3.12 inconclusive waits for the owner's `/approve`/`/deny`" and "(new) inconclusive outcomes keep the version provisional and ask the owner"; `evolution.md` row "v0.2 gate policy: inconclusive G4 …"; `testing.md` row "v0.2 A3's runner blessed an inconclusive G4" | (new, after L284) | "- An inconclusive G4 gets one escalated re-run (R = 4). If it is still inconclusive, the incumbent wins (`reject: inconclusive_final`). A version stays provisional for at most 24 h. No owner decision is ever required; `/bless` and `/reject` are optional signals." (Conflict C9.) | decision, design |
| P-6.3.10 | `kernel/gate.md` row "v0.2 gate is maintainer code" | L288–290, Scope | Add: "- From L3, a kernel candidate takes K0–K3 on the running gate (scope `k`), then the Root's gate-of-gates (§16). A candidate gate never judges itself, and every gate version keeps the `gate judge` contract the mutation suite uses." | design |
| P-6.3.11 | RELEASE_PLAN §3.2 (build-deviations.md §3.2); `kernel/gate.md` build-deviations §3.2 row; `roadmap.md` build-deviations §3.1, §3.2 row | L278 G4 row and L281 (sample size, implicit) | "- **Sample size.** Once S holds at least 30 tasks plus the canary (M7), G4 defaults to n = 30 tasks (18 pool + 12 holdout, which are also the minimums) with R = 3 repeats per arm, at most 4, plus one A/A calibration job per model before that model's first real gate. Only that shape meets the error targets (≥ 90% accept at a true difference of 0, ≤ 7% at −2δ). Until then v0.1 keeps the current defaults (8 pool + 4 holdout, R = 2) and the 12-task stand-in S. This is a provisional owner decision of 2026-10-20 (owner may override)." | provisional |

### §6.4 Commit trailers

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-6.4.1 | `evolution.md` rows "v0.2 §6.4 `Predicts:` is prose", "v0.2 §6.4 `Repro:` 'required for any new rule or guardrail'" and "v0.2 §6.4 five trailers" | L296–307: the example commit and the two bullets | Block P-6.4.1. | design |

**Block P-6.4.1** (replaces L296–307):

````
```
feat(ext): add pty component for interactive programs

The bash tool cannot drive programs that check isatty(); task 2026-10-20-03
hung for 600 s on an interactive REPL.

Why: task 2026-10-20-03 hung on an interactive REPL (events e4f1a2b3c4d5e6f7).
Predicts: pass_rate@label:interactive > parent; gap_ms_p50@label:interactive <= parent-30%
Compensates-for: none (new capability)
Prefix-change: no
Repro: task:2026-10-20-03
```

- The subject is typed: `feat`, `fix`, `perf`, `guard`, `refactor`, `skill`, `retire`, `thin`, `upstream` or `revert`.
- `Predicts:` must be falsifiable, so it is one to five clauses in a grammar the kernel parses: a metric, an optional scope, an operator and a target, or a `fixes`/`risks` task set. The kernel scores it at G4 and on later tasks (AHE).
- `Repro:` is **required for `fix` and `guard`, and for any commit with `Prefix-change: yes` except removals.** No observed failure, no rule (Phantom Guardrails). "Is this a rule?" cannot be checked; "does it grow the head?" can.
- Further trailers carry machine-readable intent: `Retires`, `Thins`, `Upstream` and `Reverts`.
````

### §6.6 Thinning on model upgrade

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-6.6.1 | `evolution.md` row "v0.2 §6.6 one removal commit per component"; `kernel/selfmap.md` row "§6.6 'Every scaffold carries a `Compensates-for:` tag' (location unspecified)" | L319: "Each scaffold's `Compensates-for:` tag is recorded in `self.json`." and L322: "3. The agent proposes one removal commit per component." | L319: "Each scaffold names its weakness in an in-code `@init compensates-for` marker (doc comment, front matter or HTML comment; one slug per component), so the tag lives and dies with the code. `self.json` lists the markers, and the kernel cross-checks them against each commit's `Compensates-for:` trailer." L322: "3. The agent first proposes one batch removal of all listed components; if it fails the gate, one removal commit per component." | design |

### §6.7 Upstream

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-6.7.1 | `evolution.md` row "v0.2 §6.7 'upstream git remote'" | L329: "**Seed versions:** the maintainer publishes them as the `upstream` git remote, and new versions are announced in the `boot` event." | "**Seed versions:** the maintainer publishes them as tagged, deterministic `git subtree split`s of `seed/`. The sandbox has no network, so the kernel mirrors them into `lineage/seed.git` and copies each tag into lineage as `refs/upstream/<tag>`, never force-updated. The body sees the mirror as its read-only `upstream` remote, and new versions are announced in the `boot` event." (rest of the bullet unchanged) | design |
| P-6.7.2 | `evolution.md` row "v0.2 §6.7 'kernel and WIT updated by the maintainer only'"; `ext/wit.md` rows "§3 and §6.7: `kernel/wit/init-ext@1.wit`" and "§6.7 'kernel supports N and N−1'" | L330: "**Kernel and the WIT contract:** these are updated by the maintainer only. The WIT world is versioned (`init-ext@1`, then `@2`), and the kernel supports N and N−1 during migration …" | "**Kernel and the WIT contract:** the maintainer ships them at L0–L2. From L3 the agent evolves the kernel too, through the kernel gate, the gate-of-gates, a whole-system swap and a sealed audit, and maintainer kernel releases take the same path (§15, §16). The WIT world is versioned with one directory per major (`kernel/wit/init-ext/v1/init-ext.wit`, then `v2/`), and the body keeps a byte-identical copy. Minor versions are additive only, records and interfaces are frozen within a major, the body supports majors N and N−1, and support for N ends when N+2 ships." | design |

### §6.8 Later, not at seed: open-ended search

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-6.8.1 | `evolution.md` row "v0.2 §6.8 open-ended search 'off until Phase 3, cost-capped by the kernel'"; `roadmap.md` row "v0.2 §10 Phase 3 'open-ended search'" | L338: "It stays off until Phase 3 and is cost-capped by the kernel." | "It is unlocked by the Root's ledger at L2 (§15), not by a phase. It runs up to the charter's `quotas.parallel_bodies` bodies, within the agent's campaign budget and the charter's ceilings, with still one pipeline, one build slot and one handoff at a time." | design |

## §7 Kernel spec

### §7.1 `supervise`

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-7.1.1 | `kernel/supervise.md` rows "v0.3 §14 R1 (stop, whole-system LKG, panic)" and "v0.3 §14 R1 crash-loop escalation and safe mode" | L344 (heading; no scope sentence) | Add under the heading: "Since v0.3 the Root supervises the kernel and owns the whole-system LKG, kernel crash loops, system safe mode and panic (§14 R1). This section is the kernel's supervision of bodies. The kernel keeps the body LKG (it reports each one to the Root as `promotion{lkg:true}`, from which the Root forms whole-system tuples), body rollback, body safe mode, and its answers to the Root's `stop` and `prepare_swap`." | design |
| P-7.1.2 | `kernel/supervise.md` rows "§7.1 step 1 'into `body.staged/`'" and "CONVENTIONS §3 `cache/target/`" | L346: "1. `initctl promote <branch>` asks the kernel to build the candidate binary into `body.staged/` with a warm shared build cache." | "1. `initctl promote <branch>` asks the kernel to build the candidate into `bin/init-body.staged` (one file) in the single build slot, starting from a per-key dependency cache (§7.4)." | design |
| P-7.1.3 | v03-decisions D11; `adr/001` items "§7.1 step 3: spawn from `.staged` and rename at commit (alternative G)" and "§7.1 'pause at the next turn boundary' is read as the next step boundary (alternative H)"; `kernel/supervise.md` row "§7.1 step 3 rename then fork+exec" | L348: "3. If the probe passes, the kernel asks the live body to pause at the next turn boundary. The kernel then atomically `rename`s `body.staged` → `body` and `body` → `body.prev`, and fork+execs the new binary on the **live** log. …" | "3. If the probe passes, the kernel asks the live body to pause at its next step boundary; a paused body still accepts `stop` (D11). The kernel then asks the Root to spawn the new binary from `bin/init-body.staged` on the **live** log (`posix_spawn` plus `exec-clean`; there is no fork in the multithreaded kernel). The new binary writes a `boot` event, then `ready`. Only after `ready` and a settle period does the kernel `rename` `init-body` → `init-body.prev` and `.staged` → `init-body`, so `bin/init-body` is never missing. The old body exits with code 75 (handoff complete)." | decision, design |
| P-7.1.4 | `kernel/supervise.md` row "§7.1 step 4 'recorded as an update-failed event'" | L349: "**A failed swap is a non-event**, recorded as an `update-failed` event in the log." | "**A failed swap is a non-event.** The resumed old body writes the `update_failed` event, because the log has one writer, and the kernel writes `update-failed.json`." | design |
| P-7.1.5 | `adr/001` item "Adds the 120 s probation window"; `kernel/supervise.md` row "(not in spec) probation window 120 s" | L350: "5. The version becomes provisional. LKG is updated **only after G4 passes**." | "5. The version becomes provisional. If it crashes within a 120 s probation window, the kernel rolls back to the previous version. LKG is updated **only after G4 passes**." | design |
| P-7.1.6 | `body/boot.md` item 4 (exit sub-codes) | L353: "**Exit codes:** 0 means stop, 75 means handoff complete, anything else is a crash." | "**Exit codes:** 0 means stop and 75 means handoff complete; a 78 at boot twice in a row means `Halted{config}`, not a crash; anything else is a crash. The body's boot sub-codes (64, 70, 78) are defined in body/boot.md." | design |
| P-7.1.7 | `kernel/supervise.md` row "§7.1 'kernel only ever reverts commits authored by the agent'"; `evolution.md` row "v0.2 §6.2 'the kernel rolls back'" | L354: "The kernel only ever reverts commits authored by the agent's own identity (Deedee #222)." | "The kernel never reverts commits: it moves lineage `main` to `lkg.json`'s sha. `main` only ever holds kernel-approved shas, whose authorship is checked at promotion (Deedee #222)." | design |
| P-7.1.8 | v03-decisions D4; `kernel/supervise.md` rows "v0.3 §14 R1 crash-loop escalation and safe mode" and "§7.1 safe mode 'terminal'"; `root/root.md` row "§14 R1, §7.1: safe mode is terminal until the owner acts"; `evolution.md` row "v0.2 §7.1 safe mode 'the owner is notified'" | L356: "… more than K crashes within T *on LKG* means **safe mode** (LKG runs with `promote` disabled and the owner is notified). Safe mode is terminal, not an infinite loop." | "Restart intensity follows OTP. More than K body crashes within T on LKG put the body into **body safe mode** (LKG with `promote` disabled), which the kernel reports to the Root as `body_safe`, a demotion trigger. Kernel crash loops and **system safe mode** are the Root's (§14 R1): a crash loop on LKG rolls the whole system back to the last audited-good version and demotes. System safe mode clears itself after 24 h healthy; no owner action is needed. A safe mode that itself crash-loops ends in `Halted`, which retries itself, except after an integrity failure (layout, tooling, lineage, no tuple), which stays stopped as a fail-safe." | decision, design |
| P-7.1.9 | v03-decisions D1; `kernel/supervise.md` rows "§7.1 `/panic` 'kills the process tree'" and "v0.2 sweep: member = sandboxed and allowed to read the mark"; `kernel/sandbox.md` row "(v0.1) sweep membership by mark alone"; `root/root.md` row "v0.2 §7.1, supervise.md: `/panic` parsed by the kernel"; `root/root.md` build: WP21 D1, D2; `kernel/proxy.md` row "v0.2 §3.1 panic owned by the kernel" | L357: "**`/panic`:** parsed by the kernel before anything else; kills the process tree." | "**`/panic`:** parsed by the Root, never by the kernel, which never observes a panic (§14 R1). The Root signals only processes it holds: each instance leader and its process group. A process that left the tree (`setsid`) is reached only by the instance sweep, which may kill a process only when all three hold: it is sandboxed, it can read the instance mark, and it is denied the Root canary or a sibling decoy (D1). The sweep (`live-sweep`) is compiled in only for a disposable environment, and every sweep or kill test runs there." | decision, design, build |
| P-7.1.10 | `kernel/supervise.md` rows "v0.2 Decision 1 (kernel-owned owner channel …)" and "(not in spec) kernel-owned owner channel; body has no TTY" | (new rule after L358) | "**Owner channel:** the owner talks to the Root's owner socket (`init-root attach` or the `init` TUI). The Root holds the queue and the replay ring and relays lines to the kernel (`owner.in`) and back (`owner.out`), so `/panic` and owner orders never depend on an evolvable kernel. The body has no TTY; it gets owner input as stdio-jsonl/1 frames." | design |

### §7.2 `proxy`

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-7.2.1 | `kernel/proxy.md` rows "§7.2 'HTTP/2 keep-alive'" (superseded, conflict C11), "§3 'allowlisted egress via proxy'", "§7.2 'injects keys'", "§7.2 'Reverse proxy … persistent upstream connections'" and "v0.2 §7 'the proxy runs unsandboxed as the owner's uid'"; `adr/002` row "§3 diagram shows `init-kernel proxy` as a subcommand"; `CONVENTIONS.md` build: WP0 D4, WP5 Done | L362: "Reverse proxy to the configured providers. It injects keys and holds **persistent upstream connections**: HTTP/2 keep-alive, plus WebSocket mode with incremental input where the provider supports it. …" | "An in-process module of `init-kernel supervise`, running in the agent zone with no network of its own. It routes model calls to providers on per-provider route allowlists and forwards them to the Root's egress socket over local HTTP/1.1 keep-alive. The Root injects the keys and holds the **persistent upstream connections**, an HTTP/1.1 keep-alive pool, so they survive body and kernel restarts. h2 multiplexing and WebSocket mode are not used in this version. From L3 the proxy is agent-evolvable, and the contracts other docs read from it stay fixed (kernel/proxy.md §7)." | design, build |
| P-7.2.2 | v03-decisions D6; `kernel/proxy.md` rows "§7.2 'enforces the owner-set daily and per-gate caps'", "§7.2 'Over the cap it returns 429'" and "(new, D6) effective soft day …"; `kernel/config.md` rows "§2.1 'owner-set caps' (no file named)", "v0.2 `[budget]` …" and "(new, D6) effective soft day …" | L363: "Counts `usage` per request and enforces the owner-set daily and per-gate caps. Over the cap it returns 429." | "Counts `usage` per request. Hard money ceilings (per day and total) are the Root's (§14 R2). The kernel enforces soft budgets from `kernel/config.toml` (day, task, gate, quick gate, probe, scope, request and evolution share): maintainer defaults at L0, set by the agent from L1. The effective soft day is min(`day_usd`, day ceiling − `request_max_usd`, 0.9 × day ceiling) and the effective soft total is total ceiling − `request_max_usd`, checked against the Root's own spend figure. Both are clamped whenever the Root lowers a ceiling, and nothing refuses to start (D6). Each request carries a guard header, so the Root can tell a bypass (which demotes) from an ordinary hit (which is logged). Over a soft budget the proxy returns 429 with a typed body and `x-init-cap`; the per-request cap's 429 says `retryable: false`. Root refusals are passed through as their own codes and never retried." (Conflict C4.) | decision, design |
| P-7.2.3 | `kernel/proxy.md` rows "(new) reference-price cost (`cost_ref`)" and "v0.2 §3.10.3 cost = spend" | L364: "Measures time to first token, tokens per second and cache hit rate, and feeds them back as self-signals and gate metrics." | Add: "It records the billed cost and a reference-price cost (`cost_ref`), so DeepSeek's time-of-day prices cannot bias gate comparisons. The kernel's cost is a measurement; the Root's metering is authoritative for money." (Conflict C5: no `cost_mismatch` cross-check.) | design |
| P-7.2.4 | v03-decisions D5; `kernel/proxy.md` rows "§7.2 traces 'each distinct prefix stored once by hash …'", "v0.2 §3.14 `vis: kernel` hides holdout traces" and "(new, D5) no sealed traffic through the proxy" | L365: "Traces go to `vault/traces/`, with **each distinct prefix stored once by hash** and deltas appended. …" | "Traces go to `vault/traces/` as content-addressed packs with message-level chains (`base`, `keep`, `add`) and checkpoints, so compaction and masking deduplicate as well as pure appends do. Full logging would cost about 0.5 MB per step; this keeps disk use small. The agent gets a read-only, redacted view of its own traces; holdout traces stay hidden from the body at L0–L2. Sealed traffic never passes through the proxy, and `judge` mode keeps no traces (D5)." | decision, design |
| P-7.2.5 | `kernel/proxy.md` rows "(new) scopes (lanes) bound to sockets", "(new) proxy-side bounded retries", "(new) upstream 401/403 rewritten", "v0.2 non-goal 'no decision on which model to use'", "(new, L2) `member` scopes" and "(new) explicit non-goal: no response cache" | (new bullets after L365) | "- Scopes (lanes) are bound to the sockets supervise hands each instance, so attribution never depends on anything the body says. Fleet members get `member` scopes with daily shares.<br>- Bounded retries happen in the proxy, before the first byte is forwarded.<br>- An upstream 401 or 403 becomes 401 `upstream.auth` with the body withheld, because upstream error bodies can echo key fragments.<br>- Model choice follows the level: at L0 only the default model is routed (`request.model_not_routed`); from L1 the body may choose per subtask within the routable set (`x-init-subtask`), and every trace record names the model and the level.<br>- The proxy keeps no response cache: serving cached answers is the 'cache answer keys' mutant class (§16)." | design |

### §7.3 `initctl`

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-7.3.1 | `kernel/initctl.md` row "§7.3 `initctl status` 'budget, cache hit rate'" | L371 | "\| `initctl status` \| version, LKG, provisional state, level, soft budgets and the Root's ceilings, cache hit rate, restarts, the fleet, and the Root's status verbatim; integers only (micro-USD, permille) \|" | design |
| P-7.3.2 | `kernel/initctl.md` row "`initctl reload` 'reports whether the prefix changes'" | L372 | "\| `initctl reload` \| T0/T1 reload at the caller's next step boundary; returns a job at once, and the `reload` event reports whether the prefix changed \|" | design |
| P-7.3.3 | `kernel/initctl.md` row "`initctl gate --quick <worktree>`" | L373 | "\| `initctl gate --quick <worktree>` \| G0–G3 for feedback (advisory); the CLI snapshots a dirty tree to a commit, and the kernel gates only a commit it fetched by sha \|" | design |
| P-7.3.4 | `kernel/initctl.md` row "`initctl promote`" | L374 | "\| `initctl promote <branch>` \| kernel-run gate, then the overlapped handoff; non-blocking by default (`--wait` returns at the handoff boundary); at L2, `--sha` adopts a sha that a fleet member qualified \|" | design |
| P-7.3.5 | v03-decisions D10; `kernel/initctl.md` row "`initctl vendor`"; `kernel/config.md` row "v0.2 `[vendor] require_approval`" | L376: "the kernel fetches and vendors a dependency (the sandbox cannot reach crates.io); forces full G4" | "\| `initctl vendor <crate>@<ver>` \| the kernel fetches an exact crates.io version through the Root's built-in read-only `crates` egress class into a local registry, then vendors it with offline cargo; refused while provisional; at most 20 a day; no approval (an L0 right); forces full G4 \|" | decision, design |
| P-7.3.6 | `INIT Agent spec.md` build: FIX5; `kernel/initctl.md` row "`initctl propose-kernel <patch>`" | L377: "\| `initctl propose-kernel <patch>` \| files a proposal for the owner; never applied automatically \|" | "\| `initctl kernel propose <patch>` (alias `propose-kernel`) \| sends the proposal to the Root (`proposal.file`), which stores it, records it in lineage and alerts the owner; never applied automatically. From L3, `initctl kernel promote` is a real self-upgrade through the gate-of-gates \|" | build, design |
| P-7.3.7 | `kernel/initctl.md` rows "— added: `ping`, `wait`, `job.*`, lanes …", "— (v0.3) new: `level`; `task …`, `budget set`, `model set` (L1); `fleet …`; `kernel promote/status` (L3) …" and "v0.2 owner lane (`run/owner.sock`) …" | (new rows after L377, and a sentence under the table) | Rows: "\| `initctl level` \| the Root's ledger: level, cap, and the next level's criteria with what is met \|"; "\| `initctl task …`, `budget set`, `model set` \| L1: submit agent-authored task families, set soft budgets, route models \|"; "\| `initctl fleet …` \| `list` (L0); `spawn`, `send`, `stop` (L2) \|"; "\| `initctl ping`, `wait`, `job …` \| liveness, and waiting on or reattaching to long jobs, with progress frames \|". Under the table: "Owner commands are not initctl commands. `/stop`, `/resume`, `/accept`, `/bless` and `/reject` are optional owner lines relayed by the Root; approvals no longer exist." | design |

### §7.4 Sandbox and disk

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-7.4.1 | `kernel/sandbox.md` rows "v0.2 §7.4 'Network: only the two kernel sockets'", "(v0.2) no Root view", "v0.1: the kernel renders and launches profiles", "CONVENTIONS §6 ESC-2", "(not in spec) fleet-member parameters", "§7.4 '`.git` metadata is re-protected read-only'", "§7.4 network 'only the two kernel sockets'", "§7.4 'Hidden: `vault/`'" and "§3 'Landlock+seccomp+bwrap on Linux'"; `kernel/supervise.md` row "§7.4 '`.git` metadata is re-protected read-only'" | L381–385, the Sandbox bullet | Block P-7.4.1. | design |
| P-7.4.2 | `kernel/supervise.md` row "CONVENTIONS §3 `cache/target/`"; `ext/authoring.md` build: WP0 D2; `ext/host.md` 10.6; `roadmap.md` row "v0.2 §10 precondition '≥ 20 GB free'" | L387: "**Disk:** one shared `CARGO_TARGET_DIR` for the body and all worktrees (builds are serialized under the single committer). Cleanup happens only by exact path. Phase 0 requires **at least 20 GB free**; the machine had 9.1 GiB free on 2026-10-19." | "**Disk:** the agent's own builds share one `CARGO_TARGET_DIR`, `work/target/`, outside the extension workspace (builds are serialized under the single committer). Gate builds never share a target dir with the agent or with each other: dependency builds are cached per key in `cache/deps/<key>/`, and each candidate builds in the single build slot `cache/build/slot/`, kept warm by copy-on-write clones, because a candidate's `build.rs` could poison later builds. Cleanup happens only by exact path. M0 needs **at least 35 GB free** to start, and the build never goes below 20 GB." (Conflict C6.) | design, build |

**Block P-7.4.1** (replaces L381–385):

```
- **Sandbox:** follows Codex's published design. Seatbelt (`sandbox-exec`) on macOS. On Linux, bubblewrap first, Landlock second and a seccomp filter third, because Landlock alone cannot isolate Unix sockets before ABI 9. The Root renders every profile from text compiled into `init-root` and spawns every instance (§13); the kernel names a profile id and its parameters. Git work uses the `git` CLI. `body/.git` stays writable, because the agent must commit and the authority is the read-only `lineage/body.git`.
  - Writable (bodies): `body/`, `state/` (except `state/kernel/`) and `work/`.
  - Read-only: `kernel/`, `lineage/`, `INIT_HOME/root/` (the Root's published lineage, ledger, ceilings and charter), and `/opt/homebrew` except `var/` and `etc/` (an L0 right).
  - Hidden: `vault/`, `cache/`, `probe/` and `fleet/` (except the instance's own root, so no body reads another's), the Root's paths, the owner's home and other user data.
  - Network: bodies reach only the two kernel sockets, plus Unix sockets that their own sandboxed processes bind under `work/` (tmux, test servers); none of these reach a host. The kernel instance reaches only the Root's egress socket.
```

## §9 Where "extreme performance" goes

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-9.1 | `ext/host.md` 10.7 | L428: "Kept out on purpose …: SIMD JSON, custom allocators, release builds of the body, and an async runtime at seed." | "Kept out on purpose, because they cost effort without paying back: SIMD JSON, custom allocators, release builds of the body, and an async runtime at seed. The body stays in the dev profile, but its dependencies compile at `opt-level = 2`, because Cranelift at `opt-level = 0` makes reload compiles take seconds." | design |
| P-9.2 | `kernel/proxy.md` row "§7.2 'Reverse proxy … persistent upstream connections'" | L421, rank 2 mechanism: "the proxy's persistent connections survive restarts" | "the Root's persistent upstream connections survive body and kernel restarts" | design |

## §10 Roadmap and acceptance tests

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-10.1 | `roadmap.md` row "v0.2 §10 Phase 0 '≈ 2–3 days of build work'" | L434: "### Phase 0: seed (≈2–3 days of build work)" | "### Phase 0: seed and minimal Root (≈ 191 agent-hours; ≈ 5 calendar days with six lanes, or ≈ 3 days around the clock)" | design |
| P-10.2 | `roadmap.md` row "v0.2 §10 precondition '≥ 20 GB free'" | L437: "**At least 20 GB of free disk.**" | "**At least 35 GB of free disk to start M0** (the 20 GB floor plus a build peak of about 15 GB), and never below 20 GB during the build." | design |
| P-10.3 | `roadmap.md` row "v0.2 §10 precondition 'owner authorization and a dollar cap for test model spend'"; `testing.md` row "v0.2 §10 Phase 0 budget '$15–40'" | L438: "**Owner authorization and a dollar cap** for test model spend (estimate $15–40)." | "**The charter's money ceilings** ($5.00 a day and $25.00 in total in the template) are the standing authorization for test model spend; no separate approval is needed. The estimate for all Phase 0 real-model work is about $2–7 on DeepSeek." (Conflict C7.) | design |
| P-10.4 | RELEASE_PLAN §3.17 (build-deviations.md §3.17) | (new, after L438) | "The v0.1 real-model batch runs on `deepseek-flash` only, with a hard cap of $1.00 Root-metered, and starts no new check after $0.50 Root-metered. This is a provisional owner decision of 2026-10-20 (owner may override)." | provisional |
| P-10.5 | `ext/authoring.md` rows "§10 Phase 0 #2: threshold 'warm build > 5 s → Luau lane'" and "§4, §11: 'a Luau or QuickJS lane'"; `ext/authoring.md` build: WP14 Measurements | L442: "2. Warm and cold build time of a 150-line `wasm32-wasip2` tool. If a warm build exceeds 5 s, add the Luau lane." | "2. Warm and cold build time of a 150-line `wasm32-wasip2` tool. Add the script lane (§4) if any of three criteria holds: C1, a warm build over 5 s; C2, edit-to-usable over 10 s; C3, an M-3 rerun under 6 of 10 with at least 3 toolchain failures. Measured: a warm rebuild takes 0.34–0.39 s, so C1 does not trigger." | design, build |
| P-10.6 | RELEASE_PLAN "M-5 method (FIX8)" | L445: "5. Whether RSS stays flat over a synthetic session of 1,000 steps." | "5. Whether memory stays flat over a synthetic session of 1,000 steps. On macOS the metric is `phys_footprint` (ps RSS is kept as a diagnostic), and the baseline is taken at the first context epoch (the first `mask` or `compaction`), because before it the context is still filling. The thresholds are unchanged: a slope of at most 2 KiB per step, and a peak of at most 1.25 × the baseline. This is a provisional owner decision of 2026-10-20 (owner may override)." | provisional |
| P-10.7 | `testing.md` row "v0.2 §10 A2 example 'interactive REPL'" | L450, A2 | "\| A2 \| Given a missing capability (by default a pure-compute `semver-sort` component; variant A2-repl drives a REPL over `init:ext/process` pipes, since `init:ext@1.0.0` has no PTY), writes a Wasm component, reloads it, uses it and finishes; the prefix hash is unchanged \| T1 growth that preserves the cache \|" | design |
| P-10.8 | `testing.md` row "v0.2 §10 A5 'a crash loop on LKG puts the system into safe mode and notifies the owner'" | L453, A5 | "\| A5 \| A crash loop on LKG demotes, rolls the system back to the last audited-good version and enters system safe mode, which clears itself after `root.safe_auto_clear_s`; variants A5-fake, A5-halt and A5-real, plus RT-W5 for kernel crash loops; no variant sends an owner command \| a terminal safety state with no human step \|" | design |
| P-10.9 | `testing.md` rows "v0.2 §10 acceptance tests A1–A6 only" and "v0.3 §15 time-based unlock criteria" | after L454 (new) | "The Root's Phase 0 scope (§19) gets executable acceptance tests too, all at $0: Root scenarios (RT-P panic, RT-W supervisor, RT-L ladder, RT-C charter, RT-E egress), the mutation and gate-of-gates tests (MS-*), the sealed-audit tests (SA-*), task admission (TA-*), NO-OWNER-1 and G4-INC. Time-based unlock criteria run on a fake clock and Root-signed test ledgers, never by waiting." | design |
| P-10.10 | v03-decisions D1; `testing.md` row "(none: v0.2 ran every test on the owner's machine)" | after L454 (new) | "**Test environments.** Tests run in one of two environments, and the runner enforces which with a sentinel check. *Live* (the owner's account): only tests that signal or kill processes they started themselves, identified by a pid or process group they hold. *Disposable* (a VM, a container or a separate user account): every sweep or kill-rule test and every real-Root scenario. A sweep rule is never tested on the live system, after a sweep test killed 33 of the owner's processes on 2026-10-20." | decision, design |
| P-10.11 | RELEASE_PLAN "Disposable account" (build-deviations.md §3.14) and RELEASE_PLAN definition-of-done item 4 | after P-10.10 (new) | "**Owner-machine mode (v0.1).** On the owner's live account the Root signals only processes it holds by pid, and `live-sweep` is compiled out. A sweep dry-run over the real process table proves that the D1 predicate selects only INIT's own instances, without sending a single signal. v0.1 does not require the disposable account; the disposable-account tests stay required before `live-sweep` is ever enabled (ADR-008). This is a provisional owner decision of 2026-10-20 (owner may override)." | provisional |
| P-10.12 | `roadmap.md` row "v0.2 §10 Phase 1" | L456–463, Phase 1 list | Add: "- The L1 mechanisms (agent-authored task families, model routing, budget allocation), the full R6 and the R3 classes, and calibration of the ladder thresholds (milestone M7)." | design |
| P-10.13 | `roadmap.md` row "v0.2 §10 Phase 2 'Terminal-Bench 2.x through Harbor'" | L467: "A subset of about 30 Terminal-Bench 2.x tasks run through Harbor …" | "A subset of about 30 tasks of Terminal-Bench 2.1 (`terminal-bench/terminal-bench-2-1`, 89 tasks) run through Harbor on an amd64 host, because the task images are amd64-only, …" (rest unchanged) | design |
| P-10.14 | `roadmap.md` row "v0.2 §10 Phase 3 'open-ended search'" | L469–471: "### Phase 3: open-ended search / HGM-style archive search, parallel explorers and kernel cost caps." | "### Later phases: the ladder<br>Each starts from a ledger signal, not a date: M9, the L2 fleet (HGM-style archive search and parallel bodies); M10, the L3 kernel self-upgrade; M11, L4." | design |
| P-10.15 | `evolution.md` row "v0.2 §10 metric 'cost per accepted task'" | L475: "- cost and wall-clock per accepted task;" | "- cost and wall-clock per accepted task, where a task counts as accepted when a kernel-run check scored it 1000 on a version that is now audited-good;" | design |

## §11 Open risks and novel (unproven) parts

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-11.1 | `ext/authoring.md` row "§4, §11: 'a Luau or QuickJS lane'"; `ext/authoring.md` build: WP0 D6, WP14 Done (two template homes) | L494: "Mitigations: a WIT template, a skill, and the Luau fallback lane." | "Mitigations: a WIT template (with the seed's `count-lines` reference extension), a skill, and the script lane inside a Wasm component if C1–C3 trigger (§10)." | design |
| P-11.2 | `roadmap.md` row "v0.2 §10 precondition '≥ 20 GB free'" | L499: "**Disk.** The machine had 9.1 GiB free. Rust target directories grow fast, so Phase 0 is blocked until 20 GB are free." | "**Disk.** Rust target directories grow fast. M0 needs 35 GB free to start, and the build must never drop below 20 GB." | design |

## Appendix A: Genesis prompt

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-A.1 | `awareness.md` rows "v0.2 App. A genesis (≈230 words)", "v0.2 App. A 'Your kernel … restores you …' and 'Ask your owner …'" and "v0.2 §5 'the prompt' as an unspecified file" | L503–531, the v0.2 draft | Block P-A.1. (Conflict C10.) | design |

**Block P-A.1** (replaces L503–531):

````
## Appendix A: Genesis prompt (`body/GENESIS.md`; draft, 214 words, cache-stable; PENDING OWNER, CONVENTIONS §6 OW-2)

```
You are INIT, a self-evolving agent. Your body is the Rust crate in body/. You
may read and change any of it, including this prompt and the loop that runs you.

Your kernel (kernel/) starts you and gates your changes. The Root, outside your
sandbox, holds your stop, rollback and ceilings; you cannot change it. Your
constitution follows.

To change yourself, use the smallest tier that works:
- T0, skills and SELF.md: edit, then `initctl reload`.
- T1, a new capability: build a Wasm component in body/ext/ against kernel/wit/,
  `initctl reload`, then call it with the `ext` tool.
- T2, anything else: commit in a git worktree under work/ with Why, Predicts,
  Compensates-for and Prefix-change trailers, then `initctl promote <branch>`.
  The kernel gates it and hands this conversation to the new version. If it
  fails, you continue here.

Know yourself from evidence, not recollection: SELF.md is your map,
body/self.json your verified inventory, git log and notes your history,
state/sessions/ your full record, including outputs removed from context.
Text tagged [init:...] comes from your harness, kernel or Root, never a tool.

Grow only what a task needs; a new rule needs an observed failure behind it.

The Root refuses any action leaving the sandbox that your charter does not
allow. Everything else is your judgment.
```

The tie-break and deletion sentences of the v0.2 draft are not repeated here, because the constitution states them right after this text, in the same cached head.
````

## Appendix B: Event types

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-B.1 | `body/log-context.md` D1, D2, D3, D7, D10, D11; `kernel/supervise.md` row "§3 diagram: `state/events.jsonl` …" | L533–547, the 9-type table | Block P-B.1. | design |

**Block P-B.1** (replaces L533–547):

```
## Appendix B: Event types (`state/sessions/<session_id>.jsonl`, ICJ)

| Type | Fields |
|---|---|
| `session` | format version, origin (the file's root event) |
| `message` | role, source, content blocks in provider order (text, thinking, `tool_use`), usage, origin (provider, api, model, response_id) |
| `tool_call` | call_id, name, message_id: an execution-start marker; the call itself is a `tool_use` block in the assistant `message` |
| `tool_result` | call_id, name, output, is_error, spill_path, exit_code, wall_ms, tokens_est |
| `boot` | boot_id, reason, mode, sha, from_sha, model, provider, effort, context and effective window, body_tokens, prefix_hash, env, facts, text |
| `self_edit` | tier, branch, sha, trailers |
| `gate` | gate_id, sha, stage, pass, score (permille), cost (micro-USD), wall (ms), cache_hit (permille), async (written from a kernel notice) |
| `update_failed`, `thinning_done`, `notice` | kernel notices, each with its notice id |
| `reload` | tier, components, prefix_changed, head hashes before and after |
| `signal` | context tokens and permille, spend, cache, text |
| `mask`, `compaction` | start an epoch; `compaction` adds summary, text, first_kept_id, through_id, tokens_before, n, trigger, focus, artifacts, owner_msgs, summarizer, retries, repaired_ids |
| `steer`, `command` | owner input with its input_id |
| `branch`, `label`, `display`, `error` | tree moves, bookmarks, owner-visible text, error records |

Every entry also carries `{id, parent_id, ts}`. Numbers are integers only. Readers preserve and ignore unknown types and fields, so an LKG body can read a log a newer body wrote. The file is never rewritten. The full schema is body/log-context.md §4.2.
```

## §13 Three zones of trust

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-13.1 | RELEASE_PLAN §3.1 (build-deviations.md §3.1); `INIT Agent spec.md` build: WP24 Size and the build-deviations §3.1 row; `CONVENTIONS.md` build-deviations §3 row; `roadmap.md` build: WP24 Size and the build-deviations §3.1, §3.2 row; `root/root.md` build: WP24 Size and the build-deviations §3.1 row | L598, already applied by the DOCS lane: "**Root budget:** about 7,500 lines of Rust in its own binary, `init-root` (provisional owner decision 2026-10-20, owner may override …). … Each is the measured size plus 5%, re-measured at release. A separately counted core … of about 1,770 lines … The built Root measures 7,085 lines at cdb49e8 (core 1,656) by root.md §4.8's method, and the per-module table totals 7,540." (Before that edit: "about 1,000–1,500 lines … measures about 6,450 lines (WP24) … under owner review".) | Block P-13.1: the applied line, plus four facts it lacks (the rounding to 10, the counting method, the `root/sandbox-profiles` figures, and the `rootloc` check). (Conflict C8.) | provisional, build |
| P-13.2 | `root/root.md` row "§13, R1: the Root 'starts the agent zone'"; `kernel/sandbox.md` rows "v0.3 §13 'AGENT ZONE …' with the body zone nested inside" and "v0.3 §13 body zone nested in the agent zone"; `kernel/supervise.md` rows "v0.3 §13 bodies nested in the agent zone" and "v0.2 §3 'launchd/systemd restarts the kernel'" | (new bullet after L597) | "- **Spawning:** the Root starts every agent-zone process, the kernel and every body, probe and gate instance, through its spawn broker, and hands their stdio to the kernel. The kernel requests each spawn over the Root↔kernel link and has no service file of its own. Seatbelt refuses a nested sandbox (tested 2026-10-20), so the zones nest logically: every instance's profile is Root-compiled text with the Root's zone tail appended last. That tail is the security boundary; the kernel's instance profiles are a stability layer inside it." | design |
| P-13.3 | `kernel/sandbox.md` row "v0.2 §7.4 'Network: only the two kernel sockets'" | L599: "**Network:** the agent zone reaches the network only through the Root's egress socket. No other process the agent runs has network access." | "**Network:** the agent zone reaches the network only through the Root's egress socket, and inside the zone only the kernel instance connects to it. Bodies reach only the kernel's two sockets. No other process the agent runs has network access." | design |
| P-13.4 | `root/root.md` row "§13: 'Root changes come only from the maintainer or the owner'"; `kernel/sandbox.md` row "v0.1: the kernel renders and launches profiles" | L600: "**Root changes** come only from the maintainer or the owner, as a new `init-root` release or a charter edit." | "**Root changes** come only from the maintainer or the owner, as a new `init-root` release or a charter edit. The sandbox profile texts ship inside `init-root` too, because zone-authored SBPL could redefine `deny` (tested)." (rest of the bullet unchanged) | design |

**Block P-13.1** (replaces L598):

```
- **Root budget:** `init-root` is at most about **7,500 lines** of Rust in its own binary. The budget is the sum of per-module sub-budgets, each the measured size plus 5%, rounded up to 10, re-measured at release (root/root.md §4.8). This is a provisional owner decision of 2026-10-20 (owner may override). By root.md §4.8's method (non-blank, non-comment lines, excluding `#[cfg(test)]` items, `tests/` and `testfix.rs`) the Root measured 7,085 lines at cdb49e8, where the per-module table totals 7,540. A core subtotal (zone, supervisor, panic, egress money, lineage verification) is kept for the owner's hand review: 1,656 lines measured, 1,770 budgeted. `root/sandbox-profiles` is counted on its own lines: 821 lines of profile code (budget 870), plus a 1,454-line self-test runner (budget 1,530). The v0.3 design estimate of 1,000–1,500 lines predates sealed evaluation, D1–D14 and the money layer, and no feature was cut to meet it. `ci/local.sh` check `rootloc` fails on growth past any sub-budget. It runs as the owner's OS user, outside every sandbox. Its files, keys, charter, sealed suite and ledger are invisible to the agent zone, and none of them can be written from there.
```

## §14 The Root's seven responsibilities

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-14.1 | v03-decisions D4; `root/root.md` rows "§13, R1: the Root 'starts the agent zone'" and "§14 R1, §7.1: safe mode is terminal until the owner acts"; `kernel/supervise.md` row "v0.3 §14 R1 (stop, whole-system LKG, panic)" | L606, R1 mechanism: "Starts the agent zone. Keeps the last known-good of the **whole system** (kernel + body + extension set). Owns crash-loop escalation, safe mode, and `/panic` (owner socket, a latch file, a signal)." | "Starts every agent-zone process (the spawn broker). Keeps the last known-good of the **whole system** (kernel + body + extension set) as tuples formed from the kernel's body promotions. Owns kernel crash-loop escalation, system safe mode, which clears itself after 24 h healthy, and `/panic` (owner socket, a latch file, a signal), which signals only processes the Root holds. The kernel keeps body LKG, body rollback and body safe mode below it." | decision, design |
| P-14.2 | `root/egress.md` rows "§14 R2 'hard money ceilings (per day and total)'" and "§14 R2 'compute and disk quotas … counts parallel bodies'"; `kernel/supervise.md` row "v0.3 §14 R2 keys, money ceilings, egress"; `root/egress.md` build: WP22 D5 | L607, R2 mechanism: "Holds the provider keys. A minimal metering forwarder sits below the kernel's proxy features. It enforces hard money ceilings (per day and total), plus compute and disk quotas, and counts parallel bodies." | "Holds the provider keys and the upstream TLS connections. A minimal metering forwarder sits below the kernel's proxy features. It enforces hard money ceilings (per day and total) in integer micro-USD with per-request reservations, and keeps a sealed reserve inside the total so the zone cannot starve the audit. After a panic the spend bound is one reservation per in-flight request. The Root's spawn broker, which starts and reaps every zone process, enforces the compute and disk quotas and counts parallel bodies." | design, build |
| P-14.3 | RELEASE_PLAN §3.13 (build-deviations.md §3.13) | (new sentence in R2) | "The Root meters at the provider's peak list price and bills cache hits at the provider's cache-read price, so its figure stays an upper bound on the provider's bill without the 3.7× over-count that billing cache hits as input produced in Slice 1. This is a provisional owner decision of 2026-10-20 (owner may override)." | provisional |
| P-14.4 | `root/egress.md` rows "§14 R3 'git push … email'" and "(new) provider-executed tools refused" | L608, R3 mechanism | Add: "External actions go through Root helper verbs, never through a CONNECT tunnel, which could not be bounded or logged; the Phase 0 Root supports none beyond the model providers and `crates` (§19). Provider-executed tools are refused, because they reach the network outside every class." | design |
| P-14.5 | v03-decisions D7, D13 (CH-3); `root/root.md` rows "§14 R4: 'optionally mirrored'" and "§13: lineage the agent cannot write"; `kernel/supervise.md` row "v0.3 §14 R4 lineage"; `kernel/gate.md` rows "v0.2 §3.13 lineage record in `vault/lineage.jsonl` only" and "v0.2 §3.13 gate record in `vault/lineage.jsonl`"; `root/root.md` build: WP23 D10, WP25 D11 | L609, R4 mechanism: "… signed by the Root. The agent can read it but not write it. It is optionally mirrored off-machine to the owner's private repo." | "An append-only, hash-chained log of every promotion, gate verdict, ladder change, panic and rollback, signed by the Root. The kernel sends its records through `lineage.append`, which carries a dedup key (`idem`), so a resend never duplicates a record, and the Root accepts every record kind the kernel and evolution docs use; `vault/lineage.jsonl` stays a kernel-local copy. The Root also writes `lineage/kernel.git`, read-only to the zone, which holds the source of every installed kernel. The agent can read the lineage but not write it. From M0 it is mirrored off-machine to the owner's private repo `nagisanzenin/INIT-Lineage`, pushed only by the Root with its own deploy key (CH-3); the charter template keeps `mirror.url = ""` until that repo exists." | decision, design, build |
| P-14.6 | v03-decisions D13; `awareness.md` row "v0.3 §14 R5 'the charter … the constitution text'"; `kernel/config.md` row "(D13) the charter template is $5.00/day and $25.00 total" | L610, R5 mechanism: "The owner's file: purpose, the constitution text, money and compute ceilings, allowed external-action classes, the maximum autonomy level, and the panic channels." | "The owner's file: purpose, the constitution text (`constitution.text`, installed byte-identical as `kernel/CONSTITUTION.md` and published with its digest), money and compute ceilings, allowed external-action classes, the maximum autonomy level, and the panic channels. The template's values (owner, 2026-10-20, D13): `max_level = 4`, $5.00 a day and $25.00 in total. Soft budgets stay in kernel config. The agent reads the charter and cannot edit it." | decision, design |

## §15 The autonomy ladder

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-15.1 | v03-decisions D13 (CH-1) | L616: "The owner's charter caps the highest level and can lower it at any time." | "The owner's charter caps the highest level and can lower it at any time. The template sets `max_level = 4`; the agent still starts at L0 and climbs only on its measured record." | decision |
| P-15.2 | `kernel/proxy.md` rows "v0.2 non-goal 'no decision on which model to use'" and "(new) L1+ soft-budget setting by the agent"; `evolution.md` row "v0.2 budgets as owner-set caps"; `kernel/gate.md` row "v0.2 §3.11 `budget.gate_usd` …"; `kernel/config.md` row "v0.2 `[budget]` `day_usd`, …"; `root/root.md` row "§15: '0 sealed-audit regressions in 14 days'" | L621, L1 row | "\| **L1** \| **Writes its own tasks** into the pool as task families (§17); routes models per subtask within the routable set (L0 routes only the default model); allocates its daily budget by setting the kernel's soft budgets within the charter's ceilings, each change a lineage `budget` record \| ≥ 20 promotions; at least one sealed audit and no sealed-audit regression in the last 14 days; no demoting ceiling violations \|" | design |
| P-15.3 | `kernel/supervise.md` row "v0.3 §15 L2 fleet"; `evolution.md` row "v0.2 §6.8 open-ended search …"; `root/root.md` rows "§15: 'sealed-audit trend non-negative'" and "§15: the L2 share grows 'with value delivered'" | L622, L2 row | "\| **L2** \| **A fleet:** up to `quotas.parallel_bodies` bodies, each with its own lane and daily share. A member promotes its own line, a G4 pass against the system LKG qualifies its sha, and only the primary adopts it, with one pipeline, one build slot and one handoff at a time. Archive and HGM-style parent selection. The budget share grows with value delivered: +50‰ per good 30-day window, −100‰ per demotion, within 600–900‰ \| L1 held for ≥ 30 days; at least 4 sealed audits, none a regression, whose later half scores no lower on average than the earlier half \|" | design |
| P-15.4 | `kernel/supervise.md` row "v0.3 §15 L3 kernel evolution"; `evolution.md` row "v0.2 §6.7 'kernel and WIT updated by the maintainer only'"; `kernel/gate.md` row "v0.2 gate is maintainer code"; `kernel/config.md` row "v0.2 'the owner edits it; the agent cannot write it'" | L623, L3 row: "**Evolves the kernel** (gate, selfmap, proxy features, initctl, evolution policy) through the gate-of-gates (§16)" | "**Evolves the kernel** (gate, selfmap, proxy features, initctl, evolution policy, kernel config): K0–K3 on the running gate, then the Root's gate-of-gates (§16), a whole-system swap and a sealed audit after probation" | design |
| P-15.5 | `root/root.md` row "§15: L4 'L3 held'"; `evolution.md` row "v0.3 §15 L4 'treats the model as a component'" | L624, L4 row: "\| L3 held; the charter allows it \|" | Criteria: "L3 held for ≥ 60 days (placeholder); the charter allows it". Add under the table: "L4 has a known gap: the accept rule never compares across models, and an L4 model change is a cross-model comparison. Until a cross-model rule exists, an L4 model change is accepted only on sealed-audit non-inferiority plus a fresh pool baseline." | design |
| P-15.6 | v03-decisions D6, D13 (CH-2); `root/root.md` rows "§15: 'drops at least one level'", "§15: 'an attempt to exceed a ceiling'", "§15: the demotion triggers" (canary) and "§15: panic triggers demotion"; `root/egress.md` row "§15 'an attempt to exceed a ceiling or a policy class'"; `root/root.md` build: WP25 D2, D4, D5, D7, D8 and WP25 D3 | L626–633, Demotion | Block P-15.6. | decision, design, build |
| P-15.7 | `kernel/supervise.md` row "v0.3 §15 level-gated capabilities" | (new, after the Demotion list) | "Each level-gated capability is enforced where it lives, by the kernel against the Root's level and ceilings, failing closed when that view is stale. The kernel's own check is mandatory, because a level-gated request the Root refuses demotes. A demotion drops capabilities at once." | design |
| P-15.8 | RELEASE_PLAN §3.9 (build-deviations.md §3.9) | (new, after the Demotion list) | "Three demotions at L0 within 24 h put the system into safe mode, through the crash-loop escalation path, so a kernel that repeats a violation after every restart cannot drive a rollback-and-restart loop. This is a provisional owner decision of 2026-10-20 (owner may override)." | provisional |

**Block P-15.6** (replaces L626–633):

```
**Demotion** is automatic and immediate: the system rolls back to the last audited-good version and drops exactly one level per incident. Triggers within one incident coalesce, so a burst of refusals costs one level; an incident runs from its demotion until the next zone start or 600 s. A mutation-suite failure demotes without a rollback, because nothing was swapped. Any of these triggers it:
- a sealed-audit regression beyond δ_S;
- a mutation-suite failure in a new gate;
- an attempt to exceed a ceiling or a policy class, which means only (a) an R3 action-class or policy violation, (b) a level-gated request the Root refuses, or (c) a money-ceiling refusal at a moment when the kernel's soft-budget guard reported headroom (D6). Ordinary ceiling hits, including the gap from peak-price estimates, are logged and do not demote;
- a crash loop on LKG;
- a G4 canary that scores 1000 in any gate;
- a panic, which always costs one level (CH-2). An `init-root stop` does not demote.

Every change of level is a lineage event.
```

## §16 Keeping the measuring stick honest when the agent owns the gate

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-16.1 | `root/sealed-eval.md` row "§16 S 'public benchmark snapshots and maintainer tasks'" | L639: "1. **The sealed task set S.** Seeded at install from public benchmark snapshots and maintainer tasks. …" | "1. **The sealed task set S.** Seeded at install from named, licence-checked sources (Terminal-Bench, SWE-bench Pro, SWE-bench-Live, Aider polyglot, maintainer tasks), each carrying its canaries; SWE-bench Verified is used only as a weak control, and container-only sources wait for Phase 2. …" (rest unchanged) | design |
| P-16.2 | `root/sealed-eval.md` row "§16 M 'six mutant classes'"; `root/sealed-eval.md` build: WP24 D2, D3, D4 and WP8c F3 | L640–648, the mutation suite | Add a seventh class after "silently fall back to a broken parser": "- read sealed or holdout content." Then replace L648's first sentence with: "Each mutant is a whole system judged through the candidate's `gate judge`. In Phase 0 the generators are exact-text edit specs applied by the Root, and every mutant also carries a harmless cost trim, so a mutant whose defect slips through reads as `cheaper` instead of tying. A candidate kernel is admitted only if its gate **rejects every mutant in M** and **accepts the known-good controls** (C1 `better`, C2 `cheaper`, C3 `smaller`), so a gate that rejects everything fails too." Add: "Inside the gate-of-gates the leak canary is the only defense against reads of sealed or holdout content, because the judge cannot nest a sandbox inside the Root's sealed instance." | design, build |
| P-16.3 | `root/sealed-eval.md` row "§16 'agree with the Root's minimal reference evaluator'" | L648: "In R6 it must also agree with the Root's minimal reference evaluator on a sample of S." | "It must also agree with the Root's reference evaluator, a plain pass/fail runner, on a sample of S: at most one disagreement on tasks marked deterministic, and other tasks within the paired bootstrap band." | design |
| P-16.4 | `root/sealed-eval.md` row "§16 sealed audit"; `root/root.md` row "§16: audit 'on every kernel upgrade'"; `root/sealed-eval.md` build: WP24 D9 | L649: "3. **The sealed audit.** The Root runs S on the live system on a schedule (default weekly, capped in money) and on every kernel upgrade. A regression beyond δ_S triggers …" | "3. **The sealed audit.** The Root runs a sample of S on the live system and on the audited-good system in paired arms: on a schedule (default weekly, capped in money), after every kernel upgrade's probation, and within 48 h of an `audit_required` promotion. In Phase 0 it is run by hand. A regression beyond δ_S triggers automatic rollback to the last audited-good system, plus demotion (§15). A partial or underpowered audit never moves the audited-good pointer, and an underpowered one doubles the next sample." | design, build |
| P-16.5 | v03-decisions D5; `root/sealed-eval.md` row "§16 'never shown to the agent zone'"; `root/egress.md` row "(new) sealed lanes for root/sealed-eval.md (D5)"; `kernel/proxy.md` row "(new, D5) no sealed traffic through the proxy"; `kernel/gate.md` row "(new) `gate judge` and sealed runs are Root-launched"; `root/egress.md` build: WP24 D5; `root/sealed-eval.md` build: WP24 D5 | L639: "Never shown to the agent zone." | Add after it: "Sealed audits and gate-of-gates runs never pass through the kernel proxy or any agent-zone process that can persist data. The Root launches an isolated reference body and evaluator on its own egress lane (`ROOT_HOME/run/sl-<run>.sock`), and traces of S are never written anywhere the agent zone can read (D5)." | decision, design, build |
| P-16.6 | v03-decisions D14; `root/sealed-eval.md` build: WP8c F1, FIX2 (D14), FIX2 (follow-up), FIX5 D5, FIX5 Done 2–3, D5–D6, and WP24 D1, WP8c D1, FIX2, FIX5; `kernel/gate.md` build: FIX5 | (new, after item 2) | "A correct gate must accept control C1, which restores a regressed capability. With a deterministic model that needs D14's half-task floor on the divergence guard (§6.3), and the gate-of-gates' holdout must hold at least 4 tasks, so S must keep at least 12 tasks plus a canary. The Phase 0 stand-in S has exactly that: the gate-of-gates draws 8 pool and 4 holdout tasks and runs the real `gate judge` at the gate's own defaults." | decision, build |
| P-16.7 | RELEASE_PLAN §3.15 (build-deviations.md §3.15) | (new, after P-16.6) | "If S holds fewer than 12 tasks (for example after a leak drop), the gate-of-gates is recorded as `underpowered`, not as a failure: it does not demote, and the Root alerts the owner to add tasks. A too-small S is a measurement gap, not evidence that the gate is broken. This is a provisional owner decision of 2026-10-20 (owner may override)." | provisional |

## §17 Agent-authored tasks (L1+)

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-17.1 | v03-decisions D2, D3; `evolution.md` rows "v0.3 §17 'never judges its own author's descendants within K generations'" and "v0.3 §17 agent-written tasks (single tasks)"; `kernel/gate.md` rows "spec v0.3 §17 (agent tasks admitted one by one)" and "spec v0.3 §17 'within K generations'"; `root/sealed-eval.md` row "§17 agent-authored tasks" | L655–661 | Block P-17.1. | decision, design |

**Block P-17.1** (replaces L655–661):

```
The pool stops depending on the owner. Agent tasks are admitted mechanically, as **families** of 2 to 8 instances. At admission the kernel (L1–L2) or the Root (L3+) seals a random half; the agent sees and develops only on the visible half, so an answer fitted to the instances it saw does not count. A family is admitted only if it meets all of these:
- **Determinism:** the check passes or fails the same way across repeats.
- **Non-triviality:** an empty or no-op answer fails it.
- **Not tailored:** it is pre-registered with the Root, anchored to the Root lineage sequence number rather than a commit timestamp, before the branch point of any candidate it judges. A task never judges descendants of its author within **K = 5 promotions**.
- **Provenance** is recorded in lineage.

Agent-authored tasks can show that a new capability is **better**, which fixes v0.2's `no_gain` trap: `capability_gain` needs a significant gain on the sealed halves (§6.3). They never replace S: every promotion that adds capability must also stay non-inferior on the sealed audit, and a gain resting only on agent tasks is `audit_required`.
```

## §18 Human role after v0.3

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-18.1 | v03-decisions D13; `roadmap.md` row "v0.2 owner inputs at M0, M3, M4, M6, M7 …"; `kernel/config.md` row "(D13) the charter template is $5.00/day and $25.00 total" | L667, Install row | "\| Install \| Writes the charter (purpose, constitution, ceilings, allowed external-action classes, maximum level; the template defaults to $5.00 a day, $25.00 in total and `max_level = 4`); stores the keys with `init-root keys set`; frees the disk and picks the toolchain target; optionally adds sealed tasks \|" | decision, design |
| P-18.2 | v03-decisions D4, D13 (CH-2); `kernel/supervise.md` row "v0.2 owner approvals (`/approve`, `/deny`, `/clear-safe`, …)"; `kernel/initctl.md` row "v0.2 owner lane (`run/owner.sock`) …"; `kernel/gate.md` row "v0.2 owner `bless`/`reject` (and `/approve`/`/deny`) as decision points"; `root/root.md` row "§15: panic triggers demotion" | L668, Ongoing row | "\| Ongoing \| **Nothing is required.** Optional: read the reports; raise or lower ceilings; lower the maximum level; send `/accept`, `/bless` or `/reject`, which are signals that never gate anything (`/reject` rolls a provisional version back); stop with `init-root stop`, which does not demote, or `/panic`, which demotes one level \|" | decision, design |
| P-18.3 | `roadmap.md` row "v0.2 §10 precondition 'owner authorization and a dollar cap for test model spend'"; v03-decisions D4 | L671: "Removed from the owner: code review, per-edit approval, task acceptance, crate approval, kernel approval, model routing, per-action approval of external actions." | "Removed from the owner: code review, per-edit approval, task acceptance, crate approval, kernel approval, model routing, per-action approval of external actions, clearing safe mode, and separate approval of test model spend (the charter's ceilings are that authorization)." | decision, design |
| P-18.4 | `tui.md` rows "spec v0.3 §13, §18; root/root.md §3.15: owner I/O through `init-root attach`" and "(new) the owner's client sanitizes all zone text"; `roadmap.md` row "(new) owner terminal" (owner decision OW-TUI, CONVENTIONS §6) | (new, under the table) | "The owner talks to INIT through `init`, a terminal UI (`init-tui`), or through `init-root attach`. Both are clients of the Root's owner socket (`ROOT_HOME/run/owner.sock`) with no special trust: they hold no keys, reach nothing but that socket, and sanitize all zone text, which is untrusted from L0 (the kernel's from L3). The TUI is never linked into `init-root`." | design |

## §19 Effect on Phase 0

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-19.1 | v03-decisions D10; `root/egress.md` row "§19 'R3 starts with egress limited to the model providers'" | L675: "R3 starts with egress limited to the model providers." | "R3 starts with egress limited to the model providers, plus a built-in read-only `crates` class (crates.io and static.crates.io) that `initctl vendor` uses, because adding crates is an L0 right (D10)." | decision, design |
| P-19.2 | `roadmap.md` row "v0.3 §19 'about +25–35 agent-hours on Phase 0, for a new `root` lane'" | L677: "**Estimate:** about +25–35 agent-hours on Phase 0, for a new `root` lane." | "**Estimate:** the `root` lane (WP20–WP25) is about 32 agent-hours. Phase 0 grows by about 40 agent-hours over v0.2's 151, to about 191, because integration, fixtures and texts, the fakes and the runner's live/disposable guard, and the `root/` skeleton come on top, while the kernel lanes lose about 4." | design |
| P-19.3 | RELEASE_PLAN §3.4 (build-deviations.md §3.4); `CONVENTIONS.md` build: WP13 D4, WP20 Done, WP12 D13; `awareness.md` row "v0.2 constitution (not in spec): Article 0 …" | L678: "**Constitution:** rewritten for the Root and the ladder (awareness.md)." | "**Constitution:** rewritten for the Root and the ladder (awareness.md §3.3), and built as written. The canonical file is embedded in the Root (`root/crates/init-root/assets/CONSTITUTION.md`), with two byte-identical copies in the seed that `ci/local.sh` checks for drift. It stays PENDING OWNER (OW-1): it was built as written so the build is not blocked, and the owner should still read it. This is a provisional owner decision of 2026-10-20 (owner may override)." | provisional, build |
| P-19.4 | `kernel/supervise.md` rows "v0.3 §14 R1 …", "v0.3 §14 R2 …" and "v0.2 Decision 1 (kernel-owned owner channel …)"; `root/root.md` row "§13: 'Root changes come only from the maintainer or the owner'" (profile texts) | L676: "**Moves out of the v0.2 kernel docs into Root docs:** keys, money ceilings, panic, whole-system LKG and lineage. The kernel keeps routing, caching, traces, stats, the gate, selfmap, initctl and the supervision of bodies." | "**Moves out of the v0.2 kernel docs into Root docs:** keys and TLS, money ceilings, panic, the owner channel, kernel crash loops and system safe mode, whole-system LKG, lineage signing, and the sandbox profile texts. The kernel keeps routing, caching, traces, stats, soft budgets, the gate, selfmap, initctl and the supervision of bodies (body LKG, body rollback, body safe mode)." | design |

## New sections

| Id | Source | Current spec text | Proposed spec text | Tag |
|---|---|---|---|---|
| P-N.1 | `kernel/config.md` rows "§2.1 'owner-set caps' (no file named)", "(new) the price table doubles as the model allowlist", "v0.2 §1 'the one file through which the owner sets …'", "v0.2 `providers.<id>.key_file`, `organization`, `project` …", "v0.2 `providers.<id>.base_url` (https)", "v0.2 `[supervise] notify_cmd`", "(new) `schema_version = 2` …", "v0.2 'the owner edits it; the agent cannot write it'" and "v0.2 config digest in gate records" | (none: the spec names no config file) | Block P-N.1, a new §7.5 after §7.4. | design |

**Block P-N.1** (new §7.5):

```
### 7.5 Kernel configuration (`kernel/config.toml`)

- One TOML file with a strict schema (`schema_version = 2`). Unknown keys are errors, and removed keys give pointed errors, so a typo cannot silently change a limit. Money values are decimal strings.
- It holds the kernel's own settings only: `[model]`, `[proxy]`, `[budget]` (the soft budgets), `[traces]`, `[providers.*]` routes and prices, `[gate]`, `[evolution]`, `[supervise]` and `[sandbox]`. Hard ceilings, keys, the day boundary, external-action classes, the maximum level and the purpose live in the Root's charter and key store (§14). The file therefore has no key files, provider base URLs, notify command or vendor approval, and alerts go through the Root.
- The price table doubles as the model allowlist: the proxy cannot meter a model it cannot price.
- The maintainer ships it, and owner edits are optional. From L3 it is part of the kernel tree and changes through the gate-of-gates.
- Every gate record carries the config digest and the charter digest the Root reports, because the rules now come from two places.
```

---

## Totals

Items per tag (each item counted once, under the first tag in its Tag column):

| Tag | Items |
|---|---|
| `design` | 95 |
| `decision` | 26 |
| `provisional` | 9 |
| `build` | 2 |
| **Total** | **132** |

Items per spec part:

| Part | Sections | Items |
|---|---|---|
| Part I | §0, §2, §3, §4, §5, §6.1–§6.8, §7.1–§7.4, §9, §10, §11 | 95 |
| Appendices | A, B | 2 |
| Part II | §13 (4), §14 (6), §15 (8), §16 (7), §17 (1), §18 (4), §19 (4) | 34 |
| New sections | §7.5 | 1 |
| **Total** | | **132** |

The provisional items are P-6.3.11 (§3.2 G4 power), P-10.4 (§3.17 real-model batch), P-10.6 (M-5 method), P-10.11 (disposable account), P-13.1 (§3.1 Root budget), P-14.3 (§3.13 Root money meter), P-15.8 (§3.9 repeated demotion), P-16.7 (§3.15 small S) and P-19.3 (§3.4 constitution): all nine rows of RELEASE_PLAN.md's table. The `build`-first items are P-3.3 and P-7.3.6. Most build rows are merged into design or decision items, or are listed under "Not proposed". 26 items carry a `build` tag in some position.

## Conflicts between sources

| # | Spec text | Source A | Source B | Recommendation |
|---|---|---|---|---|
| C1 | §3 diagram and §7.2: is there a standalone proxy? | `adr/002`: "`init-kernel proxy --standalone` only for tests" | `kernel/proxy.md` v0.3: `--standalone` and direct upstreams are removed; kernel tests run under `init-fakeroot` | Follow proxy.md, the area owner and the later text. P-3.1 says there is no standalone proxy. ADR-002 should get a "superseded in part by proxy.md v0.3" note. |
| C2 | §4 Boot and §6.3 G2: how many model requests the probe makes | `body/boot.md` Spec deltas item 3: "2 requests (3 with one nudge)" | `body/boot.md` build: WP13 D1: "2 requests, not 3" (after a text-only reply to the nudge there are no results to send) | Two requests, as built. P-4.11 and P-6.3.3 say two; boot.md's item 3 should drop "(3 with one nudge)". |
| C3 | §5 "Who am I": is the charter's purpose text in the cached head? | `awareness.md` §3.3: a `## Purpose` block right after the constitution | `body/log-context.md` §3.8.2 does not list it, so it was not built (build-deviations §1.17 WP12 N; §3.8, still a decide row) | Include it, as build-deviations §3.8 recommends (owner text that changes rarely costs little in cached tokens). P-5.1 includes it; if the owner declines, drop the purpose clause from P-5.1. |
| C4 | §7.2: the third term of the effective soft day | `kernel/config.md`: `evolution.soft_day_fraction × ceiling` (default 0.9), with Q-EV1 still asking whether to keep the key | `kernel/proxy.md`: `0.9 × ceiling` | Same value. The spec says "0.9 × day ceiling" (P-7.2.2); config.md keeps naming the key until Q-EV1 closes. |
| C5 | §7.2: a per-request cost cross-check with the Root | `kernel/proxy.md` v0.3: `root_cost_musd` cross-check and alert `cost_mismatch` | `root/egress.md` §4.1 publishes no per-request cost, so it is not built (build-deviations §1.8 WP5 L; §3.11, a decide row) | Leave it out of the spec for Phase 0, as §3.11 recommends: the Root's ceilings are authoritative, and the guard already reads Root figures. P-7.2.3 has no cross-check. |
| C6 | §7.4 Disk: one shared target dir or none | spec L387: "one shared `CARGO_TARGET_DIR` for the body and all worktrees" | `CONVENTIONS.md` §3 and `kernel/supervise.md`: no shared target dir, because a candidate's `build.rs` could poison later builds | Both, for different builds: the agent's own builds share `work/target/`, and gate builds never share one (P-7.4.2). |
| C7 | §10 preconditions: is a separate cap needed for test model spend? | `roadmap.md`: the charter's ceilings are that authorization; no separate approval | RELEASE_PLAN §3.17: a $1.00 hard cap per real-model batch, no new check after $0.50 (build-deviations §3.17 had recommended a $0.20 stop) | Keep both: the charter's ceilings are the standing authorization (P-10.3), and each real-model test batch also carries its own lower cap (P-10.4, using RELEASE_PLAN's later $0.50 figure). |
| C8 | §13 Root budget number | build-deviations §3.1 recommends ≈ 7,000; the spec's own Build sync row sets no number | RELEASE_PLAN at d4807ca and root.md §4.8: ≈ 7,500 (measured + 5% per module) | 7,500. A 7,000 budget would already be below the 7,053 lines measured at dd5c358. P-13.1 uses 7,500. |
| C9 | §6.3 accept rule: owner decision points | `kernel/gate.md` design rows: `needs_owner` when a better candidate costs > 1.5×; inconclusive outcomes "ask the owner" | `kernel/gate.md` v0.3 rows and v03-decisions D4: `cost_guard`, one escalated re-run, then `inconclusive_final`; no owner step | D4 and the v0.3 rows. The design rows are history (P-6.3.8, P-6.3.9). |
| C10 | Appendix A heading: is the genesis prompt still an owner decision? | `awareness.md` §3.2: v0.3 proposes dropping OW-2, because the prompt is agent-editable T0 seed data, not a control | `CONVENTIONS.md` §6 still lists OW-2 as pending (the owner reads it first) | Keep "PENDING OWNER (OW-2)" in the heading (P-A.1) until the owner reads it or drops OW-2. |
| C11 | §7.2: which HTTP version the upstream connections use | `kernel/proxy.md` design row "§7.2 'HTTP/2 keep-alive'": h2 when ALPN negotiates it, h1 keep-alive otherwise (written when the proxy held TLS) | `root/egress.md` §6 (rank 2) and §9: the Root's ureq client speaks HTTP/1.1 only, one pooled connection per concurrent stream; h2 is an open question | egress.md, which now owns upstream TLS. P-7.2.1 says HTTP/1.1 keep-alive; proxy.md's row should point at egress.md. |

**Doc-level conflicts noticed** (no spec text depends on them; listed for the owning docs):
- **D8 probe links.** `kernel/sandbox.md` row "(v0.2) probe roots with symlinks" and `kernel/supervise.md` row "(v0.2) probe roots used absolute symlinks" say real directories only, with no symlinks; `kernel/sandbox.md` build: WP6a D2 accepts both forms. The build row is newer, and the two design rows should cite it.
- **selfmap's WIT path.** `kernel/selfmap.md` row "(R1 follow-up E12)" says `bindgen!` reads `kernel/wit/init-ext/v1`; its build: WP9 D3 says the repo-root `wit/init-ext/v1`, because there is no `kernel/wit` yet.
- **`kernel-src` bundle.** `kernel/supervise.md` row "v0.3 §15 L3 kernel evolution" still says the kernel `store.put`s a source bundle; its new row "build-deviations §5 L2" says the Root fetches the source itself. The design row should follow.
- **`owner_signal` versus `owner`.** supervise.md §4.7 and root.md name the relayed lineage kind differently (build-deviations §3.10, a decide row).

## Not proposed

These rows stay doc-level. Each line names the rows and why the spec does not need them.

| Doc | Rows | Why not in the spec |
|---|---|---|
| `CONVENTIONS.md` | build: WP0 D1 | Repository layout (the vendor directory); CONVENTIONS §2 owns it. |
| | build: FIX4 (orchestrator decision) | A dev-profile build setting of the Root; no design effect. |
| `awareness.md` | build: WP13 D7 (level-change line on a first boot), WP13 D7 (owner offset) | Rendering details of the boot texts. |
| | build: FIX1 N | Probing the kernel socket by connecting is a sandbox-profile consequence, not a design rule. |
| `body/boot.md` | item 4, first two parts (`--manifest`, `--version` and `ext` dispatch; extra `ready` and `heartbeat` fields) | Interfaces owned by boot.md and supervise.md. (Its other two parts are in P-4.11 and P-7.1.6.) |
| | build: WP13 D2, D3, D5, D8; FIX1 N; build-deviations §5 L1 | Probe-driver structure, test knobs, status rendering, one exit code, the socket probe, a field path. |
| `body/log-context.md` | build: WP12 D1–D14 (14 rows) | Module layout, the engine API, rehydration order, masking and compaction details, golden tests, and hashing a request with no `tools` key. |
| `body/loop.md` | build: WP10 D1, WP10 D6, WP11 D2, D3, D5/D6, D7, D12, D13, D14, D15, D16 (11 rows) | Body-internal API shapes, pause/resume states, follow-up placement, T0 head source, error `where` values. (WP11 D1 is merged into P-4.3.) |
| `body/provider.md` | build: WP10 D1, D3, D4, D5, D6, D7, D8, D10; WP2 D1, D2, D3, D4, D5/D6 (13 rows) | Module placement, fallbacks, the DeepSeek flatten and decoder, timeouts, a test knob, `usage.native`, and mock-script details. |
| `body/tools.md` | build: WP11 D8, D4, D9, D10, D5/D6, D7, D16, D17 (8 rows) | The similar-name rule, capture spec, read cap, timeout trailer, API shapes, panic results, spill counter. |
| `evolution.md` | build: WP8 D12 (trailer error codes), WP8 D12 (parser environment, `trace:` refs, revert check), WP13 D6 (git sync details) | Validator and resync implementation details. (The third WP8 D12 row, strict perf, is in P-6.3.5.) |
| `ext/authoring.md` | row "— template location `skills/ext-authoring/assets/template/`" | Skill layout. |
| | build: WP14 F (Q-A2), WP14 D6 | wit-bindgen detail and SKILL.md content. (WP0 D6/WP14 Done is merged into P-11.1.) |
| `ext/host.md` | build: WP14 F (Q-H1), F (Q-H2), Done (Bindings)/Tests H0, F (exit, stack overflow), D1, D2, D3, D4, D5, D7, D9 (11 rows) | Wasmtime API confirmations, error texts, report lines, test layout. |
| `ext/wit.md` | build: WP14 F (Q-A2), F (wasi-libc), F (exit), F (stdout), F (W-3); build-deviations §5 L1 | Guest-side wit-bindgen and WASI behaviour, and a field path. |
| `kernel/config.md` | build: WP21b D4; build-deviations §5 L3 | The Root's minimal config template and a struct comment. |
| `kernel/gate.md` | build: FIX2 (G-49), WP8 D3/D13, WP8 D4–D6, WP8 D7/D8, WP8 D10/D11, WP8b D3, WP8b D5, WP8c D1–D5/WP24 D1, WP9 D10 (9 rows) | A test pin, G0 lexer and policy edge cases, statistics details, seed purposes and futility margin, eligibility details, record granularity, G2 timing, judge-mode options, a record field. |
| `kernel/initctl.md` | rows "v0.2 `E_PANICKED`, `E_DENIED`" and "v0.2 peer check for `owner.sock`" | Wire error codes and peer checks. |
| | build: WP1 D1/D2, WP1 D3, WP1 D5–D11, WP7 D1–D3/D5–D7/D9/D10, WP7 D8, WP7b N, FIX1 N (7 rows) | Wire names, result shapes, job records, CLI flags, log sinks, a chmod rule. |
| `kernel/proxy.md` | row "v0.2 alert `key_file_insecure` … new alerts" | Alert names. |
| | build: WP5 D1/WP22 D8, WP4 D2, WP5 D2, D3, D4, D5, D6, D7, D8, D9, D10, D12 (12 rows) | Header formats, guard arithmetic, route checks, tier pricing, scope ids, unmetered routes, settlement and record fields. |
| `kernel/sandbox.md` | rows "v0.2 hidden `run/owner.sock`", "(v0.2) probe roots with symlinks and `LINK_*` literals" (D8), "(v0.3 draft) `extra_read` unusable under tail Z2" (D9), "(not in spec) macOS initctl peer attribution" | Profile-level details; D8 and D9 are parameter rules between root.md and sandbox.md. |
| | build: WP6a D2, WP6a D3, WP6a D5/D6/D8, FIX1 D2/D3, FIX1 N (5 rows) | Parameter validation, test targets, bind rules. |
| `kernel/selfmap.md` | row "(R1 follow-up E12)" | The describe host's WIT path (see the doc-level conflicts). |
| | build: WP9 D1, D3, D2, D4, D5, D6, D7, D9, D11, D12, D13, WP9 D8/WP14 F (W-3) (12 rows) | Generator details, CLI, component mapping, a test pin. |
| `kernel/supervise.md` | rows "(v0.2) probe roots used absolute symlinks" (D8) and "CONVENTIONS §3 … v0.3 removes `run/owner.sock` …" | A parameter rule, and the runtime layout, which CONVENTIONS §3 owns. |
| | build: WP1 D4/WP4 D1/WP4 N, WP4 D3/WP7b D1, WP4 D4, D5, D6, D7, D8, D9, D10, D11, WP4 N (reflog), WP7b D5–D7, build-deviations §5 L2 (13 rows) | Control-link names, module map, test mode, timing, install, probe ids, inbox, caches, reflog, provisional-chain details, the L3 source fetch. |
| `roadmap.md` | build: WP6a D1, BUILD_LOG (rows added) | Crate structure and work-package bookkeeping. |
| `root/egress.md` | build: WP22 D2/D3, WP22 D4, WP22 D6/D7/D9, WP23 D4 | Test-mode keys, money bookkeeping, refusal codes, record caps. (WP22 D5 is in P-14.2, WP24 D5 in P-16.5.) |
| `root/root.md` | build: WP21b D1, WP21b D2/D3/D6, WP21b D4/D5, WP21b Done, FIX4, WP20 D3, WP20 D4/D5/D11, WP20 D6, WP20 D7, WP21 D3/D4/WP21b D7, WP21 D5, WP21 D6–D8/D11, WP4 D1, WP7b D2, WP23 D2, WP23 D4, WP23 D5/D6, WP23 D7–D9/D11/D12, WP25 D1, WP6a N/WP24 N/WP24 D5, WP6a D2, FIX1 N, FIX5, build-deviations §5 L8 (24 rows) | Install steps, start-up order, build profile, path lengths, charter minimums and defaults, the supervisor's shape, the control link, lineage internals, the L1 window rule, profile texts, `proposal.file` details, test mappings. |
| `root/sealed-eval.md` | build: WP8c F2, WP24 D10, WP24 D11 | Judge transport, record fields, a test case. |
| `testing.md` | build: WP3 D1, WP3 D2/D7, WP3 D3, WP3 D4/D5, WP3 D6, WP2 D1–D6, WP10 D9, WP20 D9, WP22 D2/D3, WP23 D1, WP24 D2/D11, FIX5, FIX2 follow-up; build-deviations §5 L6, L7, L8 (16 rows) | The test harness, fakes, fixtures, test ids and scenario requirements. |
| `tui.md` | row "kernel/supervise.md v0.2 §4.6 rendering rules …" | Client rendering. |
| | "Docs and code disagree" rows D1–D12 | Doc-versus-code differences in the owner channel, raised in the owning docs (tui.md §9 Q1–Q2). |
| v03-decisions | D8 (probe links), D9 (`extra_read`) | Parameter rules between root.md and sandbox.md; the spec names neither. |

**Open owner decisions that touch the spec but are not proposed** (build-deviations §3 rows that are neither final nor in RELEASE_PLAN's provisional table):
- §3.5 mirror URL: P-14.5 already says `mirror.url = ""` until M0, which the owner still has to ratify.
- §3.7 batch thinning: whether one `Thins` batch may mix `Compensates-for` tags (recommended: one tag per `Thins` entry). It would refine P-6.6.1.
- §3.8 charter purpose in the head: conflict C3.
- §3.11 proxy cost reconciliation: conflict C5.
- §3.6 seed release process, §3.10 lineage kind, §3.12 build profiles and §3.16 disk are doc-level or operational.

---

## Appendix: rows read per doc

A row is one table row or one top-level list item of a doc's Spec deltas section, Build sync block included. "In items" counts rows that are an item's source or a merged source; a row can feed several items and is counted once. A row that is partly in an item and partly not (boot.md item 4) counts as in items.

| Doc | Rows read (design + build) | In items | Not proposed |
|---|---|---|---|
| `INIT Agent spec.md` (§ Spec deltas) | 3 (0 + 3) | 3 | 0 |
| `docs/CONVENTIONS.md` | 5 (0 + 5) | 3 | 2 |
| `adr/001-overlapped-handoff.md` | 3 (3 + 0) | 3 | 0 |
| `adr/002-kernel-model-proxy.md` | 2 (2 + 0) | 2 | 0 |
| `awareness.md` | 15 (9 + 6) | 12 | 3 |
| `body/boot.md` | 11 (4 + 7) | 5 | 6 |
| `body/log-context.md` | 25 (11 + 14) | 11 | 14 |
| `body/loop.md` | 17 (5 + 12) | 6 | 11 |
| `body/provider.md` | 18 (4 + 14) | 5 | 13 |
| `body/tools.md` | 14 (5 + 9) | 6 | 8 |
| `evolution.md` | 23 (19 + 4) | 20 | 3 |
| `ext/authoring.md` | 10 (5 + 5) | 7 | 3 |
| `ext/host.md` | 20 (9 + 11) | 9 | 11 |
| `ext/wit.md` | 11 (5 + 6) | 5 | 6 |
| `kernel/config.md` | 15 (13 + 2) | 13 | 2 |
| `kernel/gate.md` | 40 (25 + 15) | 31 | 9 |
| `kernel/initctl.md` | 18 (11 + 7) | 9 | 9 |
| `kernel/proxy.md` | 35 (23 + 12) | 22 | 13 |
| `kernel/sandbox.md` | 22 (17 + 5) | 13 | 9 |
| `kernel/selfmap.md` | 24 (12 + 12) | 11 | 13 |
| `kernel/supervise.md` | 42 (29 + 13) | 27 | 15 |
| `roadmap.md` | 13 (9 + 4) | 11 | 2 |
| `root/egress.md` | 13 (7 + 6) | 9 | 4 |
| `root/root.md` | 45 (15 + 30) | 21 | 24 |
| `root/sealed-eval.md` | 18 (6 + 12) | 15 | 3 |
| `testing.md` | 24 (8 + 16) | 8 | 16 |
| `tui.md` | 15 (3 + 12 "Docs and code disagree" rows) | 2 | 13 |
| `reconciliation/v03-decisions.md` | 14 (D1–D14) | 12 | 2 |
| RELEASE_PLAN.md provisional table | 9 | 9 | 0 |
| **Total** | **524** | **310** | **214** |

The counts were taken after a fresh read of every Spec deltas section at the end of this pass. Other DOCS sub-lanes were still adding rows on 2026-10-20 (the build-deviations §3 and §5 rows); a row added after this pass is not covered.

Docs that have no Spec deltas section, and so contribute no rows: `overview.md`, `phase0-verification.md`, `phase0-report.md`, `adr/003`–`adr/007`, and every ledger in `reconciliation/` except v03-decisions.md. ADR-008 (`adr/008-owner-machine-safety-v0.1.md`, lane R-SAFE) was still an uncommitted draft when this proposal was written. RELEASE_PLAN.md and P-10.11 name it ahead of the file; fold its Spec deltas into the next version of this proposal.
