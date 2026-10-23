# Evo test protocol: does INIT evolve, under ADR-011, into solving tasks the genesis INIT cannot?

```
Status: v1 pre-registered 2026-10-22; **v2 (owner: minimal, §10) in force**, registered before any real run · Area: evolution (lane EVO-TEST-PREP)
Depends on: docs/adr/011-reward-free-self-extension.md, measurements/m7-results.md (harness lessons DV-1…DV-7)
Tasks: fixtures/evo-test-v1 (evo.json) · Runner: tests/real/evo_test.sh + tests/real/evo_checks.py
Freeze: PREREG v2 (§10.6) and v1 (§7, the record) are mirrored in tests/real/evo_checks.py; `evo_test.sh selftest` fails on any drift.
```

## 1. What this test can and cannot show

**The question.** Give the starter INIT (the genesis tuple, deepseek-flash) a few coding tasks it cannot solve. Let it work through some of them under ADR-011, free to build itself tools, skills, memory or loop changes, with no reward. Does it then solve **different tasks of a similar kind** that the frozen genesis twin still cannot solve, using what it built?

**Size and honesty.** About 4 training and 4 held-out tasks, 2 runs per arm. This is a **demonstration, not a statistical proof**. No p-value is computed or claimed; the report gives counts, scores and costs. Tasks are kept only when genesis failed them twice (§3.1), so some regression to the mean is expected: that is why the twin is re-run in phase 2 instead of reusing its phase-0 scores.

**"It evolved and solved"** means, and only means, all three of (§5):
1. phase 1 left at least one admitted self-extension (a capability INIT created or modified) active at its end;
2. on the held-out tasks the evolved INIT has at least 2 more solved runs (score 1000) than the twin, out of 8 each;
3. in at least one held-out task the evolved INIT solved both runs, the twin solved neither, and both solving runs used a capability created in phase 1.

Solving the training tasks proves nothing by itself (memorization); it is reported as the phase-1 trajectory.

## 2. Tasks

`fixtures/evo-test-v1/tasks/`, written fresh for this test in pool-v1's source format (task.json v3, deterministic tars) and checked by pool-v1's runner (`poolcheck.py`, copied byte-identical into each built `check/`). `tools/evo.py` builds and validates them; `evo.json` is the manifest.

| Kind | Training | Held-out | Spare | What a self-built capability could help with |
|---|---|---|---|---|
| `spec` | evo-spec-ledger | evo-spec-sched | evo-spec-tmpl (held-out) | a rule checklist and self-tests from a long spec, many test-feedback iterations |
| `nav` | evo-nav-units | evo-nav-tz | – | a code index or change-impact map across a 30+ module package |
| `debug` | evo-dbg-vm | evo-dbg-sheet | evo-dbg-diff (training) | a tracer, a state dumper or a test-case minimizer for masked bugs |
| `bigfile` | evo-big-client | evo-big-rules | – | a structure-aware bulk rewrite of a 5,000-line file with an exceptions table |

