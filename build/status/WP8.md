# WP8 (core): gate G0, G1, G4 statistics, accept rule, `evo` validator

Status: **partial, as planned.** The core part (lane K2) is done. G2, G3, the probe wiring, the G4 run driver,
the gate record/notes and the job glue wait for supervise's `probe()` (WP4). Date: 2026-10-20, about 03:00 +07.

Crate: `kernel/crates/kernel-gate/` (`kernel_gate`). The roadmap's `init-kernel::gate` and `init-kernel::evo` are
the modules of this crate.

## Done
- **`scope`** (gate.md §3.1). The scope table: t0, t0_escalated (wholesale T0 rewrite, evolution.md §3.3), dep,
  t1, t2. It returns `needs_handoff`, `needs_g4` and the stages. The lockfile dependency rule (E10) counts only
  entries that have a `source`.
- **`g0`** (gate.md §3.3):
  - The exact command list (`$C`/`$E`, timeouts, failure codes, 900 s total) and the env: supervise.md §3.6.2's
    table plus the G0 additions (empty wrapper vars, `RUSTC`/`RUSTDOC`, `CARGO_TERM_COLOR`, `INIT_MOCKLLM_BIN`).
  - The build-config policy on `.cargo/config.toml` and the legacy `.cargo/config`.
  - The hand-written `#![forbid(unsafe_code)]` lexer (nested comments, doc comments, shebang).
  - The shape checks from `cargo metadata`: unchecked crate, git dependency, forbid missing, ext not cdylib, ext
    dependency.
  - The runner, behind a `Spawner` trait so WP6a's build sandbox can wrap it. It stops at the first failure,
    enforces per-step and total timeouts (it kills only its own child), caps logs at 16 MiB, returns ≤ 4 KiB of
    diagnostics with errors first, and records `build_config_present` and `lints_changed`.
- **`g1`**:
  - The drift comparison, D12 style: `live` is dropped from both sides and `live_ignored` is a warning;
    `generator_changed`; `non_canonical_format`.
  - The drift report: changed files first, then sorted RFC 6901 pointers, values cut at 120 bytes, ≤ 4 KiB.
  - The trailer/marker check (`tag_marker_mismatch`).
  - The WIT copy drift check (`wit_copy_drift`, with the `cp` fix in the detail).
- **`stats`** (gate.md §3.8):
  - The primitives `seed_u64`, SplitMix64, Lemire `idx` and Fisher–Yates, bit-exact.
  - Metric statistics T_m(a).
  - δ(m) from sign-flip splits of the LKG repeats, with `noise.json` smoothing (w = 0.5 within 30 days,
    δ_hist·√(n_prev/n)) and natural-unit reporting.
  - The paired cluster-bootstrap NI test with margin max(2δ, 0.5/n) at α = 0.05, L = D*[500], U = D*[9499].
  - The futility look: U99, seed `futility`, δ from history only.
  - The family-clustered capability test (seed `cap`).
  - The G-21 simulator (`sim::simulate`, for `gate simulate`).
- **`tasks`** (gate.md §3.7.1, §3.7.2, §3.7.5):
  - `family.json` validation.
  - D2 sealing of ⌈n/2⌉ instances, and `sealed_commitment`.
  - D3 eligibility from R4 records only: registration seq < the LKG's promotion seq; d = 1 + ancestor
    `promotion{lkg:true}` records after registration, eligible iff d > K (K = 5, passed in from the Root);
    suspension by `task_suspended` or by a rollback/`ladder{down}` past the audited-good point, lifted by the next
    passing audit. `families_to_suspend` lists who gets a `task_suspended` record.
  - The pool selection: a stratified, seeded round-robin. The agent stratum holds only visible instances of
    eligible families, at most min(stratum_max, ⌊K/2⌋), and never pushes owner/maintainer tasks below
    `min_pool`. Repro tasks are added on top (≤ 5).
  - Capability set A: sealed instances of eligible, claimed families only, whole families, ≤ 8.
  - Holdout rotation and the canary sample.
- **`accept`** (gate.md §3.9, §3.9.1, §3.9.2, §3.12):
  - The v0.3 rule, in order: canary, insufficient, margin with top-up or `underpowered`, `score_inferior`, the
    pool and holdout guards, `pool_holdout_divergence`, then perf.
  - Then `better`, `capability_gain` (sealed half, l_cap > 0) and `cost_guard`: ≤ ρ_max, or accepted if the
    projected task day fits the task part of the soft day, else `costlier_than_budget`.
  - Then the tie-breaks: cheaper, costlier, smaller, `repro_fixed`, `no_gain`.
  - `repro_fixed` on agent families counts only eligible sealed flips.
  - `audit_required`: a recompute without agent tasks, with the same δ, margin and seeds.
  - `judge`: δ comes before the rule; `no_noise_estimate` when it is missing.
  - `resolve`: the first inconclusive escalates once (R = 4); the escalated one becomes `inconclusive_final`.
    `needs_owner` does not exist.
  - Also: F16 `provisional_expired`, and `futility_verdict` (`score_inferior_early`).
