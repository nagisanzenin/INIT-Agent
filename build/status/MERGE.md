# MERGE: STEER + DELEGATE into main, sentinel fixes, warm T2 builds, v0.3.0 (lane MERGE, 2026-10-24)

Status: **done** (pushed to origin/main after CI). $0: mock models only; `~/.init` and `~/.init-root` untouched.

## Done
- e7d44fd merge STEER (no conflicts; docs/kernel/supervise.md and root cli.rs auto-merged).
- 569fedd merge DELEGATE. Conflicts: `seed/src/model.rs` `Abort` keeps both STEER's continue flag
  (`trigger_continue`/`wants_continue`) and DELEGATE's child aborts (`child()`; `fire()` reaches children);
  `seed/self.json` taken from DELEGATE, regenerated at the end.
- 3bd6178 version 0.3.0 in kernel/root/seed/tui and their locks; `SEED_TAG=seed-v0.3.0` in tests/real/*.sh;
  get.sh example; install.sh's already-installed message names the target version and the Trash-based uninstall.

- 0553317 golden `delegate_nested` takes STEER's input hint (the only merge-semantic test break).
- d797134 + 07221d2 sentinel (sub-lane MERGE-SENTINEL):
  - a correction is only an owner interrupt: Esc/Ctrl-C (also `then:"continue"`), `/stop`, `/panic`, `/undo`, plus
    `/reject` and `/rollback` (explicit owner reversals). Plain steers add to a weak `steered` count; `/later`,
    `/status`, `/kernel` count nothing; a relayed `retract{seq}` takes its steer back (seq matched via the kernel echo).
    `v2_fix` tests pass unchanged.
  - kernel-proxy forwards a well-formed `x-init-subtask` (≤ 64 B, `[A-Za-z0-9][A-Za-z0-9._-]*`) as `x-egress-subtask`;
    the Root re-validates it, never relays it upstream (x5 test), and the turn record keeps `sub_requests` /
    `sub_cost_musd`; H10 and H5b test the main loop's share only.
- 89996cc install.sh bundle-mode messages: get.sh installs Rust 1.97.1 + wasm32-wasip2 (main's 1819f7f) unless --no-rust.

- ed7bc78 warm T2 builds (sub-lane MERGE-T2CACHE, details in build/status/MERGE-T2CACHE.md): on a miss a stub
  workspace builds the third-party deps in the build slot before candidate code runs; the entry is staged with a tree
  digest and published only after the candidate's G0 passes (digest re-checked); a hit is used only when the digest
  matches its manifest, else deleted and cold. Vendor mtimes pinned, fixed `SOURCE_DATE_EPOCH` for slot builds.
  **Measured** (real seed, real Seatbelt build/kernel profiles, mock, load 10–23, ignored test
  `promote_seatbelt::t2_seed_build_is_warm_on_the_second_promote`): promote 1 build 925 s / total 1029 s;
  promote 2 (no-op comment) build 32 s / total 126 s.
  **Residual risk:** G0 children still run in the kernel zone profile (WP8b deviation 1), so a candidate's tests could
  rewrite a published entry together with its manifest; the digest catches corruption, not that. Closing WP8b
  deviation 1 (G0 through the broker's build profile, which cannot write `cache/deps`) closes it.
- ca0b840 CHANGELOG entries; 82997ab `seed/self.json` regenerated through the G1 bless path (seed G0 pass, G1 pass).
- Rerun alone: root `tests/m7.rs` `m7_draw_and_g4_slice` timed out (60 s) under load 12 in a partial run; alone 3/3 pass.

- `t2_seed_build_is_warm_on_the_second_promote` is opt-in (`INIT_T2_WARM_BENCH=1`): ci/local.sh runs
  promote_seatbelt with `--ignored`, and the benchmark added about 17 min to that 1-2 min step.

## Tests: full `ci/local.sh` on macOS (CARGO_BUILD_JOBS=4, RUST_TEST_THREADS=4, load 6-12 from other lanes)
**1,870 passed, 1 failed, 33 ignored**; 2 of 18 steps failed, both load flakes, each proven by rerunning alone:
- kernel test: `kernel-selfmap --test selfmap` s08/s09 hit their 30/40 s watchdogs (the known flake), which aborted
  the binary. Alone: 12/12 pass (57 s).
- root test (test-fixtures): `rsafe` `live::dry_run_selects_exactly_our_instances_and_panic_spares_decoys` saw the
  just-spawned body as `recorded: false` (a timing race under load; rsafe untouched by this lane). Alone: 6/6, three
  runs out of three.
- Every step else passed, including seed G0 + G1 (self.json agrees), promote under Seatbelt (A3, and the warm bench
  ran and passed inside it), tui e2e, and the sealed pack steps.
- The suites STEER and DELEGATE could not run in their worktrees (SUN_LEN) all pass on main: kernel extend 4/4,
  promote 9/9, fakeroot_xcheck 1/1, init-testkit acceptance/fakes/scenarios, root `lane_connections_are_capped`,
  `rf31_rf33…`, TUI proto 14/14 (incl. tui14, wire9), tty (incl. `steer_tray_…_through_a_pty`), e2e_root 3/3.
- Clippy `-D warnings`: seed and tui clean. kernel-proxy `config.rs`, kernel `kstate.rs` and init-root
  (`sealed/`, `egress/money.rs`, `sys.rs`, `zone.rs`) keep pre-existing lints in files this lane did not touch.

## Left / for the release lane
- Cut the bundles and publish (not done here). CHANGELOG has an Unreleased section for 0.3.0.
- Sentinel: H8 (wall) and H11 (request bytes) still include sub-task traffic; retract matching is lost on a Root
  restart; STEER's `ux_epoch` proposal (restart the corrections trend's reference after a UX change) is not built.
- T2: the first promote on a bundle install still pays one stub build (~15 min); the residual cache-tamper risk above.
- Flakes to harden: selfmap s08/s09 watchdog bounds, rsafe dry-run recording race, root m7 60 s waits under load.
