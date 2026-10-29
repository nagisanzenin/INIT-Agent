# STATS-053: lane M #2–#4 and L3 (v0.5.3), measured before and after

**Label: model: none (pure simulation)**, machine Mac14,2 (Apple M2), debug builds of the repo's own code, seeded.
Every number here is in [STATS-053.json](STATS-053.json), with the commit, seed, iterations and the command that
made it. "Main" means the Root's behavior at 67d5172, measured at 525fc26 (the harness commits on top of it change no
behavior: `met_check` and `fails` became pure functions that `stop` and `complete` call; after the rebase onto
16792a0 that commit is e61d8ed). "L3" means a4bd23f, the rebased head with the round and rolled-back fixes: its null
and power tables are bit-identical to the first run at b347682 (rebased 9164ce0), so neither the rebase, the fixes nor
the optimized build moved a number.

The simulations call the Root's code, not a copy of it: `engine::judge` for the sentinel; `reading`, `fails`,
`measure`, `complete` and `met_check` for GBAL; `bootstrap_ni`, `noise` and `margin` for G4. Owner turns are synthetic:
RF-11's model (sentinel/tests.rs; ADR-010's simulation; X10's clustering), with per-turn cost and wall lognormal
(σ = 1), requests (1 + U{0..7})·e^S, provider errors Bin(requests, 2 %), corrections 10 %, cache reads 0 on 15 % of
turns else U{500..1000}‰ (a stand-in: no real distribution is measured). "iid" is a session per turn; "clustered" is
X10's heavy case, sessions of 4 turns sharing a log offset with σ_s = 0.6. ± is one Monte Carlo standard error.

