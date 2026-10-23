# Morning report: overnight build, 2026-10-20

*Written by the orchestrating Claude session; final at 12:00 +07. Detailed log: `BUILD_LOG.md`. Per-WP detail: `build/status/*.md`. Every deviation from the docs: `docs/reconciliation/build-deviations.md`.*

## Bottom line

- **INIT runs end to end on a real model.** The path is owner → `init-root` → sandboxed kernel → sandboxed body → proxy → Root egress → DeepSeek → tool call → reply. The test (deepseek-flash) wrote `hello.txt = INIT` in 2.39 s, cost < $0.01, hit the prompt cache at 959‰ on turn 2, and the key never appeared in any of 218 artifacts.
- **The self-evolution loop works at $0 on the mock model.** Acceptance A1–A6 and NO-OWNER-1 pass, twice each. In A3 the body edits its own source, passes G0–G3, hands off in the same conversation, then loses G4 and is rolled back automatically. A4 shows a crashing candidate is a non-event.
- **About 75% of Phase 0 is built**, measured in roadmap agent-hours (≈ 145 of 191; my estimate). Almost everything left needs one of three things from you: a disposable test account, more free disk, or a small model budget.

## What is green

| Area | State | Tests |
|---|---|---|
| Contracts (`init-proto`, `initctl`) | done | 60 |
| $0 test kit (mock LLM, fake body/kernel/Root, scenario runner) | done | 105 |
| Kernel: supervise, initctl server | done | 104 + 2 |
| Kernel: proxy (lanes, soft budgets, traces, forwards to the Root egress) | done | 83 |
| Kernel: gate G0–G4 + statistics + `gate judge` (gate-of-gates) | done | 186 unit + 12 G0 + 4 judge |
| Kernel: selfmap | done | 49 |
| macOS sandbox profiles + self-test | done | 222 pass / 0 fail / 11 skipped (disposable-only) |
| Root: charter, R1 stop/panic/broker, R2 egress/keys/ceilings, R4 signed lineage, R6 sealed eval (stand-in), R7 ledger | done | 103 (test-fixtures) |
| Body: provider (DeepSeek/Anthropic/OpenAI), loop, tools, log, context, boot, awareness files | done | 269 unit + 16 boot e2e + 8 self_md |
| Wasm extension host (Wasmtime 48.0.6 LTS) wired into the body | done | 21 + A2 |
| Real install path (`init-root install` → sandboxed kernel install → genesis) | done | Slice-1 harness 4/4 + real 1/1 |
| Acceptance A1–A6 + NO-OWNER-1 ($0) | done | all pass, run twice (and again after FIX6/FIX7) |
| Fast kernel start (FIX6): `kernel.sock` up before the body is ready | done | e2e 27 s → 12.9 s |
| Final full `ci/local.sh` (after FIX7–FIX9) | **green except 1 timing-flaky test** | 1,137 passed: kernel 682 (re-run with no-fail-fast), root + seed + ext 455. `h21` failed once under load and passed 3/3 alone. Logs in `build/ci/` |

Code: ≈ 143k lines of Rust across `kernel/`, `root/` and `seed/` (tests included), 98+ commits, clippy `-D warnings` clean per crate.

Key numbers:
- Extension instantiation p50 31 µs; warm extension rebuild 380 ms.
- Log fsync p50 2.8 ms; 0 events lost across 500 SIGKILLs.
- Root start to ready 1.5 s (was 25 s).
- Phase 0 measurements (`measurements/phase0.md`):
  - **M-1**: rebuild after touching one file: seed 6.7 s, root 3.3 s, kernel 1.3 s. PASS.
  - **M-2**: warm extension rebuild 0.34–0.39 s. PASS.
  - **M-4**: prefix identical across 5/5 handoffs, and the negative control changed it. The body's handoff window is 36 ms; a full promote takes 12.4 s. PASS.
  - **M-5: FAIL.** Body RSS grows 18.5 → 32.9 MB over 1,000 steps, 15.2 KiB per step against a limit of 2. FIX8 found it is **not a leak**: live heap returns to 1.4–2.5 MiB after each compaction. It fixed a real bug, though: the view kept masked tool outputs until the next compaction. Peak live heap went from 6.9 to 5.0 MiB. What still fails is the measurement method (see decision 4).
  - **M-3**: pending budget (10 real trials).

## Phase 0 progress by milestone

| M | State | What is left |
|---|---|---|
| M0 skeleton | build side done | Owner items: write the charter, create `INIT-Lineage`, `init-root keys set deepseek`, disk ≥ 35 GB, disposable account |
| M1 contracts + $0 harness | **done** | none |
| M2 kernel + Root core | built; live-safe tests green | SW-1…6, RT-P*/C*/E*, the sweep and kill tests. These need the disposable account (by rule, nothing that kills runs on your live account) |
| M3 body core | built; A1 (mock) green; one real flow green | E2E-1…4 on DeepSeek (small budget) |
| M4 extensions | built; A2 (mock) green; M-2 measured | M-3: 10 trials on DeepSeek (small budget) |
| M5 T2/gate/handoff/rollback/R6/R7 | built; A3, A4, A5-fake, A6 (mock), NO-OWNER-1 green; real gate-of-gates `pass:true` | A5 Root half, A2b, A6 steps 5–6 and 10, RT-W/L, MS/SA on the disposable account; G0 on the full seed needs disk |
| M6 Phase 0 exit | not started | real A1–A6 once each, E2E-5, spec v0.4, GitHub `fast-linux` CI, Phase 0 report |

## Decisions waiting for you

All of these have a recommendation in `docs/reconciliation/build-deviations.md` §3. None blocks the build. The first three matter most.

