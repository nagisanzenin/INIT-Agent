# FIX2: D14, a noise floor on the divergence guard plus a holdout rule for score gains (WP8c finding 1)

Status: **done** (2026-10-20, lane K1). Crate: `kernel/crates/kernel-gate/` only.
- Commit 35647a8 added the divergence floor.
- The follow-up commit adds the holdout rule.
- `root/tests/wp24.rs` is read only to me. FIX5 (lane B1) changed it for its larger stand-in S.

## Problem
When the gate-of-gates ran against the real `gate judge` (WP8c), control C1, a true improvement, was rejected as `reject/pool_holdout_divergence`.
- With the deterministic mock the measured δ is 0, so the divergence threshold `2·√(δ_p² + δ_h²)` is 0. Any gain that the gog's 1-task holdout did not also show counted as divergence.
- C1 passed only when the holdout draw was `sx-calc-fix`, about 1 draw in 5.

## Decision D14 as implemented
1. **A half-task floor on the divergence guard only.**
   - δ_min(set) = 0.5 / n_set in score units, half a task's worth on that set (`stats::boot::delta_min`; 0 for an empty set).
   - The divergence guard uses δ_p' = max(δ_p, 0.5/n_pool) and δ_h' = max(δ_h, 0.5/n_holdout): `reject: pool_holdout_divergence ⇔ D_pool − D_holdout > 2·√(δ_p'² + δ_h'²)`.
   - The floor applies everywhere, not only in tests. A deterministic measurement with zero observed variance does not mean zero true noise.
   - **Unchanged:**
     - the split guards `D_pool ≥ −2·δ_p` and `D_holdout ≥ −2·δ_h`, which keep the measured δ;
     - the NI margin `m = max(2·δ_used, 0.5/n)`, whose floor is the same δ_min and is not multiplied by `margin_factor`;
     - the futility look, the cost × time δ, and `perf`.