| What | Table (STATS-053.json key) | Iterations | Command |
|---|---|---|---|
| M #2 sentinel | `sentinel_null` (525fc26; L3 changes nothing here) | 20,000 null windows per turn model | `cargo test --offline -p init-root --bin init-root sentinel::tests::sim -- --ignored --nocapture --test-threads 1` |
| M #2 per reading, M #3 | `gbal_null_main` (525fc26), `gbal_null_l3` (a4bd23f) | 20,000 readings per cell (4,000 with 40 before); 4,000 goals per cell (1,000 waiting 40 turns) | `cargo test --offline -p init-root --bin init-root goal::sim -- --ignored --nocapture --test-threads 1` |
| power of a guardrail | `gbal_power_main` (525fc26's goal code with the harness of b40577c), `gbal_power_l3` (a4bd23f) | 4,000 readings per cell (2,000 with 40 before) | same command |
| discrete-card guardrails | `gbal_discrete_guard` (6ef9cba, the built rule; bit-identical to 57bd941's) | 4,000 null readings and 1,000 per rise per cell | `cargo test --offline -p init-root --bin init-root --config 'profile.dev.package.init-root.opt-level=3' goal::sim::gbal_discrete_guard_table -- --ignored --nocapture --test-threads 1` |
| M #4 G-21 | `g21_widths` (525fc26; measurement only) | 1,000 gates per cell, B = 2,000 | `cargo test --offline -p kernel-gate --lib stats::sim::tests::g21_widths -- --ignored --nocapture` |

`INIT_STATS_OUT=<dir>` writes each table as JSON; STATS-053.json joins them.

## M #2: the sentinel's false rollbacks when nothing changed

The chronic tests as `judge` runs them: H5b, H8, H10, H11 on units (the 8th largest of 40 at α = 0.005), H7 per turn
and H9 per request at α = 0.002. foundations.md P-3 stated only the union bound, 3 looks × (4 × 0.005 + 2 × 0.002) =
7.2 %.

| Turn model | False rollback per window | Fires at look 5 / 10 / 20 (each look alone) | Triggering signal (H5b / H8 / H10 / H11 / H7 / H9) |
|---|---|---|---|
| iid, a session per turn | 1.88 % ± 0.10 | 0.30 / 0.80 / 1.17 % | 0.57 / 0.61 / 0.03 / 0.14 / 0.35 / 0.17 % |
| iid, one long session | 1.12 % ± 0.07 | 0.11 / 0.13 / 0.92 % | 0.35 / 0.29 / 0.00 / 0.00 / 0.38 / 0.10 % |
| clustered L=4 σs=0.6 | 1.70 % ± 0.09 | 0.17 / 0.55 / 1.19 % | 0.52 / 0.45 / 0.22 / 0.10 / 0.29 / 0.12 % |
| clustered L=8 σs=0.6 | 2.97 % ± 0.12 | 0.09 / 0.37 / 2.70 % | 1.21 / 0.74 / 0.48 / 0.15 / 0.26 / 0.13 % |
| clustered L=4 σs=0.3 | 1.40 % ± 0.08 | 0.21 / 0.47 / 0.89 % | 0.41 / 0.39 / 0.14 / 0.05 / 0.27 / 0.14 % |

Exact size of one unit test (the exceedance tail at the threshold the engine uses): 648 ppm at 5 units (K = 5),
2,576 at 10 (K = 7), 3,967 at 20 (K = 11); in one long session (units of 6) no K exists at 1 or 2 units, so the unit
tests cannot fire before the 20-turn look. Sessions of 8 at σ_s = 0.6 stay at about 3 %, above RF-11's 2.5 % (X3.md
keeps that case open, pinned at ≤ 4 %).

## M #2 per reading and D1: GBAL's card readings when the change does nothing

Per reading, independent turns. "Guard rollback" is an owner guardrail at tolerance 0 (the strictest); "`better`"
is the verdict an agent's prediction or `met` would use. The L3 column is the exact permutation test at Φ(−2) =
2.275 %; the Welch–t column is the candidate v0.5.1 planned (t at Satterthwaite's df, same level), measured on the same
readings.

| Card (unit) | Turns before × after | Guard rollback, main | Guard rollback, L3 | `better`, main | `better`, L3 | `better`, Welch–t |
|---|---|---|---|---|---|---|
| `cost_turn` (musd) | 5 × 5 | 2.62 % | 1.79 % ± 0.09 | 3.01 % | 2.04 % ± 0.10 | 0.88 % |
| `cost_turn` (musd) | 10 × 5 | 1.39 % | 2.25 % ± 0.10 | 4.83 % | 2.20 % ± 0.10 | 2.90 % |
| `cost_turn` (musd) | 40 × 5 | 0.73 % | 2.43 % ± 0.24 | 14.88 % | 1.77 % ± 0.21 | 12.45 % |
| `cost_turn` (musd) | 40 × 3 | 1.25 % | 2.17 % ± 0.23 | 20.40 % | 2.20 % ± 0.23 | 16.40 % |
| `wall_turn` (ms) | 5 × 5 | 2.77 % | 2.01 % ± 0.10 | 2.73 % | 1.97 % ± 0.10 | 0.81 % |
| `wall_turn` (ms) | 10 × 5 | 1.56 % | 2.25 % ± 0.10 | 5.22 % | 2.31 % ± 0.11 | 3.13 % |
| `wall_turn` (ms) | 40 × 5 | 0.70 % | 2.45 % ± 0.24 | 14.25 % | 2.00 % ± 0.22 | 12.53 % |
| `wall_turn` (ms) | 40 × 3 | 1.45 % | 2.35 % ± 0.24 | 20.95 % | 2.08 % ± 0.23 | 17.03 % |
| `requests_turn` (requests) | 5 × 5 | 4.28 % | 1.30 % ± 0.08 | 4.03 % | 1.31 % ± 0.08 | 2.12 % |
| `requests_turn` (requests) | 10 × 5 | 4.06 % | 1.57 % ± 0.09 | 4.17 % | 1.69 % ± 0.09 | 2.78 % |
| `requests_turn` (requests) | 40 × 5 | 5.22 % | 1.90 % ± 0.22 | 5.35 % | 1.82 % ± 0.21 | 3.52 % |
| `requests_turn` (requests) | 40 × 3 | 9.28 % | 1.35 % ± 0.18 | 9.18 % | 1.65 % ± 0.20 | 5.73 % |
| `cache_read` (permille) | 5 × 5 | 3.40 % | 1.94 % ± 0.10 | 3.58 % | 1.90 % ± 0.10 | 1.13 % |
| `cache_read` (permille) | 10 × 5 | 2.01 % | 1.99 % ± 0.10 | 5.48 % | 2.13 % ± 0.10 | 3.59 % |
| `cache_read` (permille) | 40 × 5 | 1.25 % | 1.98 % ± 0.22 | 12.55 % | 2.48 % ± 0.25 | 9.57 % |
| `cache_read` (permille) | 40 × 3 | 2.02 % | 2.08 % ± 0.23 | 15.07 % | 1.62 % ± 0.20 | 9.55 % |
| `provider_errors` (errors) | 5 × 5 | 0.35 % | 0.00 % ± 0.00 | 0.32 % | 0.00 % ± 0.00 | 0.01 % |
| `provider_errors` (errors) | 10 × 5 | 0.21 % | 0.19 % ± 0.03 | 0.38 % | 0.00 % ± 0.00 | 0.33 % |
| `provider_errors` (errors) | 40 × 5 | 0.33 % | 0.50 % ± 0.11 | 26.80 % | 0.00 % ± 0.00 | 26.72 % |
| `provider_errors` (errors) | 40 × 3 | 0.10 % | 0.88 % ± 0.15 | 31.97 % | 0.00 % ± 0.00 | 31.95 % |
| `corrections` (permille) | 5 × 5 | 0.62 % | 0.00 % ± 0.00 | 0.53 % | 0.00 % ± 0.00 | 0.03 % |
| `corrections` (permille) | 10 × 5 | 0.33 % | 0.33 % ± 0.04 | 0.68 % | 0.00 % ± 0.00 | 0.68 % |
| `corrections` (permille) | 40 × 5 | 0.55 % | 0.65 % ± 0.13 | 34.45 % | 0.00 % ± 0.00 | 34.45 % |
| `corrections` (permille) | 40 × 3 | 0.07 % | 0.62 % ± 0.12 | 41.95 % | 0.00 % ± 0.00 | 41.95 % |

**D1's choice: the exact permutation test.** It is at the stated level on every card and size (the guarantee is exact
under exchangeable turns; the sample rates are within Monte Carlo error of it), where main's z ranged 0.07–9.3 % for
rollbacks and 0.3–42 % for `better`. Welch–t fixes the small symmetric cases but not the ones that matter: with a
skewed card (cost, wall) and 40 turns before, `better` is 12–17 %, and on the discrete cards (corrections,
provider errors) 27–42 %, because 5 turns that are all 0 have a sample variance of 0, so se comes from the baseline
alone. Both methods assume exchangeable turns. Under X10's clustered model neither holds the level: L3's guard
rollbacks are 2–9 % per reading (main 0.1–9 %), `better` 0–7.5 % (main 0.6–42 %).

## M #3 and D2: false `met` per goal

The agent predicts `better` every round and stops `met` the first time the Root would verify it; the threshold is
always reached, so attribution is the only guard (the worst case). "stacked": each change goes live after the
previous one's 5 turns (its baseline is those 5; the first change's is 40 incumbent turns). "wait40": the agent waits
40 owner turns per change.

