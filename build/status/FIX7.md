# FIX7: promote tests meet the D14 holdout rule (`GAIN_MIN_HOLDOUT` = 4)

Status: **done** (2026-10-20, lane K1). Path: `kernel/crates/kernel-supervise/tests/promote.rs` only. The rule (kernel-gate, FIX2 35647a8/194ba30; gate.md §3.9, v03-decisions D14) is unchanged.

## Done
- **Root cause (FIX6 "Needs from others" item 2).** The promote test world wrote one holdout task (`h1`, `holdout_h = 1`). `g31`, `g46` and `sv26` expect a G4 `accept` (`better`: the LKG fails every task, the candidate passes them), which D14 now turns into `inconclusive: underpowered:holdout` below 4 holdout tasks.
- **Fixture fix, not an expectation change.** All three tests are about the accept path (G-31 accept -> LKG, G-46 lineage down at the accept, SV-26 t1 accept -> LKG), so each now runs a 4-task holdout:
  - `WorldOpts.holdout` (new): the world writes `h1..h<holdout>` (each solved by the mock like the pool tasks) and sets `gate.g4.holdout_h = <holdout>`. `min_holdout` stays 1, `min_pool` stays 2 (the test config's own minimums; no pool ≥ 8 is needed).
  - `g31`, `g46`, `sv26` use `holdout: GAIN_MIN_HOLDOUT` (imported from `kernel_gate::accept`, so the fixture follows the constant).
  - The reject/inconclusive worlds (`f1..f9`, `g10..g16`, `f7/f11/f13/f14`) keep `holdout: 1`: a reject or an `insufficient` inconclusive is not a score-gain accept, and 1 task keeps their G4 runs short.
- **Assertions tightened.** `g31` and `sv26` now assert `(verdict, reason) = (accept, better)`, so the D14 path is pinned. `g31`'s G-32 leak check covers every holdout id `h1..h4`, and `n.holdout` is 4.
- **Workspace sweep (holdout / min_holdout / holdout_h, better / capability_gain / repro_fixed):**
  - `kernel-gate` `accept/tests.rs`, `g4.rs` tests, `tests/judge.rs`: already on the rule (FIX2: 4-task holdouts, or the `inconclusive` expectation at 1).
  - `init-testkit/tests/acceptance.rs` and `scenarios/a3.json`, `a4.json` (`holdout_h = 1`): no gain expected. A3's G4 ran `reject: no_gain` (log: `G4 jobs ["j-000002" escalated=null -> "reject" "no_gain"]`), carried out by a rollback; A4's candidate fails G2 and never reaches G4. No change.
  - MS-C1 and TA-11 (testing.md): no kernel-workspace test implements them. MS-C1 lives in the root workspace's `init-root/tests/wp24.rs` gog, which samples 8 pool + 4 holdout (FIX5: real-judge gog `pass:true` at 8+4). TA-11 is not implemented yet; testing.md §3.4.6 already says it must set `holdout_h` ≥ 4.
  - `kernel-gate/src/record.rs` (`root_subset` test) and `kernel-supervise/src/promote/tests.rs` (`g46_pending_record...`) hold static `accept/better` records with `n.holdout: 1`. They test redaction and resubmission, and no gate judges them, so the rule does not apply. Left as is (not mine to edit; cosmetic only).

## Tests
- `cargo test --offline -p kernel-supervise` (all targets): **pass**. lib 66, fakeroot_xcheck 1, handoff 8, install 1, probe 3, process 3, promote 6 (95 s), recovery 5, serve_seam 1, slice1 10.
- `cargo test --offline -p kernel-gate`: **pass**. lib 186 (2 ignored), g0 12, judge 4.
- `cargo test --offline -p init-testkit --test acceptance`: **pass**, 6/6 (A1, A2, A3, A4, A5-fake, NO-OWNER-1), 107 s.
- `cargo test --offline -p init-kernel`: **pass**. unit 2, e2e 1, judge_cli 1.
- `cargo clippy --offline -p kernel-supervise --all-targets -- -D warnings`: clean.
- All with `CARGO_TARGET_DIR=kernel/target/k1 CARGO_INCREMENTAL=0`, mock model only.
- Disk: 6.5 GiB free throughout. After the acceptance run I removed the template dirs it had just recreated, by exact path: `target/t15/{bin,fxt,gen,tm-fake,tm-fx,tm-seed}` (134 MB, created 06:58 by this run; no other acceptance process was running). Every process was a test's own child.

## Deviations
- None. The preferred fix (a holdout of at least 4 mock tasks) was enough for all three tests; no expectation was changed.

## Needs from others
- Nothing.

## Left
- Nothing for FIX7.