1. **Root size budget (§3.1).** The docs say ≤ 1,500 lines; the built Root is ≈ 6,450. Nothing was cut and every line maps to a doc section. *Recommended:* raise the budget to ≈ 7,000 with per-module sub-budgets, plus a small "core" subtotal for review.
2. **G4 statistical power (§3.2).** At n = 16 tasks and R = 2, the gate accepts a truly-equal candidate 83.8% of the time (target ≥ 90%) and a 2δ-worse one 8.2% of the time (target ≤ 7%). Only n = 30, R = 3 meets both targets. *Recommended:* default to n = 30, R = 3, plus a one-time A/A calibration run per model. Each gate costs ≈ 2.8× more model calls.
3. **Constitution text (§3.4)**, `docs/awareness.md` §3.3, 649 words. Still PENDING OWNER; it is built as written.
4. **M-5 measurement method** (build/status/FIX8.md). On macOS, `ps` RSS counts freed malloc pages that stay resident: 29.7 MB RSS against a 12.0 MB real footprint. The step-100 baseline also falls before the first context epoch. *Recommended:* measure `phys_footprint` and take the baseline at the first epoch, keeping both thresholds. Under that rule the fixed body passes 3/3 and the old body fails 2/3, so the rule still discriminates. One run was close to the limit (1.98 vs 2 KiB/step); boxing `MessageEv` would cut ≈ 40% more if you want margin.
5. Ratify **D14** (§3.3), done tonight: a half-task noise floor on the divergence guard only, plus `GAIN_MIN_HOLDOUT = 4`. Score-gain accepts on a holdout of fewer than 4 tasks become inconclusive. It closes an "answer-key" cheat: a candidate fitted only to the visible pool was accepted 30–33% of the time at a 1-task holdout under the floor alone, and is now accepted 0% at every size. False accepts are unchanged in 28 of 30 simulation cells; the 2 exceptions (+0.15 points) are below the gate's minimum pool.
6. **Root money meter (§3.13).** The Root bills 3.7× the proxy's price because it uses peak prices and bills cache hits at the input price. At that ratio the $5/day ceiling stops at ≈ $1.35 of real spend. *Recommended:* bill cache hits at the cache-read price.
7. **Stand-in S below 12 tasks (§3.15).** Under the new holdout rule a smaller S makes the gate-of-gates report `pass:false`, which can demote for the wrong reason. *Recommended:* report `underpowered`, do not demote, alert.
8. Repeated demotion at L0 (§3.9): enter safe mode after 3 demotions in 24 h.
9. Smaller items (§3.5–§3.8, §3.10–§3.12, §3.14): the mirror URL is `""` until M0, the seed release script, the batch-thinning rule, the Purpose block in the head, the lineage kind name, proxy cost reconciliation, debug-only opt-levels, and the disposable account as a separate macOS user.

## What I need from you

1. **Restart Docker Desktop.** A sandbox sweep test killed it at 01:20, along with 32 other processes. That class of test now runs only on a disposable account (decision D1 + CONVENTIONS safety rule).
2. **Rotate the DeepSeek key.** It appeared in chat. It is stored only at `~/.config/init-agent/deepseek.key` (0600) and was never printed, logged or committed. Balance $10.62 before and after tonight.
3. **Free disk.** Free space is about 6.4 GB now. The floor was lowered to 5 GB for tonight; the normal floor is 20 GB, and a full Phase 0 needs 35 GB. The Trash holds ≈ 1.1 GB of caches I moved there; empty it when you're ready.
4. **Create a disposable macOS user account** for the sweep, kill and rollback tests ($0; build-deviations §3.14).
5. **Authorize one small real-model batch (§3.17):** deepseek-flash only, a $1 hard cap and a stop at $0.20. It covers V-DS-1…6, E2E-1…3, the real A1–A6 and M-3 (10 trials). Expected spend is well under $0.20 at Slice-1's rate.
6. Read the constitution and decisions 1–4 above.

## Flaky tests found by the final CI (both green when run alone)

- `kernel-supervise --test handoff` `sv10`: hung for 3 h 25 min during the final CI after the Mac went to sleep (my keep-awake expired at 08:09). Run alone it passed 3/3 in about 5 s. The test has no timeout, so a hang blocks CI forever. *Fix next:* a bounded timeout on every handoff test.
- `init-body` `h21_incremental_cache_warm_vs_cold_recompile`: it compares two wall-clock compile times (310 vs 148 ms under parallel load). Run alone it passed 3/3. *Fix next:* compare cache hits, not milliseconds, or mark it as a measurement and move it out of the unit suite.
- Earlier, FIX9 fixed one more race: the Root wp21 Slice-1 test, now 20/20 green.

## Incidents tonight (both fixed, both reported at the time)

- **Process kills.** A sandbox sweep test matched 33 of your processes and SIGKILLed them, including Docker Desktop. Fixed: the three-condition sweep rule (D1), sweep tests only in a disposable environment, and a binding "no live-system kills" rule in CONVENTIONS and the build brief.
- **Email in User-Agent.** Four doc-research agents put your email in crates.io User-Agent headers. Fixed by a rule in CONVENTIONS and the brief. No later agent did it.

## Spend

- DeepSeek: < $0.01 of tonight's $1 cap (one Slice-1 real run).
- Cloud: $0.
- Agent quota: the 5-hour window was at 63% at 06:26; weekly 28%.

## Where to look

- `BUILD_LOG.md`: the timeline.
- `docs/reconciliation/build-deviations.md`: deviations, gaps, decisions, the test inventory and measurements.
- `build/status/S1.md`: the real end-to-end run. `build/status/WP15.md`: acceptance.
- `build/status/FIX2.md`: D14, with simulation tables.
- Docs were synced to the code tonight (commit 986669f): each changed doc has a "Build sync 2026-10-20" block in its Spec deltas.