| Card | Baseline | Rounds | False `met` per goal, main | L3 | Per-reading `better`, main | L3 |
|---|---|---|---|---|---|---|
| `cost_turn` | stacked | 3 | 19.48 % | 4.42 % ± 0.33 | 7.26 % | 1.92 % |
| `cost_turn` | stacked | 5 | 23.82 % | 4.10 % ± 0.31 | 5.56 % | 2.11 % |
| `cost_turn` | stacked | 10 | 35.08 % | 4.65 % ± 0.33 | 4.49 % | 2.05 % |
| `cost_turn` | stacked | 20 | 51.45 % | 0.30 % ± 0.09 | 3.81 % | 2.02 % |
| `cost_turn` | wait40 | 10 | 80.50 % | 4.60 % ± 0.66 | 15.09 % | 2.34 % |
| `requests_turn` | stacked | 3 | 14.03 % | 3.40 % ± 0.29 | 4.94 % | 1.53 % |
| `requests_turn` | stacked | 5 | 20.08 % | 2.23 % ± 0.23 | 4.39 % | 1.30 % |
| `requests_turn` | stacked | 10 | 37.38 % | 2.17 % ± 0.23 | 4.59 % | 1.38 % |
| `requests_turn` | stacked | 20 | 60.12 % | 0.10 % ± 0.05 | 4.48 % | 1.34 % |
| `requests_turn` | wait40 | 10 | 43.80 % | 4.20 % ± 0.63 | 5.55 % | 2.03 % |
| `corrections` | stacked | 3 | 35.23 % | 0.00 % ± 0.00 | 15.27 % | 0.00 % |
| `corrections` | stacked | 5 | 35.93 % | 0.00 % ± 0.00 | 9.96 % | 0.00 % |
| `corrections` | stacked | 10 | 36.75 % | 0.00 % ± 0.00 | 5.35 % | 0.00 % |
| `corrections` | stacked | 20 | 42.10 % | 0.00 % ± 0.00 | 3.32 % | 0.00 % |
| `corrections` | wait40 | 10 | 98.60 % | 0.00 % ± 0.00 | 35.67 % | 0.00 % |
| `cache_read` | stacked | 3 | 18.57 % | 5.05 % ± 0.35 | 6.82 % | 2.12 % |
| `cache_read` | stacked | 5 | 24.98 % | 4.20 % ± 0.32 | 5.81 % | 1.96 % |
| `cache_read` | stacked | 10 | 36.70 % | 3.92 % ± 0.31 | 4.64 % | 1.90 % |
| `cache_read` | stacked | 20 | 56.27 % | 0.30 % ± 0.09 | 4.25 % | 1.91 % |
| `cache_read` | wait40 | 10 | 72.40 % | 4.40 % ± 0.65 | 12.08 % | 2.29 % |

With rounds = 20 and stacked changes, L3 attributes almost nothing: p ≤ 5 %/20 = 2,500 ppm is below the smallest
p of 5 against 5 (1/252 = 3,969 ppm), so only the first change (5 against 40) can be attributed. Under clustering, L3's
false `met` is 12–24 % on the skewed and count cards (main 12–98 %).

## Power: a guardrail against a real regression

The other side of the per-reading table: how often an owner guardrail (tolerance 0) rolls back a change that really
harms its card, independent turns, the same code paths.

| Card | Harm | Turns before × after | Caught, main | Caught, L3 |
|---|---|---|---|---|
| `cost_turn` | cost ×2 | 5 × 5 | 14.95 % | 13.38 % ± 0.54 |
| `cost_turn` | cost ×2 | 40 × 5 | 10.00 % | 22.85 % ± 0.94 |
| `wall_turn` | wall ×2 | 5 × 5 | 14.03 % | 12.95 % ± 0.53 |
| `wall_turn` | wall ×2 | 40 × 5 | 10.45 % | 25.00 % ± 0.97 |
| `requests_turn` | requests ×2 | 5 × 5 | 46.17 % | 26.38 % ± 0.70 |
| `requests_turn` | requests ×2 | 40 × 5 | 54.65 % | 76.05 % ± 0.95 |
| `cache_read` | cache reads ÷2 | 5 × 5 | 50.82 % | 44.70 % ± 0.79 |
| `cache_read` | cache reads ÷2 | 40 × 5 | 99.00 % | 55.10 % ± 1.11 |
| `provider_errors` | errors 2 → 10 % | 5 × 5 | 17.25 % | 0.47 % ± 0.11 |
| `provider_errors` | errors 2 → 10 % | 40 × 5 | 15.60 % | 24.30 % ± 0.96 |
| `corrections` | corrections 10 → 50 % | 5 × 5 | 34.90 % | 2.02 % ± 0.22 |
| `corrections` | corrections 10 → 50 % | 40 × 5 | 38.80 % | 39.20 % ± 1.09 |