- **`evo`** (evolution.md §3.8–§3.10.1):
  - The subject ABNF.
  - `GitTrailerParser` runs the real `git interpret-trailers --parse --no-divider` with a hardened env.
  - The strict block check (block_missing, block_mixed, parser_mismatch).
  - Keys, values (every §3.8 ABNF and the `Predicts` clause grammar), the mandatory-trailer matrix and the
    prefix rule.
  - `capability_labels` (step 11).
  - The repo steps through the `EvoRepo` trait, with `GitCliRepo` on a bare lineage repo: shape and upstream,
    skill paths, deletion shape, retire/thin ids and tags, the revert tree via a kernel-owned worktree, and repro
    existence.
  - Every error is collected; nothing short-circuits.

## Tests
- `cd kernel && CARGO_TARGET_DIR=target/k2 cargo test --offline -p kernel-gate`:
  - lib: **157 passed**, 1 ignored (`g21_full`);
  - `tests/g0.rs`: **12 passed** (real cargo 1.97.1 on fixture workspaces);
  - about 29 s wall.
- `cargo clippy --offline -p kernel-gate --all-targets -- -D warnings`: clean. `rustfmt --edition 2024`: applied.
- Coverage by test id:
  - G-01; G-02 (all four cases); G-03 (`evil.sh` never runs; the empty `RUSTC_WRAPPER` env alone also stops it
    on cargo 1.97.1; without the policy, `--cap-lints=allow` lets a clippy warning through, so the policy is
    needed).
  - G-20 (pinned L, U), G-21 (below), G-22 (every branch of §3.9 and §3.9.2, one test each), G-23, G-26
    (δ within 10% of the analytic value), G-41, G-43, G-44 (unit half), G-48, G-49.
  - The evolution.md §8 validator table: every row of the §3.9 git-behaviour table runs against the real git
    2.39.5. All matrix cells for all 10 types, and the scratch-repo shape/upstream/skill/retire/thin/revert cases.
- Test homes live only under `kernel/target/test-homes/WP8/` and are removed by exact path. None are left over.
- **G-21 (finding, see Needs).** Full grid: 2,000 gates per cell, B = 10,000, `cargo test --release ... g21_full
  -- --ignored`, ≈ 10 min. Design: a quarter of the tasks flaky (p = 0.5), the rest deterministic. Accept rate at
  0 / at −2δ:

  | δ used | n=16 R=2 | n=16 R=3 | n=30 R=2 | n=30 R=3 |
  |---|---|---|---|---|
  | measured, no history (first gate on a model) | 73.9% / 11.6% | 83.9% / 9.3% | 87.9% / 12.6% | 90.5% / 7.2% |
  | measured, steady-state `noise.json` smoothing | 83.8% / 8.2% | 88.9% / 8.4% | 91.5% / 9.7% | **91.7% / 6.5%** |
  | oracle (true δ) | 91.0% / 6.7% | 93.6% / 9.2% | 93.9% / 8.3% | 93.3% / 6.1% |

  - gate.md's targets are ≥ 90% at 0 and ≤ 7% at −2δ at n = 16. They hold only for n = 30 / R = 3 with history.
  - The main cause is the noise in δ̂: at n = 16 only 4 flaky tasks feed it, and with the true δ, n = 16 R = 2
    passes.
  - The unit test `g21_calibration_reduced` asserts the targets for n = 30 / R = 3 and pins the measured n = 16
    baseline (≥ 78% / ≤ 12%), so a regression shows. It does not pretend the spec's n = 16 target is met.

## Deviations
1. **No `init-proto` dependency for now.** K1's work in progress on init-proto broke this crate's build, and the
   gate core needs no wire types yet. Re-add it with the job glue. `kernel/Cargo.lock` changes only in
   kernel-gate's dependency list (`toml` added, `init-proto` dropped).
2. **`toml` added** (gate.md §3.1 names toml 1.1.8 for lockfiles; also used by the G0 policy). It was already
   vendored.
3. **No syn for the forbid check.** A hand-written lexer does it, so this crate needs no syn.
4. **Log-scale floor.** `ln` of a natural-unit value is floored at 1 unit (1 µUSD, 1 ms), so $0 mock arms give an
   effect of 0, not NaN.