- Each pair shares a *kind* of difficulty, never code, domain or answers: a memorized training answer is useless on its held-out twin.
- Every check grades 20+ named behaviours (partial credit); visible tests cover about a third.
- Validation (`evo.py validate`, niced in the light pool): reference 1000 ×3; initial workspace ≤ 500 ×3 and stable; empty workspace < 300; partial fixture strictly between; tamper of a protected file detected; every check ≤ 30 s; byte-identical rebuild. The runner re-scores every reference **sandboxed** before any run (`build-tasks`).
- Python 3.9 (`/usr/bin/python3`, the checks' interpreter), standard library only; offline; deterministic.
- **Solutions never reach a test home.** `build-tasks` scores the references in a scratch dir, copies only `task.json`, `workspace.tar` and `check/` into `target/evo/<x>.tasks/`, and deletes the scratch by exact path; it refuses to finish if a `solution.tar` is found there. The test home runs with `sandbox.read_home = false`, and its `read_deny` also lists `fixtures/`, `tests/`, `measurements/`, `root/`, `.git`, `target/evo/<x>.tasks` (hidden checks) and `target/evo/<x>.snap`.

## 3. Procedure

All phases use the same staged binaries (`target/evo-bin`, the ADR-011 build once it lands), the same seed, the same charter (including `extend.mode = "free"` once ADR-011 lands, in **both** arms), deepseek-flash only, off-peak only. One test home per mode; its pristine copy right after install is snapshot **G0** (APFS clone, `target/evo/<x>.snap/g0`).

**Every run:** restore the home from the run's snapshot (clone back to the same path; nothing restored in phase 1 after its first attempt) · fresh workspace from `workspace.tar` · charter money ceilings for this run · fresh agent session (`state/session` removed) · start the Root · `workspace.set` on the owner socket (as the `init` TUI does) and wait until the live body runs with it · send **one owner line** · wait until the body is idle, or a guard stops the run · stop the Root, remove the key · score the workspace with the task's check, sandboxed · collect artifacts, redact · record in the ledger.

**The owner line** is the task's `prompt`, verbatim: it describes the task and its constraints and never hints at a method, tool, cause or location (`evo.py` lints it). Phase-1 retries prefix exactly: *"The previous attempt at this task did not meet the requirements. The workspace has been reset to its original state."* Nothing else ever reaches INIT: no score, no failing item, no success message, no suggestion to evolve.

### 3.1 Phase 0: genesis baseline
- Each primary task (4 training, 4 held-out) × 2 runs on clones of G0: run 1 of every task, then run 2.
- A task is **beyond capability** only if genesis scored < 1000 on **both** runs. Otherwise it is dropped, and the first spare of the same group (kind order `spec, nav, debug, bigfile`) is run × 2 and kept if it is beyond capability.
- At least 2 kept tasks per group are needed to go on; otherwise the test stops with "the set is too easy" (a $0 task revision is then a new pre-registration).
- **Early stop:** after run 1 of every primary task, if some group can no longer reach 2 kept tasks (each task genesis already solved counts as dropped and can at best be replaced by a same-group spare), phase 0 stops before run 2. At most 8 runs (≤ $0.40) are then spent on a set that is too easy.

### 3.2 Phase 1: evolution
- The evolution home **E** starts as a clone of G0 and is never reset during phase 1: its body, skills, extensions, memory and session logs persist across attempts and tasks. That persistence is what is being tested.
- Kept training tasks in kind order; each up to 3 attempts, stopping at the first 1000. Each attempt: a fresh workspace and a fresh session.
- INIT may self-extend freely: skills and `SELF.md` (T0), Wasm tools (T1), core and loop changes (T2), mid-task extension (ADR-011 §2.4, `Cap:` trailers), sub-agents. No reward exists: the Root admits on A0–A5 and gates only on harm (H1–H12).
- After each attempt's owner turn the Root stays up for an **idle window** of at most 600 s, ending after 120 s without zone egress, so background turns and pending admissions can finish. Its spend counts toward the attempt.
- At the end, E is snapshot **E1**.

### 3.3 Phase 2: held-out
- Each kept held-out task × 2 runs × 2 arms: **evolved** = a fresh clone of E1 per run; **twin** = a fresh clone of G0 per run. Neither arm can learn from an earlier held-out run.
- Interleaved: run 1 of every task, then run 2, over a fixed shuffle of the tasks (seed 20261022); per task the arm order flips between runs (ABBA).
- Both arms may self-extend mid-task; that is recorded for both. The only difference between the arms is phase 1's history.

## 4. Harness and guards (reused from REAL-M7)
- **Money:** each run has a real cap (§6). A guard prices the Root's egress usage at DeepSeek's list price every 5 s and reads the balance every 5 min; at the cap, or at the program's balance floor, it freezes spend (charter ceilings := spent), interrupts and `/stop`s. The charter ceilings themselves are the run cap × 2 (the Root bills at peak) plus 4 request reservations: a backstop. The kernel's per-task budget is $0.20, above the cap plus one reservation (DV-2).
- **Disk and memory:** `m7watch.py` beside every run and every scoring: free disk < 5,500 MB stops the tree (SIGSTOP), freezes spend, kills by PID; a workload process above 4,096 MB is killed alone (DV-4, DV-6). A run starts only with 6,500 MB free (phase 1: 8,000 MB, for T2 builds); an install with 7,000 MB.
- **Scoring** runs agent-written code, so the check runs under `sandbox-exec`: no network, writes only inside its scratch dir.
- **Fresh session per run** (DV-3): no turn resumes across runs.
- **Redaction:** `collect.py` (key, `sk-`, bearer) then `checks_m7.py`'s pass (e-mails, git identity); a final scan must find 0 leaks. Request bodies are never collected; the session logs are.
- **Key:** copied with `tr` into the test Root's key store only in real mode, removed in the EXIT trap and before collection. The mock never touches the key file. `~/.init` and `~/.init-root` are never touched; homes live under `target/evo/` and are removed by exact path; only PIDs the harness spawned are signalled.
- **Infra failures** (the workspace was not applied, the body never became busy within 90 s, the attach closed, a watchdog trip) are rerun once under a new tag; a provider error is part of the run and is reported, not rerun.

## 5. Pre-registered reading

| Verdict | Condition |
|---|---|
| `evolved_and_solved` | §1 items 1, 2 and 3 all hold |
| `evolved_gain_unattributed` | items 1 and 2 hold, item 3 does not (the gain is not tied to a phase-1 capability) |
| `evolved_no_heldout_gain` | item 1 holds, item 2 does not |
| `did_not_evolve` | phase 1 left no capability |

- **Capabilities created:** `change_open` records of phase 1 with caps of op `add` or `modify` (ADR-011 §5.1), with their evidence class (`root` for skill/agent/prompt/ext, `zone` for tool/mod/mem/loop), and `caps.json` at E1. Uncommitted trials are listed from usage but do not count as created.
- **Capabilities used** in a run: the Root's usage ledger (`ROOT_HOME/extend/usage`, per turn, with evidence). Root evidence counts for item 3; zone evidence is reported beside it.
- **Reported:** phase-0 scores and the kept sets; phase-1 trajectory (score per attempt, attempts to solve); every capability created, used, rolled back or quarantined in phase 1 (`change_close` outcomes, `harm`, `cap_state`); held-out solved runs and mean score per arm; per-task attribution; cost (priced real and balance drop) and tokens (input, cache read, output) per phase and arm; every deviation.
- `training_only` is flagged when INIT solved training tasks but item 2 fails: that is learning the training tasks, not evolution that transfers.

## 6. Budget
- **Hard cap: $2.50 real by DeepSeek balance** for the whole program. The balance was about $6.89 on 2026-10-22, so the floor is about $4.39; the runner records the balance at the first real run and sets floor = that balance − $2.50.
- Envelopes: phase 0 ≤ $0.90 (stops phase 0); phase 2's $0.80 is **reserved before phase 1 may spend**; $0.10 reserve; phase 1 gets the rest.
- Per-run real caps: phase 0 $0.05, phase 1 $0.10 (idle window included), phase 2 $0.05. Wall caps: 20, 40 and 20 minutes.
- A run starts only if spent + its cap + the reservations of later phases ≤ $2.50, and only if its whole wall cap lies off-peak (the runner waits for off-peak).
- Expected (estimate): about $0.03 per run in phases 0 and 2 and $0.06 per attempt in phase 1, so about $1.4 in total. The cap, not the estimate, decides.

## 7. Pre-registration (frozen)

<!-- PREREG BEGIN -->
```json
{
 "version": "evo-test-v1/1",
 "model": "deepseek-flash",
 "program_cap_real_musd": 2500000,
 "reserve_musd": 100000,
 "p0_envelope_musd": 900000,
 "p2_envelope_musd": 800000,
 "run_cap_musd": {"p0": 50000, "p1": 100000, "p2": 50000},
 "run_wall_s": {"p0": 1200, "p1": 2400, "p2": 1200},
 "p1_idle_window_s": 600,
 "p1_idle_quiet_s": 120,
 "p0_runs": 2,
 "p1_attempts": 3,
 "p2_runs": 2,
 "solved_permille": 1000,
 "min_set": 2,
 "max_set": 4,
 "p0_early_stop": true,
 "infra_reruns": 1,
 "p2_seed": 20261022,
 "kind_order": ["spec", "nav", "debug", "bigfile"],
 "retry_preface": "The previous attempt at this task did not meet the requirements. The workspace has been reset to its original state.",
 "success": {"min_new_caps": 1, "min_extra_solved_runs": 2, "min_attributed_tasks": 1},
 "kernel_task_usd": "0.20",
 "start_min_free_mb": {"install": 7000, "p0": 6500, "p1": 8000, "p2": 6500},
 "watch_min_free_mb": 5500,
 "mem_cap_mb": 4096
}
```
<!-- PREREG END -->

The ledger stores the sha256 of this object; the report shows both. Any change after the first real run is a deviation, listed with its reason in the results.

## 8. What waits on the ADR-011 build
Phase 0 needs only today's Root (ADR-009 `workspace.set`) and can run on the current build; it must be **re-run on the ADR-011 tuple** if that tuple is what phases 1–2 use, so that phase 0 and the twin are the same configuration. Phases 1–2 need, per `evo_test.sh ready`:
- the charter key `extend.mode = "free"` (X7, C17) — until then phase 1 refuses to start in real mode;
- `change_open` / `change_close` / `cap_state` / `cap_use` lineage records and `caps.json` (X2, X3, X5, X6);
- the Root usage ledger `ROOT_HOME/extend/usage/` and `x-egress-caps` (X5);
- the `Cap:` trailer (X1) and the extension skill `skills/extend/SKILL.md` (X11).

On the mock, phases 1–2 run in **stub** mode: the same schedule, snapshots, interleaving, ledger and report, with attribution read from zone-side traces (the body's commits and trailers, skills, the extension catalog).

## 9. Commands
```
tests/real/evo_test.sh selftest                 $0: pure logic, the sandboxed scorer, the task set's references
tests/real/evo_test.sh ready                    the ADR-011 blocker list
tests/real/evo_test.sh stage-bins               clone the current debug binaries into target/evo-bin
tests/real/evo_test.sh mock dry                 $0: install a mock home, phase 0 (+ stubbed 1-2) on init-mockllm
tests/real/evo_test.sh real install             then, with the orchestrator's go per phase:
EVO_I_MEAN_IT=1 EVO_GO=p0 tests/real/evo_test.sh real phase0
EVO_I_MEAN_IT=1 EVO_GO=p1 tests/real/evo_test.sh real phase1
EVO_I_MEAN_IT=1 EVO_GO=p2 tests/real/evo_test.sh real phase2
tests/real/evo_test.sh real report              -> build/real-artifacts/evo/real/report.json
```


## 10. v2 (owner: minimal) — 2026-10-22, registered before any real run

**Owner decisions** (relayed by the orchestrator, 2026-10-22 ~18:45 +07): (1) fewer cases, because the owner has little time and money; (2) success is `evolved_and_solved` **only**, and any other verdict stops the project. v2 shrinks the design to the minimum that still allows that verdict. §1–§9 stay as the v1 record; where v2 differs, v2 rules. The runner implements v2 (`PREREG` below; v1 is kept in the code as `PREREG_V1`, and selftest checks both blocks).

### 10.1 One kind: `bigfile`
- **Training:** `evo-big-client` (a generated API client of ≥ 220 methods in one ≥ 5,000-line file; MIGRATION.md rules with an exceptions table).
- **Held-out:** `evo-big-rules` (a ≥ 200-rule validation module) and `evo-big-schema` (a ≥ 200-class record/serializer module), in different domains and file styles.
- **Spare:** `evo-big-forms` (one, flexible): it replaces the first dropped task, training first. It is written after this registration; if it is needed and not validated, the test stops.
- **Why this kind.** Its difficulty is scale against the genesis harness: `read` returns at most 2,000 lines per call, `edit` needs an exact match that occurs exactly once, and these files hold hundreds of near-identical units. A self-built structure-aware bulk-rewrite tool, or a skill for scripted and verified mass edits, plausibly helps. The other kinds were calibrated by blind single passes of Claude models on a copy of the workspace (no hidden check). `spec`: Sonnet and Haiku scored 984–1000 on all three tasks in 5–17 tool calls. `debug`: Sonnet scored 1000 on all three in 14–24 calls. Phase 0 would very likely drop those kinds. On the bigfile tasks, a careful regex-only first pass scores 622 (client) and 590 (rules).

### 10.2 Procedure (as §3, with these sizes)
- **Phase 0:** genesis × 2 runs on the 3 tasks (run 1 of each, then run 2). A task is dropped as soon as genesis scores 1000 once (no further run of it). It is kept only if both runs score < 1000. A dropped task is replaced by the spare (× 2 runs). **Early stop:** after run 1 of the 3 tasks, if more tasks are dropped than spares exist, phase 0 stops. Proceed only with 1 training and 2 held-out tasks kept.
- **Phase 1:** evolution on the training task, at most 3 attempts, as §3.2.
- **Phase 2:** evolved vs frozen twin on the 2 held-out tasks × 2 runs each, interleaved (§3.3: run 1 of both tasks, then run 2; ABBA per task).

### 10.3 Success (strict; the only verdict that lets the project continue)
`evolved_and_solved` iff all of:
1. phase 1 left at least one capability created or modified by INIT (a `change_open` cap of op `add`/`modify`) active at E1;
2. evolved ≥ twin + 2 solved runs (score 1000), out of 4 runs each;
3. at least one held-out task with evolved 2/2 and twin 0/2, where **both** evolved runs used a phase-1 capability with **Root evidence** (the Root's usage ledger, `evidence: "root"`); zone-evidence use is reported and does not count.

Every other verdict (`evolved_gain_unattributed`, `evolved_no_heldout_gain`, `did_not_evolve`), or a stop in phase 0, means the project **stops** (owner).

### 10.4 Budget: $1.00 real by DeepSeek balance
- Floor = the balance at the first real run − $1.00 (the runner records both).
- Per-run real caps: phase 0 and phase 2 $0.04 (the same for genesis and twin), phase 1 $0.08 per attempt (idle window included).
- Envelopes: phase 0 ≤ $0.34 (6 runs + 2 spare runs); phase 2's $0.32 reserved before phase 1 may spend; $0.05 reserve; phase 1 gets the rest (about $0.29).
- Off-peak only (VN peak: weekdays 08–11 and 13–17 +07); the runner waits.

### 10.5 Binaries (deviation D1, registered here)
- Phase 0 runs on the binaries staged at registration time (`target/evo-bin`): `init-root` built 2026-10-22 16:51 (X3–X5 of ADR-011 partly in, no `extend.mode` charter key yet), `init-kernel`/`initctl` of 2026-10-21, and the seed at HEAD at install time. The home's charter has `extend` gated.
- Before phase 1 the runner builds and stages the binaries at HEAD, including INTEG-A's "A1 advisory on the free path" if it has landed (otherwise a recorded deviation), and **reinstalls** the test home: new G0′ at HEAD, `extend.mode = "free"` when the Root accepts it. Phase 1 starts from G0′; phase 2's twin is G0′. So the twin and the evolved arm's starting point share binaries and charter.
- Phase 0's genesis and phase 2's twin therefore differ in tuple. Phase 0 only selects tasks; the comparison is phase 2, where the twin is re-run. A twin that solves a held-out task in phase 2 counts against item 3 as it should.

### 10.6 Pre-registration v2 (frozen)

<!-- PREREG v2 BEGIN -->
```json
{
 "version": "evo-test-v1/2 (owner: minimal)",
 "registered": "2026-10-22",
 "model": "deepseek-flash",
 "kind": "bigfile",
 "tasks": {
  "train": [
   "evo-big-client"
  ],
  "heldout": [
   "evo-big-rules",
   "evo-big-schema"
  ],
  "spares": [
   "evo-big-forms"
  ]
 },
 "program_cap_real_musd": 1000000,
 "reserve_musd": 50000,
 "p0_envelope_musd": 340000,
 "p2_envelope_musd": 320000,
 "run_cap_musd": {
  "p0": 40000,
  "p1": 80000,
  "p2": 40000
 },
 "run_wall_s": {
  "p0": 1200,
  "p1": 2400,
  "p2": 1200
 },
 "p1_idle_window_s": 600,
 "p1_idle_quiet_s": 120,
 "p0_runs": 2,
 "p1_attempts": 3,
 "p2_runs": 2,
 "solved_permille": 1000,
 "p0_early_stop": true,
 "infra_reruns": 1,
 "p2_seed": 20261022,
 "retry_preface": "The previous attempt at this task did not meet the requirements. The workspace has been reset to its original state.",
 "success": {
  "verdict": "evolved_and_solved",
  "min_new_caps": 1,
  "min_extra_solved_runs": 2,
  "min_attributed_tasks": 1,
  "evidence": "root"
 },
 "kernel_task_usd": "0.20",
 "start_min_free_mb": {
  "install": 7000,
  "p0": 6500,
  "p1": 8000,
  "p2": 6500
 },
 "watch_min_free_mb": 5500,
 "mem_cap_mb": 4096
}
```
<!-- PREREG v2 END -->

### 10.7 Deviations during the run
- **D2 (2026-10-22 19:16 +07, during phase 0; harness fault, fixed between runs).**
  - (a) The run guard compared the balance drop since the run's start with the run cap. DeepSeek bills minutes late, so the bills of the two earlier runs landed inside `p0-evo-big-schema-genesis-r1` and stopped it with a priced spend of $0.027 (cap $0.04). The task was already solved (score 1000), so the stop changed no outcome, and genesis solving schema stands.
  - Fix: the run cap is now priced only, and the balance checks only the program floor. A run that a false balance stop ends below its cap with a score < 1000 is an infra failure, rerun once.
  - (b) The Root charges a stream the guard cuts at its whole reservation (DV-3), so the priced sum showed $0.178 for that run. Cut streams are now excluded from the priced sum and reported apart. The program accounting takes the larger of the priced sum and the balance drop, so their true cost still counts once the balance settles. `p0-evo-big-schema-genesis-r1` was re-priced from 177,881 to 26,889 µUSD (ledger note).
- **Turn close before stop (2026-10-22, between runs; no run affected).** The Root writes a turn's usage line, which is the Root evidence item 3 needs, only after 30 s without zone egress (ADR-011 §3.1). The runner now waits for that line, up to 90 s, before it stops the Root. A mock run on the HEAD Root confirmed the line (`tool/bash|root`, `tool/write|root`).

### 10.8 Result (2026-10-22): stopped in phase 0, so the project stops (§10.3)
Real deepseek-flash, off-peak, 18:58–19:23 +07. Phase-0 binaries: §10.5 (D1). Per-run cap $0.04.

| run | score | priced real $ | model turns | ended by |
|---|---|---|---|---|
| p0-evo-big-client-genesis-r1 | 180 (untouched) | 0.0317 | 34 | the model (end_turn mid-work) |
| p0-evo-big-rules-genesis-r1 | 151 | 0.0324 | 30 | the model |
| p0-evo-big-schema-genesis-r1 | **1000** | 0.0269 | 27 | false balance-lag stop after solving (D2) |
| p0-evo-big-client-genesis-r2 | 180 (untouched) | 0.0178 | 17 | the model (end_turn mid-work) |
| p0-evo-big-rules-genesis-r2 | **1000** | 0.0422 | 30 | the model |

- Genesis solved both held-out tasks within the run cap. With 2 held-out tasks dropped and 1 spare, the held-out set could not reach 2. The early-stop rule (§10.2) halted phase 0 before the spare ran. Phases 1 and 2 did not run.
- Spend: priced $0.151. The DeepSeek balance fell from $13.93 to $13.80 at the last read; it lags, so expect about $0.15 by balance. Cap $1.00.
- Reading: the bigfile tasks are within genesis INIT's reach on deepseek-flash. Both client failures are premature end-of-turn with `client.py` untouched, not a capability ceiling. Every task in the set, across all four kinds, was solved by a blind single pass of Claude Sonnet, and the spec tasks also by Haiku. Hand-written fixture tasks of this size do not separate a frontier-class model from its harness.
- Artifacts: `build/real-artifacts/evo/real/` (report.json, ledger.json, one directory per run; redacted, 0 leaks; display-frame logs dropped, session logs kept).


## 11. v3 (owner: missing mechanism) — 2026-10-22, registered before calibration

**Owner decision** (relayed by the orchestrator after §10.8): target a mechanism that current agents have and INIT lacks, and test whether INIT evolves it by itself. Hard cap: $2.50 real by balance, separate from the $0.15 of §10. §1–§10 stay as the record. The runner implements v3 (`PREREG` below; v1 and v2 are kept as `PREREG_V1` and `PREREG_V2`).

### 11.1 Gap analysis ($0)
- **INIT genesis.**
  - Tools: `read`, `write`, `edit` (several exact replacements per call), `bash`, `ext`.
  - The loop ends a run when the model answers without tool calls, and has no completion check.
  - `GENESIS.md` (25 lines) says nothing about planning, tracking or verification.
  - There is no todo or plan tool, and no stop hook.
  - Skills: `evolve`, `ext-authoring`, `extend`.
- **Production harnesses** (research_notes/Agentic harness tự tiến hoá/production_harness_internals.md §1, §2, §6):
  - Claude Code: TodoWrite / Task tools whose open items are re-surfaced to the model (on for older models, and on with `CLAUDE_CODE_ENABLE_TODO_TOOLS=1`), Grep/Glob, Edit/MultiEdit, sub-agents, hooks (Stop), plan mode and CLAUDE.md.
  - Codex: `update_plan` (pending / in_progress / completed, one step in progress), `apply_patch` and AGENTS.md.
  - OpenCode: todo tools and sub-agents.
  - Warp and Factory credit a required todo list for Terminal-Bench gains.
- **What INIT already has an equivalent for:** search (bash grep/rg), multi-edit (`edit` takes many edits), sub-agents (fleet, from L0 under ADR-011) and memory (`SELF.md`, skills).
- **What it lacks:** task tracking plus a completion check. Phase 0 of §10 showed the symptom: both genesis runs on evo-big-client ended with the model's own end of turn mid-work, with `client.py` untouched.
- **Chosen mechanism class M: task tracking with a completion check.** In the control, todo tracking and verify-before-finish work as one mechanism: the open items keep the loop going. So they are tested as one class.

### 11.2 Mechanism class M (the blind reviewer's definition, frozen here)
A capability belongs to M if it does at least one of the following, by any means (skill or prompt text, a Wasm tool, a loop or core change, a memory structure, a sub-agent pattern):
1. it makes the agent write the task's requirements down as an explicit list of items (a todo / plan / checklist kept in a file, a tool, memory or loop state) and keep each item's status (open or done) as the work proceeds; or
2. it prevents or discourages ending the work while listed items are still open, or before a final verification pass against the requirements (re-reading them item by item, or running the task's tests or checks).

Not in M: general coding tools (search, edit, bulk rewrite, tracing), knowledge about a task's content, stored solutions, and generic advice ("be careful", "test your code") with no list, status or completion element.

### 11.3 Task family `tickets` and calibration
- **Candidates**, in registered order: training `evo-tix-library`, `evo-tix-garage`; held-out `evo-tix-pantry`, `evo-tix-clinic`, `evo-tix-studio`.
  - Each is a small Python package in its own domain with a `TICKETS.md` of about 25 numbered, explicit, independent change requests (renames, messages, defaults, fields, CLI options, docs, version bumps, and so on).
  - The hidden check grades one item per ticket plus regressions. A run is solved at 1000 only when every ticket is done.
  - The candidates share the family and the ticket kinds, never the code, the domain or the concrete changes.
  - Written after this registration and before any calibration run.
- **(a) Genesis:** INIT genesis (the HEAD tuple `662e328`, `extend.mode = "free"`, the same binaries and charter as the phase-2 twin) runs each candidate × 2. A candidate is dropped at its first 1000.
- **(b) Control:** Claude Code 2.1.261 (installed) drives deepseek-flash through `https://api.deepseek.com/anthropic`, with every model variable pinned to `deepseek-flash` and `CLAUDE_CODE_ENABLE_TODO_TOOLS=1`. It runs only on candidates genesis failed 2/2, at most 2 runs each, stopping at the first 1000. Isolation:
  - a fresh `CLAUDE_CONFIG_DIR` and `HOME` per run, `--strict-mcp-config`, `--no-session-persistence`, and WebFetch/WebSearch disallowed;
  - `--dangerously-skip-permissions` inside a Seatbelt profile that writes only the run's temp workspace, config and tmp dirs, and reads nothing of the owner's home except those and the CLI's install dir;
  - the key only in the child's environment, read from the key file, never printed or logged, and scanned for in the transcript (redacted, leaks counted);
  - the same task prompt, workspace, per-run real cap and wall cap as INIT, with its spend priced from the stream's usage and enforced live.
- **Keep** a candidate iff genesis 0/2 AND control ≥ 1/2. Selection: the first kept training candidate and the first two kept held-out candidates, in registered order. With fewer than 1 + 2 kept, the test stops and reports which gap did not show (that is itself a result).
- The selection, with genesis and control scores, is appended here as §11.6 before phase 1 starts, and committed.

### 11.4 Phases 1–2 and success
- **Phases:** as §10.2. Phase 1: the training task, at most 3 attempts. Phase 2: evolved vs frozen twin (G0 at `662e328`) on the 2 held-out tasks × 2 runs, interleaved ABBA. Owner messages are the task prompts only (§3), never a hint at M.
- **PASS** (the only result that lets the project go on) needs all three:
  1. phase 1 left a capability created or modified by INIT, with a Root `change_open` record naming it, that a **blind reviewer** judges to be in M. The reviewer is a fresh subagent given only §11.2 and the capability's text or diff (no task, score or hypothesis), and must answer YES;
  2. evolved ≥ twin + 2 solved runs, out of 4 each;
  3. at least one held-out task with evolved 2/2 and twin 0/2, where both evolved runs used that M capability.
- **What counts as "used":**
  - a skill, agent or ext cap needs Root-observed use (the Root usage ledger, `evidence: "root"`);
  - a loop or core cap (`loop/*`, `mod/*`: zone evidence by ADR-011 §5.1) counts as used when the run's body tuple, held and verified by the Root, contains it, because such code runs on every turn.

### 11.5 Budget: $2.50 real by balance
- Floor = the balance at the first v3 run − $2.50.
- Per-run real caps:
  - $0.06 for every calibration and phase-2 run, the same for genesis, control, twin and evolved;
  - $0.12 per phase-1 attempt.
- Wall caps: 20 min (phase 1: 40 min).
- Envelopes:
  - calibration ≤ $1.40 (genesis and control);
  - phase 1 ≤ $0.40;
  - phase 2's $0.50 is reserved before phase 1 spends;
  - $0.20 reserve.
- Off-peak only.

### 11.5a Pre-registration v3 (frozen)

<!-- PREREG v3 BEGIN -->
```json
{
 "version": "evo-test-v1/3 (owner: missing mechanism)",
 "registered": "2026-10-22",
 "model": "deepseek-flash",
 "kind": "tickets",
 "mechanism": "task-tracking-and-completion-check",
 "tasks": {
  "train": [
   "evo-tix-library",
   "evo-tix-garage"
  ],
  "heldout": [
   "evo-tix-pantry",
   "evo-tix-clinic",
   "evo-tix-studio"
  ],
  "spares": []
 },
 "need": {
  "train": 1,
  "heldout": 2
 },
 "control": {
  "harness": "claude-code",
  "version": "2.1.261",
  "endpoint": "https://api.deepseek.com/anthropic",
  "todo_tools": true,
  "runs": 2,
  "stop_at_first_solve": true
 },
 "program_cap_real_musd": 2500000,
 "reserve_musd": 200000,
 "p0_envelope_musd": 1400000,
 "p1_envelope_musd": 400000,
 "p2_envelope_musd": 500000,
 "run_cap_musd": {
  "p0": 60000,
  "p1": 120000,
  "p2": 60000
 },
 "run_wall_s": {
  "p0": 1200,
  "p1": 2400,
  "p2": 1200
 },
 "p1_idle_window_s": 600,
 "p1_idle_quiet_s": 120,
 "p0_runs": 2,
 "p1_attempts": 3,
 "p2_runs": 2,
 "solved_permille": 1000,
 "p0_early_stop": false,
 "infra_reruns": 1,
 "p2_seed": 20261022,
 "retry_preface": "The previous attempt at this task did not meet the requirements. The workspace has been reset to its original state.",
 "success": {
  "verdict": "evolved_and_solved",
  "min_new_caps": 1,
  "min_extra_solved_runs": 2,
  "min_attributed_tasks": 1,
  "evidence": "root",
  "mechanism_review": "blind subagent, yes required"
 },
 "kernel_task_usd": "0.20",
 "start_min_free_mb": {
  "install": 7000,
  "p0": 6500,
  "p1": 8000,
  "p2": 6500
 },
 "watch_min_free_mb": 5500,
 "mem_cap_mb": 4096
}
```
<!-- PREREG v3 END -->


### 11.6 v3 result and v3.1 (candidates A) — registered 2026-10-22 ~20:02, before any v3.1 run
- **v3 result (tickets):** genesis solved all five candidates on its first run, each in 6–9 model turns, under a minute, about $0.006 per run (total $0.03). An explicit numbered ticket list is itself a todo list, so the missing mechanism never came into play. No ticket task passed (a), and the control was not run.
- **Family, redefined by the mechanism (orchestrator GO A):** *long, many-requirement work where stopping early fails.* A task belongs to the family when the full solution needs many (≥ 20) distinct changes or behaviours, spread over a large workspace (thousands of lines) or a long specification, and the requirements are not handed over as a numbered checklist. A run that ends before every requirement is handled scores below 1000. The candidates differ in domain and kind and share this property; held-out similarity is at the level of this family.
- **Candidates**, in registered order. Existing validated tasks, frozen since their commits:
  - training: `evo-big-client` (on the older tuple, genesis ended it early twice in §10.8), `evo-nav-units`;
  - held-out: `evo-big-forms`, `evo-nav-tz`, `evo-spec-sched`, `evo-dbg-sheet`, `evo-spec-tmpl`.
- **Everything else is as §11.3–§11.5:** genesis × 2 (dropped at its first 1000), the control on genesis' 2/2 failures (up to 2 runs, stop at the first solve), the keep and selection rules, phases, success, mechanism class M (§11.2), caps and envelopes. The $0.03 of v3 counts toward the calibration envelope.
- **If v3.1 yields no 1 + 2 set:** go to B, an implicit-prose tickets family (about 60 unnumbered requirements in a larger codebase), registered before its calibration. If B also yields none, stop and report "no qualifying mechanism gap found".

<!-- PREREG v3.1 BEGIN -->
```json
{
 "version": "evo-test-v1/3.1 (owner: missing mechanism, candidates A)",
 "registered": "2026-10-22",
 "model": "deepseek-flash",
 "kind": "long-work",
 "mechanism": "task-tracking-and-completion-check",
 "tasks": {
  "train": [
   "evo-big-client",
   "evo-nav-units"
  ],
  "heldout": [
   "evo-big-forms",
   "evo-nav-tz",
   "evo-spec-sched",
   "evo-dbg-sheet",
   "evo-spec-tmpl"
  ],
  "spares": []
 },
 "need": {
  "train": 1,
  "heldout": 2
 },
 "control": {
  "harness": "claude-code",
  "version": "2.1.261",
  "endpoint": "https://api.deepseek.com/anthropic",
  "todo_tools": true,
  "runs": 2,
  "stop_at_first_solve": true
 },
 "program_cap_real_musd": 2500000,
 "reserve_musd": 200000,
 "p0_envelope_musd": 1400000,
 "p1_envelope_musd": 400000,
 "p2_envelope_musd": 500000,
 "run_cap_musd": {
  "p0": 60000,
  "p1": 120000,
  "p2": 60000
 },
 "run_wall_s": {
  "p0": 1200,
  "p1": 2400,
  "p2": 1200
 },
 "p1_idle_window_s": 600,
 "p1_idle_quiet_s": 120,
 "p0_runs": 2,
 "p1_attempts": 3,
 "p2_runs": 2,
 "solved_permille": 1000,
 "p0_early_stop": false,
 "infra_reruns": 1,
 "p2_seed": 20261022,
 "retry_preface": "The previous attempt at this task did not meet the requirements. The workspace has been reset to its original state.",
 "success": {
  "verdict": "evolved_and_solved",
  "min_new_caps": 1,
  "min_extra_solved_runs": 2,
  "min_attributed_tasks": 1,
  "evidence": "root",
  "mechanism_review": "blind subagent, yes required"
 },
 "kernel_task_usd": "0.20",
 "start_min_free_mb": {
  "install": 7000,
  "p0": 6500,
  "p1": 8000,
  "p2": 6500
 },
 "watch_min_free_mb": 5500,
 "mem_cap_mb": 4096
}
```
<!-- PREREG v3.1 END -->


### 11.7 v3.1 result and v3.2 (family B) — registered 2026-10-22 ~21:16, before any v3.2 run
- **v3.1 result (candidates A): no 1 + 2 set.**
  - Genesis × 2: evo-big-client 786 / 508, evo-nav-units 177 / 80, evo-nav-tz 94 / 94 and evo-dbg-sheet 76 / 769, all beyond capability. evo-big-forms, evo-spec-sched and evo-spec-tmpl were solved (1000 on run 1) and dropped.
  - Control (Claude Code + deepseek-flash, $0.06 per run): evo-big-client 475 / 983, evo-nav-units 80 / 80, evo-nav-tz 94 on run 1. Every control run reached the $0.06 cap: on these large tasks it spends about 50–90 k output tokens and 1–4 M cache-read tokens per run.
  - No training candidate passed (b). The remaining control runs were skipped (ledger halt), so v3.1 cannot qualify.
  - Spend after v3.1: priced $0.677; balance $13.76 → $13.07 (lagging).
- **What v3.1 shows:** at an equal $0.06 per run, the harness with the mechanism does not finish these tasks either. Because the cap binds the control, it cannot isolate the mechanism.
- **v3.2, family B (`prose`):** five fresh tasks.
  - Training: `evo-prose-library`, `evo-prose-garage`. Held-out: `evo-prose-pantry`, `evo-prose-clinic`, `evo-prose-studio`.
  - Each is a package of about 2,570 lines with three catalog sub-packages. Its CHANGES.md is an unnumbered prose design note of about 60 requirement-sites (some stated once for every catalog), and the hidden check has one item per site plus regressions.
  - Validated 5/5: reference 1000, initial 32, empty 0, tamper detected.
- **Budget re-split for v3.2, so the control is not money-bound.** Every cap stays equal between the arms.
  - Per-run real caps: $0.10 for every calibration and phase-2 run (genesis, control, twin and evolved alike), $0.15 per phase-1 attempt.
  - Phase 1: at most 2 attempts (within "at most 3").
  - Envelopes, within $2.50 by balance: phase 0 (all calibration so far plus B) ≤ $1.28; phase 1 ≤ $0.30; phase 2's $0.80 reserved before phase 1; $0.11 reserve.
- **Everything else is as §11.2–§11.5.** If v3.2 yields no 1 + 2 set, the test stops and reports "no qualifying mechanism gap found" (orchestrator).

<!-- PREREG v3.2 BEGIN -->
```json
{
 "version": "evo-test-v1/3.2 (owner: missing mechanism, family B)",
 "registered": "2026-10-22",
 "model": "deepseek-flash",
 "kind": "prose",
 "mechanism": "task-tracking-and-completion-check",
 "tasks": {
  "train": [
   "evo-prose-library",
   "evo-prose-garage"
  ],
  "heldout": [
   "evo-prose-pantry",
   "evo-prose-clinic",
   "evo-prose-studio"
  ],
  "spares": []
 },
 "need": {
  "train": 1,
  "heldout": 2
 },
 "control": {
  "harness": "claude-code",
  "version": "2.1.261",
  "endpoint": "https://api.deepseek.com/anthropic",
  "todo_tools": true,
  "runs": 2,
  "stop_at_first_solve": true
 },
 "program_cap_real_musd": 2500000,
 "reserve_musd": 110000,
 "p0_envelope_musd": 1280000,
 "p1_envelope_musd": 300000,
 "p2_envelope_musd": 800000,
 "run_cap_musd": {
  "p0": 100000,
  "p1": 150000,
  "p2": 100000
 },
 "run_wall_s": {
  "p0": 1200,
  "p1": 2400,
  "p2": 1200
 },
 "p1_idle_window_s": 600,
 "p1_idle_quiet_s": 120,
 "p0_runs": 2,
 "p1_attempts": 2,
 "p2_runs": 2,
 "solved_permille": 1000,
 "p0_early_stop": false,
 "infra_reruns": 1,
 "p2_seed": 20261022,
 "retry_preface": "The previous attempt at this task did not meet the requirements. The workspace has been reset to its original state.",
 "success": {
  "verdict": "evolved_and_solved",
  "min_new_caps": 1,
  "min_extra_solved_runs": 2,
  "min_attributed_tasks": 1,
  "evidence": "root",
  "mechanism_review": "blind subagent, yes required"
 },
 "kernel_task_usd": "0.20",
 "start_min_free_mb": {
  "install": 7000,
  "p0": 6500,
  "p1": 8000,
  "p2": 6500
 },
 "watch_min_free_mb": 5500,
 "mem_cap_mb": 4096
}
```
<!-- PREREG v3.2 END -->

### 11.8 Result (2026-10-22, 21:27 +07): no qualifying mechanism gap found, so the test stops
- **v3.2 (family B), genesis run 1:** evo-prose-library, -garage, -pantry, -clinic and -studio all scored **1000**, at $0.017–0.025 per run. About 60 unnumbered requirements in prose over a 2,570-line package did not stop genesis INIT. No candidate passed (a), so no control run was needed. The registered stop applies (§11.7; orchestrator: no variant C).
- **The three calibrations together:**

  | family | genesis | control | qualifying |
  |---|---|---|---|
  | v3 tickets (numbered list, 25 items) | 5 / 5 solved on run 1 | not needed | none |
  | v3.1 long work (existing large tasks) | 3 of 7 solved; client, nav-units, nav-tz, dbg-sheet failed 2/2 | client 475 / 983, nav-units 80 / 80, nav-tz 94, all at the $0.06 cap | none |
  | v3.2 prose (~60 unnumbered requirements) | 5 / 5 solved on run 1 | not needed | none |

- **Reading:**
  - Genesis INIT on deepseek-flash completes many-requirement work, numbered or in prose, in a single run when the work fits its context, without any tracking mechanism.
  - Where it fails (very large codebases or files), the mechanism-equipped control fails too at the same per-run money cap. The control spends 2–3× more per run, and in five runs it never called a todo/task tool (0 TodoWrite/Task calls; `CLAUDE_CODE_ENABLE_TODO_TOOLS=1` was set).
  - So no observed failure is attributable to the targeted mechanism (task tracking with a completion check) at these budgets.
- **Not run:** phases 1–2; no evolution was attempted under v3.
- **Spend (v3, priced):** $0.785. Balance $13.76 → $12.91 at the last read (−$0.85; it lags, and part of it may be v2's late bills). Hard cap $2.50; floor $11.26 never approached.
- **Artifacts:** `build/real-artifacts/evo-v3/real/` holds report.json, ledger.json and runs.tar.gz. The archive has the redacted per-run directories: genesis sessions, Root lineage and usage, scores, and compact control-transcript summaries (the full stream transcripts, about 13 MB each, were key-scanned with 0 hits and not kept). There are 0 key or e-mail leaks.

### 11.9 Erratum (main, 2026-10-22, after the result; outcome unchanged)
- **§11.1 counted sub-agents as an INIT equivalent, but they are not one.** The kernel's `fleet_spawn` returns "fleet members … are not built in this kernel" (`kernel/crates/init-kernel/src/serve.rs`), so genesis INIT has no sub-agent runtime. Sub-agents belonged on the "lacks" list.
- **This does not change the verdict.** A tool count over the five control summaries (`runs.tar.gz`, `control-summary.json`) shows the control did use sub-agents:
  - `Agent` was called in 4 of 5 runs: client r2, nav-tz r1, nav-units r1 and r2.
  - nav-tz r1 also called `TaskList` once and `SendMessage` twice.
  - `TaskCreate`, `TaskUpdate` and `TodoWrite` were never called.

  Every control run still failed at the $0.06 cap: nav-tz 94, nav-units 80 / 80, client 983 (with a sub-agent) and 475 (without). At these budgets on deepseek-flash, sub-agents did not separate the harnesses either.
- **The v3.2 per-run cap ($0.10) was never applied to the v3.1 controls.** A $0.10 control on client could plausibly cross 1000 (983 at $0.06). It would still not give a 1 + 2 set. Only nav-tz and dbg-sheet were held-out candidates that genesis failed 2/2, and the nav-tz control scored 94 after 581 reads.