**L3 lets some harmful changes through more often, and this is the cost of the exact level.** With 40 turns before,
L3 catches more on cost, wall time, requests and provider errors (main's z was conservative there), and fewer on
cache reads (99 → 55 %). With 5 turns before (stacked changes), it catches fewer on every card, and the discrete cards
nearly go blind: provider errors 17 → 0.5 %, corrections 35 → 2 %. With 10 binary turns and 3 events, no test whose
false rate is at most 2.275 % given the pooled turns can fire (all 3 among the 5 turns after: p = 21/252 = 8.3 %); main's
z fired there because it took the 5 clean baseline turns as zero noise. The sentinel's H7 (corrections) and H9
(provider errors) still watch these harms against a 40-turn and a 400-request baseline, independently of goals.
Options for the owner, not built: guardrails on discrete cards read 10 or more turns by default; a studentized
permutation statistic (Welch's t as the statistic; still exact) to recover z's power on the bounded cards.

## Discrete-card guardrails: window length, rule and statistic

The cards whose guardrails lost the most power at short windows: `requests_turn` (a count), `cache_read`,
`provider_errors` and `corrections`. Each cell holds three rules on the same readings, in percent, as **exact · z ·
studentized**:
- **exact**: L3 as built (the Root's `reading` and `fails`), here reading 5, 10, 15 or 20 turns after the change, the
  option (a) for a longer default guardrail read;
- **z**: (b), v0.5.2's z ≥ 2 kept for the harm side only (`worse` when the same reading's `z_permille` ≤ −2000),
  the exact test still deciding `better` and `met`;
- **studentized**: (c), the permutation test with Welch's t as its statistic (still exact under exchangeable turns;
  9,999 Monte Carlo relabelings; a candidate in goal/sim.rs, not in the Root).

"Before" is the parent's turns: as many as after when changes stack, or 40 when the agent waits. The rise is k× the
harm: requests ×k, cache reads ÷k, provider errors at k × 2 % of requests, corrections on k × 10 % of turns. Guardrail
tolerance 0, independent turns, 4,000 null readings and 1,000 per rise per cell (± about 0.2 points at the null's
2 %, up to 1.6 points on detection). Commit 57bd941; `cargo test --offline -p init-root --bin init-root --config
'profile.dev.package.init-root.opt-level=3' goal::sim::gbal_discrete_guard_table -- --ignored --nocapture
--test-threads 1` (this crate optimized, since most cells take the Monte Carlo path; the null tables came out bit-identical from a
debug and an optimized build).

| Card | Turns before × after | Null (false rollback) | 2× | 3× | 5× |
|---|---|---|---|---|---|
| `requests_turn` | 5 × 5 | 1.4 · 4.2 · 1.4 | 24.8 · 48.2 · 25.3 | 55.7 · 76.7 · 57.1 | 86.3 · 93.5 · 86.7 |
| `requests_turn` | 10 × 10 | 2.1 · 3.9 · 2.2 | 70.3 · 76.2 · 70.0 | 97.0 · 98.3 · 97.1 | 99.9 · 99.9 · 99.9 |
| `requests_turn` | 15 × 15 | 2.0 · 3.1 · 2.1 | 88.0 · 91.6 · 88.3 | 99.9 · 99.9 · 99.9 | 100.0 · 100.0 · 100.0 |
| `requests_turn` | 20 × 20 | 1.9 · 2.5 · 1.8 | 97.0 · 98.0 · 97.2 | 100.0 · 100.0 · 100.0 | 100.0 · 100.0 · 100.0 |
| `requests_turn` | 40 × 5 | 2.0 · 5.6 · 2.2 | 75.7 · 52.6 · 48.2 | 96.9 · 80.7 · 87.1 | 99.9 · 93.7 · 99.1 |
| `requests_turn` | 40 × 10 | 2.0 · 3.2 · 2.4 | 93.2 · 84.4 · 86.5 | 99.8 · 98.7 · 99.5 | 100.0 · 100.0 · 100.0 |
| `requests_turn` | 40 × 15 | 1.9 · 3.0 · 2.4 | 98.1 · 95.6 · 96.2 | 100.0 · 100.0 · 100.0 | 100.0 · 100.0 · 100.0 |
| `requests_turn` | 40 × 20 | 1.9 · 2.6 · 2.1 | 99.4 · 98.7 · 98.8 | 100.0 · 100.0 · 100.0 | 100.0 · 100.0 · 100.0 |
| `cache_read` | 5 × 5 | 2.1 · 3.8 · 2.2 | 46.5 · 52.3 · 47.1 | 44.3 · 77.1 · 49.4 | 45.8 · 85.2 · 54.6 |
| `cache_read` | 10 × 10 | 1.8 · 2.5 · 1.8 | 72.6 · 76.5 · 72.9 | 93.3 · 95.1 · 93.0 | 99.1 · 99.2 · 99.0 |
| `cache_read` | 15 × 15 | 2.2 · 2.5 · 2.2 | 90.6 · 92.8 · 90.8 | 99.0 · 99.1 · 98.9 | 100.0 · 100.0 · 100.0 |
| `cache_read` | 20 × 20 | 2.1 · 2.2 · 2.0 | 97.4 · 97.7 · 97.3 | 99.9 · 100.0 · 99.9 | 100.0 · 100.0 · 100.0 |
| `cache_read` | 40 × 5 | 2.2 · 1.5 · 2.4 | 55.2 · 98.7 · 97.9 | 93.0 · 100.0 · 100.0 | 99.6 · 100.0 · 100.0 |
| `cache_read` | 40 × 10 | 2.1 · 1.3 · 2.3 | 92.5 · 100.0 · 100.0 | 100.0 · 100.0 · 100.0 | 100.0 · 100.0 · 100.0 |
| `cache_read` | 40 × 15 | 2.1 · 1.5 · 2.1 | 97.6 · 99.8 · 99.7 | 100.0 · 100.0 · 100.0 | 100.0 · 100.0 · 100.0 |
| `cache_read` | 40 × 20 | 2.2 · 1.5 · 2.1 | 99.6 · 100.0 · 100.0 | 100.0 · 100.0 · 100.0 | 100.0 · 100.0 · 100.0 |
| `provider_errors` | 5 × 5 | 0.0 · 0.4 · 0.0 | 0.0 · 2.4 · 0.0 | 0.0 · 4.6 · 0.5 | 0.4 · 16.5 · 1.3 |
| `provider_errors` | 10 × 10 | 0.0 · 0.4 · 0.0 | 0.5 · 3.1 · 0.5 | 2.9 · 8.3 · 2.9 | 14.1 · 31.6 · 14.1 |
| `provider_errors` | 15 × 15 | 0.2 · 0.7 · 0.2 | 2.4 · 5.3 · 2.1 | 7.3 · 15.9 · 7.0 | 29.9 · 45.1 · 29.2 |
| `provider_errors` | 20 × 20 | 0.2 · 1.7 · 0.4 | 2.3 · 11.7 · 2.8 | 13.5 · 28.1 · 14.0 | 44.2 · 61.6 · 44.8 |
| `provider_errors` | 40 × 5 | 0.5 · 0.4 · 0.5 | 3.5 · 2.8 · 3.7 | 8.0 · 5.0 · 7.8 | 23.8 · 13.7 · 24.1 |
| `provider_errors` | 40 × 10 | 0.9 · 0.3 · 0.9 | 5.4 · 3.2 · 4.7 | 13.9 · 8.1 · 14.0 | 42.0 · 30.1 · 43.2 |
| `provider_errors` | 40 × 15 | 0.7 · 0.4 · 0.7 | 6.4 · 4.7 · 6.6 | 23.1 · 17.6 · 23.9 | 57.6 · 48.5 · 58.4 |
| `provider_errors` | 40 × 20 | 0.4 · 0.5 · 0.4 | 7.1 · 7.1 · 7.2 | 22.7 · 22.8 · 23.4 | 65.7 · 66.9 · 67.9 |
| `corrections` | 5 × 5 | 0.0 · 0.4 · 0.0 | 0.0 · 3.3 · 0.2 | 0.2 · 11.4 · 0.4 | 1.8 · 34.3 · 4.5 |
| `corrections` | 10 × 10 | 0.0 · 0.7 · 0.0 | 1.3 · 5.3 · 1.3 | 4.8 · 18.1 · 4.8 | 31.3 · 57.8 · 31.3 |
| `corrections` | 15 × 15 | 0.4 · 1.3 · 0.4 | 5.0 · 10.6 · 4.8 | 14.8 · 28.1 · 14.0 | 53.5 · 72.6 · 53.6 |
| `corrections` | 20 × 20 | 0.1 · 2.1 · 0.2 | 4.3 · 13.0 · 4.1 | 22.1 · 35.4 · 22.0 | 72.2 · 83.7 · 71.6 |
| `corrections` | 40 × 5 | 0.8 · 0.7 · 0.8 | 3.3 · 3.3 · 3.0 | 11.7 · 11.1 · 11.3 | 40.7 · 40.0 · 39.1 |
| `corrections` | 40 × 10 | 1.3 · 0.5 · 1.1 | 7.2 · 4.4 · 6.3 | 22.5 · 17.2 · 20.5 | 67.7 · 61.2 · 64.8 |
| `corrections` | 40 × 15 | 0.7 · 0.6 · 0.7 | 7.7 · 7.2 · 7.7 | 28.2 · 28.2 · 28.2 | 76.3 · 76.6 · 76.2 |
| `corrections` | 40 × 20 | 0.7 · 0.7 · 0.7 | 8.8 · 9.8 · 9.1 | 32.7 · 37.3 · 33.1 | 86.4 · 89.4 · 86.7 |

**What a longer read costs.** The guardrail decides after n turns of the change, and a stacked change waits for it:
10, 15 or 20 turns are 5, 10 or 15 more owner turns per change than today's 5. At X10's 4 owner turns an hour that is
1.25, 2.5 or 3.75 hours of the owner's work per change, and 50, 100 or 150 more owner turns for a goal of 10 stacked
rounds. A harmful change also stays live that much longer before a guardrail can roll it back.

**Reading the table.**
- (b) keeps v0.5.2's detection, and with it v0.5.2's false rollbacks: 4.2 % on requests at 5 × 5, 5.6 % at 40 × 5,
  3.8 % on cache reads at 5 × 5, over the 2.3 % gate.
- (c) holds the level everywhere (at most 2.4 %, within Monte Carlo error) and recovers z's detection on cache reads
  against 40 turns (2×: 97.9 %, against 55.2 % exact and 98.7 % z). It adds little on the binary cards at 5 × 5
  (corrections 5×: 4.5 % against 1.8 %) and loses on requests against 40 turns (2×: 48 % against 76 %).
- (a) with the exact test at 15 turns holds the level (at most 2.2 %) and catches at least what v0.5.2's 5-turn z
  caught on every discrete card and rise: requests 2× 88 % against 48 %, cache reads 2× 91 % against 52 %, provider
  errors 5× 30 % against 17 %, corrections 5× 54 % against 34 % (against v0.5.2 with 40 turns before: 1.1 points below
  it on cache reads at 2×, at or above it elsewhere).
- No rule at 5 turns catches a fivefold rise of a 2 % or 10 % rate reliably: 10 binary turns hold too few events.
  The sentinel's H7 and H9 watch those rates against long baselines whatever the goal does.

**Recommendation (one line):** option (a), the exact test reading 15 turns on the four discrete cards' guardrails,
the only option measured here that holds the 2.3 % level and catches at least v0.5.2's share on every discrete card,
at 10 more owner turns (about 2.5 h) per change.

**Built (owner decision, 2026-10-29): option (a).** Re-run at the built commit 6ef9cba with the same command: the
cells are bit-identical to 57bd941's. The built rule against v0.5.2's, per card and rise (the guardrail's rollback
rate, %; "stacked": the parent lived as many turns as the reading; "waited": 40 turns before):

| Card | Rise | Built, stacked (15 × 15) | v0.5.2, stacked (5 × 5, z) | Built, waited (40 × 15) | v0.5.2, waited (40 × 5, z) |
|---|---|---|---|---|---|
| requests | null | 2.0 | 4.2 | 1.9 | 5.6 |
| requests | 2× · 3× · 5× | 88.0 · 99.9 · 100 | 48.2 · 76.7 · 93.5 | 98.1 · 100 · 100 | 52.6 · 80.7 · 93.7 |
| cache reads | null | 2.2 | 3.8 | 2.1 | 1.5 |
| cache reads | 2× · 3× · 5× | 90.6 · 99.0 · 100 | 52.3 · 77.1 · 85.2 | 97.6 · 100 · 100 | 98.7 · 100 · 100 |
| provider errors | null | 0.2 | 0.4 | 0.7 | 0.4 |
| provider errors | 2× · 3× · 5× | 2.4 · 7.3 · 29.9 | 2.4 · 4.6 · 16.5 | 6.4 · 23.1 · 57.6 | 2.8 · 5.0 · 13.7 |
| corrections | null | 0.4 | 0.4 | 0.7 | 0.7 |
| corrections | 2× · 3× · 5× | 5.0 · 14.8 · 53.5 | 3.3 · 11.4 · 34.3 | 7.7 · 28.2 · 76.3 | 3.3 · 11.1 · 40.0 |

Null at most 2.2 %; detection at or above v0.5.2's in every cell but one: cache reads at 2× with 40 turns before,
97.6 % against 98.7 % (1.1 points, about 1.8 Monte Carlo standard errors of the difference).

## What a level certifies (the owner's level semantics A, built)

A climb to L1–L4 needs, at 95 % confidence and valid under the ledger's continuous re-checking, that the rate of
exercised change groups ending in a direct harm rollback is below X = 25, 20, 10 or 5 % (ledger.rs
`harm_certified`; ADR-011 §7.2; foundations.md P-6). An exercised group closed with at least 5 owner turns in its
window and was not rolled back, or ended in a direct harm rollback; a rollback attributed `suspect`, `collateral` or
nothing is neither (v0.5.3 review: the simulations below draw exercised groups directly, so they are
unchanged). The test is the e-process E_n = (1/10) Σ_{j<10} (j/10)^h ((1 − jX/10)/(1 −
X))^(n−h) ≥ 20 after n exercised groups with h harms, compared exactly in big integers. Exercised groups each level
needs (one fewer is not enough; `ledger::harm_tests::the_certified_boundaries_per_level` pins every cell of the first
column), against two other tests at 5 %:

| Level | X | Built: anytime-valid e-process, 0 / 1 / 2 harms | One look only (exact binomial) | Union bound over the checks (5 %/(t(t+1)) after group t) |
|---|---|---|---|---|
| L1 | 25 % | 15 / 25 / 32 | 11 / 18 / 23 | 36 / 48 / 57 |
| L2 | 20 % | 19 / 32 / 41 | 14 / 22 / 30 | 49 / 64 / 77 |
| L3 | 10 % | 41 / 66 / 85 | 29 / 46 / 61 | 120 / 152 / 178 |
| L4 | 5 % | 83 / 135 / 174 | 59 / 93 / 124 | 278 / 344 / 399 |

The one-look test would be wrong here: the ledger re-checks after every record, and a record that is re-checked
until it passes would pass a one-look test far more often than 5 %. The union bound is valid but needs about three
times the groups; the e-process needs about 40 % more than one look. The rule it replaces, "≤ 6 / 4 / 3 direct harms
in the last 30 closed groups", certified only that the rate was below 35.7 / 28.0 / 23.9 % (exact one-sided 95 %
bounds, one look).