5. **Medians of an even count** are the mean of the two middle values.
6. **`noise.json` keys for split δ** are `pass_rate@pool` and `pass_rate@holdout`, with seed purposes
   `noise:pass_rate@pool` and so on.
7. **Other seed purposes:**
   - `futility` for the futility look;
   - `cap-select` for sampling A over families;
   - `canaries` for the canary sample.

   The holdout rotates over sorted ids by a gate counter, which the caller supplies.
8. **Futility** uses m = max(2·δ_hist, 0.5/n_look) over the tasks in the look.
9. **Capability test.** Each family's d is its mean paired difference over instances; the bootstrap resamples
   families. This is the same form the Root's L3+ per-family aggregates take.
10. **Eligibility edge cases.** An LKG with no `promotion` record (the seed) makes every family
    `late_registration`. Sealing's `sealed_commitment` joins ids with `\n`.
11. **Pool quota.** The ambiguous "owner tasks fill the rest first" is read as: owner quota =
    min(|owner|, max(K − cap, min_pool)), then the agent stratum, then the owner tasks fill any free slots.
12. **`evo` additions:**
    - codes `trailer.invalid:<Key>` (a value breaks its ABNF) and `evo.io` (kernel-side failure);
    - perf "strict" means `<` on the cost and time metrics, `>` on `cache_hit`, and a violation is reported as
      `predicts.trivial_only`;
    - `GIT_CEILING_DIRECTORIES` is set, because git in an empty dir inside a checkout found the enclosing repo;
    - `self.json` is read with `cat-file blob`;
    - `trace:` repro refs use a callback that defaults to not found, until the proxy trace reader exists;
    - the revert check uses a fixed `cache/revert-check`, which assumes promotions are serialized;
    - the identity for the revert worktree is `INIT Kernel <kernel@init.local>`.
13. **G0 policy additions**, each a bypass reproduced on cargo 1.97.1:
    - top-level `paths` and `include`;
    - `@file` arguments in rustflags;
    - `[alias]` entries that shadow metadata, check, clippy, build or test;
    - snake_case key spellings.

    Other G0 details:
    - Any `vendored-sources.directory` value is allowed, because the CLI `--config` overrides it.
    - Shape treats a source-less lockfile package that is not a workspace member as `unchecked_crate`.
    - An unparsable lockfile fails closed as `git_dependency`.
    - An ext member with no lib target is `ext_not_cdylib`.
    - Step and total timeouts both give `gate.g0.timeout`.

## Needs from others
- **gate.md owner (G-21).** The stated calibration does not hold at n = 16 (table above). Options:
  - make the first gate on a model `inconclusive` until history exists, or run a calibrate (A/A) job first;
  - raise `margin_factor`, or the δ quantile, at small n;
  - raise `min_pool`/`min_holdout` toward n ≈ 30 with R = 3.

  Until then, the record's δ and m state the real resolution.
- **supervise (WP4) / sandbox (WP6a).** Cargo also reads `.cargo/config.toml` from every **parent** directory of
  the slot. If `INIT_HOME` is under `~`, it reads `~/.cargo/config.toml` despite the fresh `CARGO_HOME`. The build
  sandbox should deny that read, or the slot should live outside `~`. supervise.md §3.6.2's claim is incomplete.
- **evolution.md owner.** The `thin` rule says `Compensates-for` equals the removed component's tag. A §3.12
  batch thin that removes components with different tags can never validate. A spec decision is needed: for
  example, allow one `Compensates-for` per `Thins`, or batch only same-tag components.
- **WP4.** Wire `scope::classify`, `evo::validate` (with `GitCliRepo` on `$G`), `g0::run` (with a sandboxed
  `Spawner`) and `g1` into the promote job. `GitCliRepo` uses its own hardened env; swap in `$G` if it differs.

## Left (WP8 rest)
- G2 (both probes, prefix-hash and continuity checks) and G3 (scripted self-edit, inner gate), on supervise's
  `probe()`.
- The G4 driver:
  - runs and pairing, the RunRecords;
  - the futility look at the `gate.g4.mid` fault point;
  - top-up execution, the budget pre-flight;
  - the check protocol and the canary quarantine ref.
- Task import and family admission (`admit` jobs, `task_admit` to Root lineage).
- The gate record (permille rounding, holdout redaction by level), `refs/notes/gate`, `noise.json` I/O, lineage
  before LKG, `gate judge`, `gate simulate` CLI wiring in `init-kernel`.
- `evo::predict` (Predicts scoring, `refs/notes/predict`).
