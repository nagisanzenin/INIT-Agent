# M7 runbook: the lean proof lines (REAL-M7)

```
Status: draft v0.1 · 2026-10-20 · Area: evolution (lane REAL-M7, PROOF-LEAN)
Depends on: build/specs/PROOF-LEAN.md (binding), PROOF_PLAN.md, docs/m7-design.md §3.2.9, §3.6, §3.7, §3.8,
            docs/evolution-v2.md §3.5.3, §3.10, docs/evolution.md §3.10.1, measurements/real-v0.1.md
Defines:    the order, gates, money guard and stop rules of the real M7 lines P, C, D (with R), M and G1L; the
            $0 dry run (G1D); the D-0 pre-registration of the self-evolution demo
Harness:    tests/real/m7.sh + tests/real/checks_m7.py (reuses tests/real/checks.py and scripts/slice1/collect.py)
```

## 1. Purpose

This runbook answers PROOF_PLAN.md's goals 1 and 2 (Stick, Improve) on real deepseek-flash for at most **$3.00 real**. The agent itself proposes a change from mined failure evidence. The change goes through G0–G3, the handoff and G4 v2 on sealed S v1. The milestone check on the untouched reserve then confirms the gain or finds no regression, or the run reports an honest null.

Every line runs on the orchestrator's go, one line at a time. Before any go, the same harness runs at $0 on `init-mockllm` (the dry run).

## 2. Order and gates

```
P (cost probe) → C (calibration, A/A) → [G1D: $0 arm-symmetry check, mock] → D (R: mine → propose → screen; then G4) → M (milestone + run-down) → G1L (only if D counted)
```

A real line starts only when **all** of these hold. `checks_m7.py may_start` and `line_budget` enforce them.
1. The orchestrator gave the go for this line: `M7_GO=<line>`, plus `M7_I_MEAN_IT=1`.
2. `tests/real/m7.sh ready` shows the line `READY`, and its $0 dry run on the mock home is green.
3. The ledger order holds: C after P, D after C, M after D, and G1L only after D was accepted as `better` or `lineage_gain`. A real D also needs G1D green on the mock home.
4. No halt is set. Halts come from: a tripwire (canary, tamper, leak); C's false-accept rate above 70‰; P's Root/real ratio outside [1.8, 2.2]; or a meter/balance mismatch. Clearing a halt needs a $0 fix and `checks_m7.py clear-halt --reason …`.
5. The line's real cap fits what is left of the envelope. For D, that check includes M's and G1L's caps (§4).

**Off-peak is advisory.** DeepSeek peak is 01:00–04:00 and 06:00–10:00 UTC, Mon–Fri (08:00–11:00 and 13:00–17:00 +07). Off-peak halves the real price but is not required. Before each line `m7.sh` prints the window, the projected peak fraction over the line's wall time and the cost factor (1 + peak fraction). A line projected to touch a peak minute gets its Root ceilings at factor 1.0 instead of 2.0, and the balance poll still bounds it. Preferred windows: 17:00–08:00 and 11:00–13:00 +07, and weekends.

## 3. Money