Checked after every exercised group, 600 groups a run, at a true harm rate equal to X (the boundary of what the level
rules out) and at X/2 (`ledger::harm_tests::harm_bound_null_table`, commit 6ef9cba; `cargo test --offline -p init-root
--bin init-root --config 'profile.dev.package.init-root.opt-level=3' -- ledger::harm_tests::harm_bound_null_table
--ignored --nocapture --test-threads 1`; seeded; the unit suite runs L1 at 300 groups):

| Level | X | Runs that ever climb at rate X (10,000 runs) | at X/2 (2,000 runs) |
|---|---|---|---|
| L1 | 25 % | 3.81 % | 100.0 % |
| L2 | 20 % | 4.30 % | 100.0 % |
| L3 | 10 % | 3.93 % | 97.4 % |
| L4 | 5 % | 3.07 % | 75.0 % |

All four are within the 5 % that Ville's inequality guarantees for any run length (± 0.2 points of Monte Carlo
error).

## M #4 (G-21): betting lower bound vs the percentile bootstrap

G-21's design (⌈n/4⌉ flaky tasks at LKG pass 0.5, the rest at 1.0; candidate = LKG + effect), α = 0.05 one-sided,
both bounds on the same 1,000 gates per cell. The betting bound (kernel-gate stats/betting.rs; Waudby-Smith & Ramdas
2024) is anytime-valid with predictable plug-in bets tuned for the cell's n.

