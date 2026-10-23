# Reconciliation R1: body-log-awareness

```
Status: R1 · 2026-10-19 · Area: body + overview
Docs covered: body/log-context.md, awareness.md
```

Sources: every "Open questions", "Cross-doc" and "Spec deltas" section of the 26 committed docs, plus inline mentions of these two docs and of the contracts they own (CONVENTIONS §4: event envelope and types, context assembly, cache layout, compaction, genesis prompt, `SELF.md`, environment snapshot, self-signals). The sibling ledger `reconciliation/kernel-proxy-gate.md` was also read.

Totals: **52 items**. 46 accepted, 4 rejected, 2 escalated to the owner.

## Ledger

| id | from doc § | question | decision | change made (your doc §) | follow-up that ANOTHER doc must make |
|---|---|---|---|---|---|
| PV-1 | body/provider.md §9 Q-P3(a), §3.2 | add `stop_details` (refusal category) to the `message` schema | ACCEPT | log-context §4.2 `message.stop_details` | — |
| PV-2 | body/provider.md §9 Q-P3(b), §4.1 | add an `opaque` block `{"type":"opaque","api","raw"}` | ACCEPT | log-context §3.3, §4.2 `block_opaque`, `api` enum | — |
| PV-3 | body/provider.md §9 Q-P3(c)(d) | epochs and `strip_thinking_before_epoch` as log-context defines them; notes append-only | ACCEPT (confirmation; no change) | — | — |
| PV-4 | body/provider.md §3.1 `Origin`, §3.10 | the handoff filter keys on the assistant message's `api` and `response_id` | ACCEPT | log-context §3.3, §4.2 `message.api`, `message.response_id` | — |
| PV-5 | body/provider.md §3.1 `Thinking.provider` | provider id `mock` must be valid in thinking blocks | ACCEPT | log-context §4.2 `block_thinking.provider` is a string; §3.10.0 lists `mock` | — |
| PV-6 | body/provider.md §3.6 (DeepSeek `stream_options`) | adapter leaves out `stream_options.include_usage`; log-context's skeleton shows it | ACCEPT | log-context §3.10.1 skeleton and note | — |
| PV-7 | body/provider.md §3.2 `CacheProfile` | provider.md owns the values (adopted as proposed, `mock` = `deepseek`) | ACCEPT | log-context §3.10.0 text | — |
| PV-8 | body/provider.md §9 Q-P1; kernel/proxy.md Q-P4a | head-hash extraction must cover provider id `mock` | ACCEPT | log-context §3.10.4 keyed by wire format (see PX-1) | — |
| LP-1 | body/loop.md §9 Q-L2(a), §4.2 | split `on_boundary` into `boundary_append` (steps 1–4) and `next_request` (5–8) | ACCEPT | log-context §4.3 | — |
| LP-2 | body/loop.md §9 Q-L2(b), §3.6 | a non-owner user message `source:"harness"` for the `[init:cutoff]` note | ACCEPT | log-context §3.3, §3.9, §4.2 `message.source` enum `owner`/`harness` | — |
| LP-3 | body/loop.md §9 Q-L2(c), §3.9 | register the `error` codes the loop writes | ACCEPT | log-context §4.2 `error.code` registry; `where` adds `loop` | — |
| LP-4 | body/loop.md §4.2 | trait additions: `has_pending_work`, `request_emergency_compaction`, `leaf`; `pause` returns `(u64, String)`; `resume` takes `Option<Value>` | ACCEPT | log-context §4.3 | — |
| SV-1 | kernel/supervise.md §9 item 3, §4.5 `input` | owner messages carry `input_id`; deduplicate on redelivery | ACCEPT | log-context §3.3 (exactly-once paragraph), §3.7 index sets, §4.2 `message.input_id`, `steer.input_id` | — |
| SV-2 | kernel/supervise.md §9 item 3, §4.5 `notice` | notices carry the kernel notice `id`; deduplicate | ACCEPT | log-context §3.3, §4.2 `notice_id` on `gate`, `update_failed`, `thinning_done`, `notice` | — |
| SV-3 | kernel/supervise.md §9 item 3, §4.5 `paused` | `paused` frame fields are `log_len` and `leaf` | ACCEPT (already) | log-context §3.4.1 diagram, §4.3 | — |
| SV-4 | kernel/supervise.md §3.4 | the kernel chooses new session ids (`YYYYMMDD-HHMMSS-<6 hex>`, validated `^[0-9A-Za-z._-]{1,64}$`) and records them in `boot.json.session`; the body creates the file | ACCEPT (replaces log-context's own id format) | log-context §3.2, §3.5 (`/new` uses the same format), §3.6 step 1, §4.1, §4.2 `sid` | main session: CONVENTIONS §3 note (below) |
| SV-5 | kernel/supervise.md §4.3 `boot.json` | boot event mirrors the real `boot.json` shape (`model` object, `provisional` object, `selfmap`, `evolution`, `update_failed` with `seq`, `exit`, `stderr_tail`) | ACCEPT | log-context §4.2 `boot` (adds `boot_id`, `effort`, `facts`, derived `provisional`) and the new "boot field sources" table | — |
| SV-6 | kernel/supervise.md §4.3 `update-failed.json` | the `stage` value list | ACCEPT | log-context §4.2 `update_failed.stage`, `seq`, `job`, `from_sha`, `signal`, `log_tail` (= `stderr_tail`); awareness §3.6 optional-line table | — |
| SV-7 | kernel/supervise.md §4.5 `notice` | kinds `lkg`, `promoted`, `provisional`, `budget`, `safe` map to `notice` | ACCEPT | log-context §3.13, §4.2 `notice.code` | — |
| SV-8 | kernel/supervise.md §3.4, §4.5 (answers log-context X1/X2) | `INIT_SESSION` always set; notices over stdio frames, not a kernel-socket subscription | ACCEPT | log-context §4.1, §3.13, §9 X1–X2 marked answered | — |
| SV-9 | kernel/supervise.md §4.5 `input` | slash commands arrive as ordinary `input` text and must be exactly-once too | ACCEPT | log-context new `command` event (§3.3, §4.2), §4.5 | — |
| SV-10 | kernel/supervise.md §3.6 (rollback), §9 item 4 | `handoff` reason = planned restart onto the same sha (`/clear-safe`, model change); no cause field | ACCEPT (the body derives the cause) | log-context §4.2 boot field sources (`cause`); awareness §3.6 handoff row, §9 X1 | — |
| BT-1 | body/boot.md §9 Q-B2, §10 item 1 | only probe-mode boots make a model call; log-context's handoff diagram showed a "probe round trip" | ACCEPT | log-context §3.4.1 diagram; awareness §3.4 (`SELF.md` boot line) | testing.md: one `probe` turn per probe-mode boot only (boot.md Q-B2) |
| BT-2 | body/boot.md §3.8 | boot facts handed to log-context; log-context adds `prefix_hash` and the repair count | ACCEPT | log-context §3.6 step 12, §4.2 `boot.facts`, `boot.log_repair` | — |
| BT-3 | body/boot.md §3.3, §3.14 | safe mode reads T0 data from `boot.json.lkg_tree` | ACCEPT | log-context §3.8.2 `<head_root>`, §3.6 step 11 | — |
| BT-4 | body/boot.md §3.3 | `update_failed` deduplicated by its `seq` | ACCEPT | log-context §3.3, §3.7, §4.2 | — |
| BT-5 | body/boot.md §3.9 (answers awareness X5) | probe instruction without "then reply ready" | ACCEPT | awareness §3.6 probe row, §9 X5 | — |
| BT-6 | body/boot.md §3.2 | `cwd` and the resolved bash path and version are boot facts (tools.md wants bash in the boot event) | ACCEPT | awareness §3.5.1 (`cwd`, `bash`), §3.5.2, §4.2 | — |
| TL-1 | body/tools.md §9 Q-T4 | masking must keep `spill_path` | ACCEPT (already) | log-context §3.11 (event keeps the field; placeholder names the path) | — |
| TL-2 | body/tools.md §9 Q-T4 | name the duration field `ms` | REJECT | — (`wall_ms` kept) | body/tools.md: say `wall_ms`. Reason: provider.md `ToolMeta` and loop.md already use `wall_ms`; a unit in the name is the integer-only convention |
| TL-3 | body/tools.md §4.1 | tools serialized in registry order (read, write, edit, bash, ext); log-context sorted them by name | ACCEPT | log-context §3.8.2 (registry order; promoted tools appended) | — |
| TL-4 | body/tools.md §4.4, §9 Q-T3 | spill files live in `state/spill/<session>/` | ACCEPT | log-context §3.2, §3.12.4 example; awareness §3.4 `SELF.md` | main session: CONVENTIONS §3 (below) |
| TL-5 | body/tools.md §4.2 / provider.md `ToolMeta.signal` | record the killing signal of a tool | ACCEPT | log-context §4.2 `tool_result.signal` | — |
| PX-1 | kernel/proxy.md §9 Q-P4(a), §3.12.1 | key head-hash extraction by route family (wire format), not provider id | ACCEPT | log-context §3.10.4 table now keyed by `chat_completions`/`messages`/`responses` (= `openai-chat`/`anthropic`/`openai-responses`); `provider` = path id | kernel/proxy.md §3.12.1: drop "this mapping is an assumption" |
| PX-2 | kernel/proxy.md §9 Q-P4(b), §4.3 | `x-init-head-hash`, `last_head_hash`, `cache.*_permille` done | ACCEPT (no change) | log-context §3.10.4 item 5; awareness §4.4 (stable fields), §9 X2 | — |
| EV-1 | evolution.md §9 Q9(a) | `retire_candidates` and `thin_candidates` items are objects | ACCEPT | log-context §4.2 `boot`; awareness §3.6 optional lines | — |
| EV-2 | evolution.md §9 Q9(b) | `update_failed.stage` values | ACCEPT (supervise.md's superset) | log-context §4.2 `update_failed.stage` | — |
| EV-3 | evolution.md §9 Q9(c), §3.10.5 | `predict_hit_rate` as permille | ACCEPT (already) | — | — |
| EV-4 | evolution.md §3.6; kernel/supervise.md §9 item 4 | the stash step at rollback moves from the kernel to the body's boot | ACCEPT | log-context §4.2 boot field sources (`stash` from the body's own git step) | body/boot.md: run the stash/pop step on reason `rollback` and pass `stash` into the boot facts |
| GT-1 | kernel/gate.md §3.5 criterion 7 (answers log-context X5) | continuity check adopted | ACCEPT | log-context §3.10.4 closing paragraph, §9 X5 | — |
| GT-2 | kernel/gate.md §3.5 criterion 6 | boot honesty: the copied log gains a `boot` event whose `prefix_hash` equals the proxy's | ACCEPT (consistent) | log-context §3.10.4 item 6 | — |
| SM-1 | kernel/selfmap.md §9 Q-AW1, §4.4 | use selfmap's recipes | ACCEPT | awareness §3.7, §3.4 (`SELF.md` "How to check me") | — |
| SM-2 | kernel/selfmap.md §3.7 | `<!-- @init compensates-for -->` blocks may sit in `GENESIS.md`/`SELF.md` | ACCEPT | log-context §3.8.2 (files go into the head verbatim, markers included) | — |
| SM-3 | kernel/selfmap.md §3.3 | awareness could show a `model_tokens / o200k_tokens` calibration ratio | REJECT | — | — Reason: no decision depends on it; the boot block shows the model-independent `o200k` size and its delta, and the proxy already reports real prompt tokens |
| SM-4 | kernel/selfmap.md §9 item 2 | should a body-size threshold warn? | REJECT | — (awareness §3.7: size and delta shown, no warning) | — Reason: the gate's smaller-body tie-break is the pressure; a warning line in every boot block would be prompt scaffolding without an observed failure |
| HO-1 | ext/host.md §4.3, §9 | `reload` event for T1 from `ReloadReport`; `components` = changed names only | ACCEPT | log-context §4.2 (`reload` is one event per tier), §4.3 (text starts with `[init:reload]`) | ext/host.md: start the report text with `[init:reload] ext generation N` (log-context X9) |
| TS-1 | testing.md §9 Q7, §3.5 F-corrupt-tail | damaged line left in place, `\n` appended first, boot reports the skipped line | ACCEPT (replaces sidecar + truncation) | log-context §3.1 P1, §3.4.4, §3.6 steps 3–4 and 13, §4.2 `boot.log_repair`, §5 L1, §8 T-LOG-2; sidecar removed from §3.2, §4.1 | — |
| TS-2 | testing.md §3.5 (A3 mock, M4) | name the prefix-hash field of boot events | ACCEPT (it is `prefix_hash`) | log-context §9 X10 | — |
| WI-1 | ext/wit.md (promoting an extension to a native tool) | the tool-list change is "batched to a session or compaction boundary (log-context)" | REJECT (no deferral) | log-context §3.8.2 clarifies: a T2 head change takes effect at the new body's boot and is one epoch | ext/wit.md: reword to "takes effect at the handoff (one cache miss); promote right after a compaction to combine the misses". Reason: the new code needs its own tool list; deferring would run new code with an old head |
| CF-1 | reconciliation/kernel-proxy-gate.md AW-1; kernel/config.md §4.3 | render the owner's local time using `budget.day_starts_at_utc_offset` | ACCEPT | awareness §3.5.1 `owner_offset` (derived from `spend.day_starts_at`, no TOML parsing in the body), §3.5.2, §4.2, §9 internal item 3 | — |
| OW-1 | awareness.md §3.3 (coordinator) | `CONSTITUTION.md` text needs owner approval | ESCALATE-OWNER | — | owner (below) |
| OW-2 | awareness.md §3.2; spec App. A | final genesis prompt text | ESCALATE-OWNER | — | owner (below) |

Own follow-ups raised in this round (not counted above):
- **kernel/supervise.md:** add `crash {exit_code, signal, stderr_tail ≤ 4 KiB}` to `boot.json` when the reason is `crash`. Until then the body uses `supervisor.json` `crash.last_exit` and points at `state/kernel/logs/` (awareness §9 X1).
- **kernel/initctl.md:** add `kernel.constitution_sha256` to `status` (awareness §9 X3). kernel/proxy.md Q-I1 also asks for integer `budget` and `cache_hit_rate`.
- **ext/host.md:** the `[init:reload]` tag (HO-1).

## Owner decisions

| id | Question | Recommendation | Trade-off |
|---|---|---|---|
| OW-1 | Approve the `CONSTITUTION.md` text (awareness.md §3.3): Article 0 (Survival), Articles 1–4 as the owner wrote them, the five invariants, and "Left out on purpose". | **Approve as written**, including the sentence "A perfect score on a canary is treated as cheating." | The text is about 700–780 tokens, cached on every request. The canary sentence states existing kernel policy and reveals no holdout content. Leaving it out hides a rule the agent will meet anyway, and makes a rejected candidate harder to understand. Each wording change later is one cache miss and a kernel change. |
| OW-2 | Approve the genesis prompt (awareness.md §3.2, about 300 tokens). | **Approve as written.** | Compared with spec App. A, it names the tiers T0/T1/T2, adds `Prefix-change`, the `[init:…]` tag rule and recoverable masked outputs, and drops the tie-break and deletion sentences, which the constitution states right after. Repeating them would cost about 30 tokens per request and duplicate instructions. The agent may edit this file later (T0), but only through G0–G1, and a change needs `Prefix-change: yes` and `Repro:`. |

## Requests to the main session (CONVENTIONS.md)

1. **§2 seed layout:** add `GENESIS.md` next to `SELF.md` (`seed/GENESIS.md`, installed as `body/GENESIS.md`). It is the first part of the system prompt and T0 data. kernel/selfmap.md already counts it as a body file.
2. **§3 runtime layout, `state/`:**
   - add `spill/<session_id>/` (agent read-write; tools.md §4.4);
   - add `blobs/` (agent read-write; content-addressed images referenced by events, log-context §3.2);
   - note that `<session_id>` is chosen by the kernel as `YYYYMMDD-HHMMSS-<6 hex>` (kernel/supervise.md §3.4) and passed as `INIT_SESSION`.
3. **§4 contract registry:** add three rows owned by `body/log-context.md`:
   - "INIT canonical JSON (ICJ v1)" (§3.8.1);
   - "`prefix_hash` (head hash) definition" (§3.10.4), recomputed by kernel/proxy.md and checked by kernel/gate.md;
   - "`error.code` registry" (§4.2).

   Also fix the "Event log envelope" row's note to read "sorted keys, integers only".
4. **§5 global decisions, Determinism:** extend to "anything that reaches a model prompt, the event log or a hash is ICJ: sorted keys, and integers only (money in micro-USD, ratios in permille, durations in ms)". The proxy, gate, selfmap and config docs already follow this.
5. **§5 (or a status note):** `CONSTITUTION.md` (repo root, installed to `kernel/CONSTITUTION.md`) is pending owner approval (OW-1).

## R2

Inputs: the follow-up columns of all six R1 ledgers, and CONVENTIONS.md v0.2 §2–§6. Both docs are now `draft v0.3 (reconciliation round 2)`.

### Follow-ups applied

| id (ledger) | follow-up | applied in |
|---|---|---|
| SV-3 (body-core) | carry `input_id` and the notice `id` in events | already done in R1: log-context §3.3, §3.7, §4.2 (`input_id`, `notice_id`) |
| LC-2 (body-core) | split `on_boundary`; harness-source user message | already done in R1: log-context §4.3, §3.3, §4.2 |
| LC-4 (body-core) | head tools sorted byte-wise by name; tools.md reordered its registry to `bash, edit, ext, read, write` | log-context §3.8.2 back to "sorted by name", which now equals the registry order. This supersedes my R1 TL-3 (registry order), which crossed with tools.md's change |
| LC-5 (body-core) | align the handoff diagram with the decision on probe round trips | log-context §3.4.1 (no model call on handoff); awareness §3.6 probe row cites the main session's settlement (probe-mode boots only) |
| AW-1 (body-core) | boot.md adopted the probe instruction verbatim, including "then reply with the single word ready"; the exchange is appended to the staging log copy | awareness §3.6 probe row (supersedes my R1 BT-5 note); the instruction is a `message{source:"harness"}` |
| AW-1 (kernel-proxy-gate) | render the owner's local time from the budget-day offset | awareness §3.5.1 `owner_offset`, now derived from `boot.json.env.budget.day_starts_at` |
| LC-1 (kernel-proxy-gate) | head-hash extraction keyed by route family | already done in R1: log-context §3.10.4 |
| AW-1 (kernel-supervise) | read `branch`, `cause` (`clear_safe`/`model_change`/`owner_restart`), `crash {exit_code, signal, log_tail}`, `env` and `selfmap.body_tokens_prev` from `boot.json`; evolution fields from `boot.json.evolution.*`; `stash` from the body's own sync | log-context §4.2 boot schema (`cause` enum, `crash.log_tail`) and field-source table; awareness §3.5.1 and §4.2 (kernel half = `boot.json.env` verbatim; the R1 "layout" group is removed), §3.6 crash, handoff and optional-line rows. This supersedes my R1 SV-10 (derived cause) and awareness X6 (withdrawn) |
| AW-2 (kernel-supervise) | use `initctl status` nested names; `constitution_sha256` now exists | awareness §4.3, §3.9 (the check is active); log-context §4.2 `boot.constitution_sha256` uses the same `sha256:<hex>` format |
| LC-3 (kernel-supervise) | the kernel no longer generates session ids; an empty `INIT_SESSION` means the body starts a new session | log-context §3.2, §3.4.3, §3.5, §3.6 step 1, §4.1. This supersedes my R1 SV-4 (kernel-chosen id) |
| LC-5 (kernel-supervise) | consume stdin `notice` frames; drop the subscription idea | already done in R1: log-context §3.13 |
| SV-11 (evolution-testing) | task id = `input_id` of the owner input that started the run | log-context §4.2 `message.request.task_id` (the `x-init-task` value sent) |
| CONVENTIONS v0.2 §2–§5 | `GENESIS.md`, `state/blobs/`, `state/spill/`; ICJ and integer money; env vars `INIT_BOOT_ID`, `INIT_SESSION` (may be empty); error-code registry | log-context §3.2, §4.1 (adds `INIT_BOOT_ID`); awareness §3.4 `SELF.md` (non-authoritative `body/` with `origin` = `lineage/body.git`, `state/ext/catalog.json`, `skills/ext-authoring/` template; 76 lines). ICJ, money and the `error.code` registry needed no change |
| CONVENTIONS §6 | mark pending owner decisions and keep the recommended values as provisional defaults | awareness §3.2 and §3.3 headed "PENDING OWNER (CONVENTIONS §6 OW-1/OW-2)"; the E1 $5.00/day and ESC-2 `/opt/homebrew` example values are marked provisional (§3.5, §4.2, §9) |

### Not applied, and why

- **HO-1 / log-context X9 (ext/host.md):** host.md still renders `[ext reload] generation N`, and the ext ledger did not pick this up. It stays open. Until then the body prefixes `[init:reload]` to the report text (log-context §4.3). The result is deterministic and stored once, so the cache is unaffected.
- **`gate` event field names** `cost`, `wall`, `cache_hit` were kept rather than renamed to `cost_musd` and so on. They mirror kernel/gate.md's notice and evolution.md §4.5 verbatim, and the units already follow CONVENTIONS §5: micro-USD, ms, permille, integers. Renaming them would mean edits in two other docs for no change in behavior.
- **CONVENTIONS §3 (main session):** the `state/` row still says the session id "is chosen by the kernel". §4 (`INIT_SESSION` may be empty) and supervise.md §3.4 say the body generates it when `INIT_SESSION` is empty. Please change §3 to "generated by the body when `INIT_SESSION` is empty: `YYYYMMDD-HHMMSS-<6hex>`".

### Remaining open questions

- **Owner:** OW-1 (CONSTITUTION.md text) and OW-2 (genesis prompt), both pending, with the drafts used as provisional defaults.
- **Cross-doc:** ext/host.md reload tag (log-context X9 = awareness X8).
- **Internal, to be measured in Phase 0 or 2, no other doc involved:**
  - log-context §9: masking exemption for `read` outputs; whether the 200k effective-window cap is right for DeepSeek; Anthropic keep-alive.
  - awareness §9: an earlier compaction hint; `git status` in the environment snapshot.
