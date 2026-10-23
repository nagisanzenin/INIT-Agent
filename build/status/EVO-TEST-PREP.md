# EVO-TEST-PREP: the quick evolution test of ADR-011 (tasks, protocol, runner, dry run, real run)

Status: **done.** v2 (bigfile) stopped in phase 0. v3 (missing mechanism): **no qualifying mechanism gap found**, so the test stops (protocol §11.8) · 2026-10-22

## Done
- **Tasks** (`fixtures/evo-test-v1/`, pool-v1 format and runner via `tools/evo.py`): 10 fresh tasks written by four
  authoring sub-lanes, all validated with `evo.py validate` (niced, light pool): reference 1000 ×3, initial ≤ 500 ×3,
  empty 0, partial strictly between, tamper detected, checks ≤ 30 s, byte-identical rebuild.
  | kind | training | held-out | spare | blind single pass (Claude, workspace copy only) |
  |---|---|---|---|---|
  | spec | evo-spec-ledger | evo-spec-sched | evo-spec-tmpl | Sonnet 984–1000, Haiku 1000 on all three |
  | debug | evo-dbg-vm | evo-dbg-sheet | evo-dbg-diff | Sonnet 1000 on all three (14–24 calls) |
  | nav | evo-nav-units | evo-nav-tz | – | (v1 record only) |
  | bigfile | evo-big-client | evo-big-rules, evo-big-schema (v2) | evo-big-forms (v2) | Sonnet 1000 on client; regex-only pass 622 / 590 / 191 / 637 |
- **Protocol** `measurements/evo-test-protocol.md`: v1 (§1–§9, the record) and **v2 (owner: minimal, §10)**, registered
  and committed before any real run: bigfile only, 1 training + 2 held-out + 1 flexible spare, 2 runs per arm, $1.00
  real by balance, success = `evolved_and_solved` only (item 2: evolved ≥ twin + 2 solved runs of 4; item 3: a held-out
  task 2/2 vs 0/2 with Root-evidence use of a phase-1 capability); any other verdict stops the project.
- **Runner** `tests/real/evo_test.sh` + `tests/real/evo_checks.py` (reusing checks.py, checks_m7.py, m7watch.py,
  collect.py): install → snapshot G0 (APFS clone); per run: restore, fresh workspace, per-run charter ceilings, fresh
  session, `workspace.set` on the owner socket, ONE owner `line` frame, guards, stop, sandboxed scoring under the
  watchdog, attribution, collection + redaction, ledger. Phase 1 keeps the home, idle window, snapshot E1; phase 2
  restores E1 / G0 per run, interleaved ABBA. Money: per-run real cap (priced every 5 s), program balance floor,
  phase-2 envelope reserved before phase 1, off-peak wait, disk gates; `read_home = false` + `read_deny` (fixtures,
  tests, measurements, root, .git, the built tasks, the snapshots); built task dirs never contain `solution.tar`.

## Tests
- `tests/real/evo_test.sh selftest` (evo_checks selftest incl. both PREREG blocks, the sandboxed scorer denying writes
  outside its scratch, every registered reference scoring 1000 sandboxed; plus m7watch selftest): PASS.
- `$0` mock dry run on init-mockllm (live Root, kernel, body): phase 0, phase 1 (stub: no ADR-011 free path in the
  staged bins), snapshot E1, phase 2 interleaved, report: all green, 0 leaks after redaction. Found and fixed:
  `init-root attach` splits a multi-line owner message into a line plus steers; the runner now sends one owner-socket
  `line` frame (verified: 1 user message, 0 steers).
- Mock phase 0 on the bigfile tasks: green (scores = initial workspace, as the stand-in agent edits nothing).

## Deviations
- **D1 (registered, protocol §10.5):** phase 0 runs on the staged bins (init-root 2026-10-22 16:51 with part of
  ADR-011, kernel of 2026-10-21, extend gated); before phase 1 the home is reinstalled at HEAD (G0′), which is also
  phase 2's twin.
- The v1 four-kind design (8 tasks) was superseded by the owner's v2 before any real run; the spec/debug/nav tasks stay
  as fixtures (blind Claude passes solved spec and debug in one pass, so phase 0 would likely have dropped them).