| n | R | True effect | Bootstrap width | Betting width | Ratio | Coverage, bootstrap | Coverage, betting | NI accept, bootstrap | NI accept, betting |
|---|---|---|---|---|---|---|---|---|---|
| 16 | 2 | 0 | 0.092 | 0.305 | 3.33 | 94.4 % | 100.0 % | 81.4 % | 4.8 % |
| 16 | 2 | −2δ (-0.206) | 0.136 | 0.267 | 1.96 | 90.7 % | 100.0 % | 9.3 % | 0.0 % |
| 24 | 2 | 0 | 0.078 | 0.223 | 2.88 | 94.2 % | 100.0 % | 88.5 % | 14.3 % |
| 24 | 2 | −2δ (-0.168) | 0.110 | 0.205 | 1.87 | 90.3 % | 100.0 % | 9.0 % | 0.0 % |
| 32 | 2 | 0 | 0.070 | 0.178 | 2.54 | 94.2 % | 100.0 % | 87.9 % | 21.7 % |
| 32 | 2 | −2δ (-0.145) | 0.092 | 0.167 | 1.81 | 92.1 % | 100.0 % | 7.3 % | 0.1 % |
| 40 | 2 | 0 | 0.063 | 0.149 | 2.38 | 95.0 % | 100.0 % | 91.0 % | 33.6 % |
| 40 | 2 | −2δ (-0.130) | 0.083 | 0.142 | 1.72 | 93.7 % | 99.7 % | 5.8 % | 0.2 % |
| 16 | 3 | 0 | 0.076 | 0.301 | 3.97 | 94.4 % | 100.0 % | 90.2 % | 0.0 % |
| 16 | 3 | −2δ (-0.168) | 0.107 | 0.263 | 2.46 | 89.4 % | 100.0 % | 7.1 % | 0.0 % |
| 24 | 3 | 0 | 0.065 | 0.220 | 3.38 | 93.8 % | 100.0 % | 90.4 % | 1.0 % |
| 24 | 3 | −2δ (-0.137) | 0.086 | 0.199 | 2.31 | 93.6 % | 100.0 % | 8.4 % | 0.0 % |
| 32 | 3 | 0 | 0.056 | 0.173 | 3.08 | 94.4 % | 100.0 % | 89.8 % | 3.7 % |
| 32 | 3 | −2δ (-0.119) | 0.074 | 0.161 | 2.19 | 93.6 % | 100.0 % | 6.5 % | 0.0 % |
| 40 | 3 | 0 | 0.050 | 0.144 | 2.87 | 94.8 % | 100.0 % | 93.7 % | 11.0 % |
| 40 | 3 | −2δ (-0.106) | 0.065 | 0.136 | 2.11 | 92.4 % | 100.0 % | 7.9 % | 0.0 % |

