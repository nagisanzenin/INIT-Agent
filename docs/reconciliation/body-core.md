# Reconciliation R1: body-core

```
Status: R1 · 2026-10-19 · Area: body
Docs covered: body/provider.md, body/loop.md, body/tools.md, body/boot.md, adr/003-std-threads-no-async.md
```

**Sources.** Every "Open questions", "Cross-doc" and "Spec deltas" section of the 26 committed docs, searched for items addressed to these five docs or to contracts they own (CONVENTIONS §4: provider adapters and stream model; agent loop, steering, abort, parallelism; tool schemas and descriptions; body-side boot). The search also covered:
- body-text requirements that name these docs: log-context §3.8.2's head tool order, supervise §3.11.5's sync contract, gate §3.5's G2 criteria;
- the sibling ledger reconciliation/kernel-proxy-gate.md.

**Totals: 48 items.** 43 accepted, 2 rejected, 3 escalated to the owner. All three escalations are one question, E1, raised by TS-1, LC-5 and TS-6.

## Ledger

| id | from doc § | question | decision | change made (your doc §) | follow-up that ANOTHER doc must make |
|---|---|---|---|---|---|
| HO-1 | ext/host.md §9.2 → loop | adopt the three reload call sites of host §3.9 rule 3, `snapshot()` once per batch, `reset_trials()` at session start, `shutdown()` on every orderly exit incl. 75 | ACCEPT (already in draft) | loop.md §3.3 (`commit_reload(ZERO)` at every boundary), §3.7 plan step 4 (`snapshot`, 60 s pre-batch commit), §3.8.1 (`shutdown` before exit 75), §3.8.2 (10 s commit), §4.2 | — |
| HO-2 | ext/host.md §9.2 → tools | `CallCtx` with call id and task workspace | ACCEPT | tools.md §3.6 step 2 (`CallCtx{call_id, workspace: cwd}`) | — |
| HO-3 | ext/host.md §9.2, §3.13 → tools | share the run-with-timeout and process-group-kill helpers with `init:ext/process` | ACCEPT | tools.md §3.5.7 (`run_group`, `kill_group`) | ext/host.md §3.13: describe the helper as a waiter thread + `recv_timeout`, not "polls `try_wait` every 10 ms" |
| HO-4 | ext/host.md §9.2 → tools | spill policy for `ExtOutcome.text` | ACCEPT (already in draft) | tools.md §3.1 (51,200-byte inline cap + spill), §3.6 | — |
| HO-5 | ext/host.md §9.2 → tools | the `ext` description should point at `state/ext/catalog.json` | ACCEPT | tools.md §4.1 (new `ext` description naming `$INIT_HOME/state/ext/catalog.json`), §3.6 (discovery paragraph), §3.1 (read/write/edit expand a leading `$INIT_HOME/`). The reserved names `_list`/`_describe` are removed | — |
| HO-6 | ext/host.md §9.2 → boot | `ExtHost::new` early in boot, not part of `ready` | ACCEPT (already in draft) | boot.md §3 execution order step 6, §3.5 | — |
| HO-7 | ext/host.md §9.2, §4.5 → boot | the `init-body ext describe\|call\|inspect` subcommands | ACCEPT | boot.md §3.1 (argv dispatch to `ext_host::cli`), §4.3 manifest `subcommands`, §4.4 exit codes 0/1/2 | — (host.md owns the semantics) |
| HO-8 | ext/host.md §3.16, §9.2 → boot | `[profile.dev.package."*"] opt-level = 2, debug = false`; `exclude = ["ext"]` | ACCEPT | boot.md §3.16 (seed `Cargo.toml`) | main session: register the seed build configuration's owner (below) |
| HO-9 | ext/host.md §10 delta 10.3 | `wasmtime-wasi` lazily starts a tokio runtime, against "no async runtime" | ACCEPT | adr/003 Consequences ("A dependency runs its own runtime") | main session: CONVENTIONS §5 wording (below) |
| WI-1 | ext/wit.md §9.2 → tools | `ext` schema `{name, input: object}`, both required, `additionalProperties: false`, input ≤ 1 MiB | ACCEPT | tools.md §4.1 (schema shape unchanged; description states the 1 MiB limit), §3.6 step 1 (validated before the host call) | — |
| WI-2 | ext/wit.md §9.2 → tools | the description says where the catalog is, and that an unknown name returns the list | ACCEPT | tools.md §4.1, §3.6 | — |
| WI-3 | ext/wit.md §9.2 → tools | define "the task workspace" mounted at `/work` | ACCEPT | tools.md §3.6 step 2: the body's cwd, where every `bash` call starts; a G4 run gets its task directory as cwd | supervise.md: state the live body's `LaunchSpec.cwd` (`$INIT_HOME/work` assumed); gate.md: spawn G4 runs with cwd = the task workspace |
| WI-4 | ext/wit.md §9.2 → tools | apply the bash spill policy to `ext` results | ACCEPT (already in draft) | tools.md §3.1 | — |
| WI-5 | ext/wit.md §9.2 → tools | share the kill and run helpers | ACCEPT (same as HO-3) | tools.md §3.5.7 | — |
| AU-1 | ext/authoring.md §9.2 → boot | `.cargo/config.toml` with vendored sources and `net.offline = true`; `exclude = ["ext"]`; the seed ships `ext/Cargo.toml`, `Cargo.lock`, `wit/…`, `skills/ext-authoring/` | ACCEPT | boot.md §3.16 (both files, verbatim; tree contents referenced to authoring.md and wit.md) | main session: CONVENTIONS §2 seed layout (below) |
| SV-1 | kernel/supervise.md §9 item 1 → loop | step boundary as §3.6.4 phase 1 needs (pause checked after each tool batch and while idle), `state{busy}` on every change, handle `interrupt` | ACCEPT | loop.md §3.3 (explicit "this boundary is supervise's step boundary"), §3.5, §4.1 | — |
| SV-2a | kernel/supervise.md §9 item 2a → boot | the readiness contract, including the probe round trip | ACCEPT | boot.md §3.9 (probe), §3.11 (`ready` only after the probe), §4.2 | — |
| SV-2b | kernel/supervise.md §9 item 2b, §3.11.5 → boot | implement the boot-time `body/` sync contract | ACCEPT | boot.md §3 order step 5, §3.6 (fetch; `ff-only` on promote; evolution's stash/switch script on rollback and post-panic cold; fetch-only in safe; nothing in probe), §3.8 `git_sync` facts, §8 test 13 | evolution.md §3.6: say the script runs in the body's boot (boot.md §3.6), not "run by the kernel" (supervise item 4 says the same) |
| SV-2c | kernel/supervise.md §9 item 2c → boot | safe mode loads no T1 and reads T0 from `boot.json.lkg_tree` | ACCEPT (already in draft) | boot.md §3.7, §3.14 | — |
| SV-2d | kernel/supervise.md §9 item 2d → boot | exit within 10 s on stdin EOF | ACCEPT | boot.md §3.13 item 3; loop.md §4.1 | — |
| SV-3 | kernel/supervise.md §3.10, §4.5 | `input` with `input_id` dedup and `ack` once durable; `notice` + `notice_ack` with dedup | ACCEPT (already in draft) | loop.md §3.4, §3.8.3 | log-context.md: carry `input_id` and the notice `id` in events (supervise item 3) |
| SV-4 | kernel/supervise.md §3.8.2 | the instance sweep (answers loop Q-L4's second half and host Q-H4 for tool groups) | ACCEPT | loop.md §9 Q-L4 (marked resolved) | — |
| GA-1 | kernel/gate.md §9 Q-BT1 → boot | probe mode must not change the head (tools, system, **params**) | ACCEPT | boot.md §3.9. The probe now uses the session's normal effort and default `max_tokens`; the draft's `effort: Low` would have changed the head hash and failed criterion 5 | — |
| GA-2 | kernel/gate.md §9 Q-BT1 → boot | probe mode must not disable the loop | ACCEPT (already in draft) | boot.md §3.9 (probe uses the loop machinery), §3.13 (serves inputs for G3) | — |
| GA-3 | kernel/gate.md §3.5 criterion 3, Q-BT1 → boot | the probe must include a tool call whose result is sent back (`tool_round_trip`) | ACCEPT | boot.md §3.9 (2 requests, 3 with one nudge; probe tool policy; results always sent), §4.2 `probe.requests`/`tool_round_trip`, §8 test 7; provider.md §3.12 example shows the probe turn pair; tools.md §4.6 `probe_policy` | testing.md: one `probe` turn **pair** per probe-mode boot in scenario scripts |
| GA-4 | kernel/gate.md §9 Q-BT1 → boot | the boot round trip sends `x-init-purpose: probe` | ACCEPT (already in draft) | boot.md §3.9 | — |
| AW-1 | awareness.md §9 X5 → boot | probe instruction `[init:probe] Gate probe: call the bash tool once with the command "true", then reply with the single word ready.` | ACCEPT (verbatim; the draft had dropped "then reply ready", which the round trip now makes possible) | boot.md §3.9 step 2 | — |
| SM-1 | kernel/selfmap.md §9 Q-B1; kernel/proxy.md §9 Q-P6 → boot | manifest `tools_sha256` with `init-tools-v1` | ACCEPT | boot.md §4.3 | kernel/selfmap.md: the mismatch can now be a G1 failure |
| SM-2 | kernel/selfmap.md §9 Q-P1 → provider, tools | where per-model profiles live | ACCEPT | tools.md §2 and provider.md §3.1. Grown profiles go in `src/profiles/<family>.rs`; the seed ships none; `src/provider/models.rs` (the model table) is not a profile | — |
| KPG-BT3 | reconciliation/kernel-proxy-gate.md BT-3 → boot | optional `ext_api_majors` field in the manifest | ACCEPT | boot.md §4.3 (`"ext_api_majors":[1]`) | — |
| PX-1 | kernel/proxy.md §9 Q-P1 → provider | answers to (a)–(f); `upstream.auth` 401 is fatal; proxy-generated 4xx are not retryable | ACCEPT | provider.md §3.9 (`x-init-error` never retried), §3.3 and §3.12 (flag renamed `proxy.test_mode`), §9 Q-P1 | — |
| PX-2 | kernel/proxy.md §9 Q-P2 → provider | DeepSeek: echo `reasoning_content`; never vary `user_id`; tolerate `: keep-alive`; `insufficient_system_resource` retryable | ACCEPT (already in draft) | provider.md §3.7, §3.8.1, §3.9 | — |
| PX-3 | kernel/proxy.md §9 Q-P3 → provider | OpenAI WebSocket mode: watch `x-init-ws-conn`, handle `previous_response_not_found` | ACCEPT (met by design) | provider.md §3.8.3. `previous_response_id` is never sent, so neither can matter; the error, if it ever arrives, is `InvalidRequest` and logged as a body bug | — |
| PX-4 | kernel/proxy.md §9 Q-P5; evolution.md §9 Q1 → provider | send `x-init-task` when a task is active | ACCEPT (mechanism) | provider.md §3.3 `Request.task`, §3.6 header, §9 Q-P7 | evolution.md / kernel/supervise.md: define how the body learns the task id (an `input` field, an initctl command, or a file) |
| EV-1 | evolution.md §9 Q1 → provider | gate runs carry an `X-Init-Gate` header set through the gate body's env | REJECT | — | evolution.md: drop the header. The body never sends it: a header the body sends can be forged or omitted, while gate traffic is attributed by the probe lane's socket (proxy.md §3.4; same decision as kernel-proxy-gate EV-1b) |
| CF-1 | kernel/config.md §9 Q-C3 → provider | `model.provider`, `model.id`, `model.effort` are copied into `boot.json` | ACCEPT | provider.md §3.3, §9 Q-P2: the body reads `boot.json.model.{provider, model, effort}` as supervise §4.3 defines | kernel/supervise.md: copy config `model.id` into `boot.json.model.model`, and fix the §4.3 example `"deepseek-chat"` (not a current DeepSeek id; the body rejects it) |
| LC-1 | body/log-context.md §9 X4 → provider | `CacheProfile` values; length errors → `ContextOverflow`; `thinking.native` and `raw_arguments`; Anthropic binding beta; normalized usage; image token formula | ACCEPT (done in draft reconciliation) | provider.md §3.2, §3.8.2, §4.1 | — |
| LC-2 | body/log-context.md §9 X7 → loop | the §3.13 boundary order, D1/D2 sync points, no epoch inside an open tool round | ACCEPT | loop.md §3.3 (table, plus an explicit "no epoch inside an open tool round" statement), §3.7 (D1 markers) | log-context.md: split `on_boundary` into `boundary_append` + `next_request` (loop Q-L2a); allow a harness-source user message for the `[init:cutoff]` note (Q-L2b) |
| LC-3 | body/log-context.md §9 X8 → tools | results carry `tokens_est`, `spill_path`, `exit_code`, `wall_ms`; images as blob blocks; `edits[].old_text`/`new_text` | ACCEPT (already in draft; `tokens_est` is computed by log-context at append) | tools.md §3.5.6 `ToolMeta`; provider.md §3.2 | — |
| LC-4 | body/log-context.md §3.8.2 (head) | head tools are sorted byte-wise by `name` | ACCEPT | tools.md §4.1 (reordered `bash, edit, ext, read, write`; size now 3,407 bytes), §3.1; provider.md §3.3 `Request.tools`; boot.md §4.3; loop.md §3.6 `unknown_tool` text | — |
| LC-5 | body/log-context.md §3.4.1 diagram | the new body does a "probe round trip" on every handoff boot | ESCALATE-OWNER (part of E1) | boot.md §3.10 (marked escalated) | log-context.md: align the diagram with the owner's E1 decision |
| TS-1 | testing.md §9 Q3, §3.4.4 → boot | every boot (cold, handoff, rollback, G2, G3) makes the boot round trip with purpose `probe` | ESCALATE-OWNER (E1) | boot.md §3.10, §9 Q-B2 | testing.md: scripts follow the E1 decision |
| TS-6 | testing.md §3.10 E2E-1 | "cold boot … did one real round trip and one tool call" | ESCALATE-OWNER (part of E1) | boot.md §8 scenario mapping notes the dependency | testing.md: if E1 keeps probe-mode-only, E2E-1 checks a probe-mode boot (or the first turn) |
| TS-2 | testing.md §9 Q9 → provider | ownership split: provider.md owns the script format and rendering; testing.md the binary and extensions | ACCEPT (already in draft) | provider.md §3.12 | overview.md: doc map "mock model: format in provider.md, harness in testing.md" |
| TS-3 | testing.md §9 Q11 → provider | accept `--strict deepseek`, `--simulate-cache`, extra recording fields | ACCEPT (already in draft) | provider.md §3.12, §9 Q-P4 | — |
| TS-4 | testing.md §9 Q13 → loop | reference M-5 instead of "±10%" | ACCEPT (already in draft) | loop.md §8 item 21 | — |
| TS-5 | testing.md §3.2 L1 "seed tools" | "`bash` … 30k tail truncation" | REJECT | — | testing.md §3.2: say "30k head (4 KB) + tail (26 KB) truncation". tools.md spec delta 1 keeps head+tail, because the first compiler error is at the top (Claude Code and mini-swe-agent do the same) |
| RM-1 | roadmap.md §9 Q2 → all component docs | each test plan names testing.md scenario IDs | ACCEPT | provider.md §8, loop.md §8, tools.md §8, boot.md §8 ("Scenario IDs from testing.md") | — |
| IC-1 | kernel/initctl.md §10 (promote non-blocking) | answers loop Q-L1b (deadlock risk) | ACCEPT | loop.md §9 Q-L1b (marked resolved) | — |
| SB-1 | kernel/sandbox.md §3 profile | `process-exec` and same-sandbox `signal` (answers tools Q-T2) | ACCEPT | tools.md §9 Q-T2 (marked resolved) | — |
| SR-1 | self-raised from GA-3 and gate Q-SV1f | every probe-mode boot now makes 2–3 requests; G4 task runs in probe mode would pay that in both arms | ACCEPT (note) | boot.md §9 Q-B6 | kernel/gate.md: run G4 tasks with `ProbeOpts.mode = normal`, or count the probe pair explicitly |
| SR-2 | self-raised: version pins | `wasmtime` `=48.0.6` (ext/host.md §3.2) vs `49.0.3` (roadmap.md M0) | ACCEPT (follow host.md, which owns the host's dependency) | boot.md §3.16 pins `=48.0.6` with a note | roadmap.md M0 and testing.md M-1: use `48.0.6` (host.md §9.2 already asks testing) |

## Escalated to the owner

**E1 (TS-1, LC-5, TS-6): should normal and safe boots also make the probe model round trip?**
- **Recommended:** keep the round trip to **probe-mode boots only**: G2, G3, and G4 runs if they use probe mode.
- **Alternative:** spec §4 literally: every boot, including cold, crash, rollback and handoff.

| | Probe-mode only (recommended) | Every boot |
|---|---|---|
| Model calls per boot | 0 on live boots | 2–3 on every boot. Cheap on DeepSeek (cached prefix ≈ $0.001–0.01); on Opus 5.5 a 200k-token history is ≈ $0.04 per boot pair |
| Handoff latency | unchanged | plus one or two model round trips before `ready`. With slow reasoning models this risks the 90 s `ready` deadline, failing a good swap because the model was slow |
| Crash loops | no extra spend | each crash restart pays the pair, up to 5 times before safe mode |
| What it catches | G2 already probed the exact binary before any handoff; on a live boot the first real turn surfaces a broken provider path as a logged error | a broken provider path before `ready` on unprobed boots (cold boot after install, crash restart) |
| Other docs | testing.md §3.4.4 scripts and E2E-1, and log-context's diagram, change to probe-mode boots | boot.md §3.10 flips; §3.9 runs unchanged in all modes. The exchange must then be appended to the **live** log (or special-cased to stay out) |

## Requests to the main session (CONVENTIONS.md)

1. **§4 contract registry, new row:** "Seed crate build configuration (`seed/Cargo.toml` dependencies, `[workspace] exclude`, profiles; `seed/.cargo/config.toml`)" → owner `body/boot.md` (§3.16). Today no row covers it, and ext/authoring.md asked "body/boot.md (or whoever owns the seed's Cargo.toml)".
2. **§2 repository layout:** under `seed/`, add `.cargo/config.toml` (vendored, offline), `wit/init-ext/v1/` and `ext/` (the extension workspace with its own `Cargo.lock`). Source: ext/authoring.md §3.3.
3. **§3 runtime layout:** add `state/spill/<session_id>/`, agent read-write, holding long tool outputs (body/tools.md §4.4; cleaned at boot by exact path after 14 days, body/boot.md §3.6).
4. **§5 global decisions, wording:** change "The body has `#![forbid(unsafe_code)]` and no async runtime: std threads only" to "…and no async runtime in body code (std threads only); the tokio runtime that `wasmtime-wasi` starts internally for WASI I/O is accepted (ext/host.md delta 10.3, ADR-003)".
5. **§4 env-var row** (owner supervise.md; consistency only): the body now also validates `INIT_BOOT_ID` and `INIT_UI` and reads `INIT_KERNEL_VERSION` (supervise.md §4.1), and honors `INIT_TEST_*` only under `INIT_TEST=1` (testing.md §3.4.6).
6. **§5 dependency versions:** pin `wasmtime`/`wasmtime-wasi` once. ext/host.md says `=48.0.6` (LTS) and roadmap.md M0 says `49.0.3`; body/boot.md §3.16 follows host.md.

## R2

Applied 2026-10-19 against CONVENTIONS.md v0.2 and the follow-up columns (and R2 sections, where present) of all six ledgers. The five docs are now `draft v0.3 (reconciliation round 2)`.

### Follow-ups applied (id → doc §)

| id (ledger) | follow-up | applied in |
|---|---|---|
| SV-1 (evolution-testing); EV-6 (kernel-supervise); EV-4 (body-log-awareness) | run evolution §3.6's current, reason-agnostic resync script in the body's boot; pass `stash` into the boot facts | boot.md §3.6 (step-for-step: fetch, stop on fetch failure; ancestry test; ff / `branch -f main` / `rolled-back/<sha12>` + stash push, `switch -C`, pop only a new stash; probe: nothing; safe: fetch only; G4 runs in probe roots move only the ref), §3.8 (`git_sync.rolled_back`; log-context's `boot.stash` = `git_sync.stash`), §8 test 13 |
| SV-11, PX-3 (evolution-testing); PV-1 (kernel-supervise) | `Request.task` = id of the owner input that started the run, on every request of the run | loop.md §3.3 "Task id" (rule: `input_id` of the latest owner `message` on the path, so follow-ups start a task, steers do not, restarts need no state; compaction requests included), §4.1, §8 test 22; provider.md §3.3 `Request.task` comment |
| RS-3 (evolution-testing) | list the probe, G3 and per-task responders as accepted harness behaviour | provider.md §3.12 "Accepted harness behaviour" |
| LP-1 (kernel-supervise) | honour `input.mode` | loop.md §3.4 (follow-up via `/later`; missing or unknown mode = steer), §4.1, §8 test 23 |
| E7 (ext) | `reload{kind:"kept", components}` → `begin_reload(Kept(components))`, `kind:"request"` → `Request` | loop.md §3.1 thread table, §3.8.2, §4.1, §4.2 (`ReloadKind`), §8 test 14 |
| E1 (ext) | pass the abort flag in `CallCtx`; map `Aborted` → `aborted`; update the abort table | loop.md §3.1 (`Abort::flag()`), §3.5 table (ext returns `Aborted` within 200 ms), §4.2, §8 test 24; tools.md §3.6 (`CallCtx.abort`, step 4, kind mapping), §4.5, §4.6, §8 item 5 |
| E3 (ext) | `run_group` takes a plain `&AtomicBool` cancel and keeps stdout and stderr separately (last 1 MiB each) | tools.md §3.5.7 (`RunSpec{stdin, timeout, cancel, on_spawn, capture}`, `CaptureSpec::{Merged, Separate{max_each}}`, 10 ms cancel slices, TERM→KILL, stdin writer; callers table: bash, host `process.run`, boot git sync), §4.5, §8 item 6 |
| E23 (ext) | adr/003: `allow_blocking_current_thread(true)` removes only the `spawn_blocking` hop, not the runtime; mention rayon | adr/003 Consequences (rewritten from host.md §3.2's verified facts), Decision ("no async code in the body", CONVENTIONS §5), Context row, rayon alternative, thread inventory, References |
| E25 (ext) | ship `skills/ext-authoring/assets/template/Cargo.toml.template` and `src/lib.rs` | boot.md §3.16 |
| BT-3 (kernel-supervise) | diagram "settle 3 s (B alive)" | boot.md §3.15 diagram, §3.12 |
| BT-4 (kernel-supervise) | two consecutive exit 78 → `Halted{config}` | boot.md §3.4, §4.4, §5, §10 item 4 |
| BT-2 (kernel-supervise) | cwd = `$INIT_HOME/work` | boot.md §3.2; Q-B3, Q-L1c, Q-T3 closed |
| BT-3 (kernel-proxy-gate) | optional `ext_api_majors` | boot.md §4.3 (present since R1; confirmed) |
| TL-2 (body-log-awareness) | say `wall_ms` | tools.md §3.5.6 already did; Q-T4 closed |
| E1 (body-core), settled by the main session | probe round trip on probe-mode boots only | boot.md §3.10 (escalation text removed), §8 E2E-1, §9, §10 item 1 |
| CONVENTIONS v0.2 §2–§5 | layout, wasmtime, ICJ, money, env vars, error and exit codes | **Layout:** boot.md §3.16 seed tree (`GENESIS.md`, `SELF.md`, separate `ext/` workspace, WIT copy, template), tools.md §4.4 (`state/spill/` in §3). **wasmtime `=48.0.6`:** boot.md §3.16 cites §5; the roadmap note is gone. **ICJ:** bash `timeout` is now `"type":"integer","minimum":1` (tools.md §4.1, registry remeasured at 3,405 bytes; §3.1, §3.5.6, §4.5, test); `Usage::hit_permille` replaces an `f64` ratio (provider.md §3.2). **Money:** these docs write no money values. **Env:** `INIT_SESSION` may be empty, and log-context generates `YYYYMMDD-HHMMSS-<6hex>` (boot.md §3.2, §3.7). **Error codes:** every loop `error.code` is in log-context §4.2's registry; tool codes stay in tools.md §4.6. **Exit codes:** boot.md §4.4 |
| CONVENTIONS §6 pending | keep the recommended value, marked | ESC-2 (`/opt/homebrew` readable; default yes): tools.md §3.5.1, boot.md §3.2, with the "no" fallback stated (`/bin/bash` 3.2.57). ESC-1 (vendor approval; default no): boot.md §3.16. OW-1, OW-2 and E1/E2 (proxy/gate) are not referenced by these docs; G4 = 90 min (settled) is not referenced either |

### Not applied, and why

- **TL-3 (body-log-awareness R1): tools in "registry order read, write, edit, bash, ext".** Not applied. log-context's own R2 (its LC-4 row) withdrew TL-3 and returned §3.8.2 to "sorted byte-wise by name", which equals tools.md §4.1's order (`bash, edit, ext, read, write`). Reverting now would re-cross the two docs.
- **BT-5 (body-log-awareness R1): probe instruction without "then reply with the single word ready", "sent but not logged".** Nothing to apply. awareness R2 (its AW-1 row) adopted boot.md §3.9's text and the staging-copy logging; boot.md §3.9 now cites awareness §3.6 instead of "adopted verbatim (X5)".
- **CONVENTIONS §3 "session id … chosen by the kernel".** It is not my doc. supervise.md §3.4, CONVENTIONS §4 and log-context say the body generates it when `INIT_SESSION` is empty, and boot.md follows them. body-log-awareness R2 already asks the main session to fix §3.

### Remaining open questions

- **Cross-doc, Q-L7 → body/log-context.md §4.3.** `next_request` should fill `Request.task` by loop.md §3.3's rule for every request it builds, compaction included. Until then, the loop sets it on turn requests itself, and compaction requests are booked as `_none`.
- **Owner, not in CONVENTIONS §6.** The stated defaults ship unless the owner objects:
  - provider Q-P5: Anthropic refusal fallbacks off;
  - provider Q-P6: DeepSeek effort `high`;
  - tools Q-T5: bash timeout 120 s;
  - tools Q-T6: kill unredirected background jobs after 2 s;
  - loop Q-L5: a plain message while busy is a steer;
  - loop Q-L6: a length stop with no tool calls auto-continues, up to 3 strikes;
  - boot Q-B8: G2 probes use the real model.

  **Request to the main session:** list these seven as one §6 row, "body-core defaults", or record them as accepted.
- **Pending in CONVENTIONS §6 that touch these docs:** ESC-1 and ESC-2 (above).
- **Phase 0 checks (UNVERIFIED until measured), unchanged:**
  - Linux `kill -s SIG -- -PGID` (tools.md T-L1);
  - the DeepSeek live checks V-DS-* (provider.md §8);
  - the Anthropic image-token formula (provider.md §4.1);
  - M-5 footprint (loop.md item 21).
