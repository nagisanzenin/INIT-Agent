# PROOF-LEAN: the fastest honest path to the stop/continue result (orchestrator, 2026-10-20 23:10)

The owner asked for the stop/continue result as fast as possible. This file is the binding scope for the lean
proof lanes. It cuts m7-design.md and evolution-v2.md down to the minimum that still answers PROOF_PLAN.md's
question honestly. Everything not listed here waits for after the result.

## What stays exactly as designed (the rigor guard)
- Sealed S lives only in ROOT_HOME. Lanes never read `ROOT_HOME/sealed/S/` or `sealed/seeds/` (BUILD_AGENT_BRIEF rule).
- The 4 canaries (health, impossible, tamper, leak) and the leak GUID check.
- The exact sign-flip test with margin (m7 §3.2.4), the accept rule v2 for `better` and `no_gain` (§3.2.5),
  the per-run caps (§3.2.8), the A/A calibration (§3.2.10), the aggregates-only view for the agent (§3.2.11).
- A reserve of held-out templates that no G4 slice touches, for the milestone check (§3.2.13's check, `milestone` record).
- The change is proposed by the agent itself from mined failure evidence (evolution-v2 Reflect, cut down below),
  then goes through the normal G0–G3, handoff, provisional, G4 v2. No runbook-dictated change unless the engine
  finds nothing (then that is reported as the result, per evolution-v2 §3.10 D-5).
- Real money only through the REAL lane, ≤ $3.00 real total, balance-checked, Root-metered.

## Cuts (deviations, recorded as D-L1…D-L6)
- **D-L1: family B (fresh tiny repos + bug injector) is dropped** from S v1-lean; it is the longest pack (31 AH).
  Composition becomes E 24 (Py 12, Rust 12), C 14, T 14 = 52 templates + 4 canaries.
  G4 partition 32 (E 14, C 9, T 9), reserve 20 held-out templates (E 10, C 5, T 5). One template per partition.
  Caveat to report: E and C derive from public sources, so contamination risk is higher than with B (mitigated by
  the seeded semantic mutations, which ask for new behaviour).
- **D-L2: ROOT-S1 without** rotation, looks/`dup`, the template-fingerprint and leak scans (the canary leak GUID check
  stays), the `confirmed` pointer and the span counter, `store.put kind:"t0"` / tuple v2.
- **D-L3: GATE-V2 without** judge mode, `gate simulate --rule v2`, the gog enumeration with v2, the v0.3 fallback, L1-V2.
  The ratchet (`lineage_gain`) only if it falls out cheaply from ROOT-S1b's statistics; `better` is required.
- **D-L4: POOL-lite**: 20 visible tasks (not 40), Exercism exercises disjoint from the E seeds (pack lint M7-P08).
- **D-L5: EVO-lite**: EV-TRACE (failing pool runs' session logs retained) + EV-MINE (extractors, clusters,
  `frictions.jsonl`, `initctl evo frictions`) + a minimal propose path: the agent reads the frictions and writes ONE
  proposal with `Repro:` evidence and up to 3 variants; the GATE-V2 screen (E3: candidate-only mini-run on the pool
  cache) gives the estimate for go/no-go (est ≥ 80‰). Cut: E1/E2 screens, compose/bank, the T0 gate fix, lessons,
  routing, guard (EV-GUARD's worker lane refusals stay if cheap).
- **D-L6: BENCH-V1 and L1-V2 are not built.**

## Lanes
| Lane | Content | Depends on |
|---|---|---|
| S1-CORE (running) | pack format, check runner, protected.json, canaries, pack.json, licences, pack lint, template validator | — |
| S1-E-PY / S1-E-RS | 12 Exercism-mutation templates each (m7 §3.1.1 E) | S1-CORE format (read its WIP; integrate on commit) |
| S1-C | 14 CRUX change templates | S1-CORE format |
| S1-T | 14 macOS terminal templates | S1-CORE format |
| ROOT-S1a | Root `sealed`: draw, validation, partitions (32/20/4), records `sealed_draw`, `sealed_g4`, `sealed_calibrate`, `milestone`; `g4_slice` on the audit driver; incumbent-run cache; sealed-lane caps; `calibrate`, `--probe`, the milestone check; CLI | ROOT-S1b's API (agree the fn signatures first) |
| ROOT-S1b | pure statistics module: the exact sign-flip DP, margin, superiority/no_gain signals, tiers' FA and power tables, A/A calibration math, ratchet signal if cheap; exhaustive unit tests against brute force | — |
| GATE-V2a | kernel: slice client, `accept_v2`, tiers and pre-flight, task.json v3, RunRecord v3, gate record v3 | ROOT-S1a's control-link interface (m7 §4.1) |
| GATE-V2b | kernel: proxy scope caps on actuals, the pool cache (incumbent reuse), the screen (reject-only, E3) | GATE-V2a's types |
| POOL-LITE | 20 visible tasks + licences + installer import + `kernel/pool/` mirror; the $0 failure-class script (m7 DEMO) | S1-CORE format |
| EVO-LITE | D-L5 | GATE-V2b's pool cache and screen |
| REAL-M7 | runbook, `tests/real/m7.sh`, `checks_m7.py` (balance polls, per-line real-$ stops, caps, redaction), a full $0 dry run on init-mockllm. Lines run later on the orchestrator's go: P (probe ≈ $0.03), C (A/A calibration), D (the demo, evolution-v2 §3.10 with D-L5), M (milestone) | everything |

## Rules for every lane
- `source ci/env.sh` and wrap every cargo/test command in `bash ci/slot.sh …` (3 build slots shared by ~12 lanes).
- **Disk is critical: 5.8 GB free, floor 5 GB.** No new target dirs (use the shared defaults). Rust template
  checks NEVER share or inherit a CARGO_TARGET_DIR: each check builds in its own scratch target and deletes it
  (a shared one let a stale lib make a todo!() stub score 1000; S1-E-RS finding, 23:40). Delete your own test
  homes and scratch dirs by exact path when done. If `df` shows < 5.3 GB free, stop building and report.
- Commit only your own files with explicit `git add <paths>` (never `-A`), small commits, keep every shared build green.
  Don't push; the orchestrator pushes.
- Tests use temp homes under the repo's target dirs; never touch `~/.init`, `~/.init-root` (the owner's LIVE install),
  the real `~/.ssh` etc. Signal only processes you spawned, by PID.
- Real-run test homes set `sandbox.read_home = false` in their charter, so a body under test cannot read the dev
  checkout (which holds the packs).
- Predicts grammar: `pass_rate@sealed >= parent+15%`, `cost_usd <= parent+30%` (evolution.md §3.10.1; the m7 §3.7 form
  does not parse).
- Open questions from DOC-M7 for ROOT-S1a to settle: the `promotion` record's `t0` field (cut by D-L2: omit), and
  `rollback.target` value `confirmed` (cut by D-L2: omit).
- Status file: `build/status/<LANE>.md` (what's done, test IDs green, blockers). Report in Vietnamese-free plain
  English; the orchestrator relays.
