# INTEG-A: ADR-011 integration (lane INTEG-A, with helper INTEG-A-ROOT)

Status: **done**, 2026-10-22; `ci/local.sh` green once (20/20 steps, 20:52 at 86ea81e). Commits: 54ec0b3 (kernel),
ea20688 (X4 snapshot cap), c3f516d (unwatched_change), 86ea81e (test fixes for a green CI);
helper INTEG-A-ROOT: 62b8410 (twin pool), c35057e (m7/wp24), ba13c1b (dep and workspace-write grants). Others at
INTEG-A's request: X3 1496814 (extend.json merge, sentinel limits from grants), X8 96a7d4f (kupgrade test after 1496814).

## Done (kernel side)
1. **A1 advisory on the free path** (ADR-011 §2.1; K2 deviation 2, K1 deviation 4).
   - kernel-gate `g0`: `G0Spec.advisory`; `code::ADVISORY` = clippy, ext clippy, the body's tests, the ext tests.
     With `advisory`, a failing (or step-limit timed-out) advisory command is recorded (`G0Report.advisory`,
     `StepRecord{pass:false, advisory:true, code}`) and G0 goes on. Build, policy, shape and `check` stay blocking.
     The gated path, G3's inner gate and the engine's E0 keep `advisory = false`.
   - kernel-supervise: `stages::build_and_g0(.., advisory)`; the promote pipeline passes `env.free_link().is_some()`.
     The G0 stage record carries `advisory[{step, code, exit, timed_out, diagnostics ≤ 1 KiB}]` and per-step
     `pass/advisory/code`. A3 (`stages::a3_run`) now reads a failed `test` step by `pass == false` or a non-zero exit,
     so a test that timed out also vetoes.
2. **`evo propose`: `predicts` optional on the free path** (X11 need). `proposal::propose_in(.., mode)` (0–5 clauses
   on free, 1–5 on gated; a clause given must still parse), `variant::build_in(.., mode)`; the kernel-written message
   omits `Predicts:` when there is none and is dry-run under the mode. init-kernel passes the supervisor's `extend_free`.
3. **`initctl grant <kind> <value> [--hours H] --why TEXT`** (X11 need; ADR-011 §7.4). init-proto `CommandName::Grant`
   (`"grant"`, live lane, L0, needs the Root) and `KernelRequest::GrantRequest` (`grant.request`, JSON args);
   kernel-serve `Backend::grant_request`; init-kernel relays through `admit::grant_request`; refusals: `root.args` →
   `E_ARGS`, `root.rate` → `E_RATE`, other `root.*` → `E_ROOT{reason:"refused"}` (`data.root_code` always set).
   The value parses as an integer (digits), JSON (`[`/`{`, for `dep`) or a string (money: `"2.00"`).
   init-fakeroot answers `grant.request` like the Root (kinds, 3 per run, `grant_request` record with `why_sha256`).
4. **Archive and quarantine moves** (X11/X6 need). kernel-gate `scope::is_inert` (`archive/**`, `quarantine/**`):
   never raises the scope (a skill move is `t0`, an ext move `t1`, a restore `t0`); a `retire` (and `thin`) may move
   files there (exempt from the deletion-shape counts and the no-new-files rule); a `skill` commit may touch them;
   G0's manifest walker skips `archive/` and `quarantine/` at the root (an archived ext crate keeps its Cargo.toml).
   `dep`: `change.open{scope:"dep"}` carries `crates:[{name, version, checksum}]` (kernel-gate `scope::dep_crates`:
   registry entries new or changed in either lockfile, sorted); the Root's `extend.dep_grant` → `E_QUOTA{quota:"extend_dep_grant"}`.
