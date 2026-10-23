# ROOT-S1b: G4 v2 statistics (pure module)

Lane of PROOF-LEAN (build/specs/PROOF-LEAN.md). Status: **done** (API stable since 82032f9; ROOT-S1a calls it
from `sealed/slice.rs`).

## Public API (for ROOT-S1a)

File `root/crates/init-root/src/sealed/exact.rs`, module `sealed::exact` (declared in `sealed/mod.rs` by this
lane in 82032f9). Use it as `use super::exact::{...}` from `sealed/*.rs`. Pure: no I/O, no clock, no
urandom. Seeded parts use `stats::{Rng, seed}` like gate.md §3.8.3. Tests: `sealed/exact/tests.rs`
(`#[cfg(test)]`, outside root.md §4.8's count).

Units: `k_i = Σ cand scores − Σ inc scores` over R runs per arm, in a per-run `unit` (1 = passes,
1000 = permille). One task-equivalent = `repeats·unit`; the Ladder margin D ≥ 1/n is the integer test
`Σk ≥ repeats·unit`.

```rust
// constants (m7 §4.3), permille so every comparison is exact
pub const ALPHA_GAIN_PERMILLE: u32 = 50;  ALPHA_RATCHET_PERMILLE: u32 = 250;  ALPHA_MILESTONE_PERMILLE: u32 = 50;
pub const RHO_PASS_PERMILLE: u64 = 1500;  RHO_RATCHET_PERMILLE: u64 = 1200;
pub const MAX_N: usize = 100;             // discordant tasks; beyond it p = 1 (never claims)
pub const SCREEN_K: u32 = 12; CANARY_RUNS: u32 = 7; SENTINEL_PERMILLE: u64 = 200; CALIBRATE_B: usize = 2000;

pub struct PValue { pub num: u128, pub log2_den: u32 }   // exact num / 2^log2_den, lowest terms
impl PValue { const ONE; fn le_permille(self, alpha: u32) -> bool; fn f64(self) -> f64; fn permille_ceil(self) -> u32 }

pub fn p_sup(k: &[i64]) -> PValue;   // P(Σ ε_i|k_i| ≥ Σk), exact DP (subset-sum counts, u128)
pub fn p_inf(k: &[i64]) -> PValue;   // P(Σ ε_i|k_i| ≤ Σk) = p_sup(−k)
pub fn diffs(cand: &[Vec<i64>], inc: &[Vec<i64>]) -> Result<(Vec<i64>, u32 /*R*/), String>; // permille, equal R
pub fn d_ratios(k: &[i64], repeats: u32, unit: i64) -> Vec<f64>;   // input of stats::lower
pub fn margin(delta: f64, n: usize) -> f64;                         // max(2δ, 0.5/n)
pub fn round_to(x: i64, step: i64) -> i64;                          // agent-visible rounding (d 50, m 10, cpp 50)

pub enum Status { Superior, Inferior, Underpowered, Tie, TieNegative }   // .as_str()
pub struct Decision { status, p_sup: PValue, ratchet_signal: bool, n, total: i64, d: f64,
                      d_permille: i64, m_permille: i64, l_permille: i64 }  // unrounded
pub fn slice_status(k: &[i64], repeats: u32, unit: i64, l_ni: f64, margin: f64, margin_max: f64) -> Decision;
pub fn futility(k_look: &[i64], repeats: u32, unit: i64, delta_hist: Option<f64>, gate_id: &str) -> bool;

pub enum Outcome { Confirmed, NotConfirmed, Regression, Partial }       // .as_str(); Partial = driver-set
pub struct Milestone { outcome, p_sup, p_inf, n, total: i64, d_permille: i64 }
pub fn milestone(k: &[i64], repeats: u32, unit: i64, l_ni: f64, margin: f64, margin_max: f64) -> Milestone;
pub fn ratchet_k_next(outcome: Outcome, k: u32, charter_k: u32) -> u32;

pub fn cpp_ratio_permille(c_cost_musd: u64, c_passes: u64, p_cost_musd: u64, p_passes: u64) -> u64;
pub fn sentinel_drift(observed_passes: u64, cached_rates_permille: &[u32]) -> bool;
pub fn in_band(passes: u32, runs: u32) -> bool;     // 0 < passes < runs
pub fn knob_step(rate_permille: u32) -> i8;         // −1 below 200‰, +1 above 800‰

pub struct Tier { pub name: &'static str, pub n: usize, pub repeats: u32 }
pub const LEAN, STANDARD, STRONG: Tier; pub const TIERS: [Tier; 3]; pub fn tier(name: &str) -> Option<Tier>;
impl Tier { fn runs(self, reuse: bool) -> u32; fn cost_musd(self, reuse: bool, per_run_musd: u64) -> u64;
            fn mde_permille(self, var_within_permille: u32) -> u32 }
pub fn expected_false_accepts_milli(looks: u32, fa_permille: u32) -> u32;

pub enum CalibVerdict { Pass, Fail, StopSpend }
pub struct Calib { n_kept, splits, fa_permille, fa_lo_permille, fa_hi_permille, fr_permille, fr_lo_permille,
                   fr_hi_permille, delta_r2: f64, delta_r3: f64, delta_r5: f64, var_between_permille,
                   var_within_permille, mde_lean_permille, mde_standard_permille, mde_strong_permille,
                   m_standard_permille, verdict }
impl Calib { fn json(&self) -> serde_json::Value }   // stats part of `sealed_calibrate`
pub fn calibrate(runs: &[Vec<i64>] /*R ≥ 6, permille*/, seed_id: &str, margin_max: f64) -> Result<Calib, String>;
pub fn calibrate_b(runs: &[Vec<i64>], seed_id: &str, margin_max: f64, b: usize) -> Result<Calib, String>;
pub fn clopper_pearson_permille(x: u64, n: u64) -> (u32, u32);
```

Slice recipe: `let (k, r) = diffs(&cand, &inc)?; let m = margin(δ_S, n); let l = stats::lower(&d_ratios(&k,
r, 1000), id, "ni"); let dec = slice_status(&k, r, 1000, l, m, margin_max);` then `round_to(dec.d_permille,
50)`, `round_to(dec.m_permille, 10)`, `round_to(cpp, 50)` for the agent-visible record.

## Done
- 82032f9: the module with real bodies (exact sign-flip DP with exact dyadic p-values; superiority, ratchet
  signal, status order; milestone outcome and `ratchet_k_next`; futility look; cpp ratio; sentinel; band and
  knob; tier runs/cost/analytic MDE; expected false accepts; A/A calibration: pseudo-A/A FA/FR with
  Clopper–Pearson, δ_S r2/r3/r5, variance components, verdict, record JSON).
- Tests (`sealed/exact/tests.rs`) and a small refactor (`aa_split`, shared by `calibrate` and the R14 test).
- The design check: m7 §3.2.6 and §3.2.13's tables recomputed **exactly** (no simulation) by a test-only
  engine: the per-task k distribution of §3.2.6's task model integrated numerically, then a sum over the
  multisets of |k| (multinomial weights) of the alternative's mass on the exact test's rejection region.

## Tests
`source ci/env.sh; cd root; bash ../ci/slot.sh cargo test --offline -p init-root --bin init-root sealed::exact`
(run against a HEAD export + these files with the shared `CARGO_TARGET_DIR=root/target`, because ROOT-S1a's
uncommitted edits kept the shared tree from compiling at first; later runs are in the shared tree, with
ROOT-S1a's `slice.rs` calling the API): **14 passed, 2 ignored, 11.9 s**. The 2 ignored (slow in a debug build)
were run once with `--include-ignored`: both green (the strong row again after the MDE change). Full root suite
(`cargo test --offline` in root/, with ROOT-S1a's WIP in the tree): all green, init-root bin 99 passed.

| Test | Covers |
|---|---|
| `m7_r03_dp_equals_brute_force` | M7-R03: p_sup and p_inf equal 2^n enumeration on 6,000+ random vectors, n = 0…14, R ∈ {1,2,3,5}, three shapes incl. permille partial credit |
| `edges_ties_n1_and_max_n` | all ties, empty, n = 1, the 5-flip floor (2⁻⁵ passes, 2⁻⁴ only the ratchet), MAX_N = 100 (2⁻¹⁰⁰ exact), MAX_N + 1, n = 100 partial credit up to ±5,000 |
| `exact_test_invariances` | p_inf(−k) = p_sup(k), common factor, order, zeros; the margin binds in permille; PValue arithmetic |
| `deterministic` | slice, futility and calibrate replay bit for bit |
| `m7_r04_status_branches` | M7-R04 (stats part): one case per status branch; `diffs` → bootstrap → status end to end; futility |
| `m7_r17_milestone_outcomes` | M7-R17 (stats part): confirmed, regression by p_inf, by NI (only with m ≤ margin_max), not_confirmed; ratchet_k reset/halve |
| `m7_r13_rounding_cpp_sentinel_band` | M7-R13 rounding; cpp ratio; M7-R09's sentinel rule incl. the floor; band; knob |
| `clopper_pearson_reference_values` | textbook CP intervals |
| `tiers_runs_cost_and_expected_false_accepts` | §3.2.6's runs (127/72, 223/122, 351/186) and $ at $0.003/run; §3.1.7's 0.72 |
| `m7_r14_calibration_on_synthetic_data` | M7-R14 (stats part): pseudo-A/A FA over 2,000 splits within ±1 point of the exact conditional FA; the full path agrees; variance components; δ near analytic |
| `design_screen_error_rates`, `design_tier_table_false_accepts_and_power`, `design_ratchet_operating_characteristic`, `design_milestone_power` | the m7 tables, exactly; assertions below |
| `design_tie_guard_and_aa_fr_simulated` (`#[ignore]`, slow in debug) | §3.2.6's last column and the pseudo-A/A FR, seeded simulation with the Root's own noise and bootstrap |

## The design's numbers, recomputed (exact unless marked)
Exact = this module's engine on m7 §3.2.6's task model (80% U[0.15, 0.85] + 20% U[0, 1]; concentrated = gain
g/0.4 on each task with probability 0.4). P(`better`) = P(screen passes) · P(superior), the screen and the slice
being disjoint tasks; the divergence guard and cpp are not modelled. Design numbers (5,000-gate simulation) in
parentheses.

| Tier | FA = P(better) at 0 | +10 uniform / conc. | +15 | +20 | +25 | expected FA per 20 looks |
|---|---|---|---|---|---|---|
| lean 24×2 | 2.97% (3.0) | 21.0 / 19.4 (21 / 19) | 39.7 / 31.3 (38 / 31) | 61.0 / 41.5 (61 / 41) | 78.9 / 48.4 (80 / 48) | 0.59 |
| standard 32×3 | 3.43% (3.6) | 38.3 / 34.3 (37 / 34) | 68.3 / 53.5 (70 / 55) | 89.3 / 66.4 (90 / 69) | 97.6 / 73.5 (98 / 74) | 0.69 (≈ 0.7) |
| strong 32×5 | 3.62% (3.4) | 56.4 / 48.6 (56 / 49) | 87.4 / 69.8 (87 / 69) | 98.2 / 80.7 (98 / 82) | 99.8 / 85.7 (100 / 85) | 0.72 |

- The superiority test's attained size at 0 (no screen): 3.14% / 3.63% / 3.83%, all ≤ 5% (the exact bound).
- Every power cell is within 2.6 points of the design (largest: standard concentrated +20, 66.4 vs 69).
- Screen (§3.2.2) rejects: 5.4% at 0 (6%), 0.67% at +15 (≤ 1.1%), 14.2% at −10 (16%), **63.9% at −40 (about
  70%)**.
- Ratchet stage 1 (§3.2.13, standard; −10, −5, 0, +5, +10): `better` 0.0 / 0.5 / 3.4 / 14.5 / 38.3 (0.0 / 0.4 /
  3.5 / 14.2 / 37.7); `lineage_gain` 0.7 / 4.4 / 15.6 / 31.4 / 36.5 (0.6 / 4.5 / 14.5 / 30.9 / 36.6). The
  lineage row takes NI and m ≤ margin_max as given whenever D ≥ 1/n, so it is an upper bound (15.6 vs 14.5 at 0).
- Milestone confirm (0, +10, +15, +20, +25): 12×3 2.6 / 14.8 / 27.8 / 44.5 / 61.7 (3.1 / 15 / 28 / 45 / 62);
  24×3 3.4 / 30.2 / 55.7 / 78.9 / 92.5 (3.4 / 29 / 56 / 79 / 93); 24×5 3.7 / 45.7 / 77.2 / 94.4 / 99.2 (3.9 / 45 /
  78 / 95 / 99). False regression at 0 (p_inf side): 2.6 / 3.4 / 3.7 (2.5 / 3.7 / 3.8).
- Simulated (seeded, 400 gates, ±≈1.3 points), cheaper regression accepted through the tie guard: standard
  −10 6.4% (5.8), −5 22.5% (20); strong −10 1.5% (2.0), −5 13.9% (14).
- Pseudo-A/A on synthetic targeted data (32 tasks, R = 6): exact conditional FA 3.63%; 2,000 splits 4.10%;
  400 full-path splits FA 40‰ [23, 65], FR 73‰ [49, 103], m 188‰ → `Pass`.
- MDE (80% power, uniform, v_w = 201‰): analytic lean 243‰ / standard 172‰ / strong 134‰; exact 253 / 173 /
  133; the design's ≈ 25 / 17 / 13 points (record example 250 / 170 / 130).

**Discrepancies with m7 §3.2.6/§3.2.13:** (1) the screen rejects a −40 candidate 63.9% of the time under the
design's own task model, not "about 70%"; (2) strong-tier FA is 3.6%, not 3.4% (still ≤ 5%), and standard
3.4%, not 3.6%: the "1 time in 28" sentence of §3.2.6 holds for standard (1 in 29); (3) a normal-approximation
MDE at the nominal α = 0.05 reads 1–2 points optimistic (161‰ at standard against the exact 173‰), so the
module uses z at the exact test's attained size (≈ 3.5%, z = 1.812); see Deviations. Everything else agrees
within the design's own simulation error.

## Deviations
- Regression side of the milestone: literal m7 §3.2.13 table, `p_inf ≤ 0.05` with no 1/n margin (with pass/fail
  scores the margin is implied anyway; it could only bind for tiny partial-credit effects).
- `ratchet_k_next` halves on `regression` too (the design leaves it open; a regression is at least as bad).
- `MAX_N = 100` discordant tasks; beyond it p = 1 (never claims). Every design n is ≤ 40.
- `Tier::mde_permille` uses z₀.₉₆₅ = 1.812 (the exact test's attained size, 3.1–3.8% at the design n) instead
  of z₀.₉₅, so the `sealed_calibrate` MDEs match the exact power (within 10‰) rather than reading low.
- Power/FA tables are a test-only engine (design verification, like G-21b), not Root runtime code, to keep the
  `sealed/` sub-budget small; the runtime keeps only the analytic MDE that `sealed_calibrate` records.
- Commit 82032f9 also carries an orchestrator edit to `build/specs/PROOF-LEAN.md` (3 lines, the template
  CARGO_TARGET_DIR rule) that was swept in by a concurrent commit (HEAD-lock race); the content is the
  orchestrator's own.

## Needs from others
- none. The shared sign-flip test-vector file for M7-G09 was not written: the lean kernel (D-L3) has no exact
  test copy to share it with.

## Left
- nothing for the lean scope.