The betting bound is 1.7–4.0× as wide. It never under-covers (99.7–100 %, against the bootstrap's 89.4–95.0 %: G-21's
known small-n gap) and its false accepts at −2δ are 0–0.2 % (bootstrap 5.8–9.3 %), but at a true effect of 0 it
accepts non-inferiority in only 0–34 % of gates (bootstrap 81–94 %). On [0, 1]-bounded data with most tasks tied, the
bet cannot exceed 1/m, so it cannot exploit the small variance: gate.md §3.8.3's "noticeably wider at n ≈ 16–40"
holds, measured.

## The L3 gate

| Gate (PLAN-052) | Main | L3 | Verdict |
|---|---|---|---|
| null false `met` ≤ 5 % per goal at rounds = 10 | 35–37 % stacked, 44–99 % waiting 40 | 0–4.7 % (± 0.3–0.7) | met (iid turns) |
| false rollback ≤ 2.3 % per reading | up to 9.3 % (requests, 40 × 3) | ≤ 2.45 % ± 0.24, every cell within MC error of the exact 2.275 % | met (iid turns) |
| `gbal-bundle-mock` keeps its verdicts on real effects | — | `goal::l3_tests::gbal_bundle_mock_verdicts_on_the_same_readings`: P-000001 `better`, registered, o = 1, Brier 0.09; P-000002 `worse`, o = 1, rolled back by `cost_turn`, Brier 0.065; stop `met`, verified, attributed P-000001 (also passes on main) | met (reproduced in a Root test, not the bundle) |