6c. **§9 constants → charter `[extend]` and grants**: X3 did the sentinel (1496814: `grant::budget/win/burst_floor_musd`
   before every engine call); INTEG-A did X4's snapshot cap (`grant::snap_max_gib`, ea20688). X3 also merged
   `select::view` into `extend.json` (1496814, item 4's X11 need).
7a. **kernel-serve `ic4_lane_and_level_matrix`**: serve checks the §3.4 lane column by name before decoding the
   arguments (`gating::check_lane_name`), so a command a lane never offers is `E_LANE` whatever its args; the test
   counts `CommandName::ALL`.

## Done (Root side)
5. **Twin money pool** (INTEG-A-ROOT, 62b8410): `egress::twin_lane` charged to a `twin` pool in money.rs, capped per
   night by `extend.twin.usd_day`; nothing goes to `sealed.reserve_usd`. Lifetime bound: zone (spent + reserved) + twin
   (spent + held) ≤ total − reserve, checked under one lock. Refusals `ceiling.twin`, `ceiling.total`. Charter
   `[extend.twin] window_utc` (default `"17:00-00:30"`, DeepSeek's off-peak: the Root's model table carries no
   schedule), C17-checked, out of the effective JSON at its default.
6a. **The broker's `root.unwatched_change`** (c3f516d): free path, live lane, kind body: the program's sha256 must be
   the body binary of `lkg`, `current`, `settled`, an open group's tuple or parent tuple, or the pending rollback's
   target (`tuple::unwatched_body`); else `ceiling_hit{code:"root.unwatched_change", demotes:true, rule:"b"}`, the
   alert, and the refusal. It judges only when every allowed tuple loads and names its body binary.
6b. **`dep` and `fleet.workspace_write` grants** (INTEG-A-ROOT, ba13c1b): A5 refuses a `dep` change whose exact
   `crates` no live grant covers (`extend.dep_grant`); a member writes the workspace only while a grant of N is live,
   at most N at once (`access::unspawned` gives a failed spawn's slot back).
7b. **root `m7`, `wp24`** (INTEG-A-ROOT, c35057e, tests only): fixture tuples carry a body commit and the harness writes
   the T0 tree; `body_home`'s production check is unchanged.

## Tests ($0; `source ci/env.sh`; every cargo command through `bash ci/slot.sh`)
- `cargo test --offline -p kernel-gate --lib -- g0:: scope:: evo::` → 107 passed
  (`advisory_clippy_and_tests_on_the_free_path`, `free_scope_decisions` + archive rows, `scratch_retire_shape_and_components`
  + moves, `walker_skips_vendor_target_git` + archive).
- `cargo test --offline -p init-proto -p kernel-evo -p initctl -p kernel-serve` → all green (init-proto 41 + golden 6,
  initctl 7 + cli 13, kernel-evo 8 + engine 5 (`free_path_predicts_optional`) + golden 3 + mine 3, kernel-serve 19 +
  ic 30 (ic4 green) + sandboxed_lane 1).
- `cargo test --offline -p kernel-supervise --lib --test extend --test promote` → lib 107, extend 4/4 (new
  `a1_advisory_clippy_and_tests_then_a3_vetoes`: a T2 with a clippy finding and a failing test passes G0 with both
  advisory entries, G1, G2, admission, handoff, then A3 vetoes on `gate.g0.test` with no G3), promote 8/8 (gated F3 still
  fails on clippy).
- `cargo test --offline -p kernel-gate -p kernel-supervise -p init-testkit -p kernel-serve --lib` → 244 / 107 / 87 / 19.
- `cargo test --offline -p init-kernel --test e2e` → ok (after `cargo build --offline`: the test reuses built helper
  bins): `initctl grant extend.open_groups 6 --hours 48 --why …` → `{id:"g00000001", status:"pending"}` through the real
  kernel and init-fakeroot, `grant_request` in the Root lineage with `why_sha256` only; an unknown kind → `E_ARGS`.
- `cargo clippy --offline` on the 8 touched kernel crates `--all-targets` → 0 warnings.
- Root: `--bin init-root -- snapshot:: grant:: tuple::` → ok (`unwatched_body_on_a_test_home`); full
  `cargo test --offline -p init-root --features test-fixtures --no-fail-fast` with the hook in: unit 182, m7 3/3,
  wp24 6/6, x3, x8, x67, redteam, wp25 and the rest green except `rsafe` (X8's two new `sys::signal` callers in
  sup/kprobe.rs not on the R-SAFE list; X8 asked) and one `wp21` crash-loop timing failure that passed on rerun.
  Clippy: nothing in INTEG-A's files (the old findings in sealed/ and sys.rs remain).

8. **`ci/local.sh`**: the first run failed two steps, both fixed in 86ea81e: `root test` (the default-feature build)
   ran eight unit tests that build a Root test home, whose path exceeds the production 64-byte ROOT_HOME bound (80 in
   test-fixtures builds); they are now ignored without `test-fixtures` (the next step runs them). `redteam`
   `rt_l3_only_capabilities_are_refused_below_l3_and_demote` raced the demotion's zone restart (about 1 in 3 runs);
   it now waits for the restart before the second call and accepts the Root's `ceiling_hit` for a lost first outcome
   (4/4 runs). The second full run: **`ci/local.sh: all steps passed`**.
   On the way, at INTEG-A's request: X8 fixed its kupgrade unit test after X3's engine change (96a7d4f) and the R-SAFE
   list for sup/kprobe.rs (6767622).

## Deviations
0. `root.unwatched_change` checks the program's hash only: `spawn` takes any absolute program path, so an
   interpreter or a wrapper still bypasses it (the ADR's "cannot run an unannounced change" needs the Root to pick the
   program itself); the hash-then-exec window is a TOCTOU; the ext set and the `t0` blob change by reloads that spawn
   nothing. No daemon-level test (the fake kernel's body is `/bin/cat` and test homes have no seeded genesis body), only
   the test-home unit test.
1. `predicts` only: `repro` stays required by `evo propose` (it is the engine's evidence rule, not a commit trailer).
2. Archive moves are not checked to be pure renames: `archive/**` and `quarantine/**` are inert (never loaded, built
   or in the `t0` blob), so content added there cannot add a capability. X11's `refactor(archive)` recipe keeps working;
   `retire` + `Retires:` now works too. The skill text is unchanged (no seed re-bless).

## Needs from others
- X8: `rsafe` R-SAFE list for sup/kprobe.rs (asked).
- X12 (docs): the twin pool (egress.md §3.3), `[extend.twin] window_utc` (root.md), `initctl grant` (initctl.md §4.1),
  `extend.dep_grant` and `crates` on `change.open`, `root.unwatched_change` (root.md §3.4), A1 advisory steps (gate.md §3.3).
- Seed (whoever touches seed next, with the G1 bless): the evolve skill can say `predicts` is optional on the free path,
  and the extend skill can give `initctl grant`'s syntax and `retire` + `Retires:` for archive moves.

## Left
- Nothing in INTEG-A's list. Open elsewhere: X3's long-session H8/H10/H11 fix landed as 8bb3848 (v2_fix) and awaits the
  orchestrator's confirmation; the X12 doc fold above.