- **D2** (protocol §10.7): a false balance-lag stop and a cut-stream overcharge in the run guard. Found during phase 0 and
  fixed between runs; no outcome changed. The schema run was re-priced from $0.178 to $0.027.
- Turn close: the runner now waits up to 90 s for the Root's per-turn usage line before stopping the Root. Without it,
  phase 2 would have had no Root evidence for item 3. Fixed and verified on the mock before any phase-2 run.
- Bigfile blind Sonnet calibration was still running at registration time.

## Needs from others
- ADR-011 build: charter key `extend.mode`, kernel `change.open` / `Cap:` trailer, `skills/extend/SKILL.md`
  (`evo_test.sh ready` lists what is missing); INTEG-A's "A1 advisory on the free path" for phase 1.
- Building HEAD bins without the other lanes' uncommitted WIP needs either a clean tree or an orchestrator OK for a
  separate target dir.

## Real run (v2, deepseek-flash, off-peak, 2026-10-22 18:58–19:23 +07)
- Phase 0: client 180 / 180 (both runs: the model ended its turn mid-work, `client.py` untouched); rules 151 / **1000**;
  schema **1000** (run 1). Two held-out tasks were dropped against one spare, so the registered early stop halted
  phase 0. **Verdict `stopped_in_phase0`: the project stops.** Phases 1–2 did not run.
- Spend: priced $0.151. Balance $13.93 → $13.80 at the last (lagging) read; expect about $0.15. Cap $1.00.
- Binaries: phase 0 ran on `target/evo-bin` (init-root 2026-10-22 16:51, kernel 2026-10-21, seed HEAD 26c1cc7,
  extend gated). The HEAD bins for phase 1 (`ea20688`: init-root from a `git archive` export, kernel built in place on a
  clean tree; `ready`: adr011_ready) were staged and rehearsed on the mock (reinstall, `extend.mode = "free"`
  accepted, phases 1–2 green), but never used for real.
- Artifacts: `build/real-artifacts/evo/real/` (report.json, ledger.json, per-run dirs; redacted, 0 leaks).
- Cleanup: target/evo*, target/evo-src (and its target dir), target/evo-bin, target/evo-bin-head and the mock artifacts
  removed by exact path.

## v3: the missing-mechanism test (protocol §11, owner decision after v2)
- Gap analysis (§11.1): INIT genesis lacks task tracking with a completion check (no todo/plan tool, no stop hook,
  nothing in GENESIS.md). Search, multi-edit, sub-agents and memory already have equivalents. That defines mechanism
  class M (§11.2, frozen for a blind reviewer).
- Control runner: Claude Code 2.1.261 + deepseek-flash through DeepSeek's Anthropic endpoint. Fresh CLAUDE_CONFIG_DIR and
  HOME, web tools off, Seatbelt-confined writes and reads, the key in the child env only, live priced cap, transcripts
  key-scanned (0 hits).
- Calibrations, each registered and committed before its runs, on the HEAD tuple 662e328 with extend free:
  - v3 tickets: genesis solved 5/5 on run 1;
  - v3.1 long work: no training candidate passed the control at $0.06; the control never used a todo tool;
  - v3.2 prose: genesis solved 5/5 on run 1.
  Result: no 1 + 2 set, so phases 1–2 were not run. Spend $0.785 priced (balance $13.76 → $12.91, lagging).
- Commits: ee9905d, f9811e1, 81988d1, plus the result commit.

## What is left
- Nothing for this lane. Takeaway: on deepseek-flash, genesis INIT already finishes many-requirement work that fits a run,
  and the long-task failures it does show are shared by Claude Code at the same budget. A future test would need a
  different mechanism hypothesis (cost-efficient context use on very large codebases looks like the real limit), or a
  stronger model. Earlier note: a future attempt would need tasks that genesis deepseek-flash really fails. Blind Claude
  passes solved every fixture here, and genesis solved 2 of 3 bigfile tasks, so hand-written tasks of this size are not
  hard enough. The client failures point to premature end-of-turn as a genesis weakness. A task family built around
  long verified loops would test that weakness, and it is the kind of thing a self-built loop change could fix.