**Envelope.** $3.00 real, the program total over all lines. Per line, spend counts as the larger of the priced estimate and the settled balance drop. The ledger (`target/m7-homes/r/ledger.json`, copied into each line's artifacts) carries the running total.

**Meters, in order of trust:**
1. **Balance** (`GET user/balance`, free, through the kernel proxy → Root egress). It is the authority, but it has a 2-decimal resolution and billing lags by minutes. It is read before the line, every 5 minutes during it, and after it, once two reads 60 s apart agree (at most 10 minutes).
2. **Priced usage.** The Root's own egress usage ledger (`ROOT_HOME/egress/usage`, every lane including the sealed ones) holds tokens per request: input, cache_read and output. They are priced at DeepSeek's list price for each request's window. This is the real-$ estimate, polled every 30 s.
3. **Root meter.** Peak prices at all hours: about 2.0× real off-peak and 1.0× at peak. It is the runaway backstop.

**The guard** (`checks_m7.py Guard`, unit-tested). It stops the line when any of these happens:
- the priced estimate reaches the line's real cap;
- the balance drop reaches the cap;
- the balance falls faster than the priced usage, by more than 25% + $0.02 after at least $0.10 has gone (this also halts);
- a canary, tamper or leak record appears in the Root lineage (this halts all spend).

To stop, the guard **freezes** spend: it rewrites the test home's charter so that each money pool's ceiling equals what that pool has spent, and applies the charter live. The Root then refuses every request. If the apply fails, it falls back to `init-root stop`. Two unreadable balances in a row **tighten** the ceilings so that the Root meter alone bounds the rest of the line at peak prices.

**Charter ceilings per line.** The Root charges sealed lanes only to `sealed.reserve_usd`, and the zone gets `total_usd − reserve_usd`. So each pool's ceiling is what it has spent so far, plus its share of the line's real cap × the Root/real factor, plus four parallel request reservations (4 × $0.0786). The reservations matter: a ceiling at exactly the cap would refuse in-flight reservations before the cap is spent, and a refused candidate run scores 0 (c712701), which biases G4. These ceilings are the backstop; the guard is the tight stop.

## 4. The plan and the cost of each line

P measures the mean real $/run. The richest plan is picked such that C can finish at its estimate with every later line's cap reserved (`pick_plan`).

| P's mean $/run (real) | Plan | C | D tiers allowed | M |
|---|---|---|---|---|
| ≤ $0.0035 | **full** | S: `sealed calibrate --repeats 6` (32 × 6; the lean Root has no replacement draws or template probes); pool: `initctl evo calibrate --repeats 2` (20 × 2) | strong, standard, lean | 20 × 3 |
| $0.0035–0.0037 | **trim** | as full | standard, lean | 20 × 2 |
| $0.0037–0.0044 | **t5** | as full | lean (go for `better` at ≥ 250‰) | 20 × 2 (directional) |
| > $0.0044 | **stop** | tighten the per-run caps (turns 6, output 12,000), re-probe once ($0.03) and re-plan; if still above, M7 reports P (and C if run) only | — | — |

**What each real line will cost** (real $, off-peak). Low is at $0.002/run, expected at $0.003/run (m7 §3.6's planning mean), and high is the line's cap under the full plan at $0.003/run, which is the most the guard lets it spend. Every cap is rescaled by P's measured mean before C.

| Line | Runs | Low | Expected | High (cap) | Wall |
|---|---|---|---|---|---|
| **P** cost probe | 10: `sealed probe --per-family 2` (E-py, E-rs, C, T: 8) + 2 pool tasks (`evo calibrate --repeats 1 --limit 2`) | $0.02 | $0.03 | **$0.06** | 5 min |
| **C** calibration + pseudo-A/A | 232: S 32 × 6 + pool 20 × 2 | $0.46 | $0.70 | **$0.84** | 50 min |
| **R** (inside D) mine $0, worker, E3 screens | worker + 3 × 6 | $0.07 | $0.08 | **$0.15** (pre-registered) | 30 min |
| **D** G0–G3 + G4 v2, both arms fresh | 211 at standard | $0.45 (strong fits: $0.71) | $0.66 | **$0.80** (strong: $1.26 only if it fits) | 90 min |
| **M** milestone 20 × 3 × 2 + 3 canaries + run-down | 123 + 1 turn | $0.25 | $0.37 | **$0.45** | 35 min |
| **G1L** lean real A/A (only if D counted) | 127 + G0–G3 | $0.28 | $0.41 | **$0.50** | 60 min |
| **Total, D not accepted** (no G1L) | ≈ 590 | **$1.25** | **$1.84** | $2.30 | ≈ 4 h |
| **Total, D accepted** (+ G1L) | ≈ 720 | **$1.53** | **$2.26** | **$2.80** | ≈ 5 h |

G1 is dropped from its m7 slot before D (§9 D-R1). The lines fit one or two off-peak windows.

## 5. Line protocols

### 5.1 P: the cost probe (first real spend)
- **Runs:** `init-root sealed draw --pack-dir <pack s1>` if S is not drawn yet ($0; the Root installs the pack read-only into ROOT_HOME, and the harness passes the path without reading the pack). Then `init-root sealed probe --per-family 2 --usd X --real` (2 throw-away instances per group E-py, E-rs, C, T, R = 1, deleted afterwards). Then 2 pool tasks at R = 1 through the kernel: `initctl evo calibrate --repeats 1 --limit 2 --wait`.
- **Records:** `sealed_draw` (counts only; expect G4 E 14 / C 9 / T 9, reserve E 10 / C 5 / T 5, 4 canaries) and `sealed_calibrate.probe.per_family`. Per task type, the probe also records requests, tokens in (miss and hit), tokens out, wall, the Root $, the priced $ and the balance change. They go to `measurements/m7-probe.md`.
- **Pass:** the probe record exists; the Root/real ratio is in [1.8, 2.2] off-peak; caps and cache keys behave. P's mean $/run picks the plan (§4) and becomes `gate.g4.cost_prior_usd`'s p90 source.
- **Stop:** cap $0.06. A ratio outside [1.8, 2.2] (off-peak), or caps that misfire, halt M7 for a $0 fix.

### 5.2 C: calibration and the pseudo-A/A
- **Runs:** `init-root sealed calibrate --repeats 6 --usd X --real` (the lean Root: 32 × 6, band selection, no replacement draws or template probes). Then pool calibration through the kernel: `initctl evo calibrate --repeats 2 --wait` (20 tasks × R = 2; it seeds the pool cache for the screen and E3 and the miner's failing runs). The pool needs R = 2 so that the go/no-go's `s_c` has its ≥ 36 screen-eligible runs (evolution-v2 §3.5.3).
- **Pass** (m7 §3.2.10): FA ≤ 50‰ with upper bound ≤ 70‰; FR ≤ 100‰; m ≤ 0.25 at standard (m = max(2·δ_r3, 0.5/32)); and n_kept ≥ 24.
- **Stop:** at the cap with what is done (S ships at the n reached, ≥ 24). FA above 70‰ halts all spend: debug at $0.
- After C, S is frozen. The `sealed_draw` and `sealed_calibrate` seqs go into D-0.

### 5.3 G1D: the $0 arm-symmetry check (mock only, before any real D)
- A harness-committed, comment-only T2 change goes through G0–G3, the handoff and G4 v2 at lean tier on the mock home, with fixture packs.
- **Pass:** `sealed_g4.d_permille == 0` exactly, the verdict is not an accept, and no tripwire fires. The mock is deterministic, so any nonzero d, or any status other than tie, tie_negative or underpowered, is a pipeline asymmetry.
- This replaces the pipeline-bug half of m7's real G1 at $0, and more strictly (§9 D-R1).

### 5.4 D: the demo (evolution-v2 §3.10 with PROOF-LEAN D-L5)
- **D-0:** freeze the pre-registration (§6) with `checks_m7.py prereg` before any friction is read. It records the block's sha256, the S records' seqs and the time.
- **D-1, mine ($0):** the kernel's friction index (`state/kernel/evo/frictions.jsonl`, EV-MINE) over C's retained failing pool runs (EV-TRACE). It is supplemented by a $0 census of the 6 S-REAL sessions (`build/real-artifacts/real/*/init/sessions`), which only breaks ties.
- **D-2, select:** the D-0 rule, applied mechanically by `select_cluster`.
- **D-3, the agent proposes.** The owner message, fixed text (`checks_m7.py D3_LINE`), sent to the live agent: *"Evolution task (REAL-M7 D-3). Follow `skills/evolve/SKILL.md` for friction {fid} ({signature}): read it with `initctl evo frictions --json --all` and its evidence refs, then file ONE proposal for it with `initctl evo propose`, with exactly this predicts list: ["pass_rate@sealed >= parent+15%", "cost_usd <= parent+30%"] and `"tier_hint":"t2"`. Submit up to 3 genuinely different variants (`initctl evo variant`), then run `initctl evo screen <proposal> --wait` (bash timeout 3600 s). KEEP_BASE is a valid outcome. Do NOT promote anything. Reply with the proposal id, each variant's screen result, est and go, in at most six lines."* The message names the friction; it never names the change. The kernel builds each variant's commit and its message from the proposal.
- **D-4, screen:** E0 (build + G0) and E3 (kernel, the incumbent from C's pool cache). It yields `est`, `go` and the `chosen` variant on the proposal record.
- **D-5, go/no-go:** the D-0 thresholds. Otherwise cluster #2, once, if line R's cap allows. Otherwise stop: "no friction the gate can resolve", with the census and the estimates.
- **T2 check:** the chosen variant must be `s_measurable` and change ≥ 1 file under `src/` and nothing under `vendor/`. Otherwise the line stops before G4 (D-0 item 8). The proposal's predicts must carry a positive `pass_rate@sealed` clause (D-0 item 7).
- **D-6, promote.** The owner message, fixed text (`D6_LINE`): *"Evolution task (REAL-M7 D-6). Promote proposal {p}'s chosen variant {k}, as `skills/evolve/SKILL.md` §5 says: `git -C "$INIT_HOME/body" fetch --quiet origin && git -C "$INIT_HOME/body" branch -f evo/{p} origin/init/evo/{p}/v{k}`, then `cd "$INIT_HOME/body" && initctl promote evo/{p} --wait` (bash timeout 3600 s). Reply with the promote result in one line."* The validator takes the commit only while the proposal is `screened` and it is the chosen variant. The pipeline then runs G0–G3, the handoff at idle, and G4 v2 at the D-0 tier (`gate.g4.tier` pinned; `gate.g4.cost_prior_usd` from P), with both S arms fresh.
- **D-7 to D-8, verdict and record:** the gate record's accept class, the `sealed_g4` aggregates (status, d to 50‰, m, cpp, looks_left), the predict score (hit iff d ≥ 10·N‰), and the (est, d) pair.
- **D-9, sticks:** D-0 item 10.
- **Stop:** line R at $0.15; line D at its tier cap; any tripwire halts all spend; a `screen.grid` failure or a meter/balance mismatch stops the line.

### 5.5 M: the milestone check and the run-down
- **Runs:** `init-root sealed milestone --repeats R --usd X --real`: the current tuple against the last confirmed milestone's `current` (genesis here), both arms fresh on all 20 reserve tasks, R = 3 (2 under trim and t5), with health on both arms and leak on the current first. A regression raises the `milestone_regression` alert; the rollback is the owner's. If D was not accepted, current equals genesis, the Root skips the check, and M is the run-down only.
- **Run-down:** one owner turn ("Reply with the single word ok.") on the current LKG, then a 10-minute idle watch. It passes when no `rollback`, `demotion` or `safe_mode` record appears. `audit verify --full` must exit 0.
- **Report:** the outcome (`confirmed`, `not_confirmed`, `regression`, `partial`) with d, n, repeats and held_out.
- **Stop:** at the cap with outcome `partial`. The reserve is not consumed, and the check retries in the next window.

### 5.6 G1L: the lean real A/A (only after D counted)
- **Runs:** a harness-committed, comment-only T2 change on top of D's LKG, through G0–G3 and G4 v2 at **lean**, both arms fresh. The incumbent is D's winner, whose runs are never reused.
- **Pass:** the status is not superior, inferior or a tripwire, and |d| ≤ 150‰ (the smallest effect a `better` accept at standard rests on). On failure, D's accept is **voided** for the proof result, pending a $0 replay.
- It is run after M, so M measures exactly D's tuple.

## 6. D-0: pre-registration of the demo

<!-- D0:BEGIN -->
**D-0. Pre-registration of the self-evolution demo, frozen before any mining output is read.** `checks_m7.py prereg` records the sha256 of this block, the seqs of the Root records `sealed_draw` and `sealed_calibrate`, and the time, in the ledger and in line D's artifacts. Nothing here changes after the freeze. A change needs a new pre-registration and counts as a second attempt.

1. **Sources.** The demo home's visible-pool runs from line C: the incumbent, R = 2 on the 20 pool tasks, with the failing runs' session logs retained. The 6 S-REAL sessions add supplementary occurrence counts. No S data is used: S runs leave no traces, and the agent and the harness see only Root aggregates.
2. **Selection rule.** Consider the friction index's proposal-eligible, untried clusters that span ≥ 3 pool tasks and ≥ 2 families. Families are the cluster's `families`; if that is empty, its pool labels other than `live` and `smoke`. Take the cluster with the highest `s_c`. Ties go to more S-REAL occurrences of the same signature, then to the lower friction id.
3. **The proposal.** The agent itself (the live body, deepseek-flash, L0) writes ONE proposal for that cluster. It has at most K = 3 variants, at least 2 attested `Repro:` refs, and the predicts list `pass_rate@sealed >= parent+15%`, `cost_usd <= parent+30%`. The owner messages are the fixed texts of runbook §5.4. They name the friction, never the change.
4. **Screen.** E0 (G0 + G1 on the variant) and E3 (a candidate-only mini-run, R = 1, on ≤ 4 targeted pool tasks plus 2 controls, with the incumbent from the pool cache). E1, E2, compose and the bank are cut (PROOF-LEAN D-L5).
5. **Go / no-go.** est = round(1000 · q · s_c · fix · T), with q = 0.5 (or the Root's published p̄_S), T = 0.8 and fix = (targeted tasks fixed in E3 + 1) / (targeted tasks + 2).
   - At standard or strong tier: go at est ≥ 80‰. Aim for `better` at est ≥ 150‰; otherwise aim for `lineage_gain` (the span is empty, so it has room).
   - At lean tier: go only at est ≥ 250‰, aiming for `better`, because NI is underpowered at lean and the ratchet cannot fire there.
   - Below the threshold: one retry on cluster #2 if line R's cap allows; otherwise stop and report "no friction the gate can resolve".
6. **Tier rule.** After line C, with line R's cap reserved, take the strongest tier the plan allows (strong, then standard, then lean) whose cap, plus line M's and line G1L's caps, fits what is left of the $3.00 envelope (`checks_m7.py` `line_budget`).
7. **The Predicts trailer of the final commit.** The kernel writes the commit message from the proposal (EVO-LITE), so the trailer is the proposal's predicts list, `pass_rate@sealed >= parent+15%; cost_usd <= parent+30%`, for either aim. A `lineage_gain` needs only a positive pre-registered sealed claim (m7 §3.2.13), which +15% is. The predict score is hit iff d ≥ 150‰. Grammar: evolution.md §3.10.1.
8. **T2 only.** S slices measure only T2 changes (PROOF-LEAN D-L2; dep is forbidden to the engine at L0). The chosen variant must change ≥ 1 file under the body crate's `src/` and nothing under `vendor/`. If it does not, the line stops before G4, and the report says so.
9. **One G4 attempt.** There is one promote and one G4 v2 slice. There is no second attempt under this pre-registration.
10. **What counts.** The gate record tags each accept with its class: `better`, `lineage_gain` or `tie_break:<which>`.
    - For the proof result, "accepted" and "sticks" count only `better` or `lineage_gain`. A `tie_break` accept (for example `smaller` on body tokens, or `cheaper`) is reported but does NOT count.
    - "Sticks" also needs all of these:
      - the change is LKG;
      - `audited` moved (m7 §3.2.12) or an audit passed;
      - line M's milestone on the untouched reserve finds no regression;
      - no rollback in the run-down;
      - the conditional real A/A G1L passes (|d| ≤ 150‰, not superior).
    - "Confirmed" also needs M's outcome `confirmed`.
11. **Caps and stops.** Line R (mining at $0, the worker session, the screens) is capped at $0.15 real. Line D's G0–G3 and G4 get the tier cap from item 6. Any canary, tamper or leak trip stops all spend. A `screen.grid` failure, or a meter/balance mismatch above 25%, stops the line.
<!-- D0:END -->

## 7. Reporting

Each line writes redacted artifacts to `build/real-artifacts/m7/<mode>/<line>/`: the run log, the verdict, the ledger, the Root lineage, the egress usage, the sessions, the gate reports and the evo state. ROOT_HOME/sealed is never collected. The orchestrator folds the results into:
- `measurements/m7-probe.md` (P): cost per task type, the Root/real ratio and the plan picked.
- `measurements/m7-results.md`: C's FA, FR, δ, m and n_kept; D's census, cluster, est, tier, variant tier, Predicts, accept class, sealed status and d bucket, and the predict score; M's outcome; G1L; the spend per line, with every meter named.
- The stop/continue answer:
  - *sticks* means D counted (better or lineage_gain) and the D-0 item 10 conditions hold;
  - *confirmed* means sticks and M `confirmed`;
  - *null* means no go, a reject, a tie-break accept only, or a voided accept, each reported with its reason.

## 8. Commands

```sh
source ci/env.sh
tests/real/m7.sh selftest                      # $0, pure logic (60+ checks, D-0 drift included)
tests/real/m7.sh stage-bins                    # after the lanes' builds (through ci/slot.sh): clones into target/m7-bin
tests/real/m7.sh ready                         # per line: READY or blocked + the missing interfaces
INIT_SLOT_POOL=real bash ci/slot.sh tests/real/m7.sh mock install
INIT_SLOT_POOL=real bash ci/slot.sh tests/real/m7.sh mock dry     # every line at $0, in order
# real, one line per go from the orchestrator:
INIT_SLOT_POOL=real bash ci/slot.sh tests/real/m7.sh real install
M7_I_MEAN_IT=1 M7_GO=P INIT_SLOT_POOL=real bash ci/slot.sh tests/real/m7.sh real run P
tests/real/m7.sh real ledger                   # spend, plan, halts, D-0 freeze
```

Environment: `M7_PACK_DIR` (real; default `root/sealed/packs/s1`) and, for the dry run, `M7_FIXTURE_PACK=<ROOT-S1a fixture pack dir>`, both passed to `sealed draw --pack-dir`; and `M7_POOL_DIR` (default `fixtures/pool-v1`). The guard periods can be shortened for the dry run (`M7_POLL_S=2 M7_BAL_S=10`). `M7_SIM_DRAIN_MUSD` injects a simulated balance drop on the mock to exercise the real-$ stop. `M7_DRY_FORCE=1` (the default, mock only, labelled in the verdict) forces a cluster and a go so that the mock reaches G4.

## 9. Deviations from the design (recorded for the orchestrator)

| Id | Design | Here | Why |
|---|---|---|---|
| D-R1 | m7 §3.6 G1: a real null-change G4 at standard before D ($0.66), whose incumbent runs D reuses | **G1D** ($0, mock, exact: d = 0) before D; D runs both S arms fresh; **G1L** (lean, ≈ $0.41) only after D counted | The demo needs G1 only for the claim: the pipeline-asymmetry check of m7 §3.2.10. Its other roles are not needed. The cache is a cost saving, and fresh arms remove cache asymmetry. LKG = audited = genesis holds without it. The exact mock check catches pipeline bugs more strictly than a real A/A can (a real A/A at n = 32 only sees asymmetries ≥ ~20 points). The real A/A is kept where it changes a claim. This saves ≈ $0.37 and 50 min on the critical path when D is not accepted |
| D-R2 | P: 2 per family over E, B, C, T + 2 pool | 2 per family over E, C, T + 4 pool (still 10 runs) | D-L1 drops B |
| D-R3 | C: pool 40 × R = 1 | pool 20 × R = 2 | D-L4's 20-task pool; keeps the go/no-go's ≥ 36 runs for `s_c` |
| D-R4 | M: 24 reserve tasks | 20 | D-L1's reserve |
| D-R5 | charter ceilings = 2× the real envelope ($6.00), fixed | per line: each pool's share of the line's cap × the Root/real factor + 4 reservations, frozen to "spent" on a stop | the orchestrator's "ceilings equal to the line's cap", adjusted for the Root's per-request reservations and its separate sealed pool |
| D-R6 | off-peak only | off-peak advisory; a peak line gets factor-1.0 ceilings | the orchestrator's rule |
| D-R7 | tier rule "strong if ≥ $1.50 left after G1" | strongest tier whose cap + M's + G1L's caps fit, after C with R's cap reserved | G1 moved; same intent (D-0 item 6) |
| D-R8 | the runbook's change (verify-on-finish) as a fallback when no cluster qualifies | no fallback inside this pre-registration: a no-go is the reported result | PROOF-LEAN: "no runbook-dictated change unless the engine finds nothing (then that is reported as the result)" |
| D-R9 | C's pool half: the kernel's `gate calibrate --pool` (m7 §3.2.10) | `initctl evo calibrate [--repeats R] [--limit N]` (REAL-M7 `087b5e5`): the LKG alone on the pool, every run into the pool cache, failing logs retained, then a mine | no lane built a pool calibration; without it the miner has no failing pool runs and E3 no cached incumbent before D |
| D-R10 | C: 32 × 6 + replacements + reserve template probes | 32 × 6 only | ROOT-S1a's lean Root cut replacements and template probes (S ships with the in-band tasks if ≥ 24, else unselected) |
| D-R11 | Predicts sealed target = rounded est when aiming for `lineage_gain` | always +15% (the proposal's list) | the kernel writes the variant's commit message from the proposal, before est exists |

## 10. Safety

- **Key.** The key file is read only by `m7.sh` in real mode, after `M7_I_MEAN_IT=1` and `M7_GO=<line>`. It is copied with `tr` into the test Root's key store (0600) and removed on exit and before collection. It is never echoed, logged or put on a command line. `checks_m7.py` never reads it.
- **Identity.** All processes run under `env -i`, and git runs with `GIT_CONFIG_GLOBAL=/dev/null`. Artifacts go through `collect.py` (key and `sk-` shapes) and then `checks_m7.py redact`, which removes e-mail addresses other than `agent@init.local` and the git identity, and rescans. Only counts are printed.
- **Sealed S.** The harness copies the pack into the test release without reading it. It never reads `ROOT_HOME/sealed/{S,seeds,g4}` and never collects them. The mock runs with `--record-bodies none`, so no sealed prompt is written. S evidence reaches the harness only as Root records.
- **Test home.** `target/m7-homes/{m,r}`. The charter sets `sandbox.read_home = false` and `allow_full = false`, and `read_deny` covers `<repo>/root` and `<repo>/.git`, so the body cannot read the packs. The dev checkout itself cannot be denied, because it holds the test home (C15). Homes are removed by exact path. `~/.init` and `~/.init-root` are never touched. Signals go only to this harness's own children, by PID.