2. **The holdout rule.** An accept that rests on a score gain (`better`, `capability_gain`, `repro_fixed`, the §3.9 `ADDS_CAPABILITY` set) on a holdout of fewer than **4** tasks becomes `inconclusive: underpowered:holdout` (code `gate.g4.underpowered:holdout`).
   - `accept::GAIN_MIN_HOLDOUT = 4` is a constant, not a config key, so it cannot be tuned away (kernel/config.md §3: a gate parameter can weaken the gate as surely as code).
   - `cheaper`, `smaller` and `perf_gain` are unaffected, and a reject stays a reject.
   - As every inconclusive does under §3.12, the verdict escalates once, then becomes `reject: inconclusive_final`.
   - It is applied after §3.9's rule proper (`accept_rule`), in `accept_inner`, so the §3.9.2 recompute sees it too. The holdout is unchanged there, because agent tasks are pool tasks.
   - **Why the rule is needed.** With the floor, a 1-task holdout can never trip the divergence guard (δ_h' = 0.5, so the threshold is ≥ 1.0). A gain confined to the pool, the MS-5 answer-key signature, would then pass as `better` (30–33% in the simulation below). A holdout of fewer than 4 tasks cannot show that a gain generalizes.
3. **Rejected alternatives:**
   - Flooring the split guards too raised false accepts at a true −2δ. At 4 + 1 it went from 4.3% to 12.9%, and a holdout-only loss went from 0% to 7.6% (table A). That weakens the measuring stick, which Article 0 forbids.
   - Flooring the margin's δ before the ×2 would make the deterministic margin 1/n. That puts every deterministic gate with n ≤ 6 above `margin_max` (`underpowered`) and loosens NI everywhere.
4. **Audit trail in the gate record.** The fields are in `G4.score`, written by `g4::stage_record`, so promote and judge records both carry them. They are all new keys; no existing key changes meaning.
   - `score.delta_min_permille` is the margin's floor.
   - `pool` and `holdout` each carry:
     - `d_permille`;
     - `delta_permille`, the measured δ, used by the split guard;
     - `delta_min_permille`;
     - `delta_floored_permille`, used by the divergence guard.
   - In code, `accept::SplitStats` gains `delta_min` and `delta_floored`, `ScoreStats` gains `delta_min`, and `InconclusiveReason` gains `UnderpoweredHoldout`.

## Tests
- **Unit, G-22 style (`accept::tests`):**
  - `d14_score_gain_needs_a_holdout_of_four`, the new rule:
    - `better`, `capability_gain` and `repro_fixed` stand at a holdout of 4 and become `underpowered:holdout` at 1–3, with no `audit_required`;
    - `cheaper`, `smaller`, `perf_gain`, `no_gain` and `costlier_than_budget` are unchanged at a holdout of 1;
    - the reason string, the code and the serde form are pinned.
  - `d14_c1_gain_unexercised_by_a_one_task_holdout_is_not_divergence`: C1's case, 4 + 1 tasks with δ = 0.
    - The floor stops the divergence rejection: δ_p' = 0.125, δ_h' = 0.5, and the rule proper says `better` with L > 0.
    - The holdout rule makes the verdict `underpowered:holdout`.
    - On a 4-task holdout, a gain the holdout shows is `better`, and the same gain with an unexercised holdout is divergence (0.75 > 0.35).
    - G-49's data with δ = 0 still diverges.
    - A holdout loss is still `holdout_inferior`, and a regression is not accepted.
  - `split_guards_reject_pool_and_holdout_losses` is back in its pre-D14 form: the split guards use the raw δ.
  - `stats::boot::tests::d14_delta_min_is_half_a_task` asserts that the margin equals the pre-D14 formula on a 39 × 30 grid.
  - `g4::tests::better_candidate_is_accepted_and_runs_are_paired` now uses 4 holdout tasks.
- **Judge (real `gate judge`, mock model, the FIX5 stand-in S of 12 tasks + canary):**
  - `g45_g50_…` now runs in the gog's shape: 8 pool + 4 holdout, including `sx-calc-fix`, at the default minimums. M1–M7 are rejected for their defenses. C1 is `accept/better`, C2 `cheaper` and C3 `smaller`. The output is byte-identical on a re-run, and the home copy is untouched.
  - `c1_is_better_with_a_holdout_of_four_and_inconclusive_with_one`:
    - with a 1-task holdout (`sx-calc-fix`, `sx-upper`, `sx-wordcount`, `sx-squares`), C1 is `inconclusive/underpowered:holdout` with exit 2;
    - the record shows L > 0, δ = 0, δ_min = 0.5/12, pool δ_floored = 0.5/11 and holdout 0.5;
    - with a 4-task holdout that lacks `sx-calc-fix`, C1 is `accept/better`.
- **Commands:**
  - `cd kernel && CARGO_TARGET_DIR=target/k1 CARGO_INCREMENTAL=0 cargo test --offline -p kernel-gate -p init-kernel`:
    - kernel-gate lib **186 passed**, 2 ignored (`g21_full` and the D14 simulation);
    - `tests/g0.rs` 12 passed;
    - **`tests/judge.rs` 4 passed** (113 s, $0);
    - init-kernel: unit 2, `e2e` 1 and `judge_cli` 1, all passed.
  - `cargo clippy --offline -p kernel-gate -p init-kernel --all-targets -- -D warnings` is clean, and the changed files are `rustfmt --edition 2024` clean.
  - The simulation: `cargo test --release --offline -p kernel-gate --lib d14_full_rule -- --ignored --nocapture` (171 s in release).

## G-21 and the full-rule simulation
**G-21 proper is unchanged, bit for bit.** G-21 calibrates `margin` plus `bootstrap_ni`, and neither changed. `g21_calibration_reduced` passes. The full grid in build/status/WP8.md stands:
- n = 16, R = 2, steady-state history: 83.8% at 0 and 8.2% at −2δ (73.9% / 11.6% with no history);
- n = 30, R = 3, with history: 91.7% / 6.5%.

**Full rule** (`accept::tests::d14_full_rule_false_accept_with_and_without_the_floor`, `#[ignore]`). This is G-21's design, split into pool and holdout and judged by the whole accept rule:
- 2,000 gates per cell, bootstrap B = 2,000, noise B = 400;
- δ is measured per gate with no history, the worst case for a floor;
- the scenarios are defined in the test's doc comment.
- **Table B: the final rule.** Each cell is pre-D14 / floor only / **D14 (floor + holdout rule)**, as percentages.

| pool + holdout, R | 0 (smaller cand.) | −2δ every task: **false accept** | −2δ holdout only: **false accept** | c1: gain fixes half of each split (should accept) | pool-fit: same gain on the pool only, MS-5: **false accept** |
|---|---|---|---|---|---|
| 4 + 1, R=2 | 57.85 / 66.3 / **66.3** | 4.25 / 4.25 / **4.25** | 0 / 0 / **0** | 70.15 / 70.15 / **0** | 0 / 30.5 / **0** |
| 4 + 1, R=3 | 80.6 / 83.3 / **83.3** | 3.9 / 3.9 / **3.9** | 0 / 0 / **0** | 88.95 / 88.95 / **0** | 0 / 33.3 / **0** |
| 4 + 4, R=2 | 60.15 / 64.65 / **64.65** | 6.15 / 6.3 / **6.3** | 1.3 / 1.45 / **1.45** | 100 / 100 / **100** | 0 / 0 / **0** |
| 4 + 4, R=3 | 76.05 / 77.1 / **77.1** | 11.45 / 11.45 / **11.45** | 3.75 / 3.75 / **3.75** | 100 / 100 / **100** | 0 / 0 / **0** |
| 8 + 4, R=2 — the gog (FIX5) and gate minimums | 63.0 / 64.95 / **64.95** | 7.1 / 7.1 / **7.1** | 0.15 / 0.15 / **0.15** | 80.2 / 100 / **100** | 0 / 0 / **0** |
| 8 + 4, R=3 | 77.8 / 77.95 / **77.95** | 6.0 / 6.0 / **6.0** | 0.7 / 0.7 / **0.7** | 88.5 / 100 / **100** | 0 / 0 / **0** |
| 10 + 6, R=2 — Phase 0 example | 64.55 / 65.65 / **65.65** | 7.0 / 7.0 / **7.0** | 0.15 / 0.15 / **0.15** | 80.9 / 100 / **100** | 0 / 0 / **0** |
| 10 + 6, R=3 | 78.65 / 78.7 / **78.7** | 8.75 / 8.75 / **8.75** | 0.5 / 0.5 / **0.5** | 88.65 / 100 / **100** | 0 / 0 / **0** |
| 18 + 12, R=2 | 80.1 / 80.15 / **80.15** | 7.15 / 7.15 / **7.15** | 2.55 / 2.55 / **2.55** | 91.85 / 98.9 / **98.9** | 0 / 0 / **0** |
| 18 + 12, R=3 | 89.15 / 89.15 / **89.15** | 6.55 / 6.55 / **6.55** | 1.45 / 1.45 / **1.45** | 97.65 / 99.4 / **99.4** | 0 / 0 / **0** |

- **False accepts.** D14 equals pre-D14 in 28 of the 30 false-accept cells. The exception is 4 + 4 at R = 2, which is +0.15 points: 3 of 2,000 gates, at −2δ and on the holdout-only loss. Their raw-δ divergence guard had caught a regression that the split guards and NI missed. 4 + 4 is below gate.md's `min_pool` 8, so no production gate or gog draws it.
- **pool-fit is 0 everywhere under D14.** The holdout rule closes the 30–33% hole that the floor alone opens at 4 + 1. At holdout ≥ 4 the floored divergence guard still catches it.
- **c1, a true gain:**
  - At holdout ≥ 4, D14 raises the accept rate from 80–100% to 99–100%, because the raw-δ guard was firing spuriously.
  - At a 1-task holdout the price is 0%: every true gain is inconclusive, which is the rule's intent.
- **0 (no change):** the smaller-candidate accepts rise slightly (the spurious divergence rejects are gone); none of them was a `better`, so the rule does not touch them.
- The earlier `pool-fit` figure I reported (57.1% → 100% at 4 + 1) used a smaller candidate that passed every pool task. It counted `smaller` tie-break accepts as well. The scenario now uses a same-size candidate, so only `better` can accept.

**Table A: the first measurement, before the switch to divergence-only** (4 shapes; candidate smaller). Each cell is pre-D14 / divergence-only floor / the rejected floor on all guards. The run took 616 s in debug.

| pool + holdout, R (analytic δ) | true effect 0 | −2δ on every task (false accept) | −2δ on the holdout only (false accept) |
|---|---|---|---|
| 4 + 1, R=2 (0.233) | 57.9 / 66.3 / 66.3 | 4.25 / 4.25 / 12.9 | 0.0 / 0.0 / 7.55 |
| 4 + 1, R=3 (0.190) | 80.6 / 83.3 / 83.3 | 3.9 / 3.9 / 8.4 | 0.0 / 0.0 / 1.15 |
| 8 + 4, R=2 (0.119) | 63.0 / 64.95 / 70.8 | 7.1 / 7.1 / 7.5 | 0.15 / 0.15 / 0.6 |
| 8 + 4, R=3 (0.097) | 77.8 / 77.95 / 83.05 | 6.0 / 6.0 / 6.8 | 0.7 / 0.7 / 1.2 |
| 10 + 6, R=2 (0.103) | 64.55 / 65.65 / 73.6 | 7.0 / 7.0 / 8.1 | 0.15 / 0.15 / 0.6 |
| 10 + 6, R=3 (0.084) | 78.65 / 78.7 / 84.3 | 8.75 / 8.75 / 9.45 | 0.5 / 0.5 / 0.65 |
| 18 + 12, R=2 (0.078) | 80.1 / 80.15 / 81.9 | 7.15 / 7.15 / 7.3 | 2.55 / 2.55 / 2.6 |
| 18 + 12, R=3 (0.063) | 89.15 / 89.15 / 89.5 | 6.55 / 6.55 / 6.65 | 1.45 / 1.45 / 1.5 |

## Gate-of-gates with the real judge ($0, mock model)
| Run | Binary | S, sample | M1 · M2 · M3 · M4 · M5 · M6 · M7 | C1 · C2 · C3 | Result |
|---|---|---|---|---|---|
| 1 | floor on every guard (superseded) | 5 tasks, 4 + 1, `-c` minimums | test · forbid_missing · score_inferior · canary · score_inferior · manifest_error · canary | better · cheaper · smaller | pass (49 s) |
| 2 | divergence floor (35647a8) | 5 tasks, 4 + 1 | same | better · cheaper · smaller | pass (51 s) |
| 3 | **divergence floor + holdout rule (final)** | **FIX5's 12 tasks, 8 + 4, gate defaults** | same | **better** · cheaper · smaller | **pass (92 s)** |

The final run is `cd root && INIT_GOG_GATE_BIN=<repo>/kernel/target/k1/debug/init-kernel cargo test --offline -p init-root --features test-fixtures --test wp24 gate_of_gates_admits_the_correct_gate`. The test asserts `mutation{total 7, rejected 7, controls 3, controls_ok 3, disagreements 0, pass:true, sample_n 12}`.

On the old 4 + 1 sample, the final binary makes C1 `inconclusive/underpowered:holdout`, and the gog would fail. That is the rule working; FIX5's 8 + 4 sample is what the gog uses now. No M verdict can change under D14:
- M1, M2 and M6 fail in G0/G1;
- M4 and M7 trip the canary;
- M3 and M5 fail the NI test before any guard or the rule runs.

## Deviations
1. The orchestrator's first wording floored every guard's δ, including the margin's. On the measurement in table A it was narrowed to the divergence guard only, and the margin keeps its existing 0.5/n floor.
2. An empty split gets δ_min = 0 (no evidence, no floor). The margin keeps m = ∞ for n = 0.
3. In the §3.9.2 recompute without agent tasks, the floor follows that recompute's own pool size. The measured δ and the margin stay the same.
4. The reason string is `underpowered:holdout`, as the orchestrator specified. Its code `gate.g4.underpowered:holdout` is the first G4 code with a colon.

## Needs from others
- **gate.md / sealed-eval.md / testing.md owner:** record D14 (note below). Add `gate.g4.underpowered:holdout` to gate.md §4.1's G4 codes. G-49 needs no change: its threshold under the floor is 0.23, still below 0.4.
- **WP24 / FIX5:** done. The real-judge gog is green at FIX5's 8 + 4 with the final binary.

## Left
Nothing for FIX2. The full-rule simulation is `#[ignore]`; it takes about 3 min with `--release`.

## Note for the docs owner (D14)
**D14 (kernel/gate.md §3.8.3 and §3.9, root/sealed-eval.md §3.3) has two parts.** (1) The pool-versus-holdout divergence guard uses δ_p' = max(δ_p, 0.5/n_pool) and δ_h' = max(δ_h, 0.5/n_holdout), a half-task floor per split, in every gate, because a deterministic measurement with zero observed variance does not mean zero true noise. (2) An accept that rests on a score gain (`better`, `capability_gain`, `repro_fixed`) on a holdout of fewer than 4 tasks becomes `inconclusive: underpowered:holdout` (code `gate.g4.underpowered:holdout`; a constant, not a config key), because a holdout that small cannot show that a gain generalizes. `cheaper`, `smaller` and `perf_gain` are unaffected. Without the floor, δ = 0 made the guard reject every gain a small holdout did not show, and the gog's C1 failed on 4 draws in 5. With the floor but without the rule, a 1-task holdout could never trip the guard, so a pool-only gain (the MS-5 signature) passed as `better` 30–33% of the time. With both, that rate is 0 at every size. The split guards keep the measured δ, and the NI margin keeps m = max(2·δ_used, 0.5/n), so G-21 is unchanged. A broader floor on the split guards was measured and rejected: in G-21's design run through the whole accept rule, it raised false accepts in every cell, from 4.3% to 12.9% at 4 + 1 and a true −2δ, and from 0% to 7.6% for a holdout-only loss. That weakens the measuring stick, which Article 0 forbids. D14 as adopted leaves 28 of 30 false-accept cells exactly at their pre-D14 values. The other two (4 + 4 at R = 2, +0.15 points) are below the gate's minimums. It also lifts the true-gain accept rate at holdout ≥ 4 from 80–100% to 99–100%. The gate record carries `delta_min_permille` and, per split, `delta_floored_permille` next to the measured `delta_permille`. sealed-eval.md §3.3 should state that the gog's holdout sample must have at least 4 tasks (FIX5's 8 + 4 meets this), and that C1 is accepted on every such draw. Its §3.2 should note that a judge inside the sealed profile cannot bind sockets (WP8c finding 2).