Both statistical gates hold under exchangeable turns, which the exact test assumes; under X10's clustered sessions
they do not (above). The bundle flow itself needs a release bundle and a test HOME and runs at release; the Root test
replays its readings (5 against 5, deterministic) through the same goal code.

## Assumptions and limits

- Above 2²¹ relabelings (for example 10 turns against 40) the verdict uses a Monte Carlo p-value with 99,999 seeded
  relabelings. It is valid at any B, but near the level it acts like a randomized test: its standard error there is
  about 0.047 points (√(0.0228 · 0.977 / 99,999)), so a reading whose exact p lies within about 0.1 points of 2.275 %
  can fall either way. The independent R1 review saw an exact p of 2.283 % at 40 × 10 rolled back about 43 % of the
  time across seeds, as the arithmetic predicts (z ≈ −0.17). The seed comes from the pooled values, so one reading
  always gets the same verdict.
- A gaming vector, open: the agent chooses when a change goes live, so it chooses the window of turns its reading
  takes as the baseline (the parent's newest turns). Going live after a run of expensive turns flatters a `better`
  reading and dulls a guardrail. Not fixed in v0.5.3; a baseline the agent cannot place (the goal's fixed reference,
  or randomized timing) is the remedy to study.

- A_exch at the turn level for GBAL readings (foundations.md §6): the cards read turns, not the sentinel's units.
  Sessions break it; the fix is a session-block permutation, which needs the turn's session in `Obs` (not done).
- The turn model is synthetic. The cache-read distribution is a stand-in.
- `met`'s 5 % holds for any stopping rule and any dependence between rounds, by the union bound; it is conservative
  when rounds are many (rounds = 20: 0.1–0.3 %).
- The corrections and provider-error cards can no longer read `better` at 5 turns: with a 10 % base rate, 5 clean
  turns are not evidence at 2.3 % (C(41,5)/C(45,5) = 61 % under the null). They need more turns.

## Tests and mutation checks (release-protocol §4.1)

Fail on main's behavior (525fc26 with only the test files added), pass at the L3 head: `goal::l3_tests::`
`d1_verdicts_come_from_the_exact_permutation_test` (main read 5 clean turns against 4 corrections in 40 as `better`),
`d2_met_spends_alpha_over_the_rounds` (main verified `met` at rounds = 10 on p = 4/252),
`d3_freedom_needs_skill_against_the_base_rate` (main gave 6 rounds to the `same`-at-0.9 forecaster),
`d3_every_live_change_in_a_card_goal_is_forecast` (main scored nothing), and `goal::sim::l3_gate_null_rates` (the gate
at reduced size). `goal::l3_tests::gbal_bundle_mock_verdicts_on_the_same_readings` passes on both. Each new guard was
broken and its test seen to fail: the noise level, the tail mapping, both branch-and-bound counts, the `met` level and
its rounds, the skill condition, the unforecast hook and the abstention forecast, a new level-gated line (caught after
the matcher learnt `lvl`), a removed cap, and a stricter `met` level against the mock flow's verdicts: 12 of 12.

After the review: `goal::l3_tests::every_live_change_in_a_goal_spends_a_round` (an unpredicted change spent no round)
and `goal::l3_tests::readings_on_a_change_the_sentinel_rolled_back_end_and_never_count_for_met` (the reading stayed
pending, and `met` counted the undone change) fail on the code before them (goal.rs of 5d032f2) and pass at a4bd23f.
Mutations caught 4 of 4: no round charged, a paused goal's changes charged, no terminal close, undone changes counted
for `met` (the first form of that last mutation was a no-op and was redone).
