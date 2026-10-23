# Phase 0 report

```
Status: draft v0.2 · 2026-10-20 · Area: overview
         v0.2 (DOCS2 lane, about 16:20): the R-SAFE, M5, REL and TUI placeholders are replaced by the lanes' results;
         S-REAL stays a placeholder (§9.2) for the orchestrator.
Depends on: INIT Agent spec.md (v0.3 §10, §11, §19), docs/roadmap.md (§3.2 M6, §3.10), docs/testing.md
            (§3.9–§3.11, §3.17), docs/CONVENTIONS.md (§1, §6), docs/reconciliation/build-deviations.md (v0.3),
            RELEASE_PLAN.md, BUILD_LOG.md, MORNING_REPORT.md, measurements/phase0.md, build/status/*.md
Defines: nothing (a report)
```

This is the report that roadmap.md §3.2 lists as the last item of the M6 definition of done ("the Phase 0 report written; the owner may read it; no sign-off is required"; WP19). It covers the overnight build of 2026-10-20 (WP0–WP25, S1, FIX1–FIX9) and the state of the v0.1.0 release push that started at 12:20. Its sources are the committed tree up to `d4807ca`, plus the DOCS lane's doc edits, which were committed as `50c553e` and `781504b`. The TUI lane's `docs/tui.md` (`ddce8fa`) landed after the draft and is noted in §9.5. Results of lanes that are still running are marked PLACEHOLDER (§9). Nothing in this report was measured for it: every number comes from a source file or commit. v0.2 adds the release-push lanes' results from their status files and `git log` through `baab414` (16:15).

**Citation keys.** `WPn`, `FIXn`, `S1`, and the release-push lanes `R-SAFE`, `R-DEC`, `R-SEC`, `K-FLAKE`, `K-LINE`, `K-GATE`, `M5`, `REL`, `WP26` (the TUI), `G0-ROOT`, `G0-SEED`, `S-REAL` = `build/status/<id>.md` (D = Deviations, N = Needs from others, L = Left). `BL hh:mm` = the BUILD_LOG.md row at that time. `MR` = MORNING_REPORT.md. `RP` = RELEASE_PLAN.md. `P0` = measurements/phase0.md. `BD §x` = docs/reconciliation/build-deviations.md v0.2 (05a9473); v0.3 marks today's fixes and adds §6, the release push. `T §x` = docs/testing.md. `RM §x` = docs/roadmap.md. `CV §x` = docs/CONVENTIONS.md. `spec §x` = `INIT Agent spec.md`. Commit ids are short shas from `git log`. Times are +07 on 2026-10-20.

## 1. Summary

- **What exists.** Three Cargo workspaces: `kernel/` (10 crates), `root/` (`init-root` and `init-sandbox-profiles`) and `seed/` (`init-body`, plus the `seed/ext` extension workspace). MR counts ≈ 143k lines of Rust, tests included. They were built by WP0–WP25, S1 and FIX1–FIX9 between 01:50 and 08:04 (BL; commits `9a71966` … `a595a9b`). The design's whole path (owner → `init-root` → sandboxed kernel → sandboxed body → proxy → Root egress → DeepSeek) ran once on the real model: 2.39 s, Root-metered 2,844 µUSD, and the key appeared in none of the 218 artifacts (S1, `5aaa40b`). That run still used two harness workarounds for the install path, which FIX4 later removed (S1 Gaps; FIX4).
- **What passes.** The final `ci/local.sh` run counted 1,137 passing tests, with 1 timing-flaky failure (`h21`, green 3/3 alone) and one hang (`sv10`) after the Mac slept (MR; BL 11:35–11:59; `build/ci/`). At $0, A1, A2, A3, A4, A5-fake (kernel half), A6 (live half, on the real Root) and NO-OWNER-1 pass (WP15, `9e4c2a1`), and they passed again in FIX6's and FIX7's runs and in the final CI run (FIX6, FIX7 Tests; `build/ci/local-full-2026-10-20-final.log`).
- **Measurements.** M-1, M-2 and M-4 pass. M-5 fails as written (15.2 KiB/step against 2), and FIX8 found it is not a leak (P0; FIX8). M-3 was not run, because it needs the real model (P0). A provisional decision changes the M-5 method (§3.2), and under it M-5 passes 3 of 3 trials (M5, `3b02364`; §9.3).
- **Release push (12:20–16:15).** Owner-machine mode and ADR-008 (R-SAFE, §9.1), the review's F1–F7 fixed (R-DEC, R-SEC, R-SAFE), M-5 PASS (§9.3), the `init` TUI (WP26, §9.5), the one-command install with a passing $0 end-to-end run (REL, §9.4), and 3× green kernel and seed suites (K-FLAKE). The real-model batch found release blockers live in G0 and G1, and they were fixed (BD §6); its final results are S-REAL's (§9.2).
- **What is open.** Every real-model variant (A1–A6 real, E2E-1…5, M-3, V-DS-*), every test that kills or sweeps (disposable environment only), G0 sandboxing, the Linux sandbox, and all L1+ mechanics (BD §2; MR). The ledger holds 149 deviation rows, of which 13 are owner decisions and 19 are code fixes (BD §1 totals).
- **Decisions.** The orchestrator took 9 provisional owner decisions at 12:16 because the owner said not to wait (RP). The owner may override any of them. The constitution text (OW-1) is still PENDING OWNER (RP; CV §6).
- **v0.1.0.** Not tagged yet (`git tag` is empty at `baab414`). RP's six-part definition of done, item by item: (1) install: built, and the $0 end-to-end run passes (REL, §9.4); (2) the `init` TUI: built (WP26, §9.5), and its run on real DeepSeek belongs to S-REAL; (3) real L0 self-evolution: A2-real passes, A3-real is pending (S-REAL, §9.2); (4) owner-machine safety: built and verified live (R-SAFE, §9.1); (5) CI: kernel and seed suites green 3 times in a row (K-FLAKE), while `ci/local.sh` 3× and the first GitHub `fast-linux` run are the orchestrator's release check; (6) docs: README, CHANGELOG and docs/release.md are committed (REL), and the tag follows a clean re-cut (docs/release.md §2).
- **Spend.** DeepSeek under $0.01 of the overnight $1 cap; cloud $0 (MR Spend). The v0.1 real-model batch has a hard cap of $1.00 Root-metered (RP).
- **Progress.** MR estimates that about 75% of Phase 0 is built (≈ 145 of 191 roadmap agent-hours), and calls this its own estimate.

### 1.1 The M6 definition of done, item by item (RM §3.2)

| M6 item | State | Source |
|---|---|---|
| A1–A6 real variants once each (A5-real with an external SIGKILL) | open: budget for the real model; disposable environment for A5-real | §4; WP15 L |
| E2E-5 (A2-real, A3-real) | open: budget | §4.3; BD §3.17 |
| Measurements in `measurements/` and summarized in the docs | M-1, M-2, M-4 done; M-5 fails as written and passes 3/3 under the provisional v0.1 rule (`measurements/m5-v0.1.json`); M-3 not run. This report is the summary. | P0; §3; M5 |
| The Root's size measured against its budget (R13) | measured: 7,053 lines at `dd5c358`, and 7,085 at `cdb49e8` (sub-budgets total 7,540), against a 1,000–1,500 design figure; provisional budget ≈ 7,500 | root.md §4.8 (DOCS lane edit); RP; §6.2 |
| Spec deltas folded into a spec v0.4 proposal | done, draft v0.1: 136 items, 10 source conflicts, 521 rows read (`docs/spec-v0.4-proposal.md`). Nothing is applied to the spec except §13's budget line (`50c553e`). | DOCS lane |
| `ci/local.sh` tiers and the GitHub `fast-linux` job green | `ci/local.sh` green except 1 flaky test (morning run); since then K-FLAKE made `h21` deterministic and bounded every process-spawning kernel test, and the kernel and seed suites passed 3 times in a row. `.github/workflows/fast-linux.yml` is committed (`b461f1f`, `baab414`) and has not run on GitHub yet | MR; K-FLAKE; REL |
| Disk cleanup (RM §3.8.4) | not done; free disk was about 6.4 GB at 12:00, and 9.5 GB of own target dirs were freed at 12:18 | MR; BL 12:18 |
| The Phase 0 report | this file, draft v0.1 | — |

## 2. What was built

Commits are listed in build order. "Green" is the count from the crate's latest status file, not from one simultaneous run.

### 2.1 `kernel/` workspace

| Crate | Role | Built by (commits) | Green |
|---|---|---|---|
| `init-proto`, `initctl` | kernel-side shared types (serde); `initctl`, the agent's thin client (std only) | WP1 `ef23939` | 60 (WP1; BD §4.1) |
| `kernel-supervise` | supervision under the Root: control link, spawns through the Root's broker, readiness, heartbeats, backoff, body safe mode, overlapped handoff and rollback, LKG, `probe()`, build slot, install, and the T2 promote pipeline | WP4 `adbf6b1`, `f22b33e`; WP8b `6754298`; FIX6 `80afa41`; FIX7 `37aef0c` | 104, plus 2 with `fault-injection` (FIX7 Tests; BD §4.1) |
| `kernel-proxy` | model proxy: routing and lanes, `x-init-*` strip, forwarding to the Root egress with the guard epoch, typed 429 soft budgets, usage, head hashes, traces | WP5 `4d0f6e1` | 83 (WP5) |
| `kernel-gate` | gate G0–G4, statistics, the accept rule with D14, the `evo` validator, `gate judge` | WP8 `316bbab`; WP8b `6754298`; WP8c `e2cdb53`; FIX2 `35647a8`, `194ba30` | 202, 2 ignored (FIX7 Tests) |
| `kernel-selfmap` | `self.json` generator, markers, token counts, WIT inventory, G1 drift | WP9 `44673b9` | 49 (WP9) |
| `kernel-serve` | initctl server: JSON lines, peer auth, lanes, job model, quotas, the early "starting" lane | WP7 `bf9c168`; FIX1 `b1b9970`; FIX6 `80afa41` | 50 (FIX6 Tests) |
| `init-kernel` | the kernel binary: `supervise --under-root` with the serve backend, data-only promotions, selfmap dispatch, `gate judge` CLI | WP7b `71b4743`; WP8c `e2cdb53`; FIX6 `80afa41` | 4 (FIX7 Tests) |
| `kernel-sandbox` | Linux sandbox (bwrap, Landlock, seccomp) | not started (WP6b, M7) | none (BD §4.1) |
| `init-testkit` | see §2.4 | | 130 |

The kernel rows sum to 682, which equals the final kernel re-run: 682 passed, 0 failed, 4 ignored, over 50 suites (`build/ci/kernel-rerun-2026-10-20.log`; BL 11:59).

### 2.2 `root/` workspace

| Crate | Role | Built by (commits) | Green |
|---|---|---|---|
| `init-root` | the Root: charter (R5), owner socket and CLI, panic and resume; R1 zone, spawn broker, supervisor, tuples and rollback; R2 egress, keys and ceilings; R4 signed lineage; R6 sealed evaluation on a stand-in S; R7 ledger; the install path; `proposal.file` | WP20 `f671c1a`; WP22 `dbbdfe4`; WP23 `855b059`, `496241f`, `e246191`; WP21 `528c051`; WP25 `d7afcb0`; WP24 `2209594`; WP21b `099022b`, `0f4f46d`; FIX4 `d84a321`; FIX5 `54fb956`; FIX9 `6c97b4c` | 103 with `test-fixtures` (65 unit + 38 integration); 62 unit without features (FIX9 Tests) |
| `init-sandbox-profiles` (`root/sandbox-profiles/`) | the Seatbelt profile texts (including `SEALED_TAIL` since FIX9), V1–V5 validation, composition, the sandbox self-test | WP6a `f3f9436`; FIX1 `b1b9970`; FIX9 `6c97b4c` | 17 (FIX9 Tests); self-test 222 pass / 0 fail / 11 skipped of 233 (FIX1; BD §4.2) |

A production build caps the level at L0: `SUPPORTED_MAX_LEVEL` is 0 unless `test-fixtures` is on (`root/crates/init-root/src/ledger.rs`). `--features live-sweep` compiles but has never run (WP21 Tests); since R-SAFE (`27dd505`) a build without `debug_assertions` that names it stops at a `compile_error!`, and the default build signals only the pids it holds (ADR-008, §9.1).

### 2.3 `seed/` workspace and `seed/ext`

| Crate | Role | Built by (commits) | Green |
|---|---|---|---|
| `init-body` | provider (DeepSeek, Anthropic, OpenAI), the loop and five tools, event log and context, boot and the awareness files (`GENESIS.md`, `SELF.md`, the constitution copy), the stdio-jsonl/1 front end, the Wasmtime 48.0.6 extension host | WP12 `e14f790`; WP10 `cc62009`, `f1f90ab`, `bceaf08`, `09f0ece`; WP11 `cb175fc`, `5454e29`; WP13 `1ca38e4`; WP14 `6949816`, `7872214`; FIX3 `0a5f043`; FIX8 `a595a9b` | 295 (271 unit, 16 `tests/boot.rs`, 8 `tests/self_md.rs`), 5 ignored (FIX8 Tests) |
| `seed/ext/count-lines` | the template T1 component (`wasm32-wasip2`) and the `ext-authoring` skill | WP0 `9a71966`; WP14 `6949816` | 3 host tests (BD §4.1) |

### 2.4 Testkit (`kernel/crates/init-testkit/`)

| Part | Built by (commits) |
|---|---|
| `init-mockllm`, the deterministic $0 model (DeepSeek SSE, strict mode, responders, cache simulation, recording) | WP2 `50e43e6` |
| `init-fakebody`, `init-fakekernel`, `init-fakeroot`, the scenario runner with Root steps, `INIT_FAULT`, the live/disposable guard; `wait_socket` and the 60 s `owner.sock` wait | WP3 `56856c3`; FIX6 `80afa41` |
| acceptance tests `tests/acceptance.rs` (6) and `tests/acceptance_root.rs` (1) | WP15 `9e4c2a1` |
| `init-measure` (`m1`, `m2`, `m4`, `m4-a3`, `m5`, `gen-steps`, `env`) | WP16 `fb5a5be` |

Green: 107 (WP3, FIX6; BD §4.1) + 16 `init-measure` (WP16 Tests) + 7 acceptance (WP15) = 130.

### 2.5 Scripts and harness

| Part | Built by (commits) | State |
|---|---|---|
| `scripts/slice1/` (`s1.sh`, `drive.py`, `zonecheck.py`, `collect.py`, `seedpack.py`) and `tests/slice1/run.sh` | S1 `951d623`, `a025abf`, `70ab3d8`, `16769b0`, `5aaa40b`; WP21b `0f4f46d`; FIX4 `d84a321` | `fakebody` and `mock` green on the real install path; `watchdog` and `realdry` pending disk there (FIX4); 4/4 on the old path and real 1/1 (S1) |
| `scenarios/`, `fixtures/` (A1–A6 scripts, charters, `semver-sort`, `fx-body`) | WP15 `9e4c2a1` | used by the acceptance tests |
| `ci/local.sh`, `ci/vendor.sh`, `ci/env.sh` | WP0 `9a71966`; constitution drift check `283e8a1`; `ci/env.sh` `9e374d2` | final run in §2.6 |
| `measurements/` | WP0 `9a71966` (`wp0-build-times.md`); WP16 `fb5a5be`; FIX8 `a595a9b` | §3 |

### 2.6 Test totals and flaky tests

- **Distinct tests, by latest status file:** kernel 682 + root 120 (103 + 17) + seed 295 + ext 3 = **1,100** green. This is this report's sum over §2.1–§2.4. BD §4.1 summed 1,079 green and 3 red at 06:55, before FIX7 fixed the 3 red promote tests and before WP16 and FIX8 added tests.
- **The final `ci/local.sh` run counted 1,137 passes** (MR): kernel 682 plus 455 for root, seed and ext. The 455 is 62 + 17 (root without features) + 103 (root with `test-fixtures`) + 270 (seed unit, 1 failed) + 3 (ext). The seed step stopped at the failing unit binary, so `tests/boot.rs` and `tests/self_md.rs` did not run in it (`build/ci/local-full-2026-10-20-final.log`). The two totals count different things, and neither is wrong.
- **Flaky tests.** `kernel-supervise --test handoff` `sv10` hung for 3 h 25 min after the Mac slept; it has no timeout, and it passed 3/3 alone in about 5 s (MR; BL 11:35). `init-body` `h21_incremental_cache_warm_vs_cold_recompile` compares two wall-clock compile times (310 vs 148 ms under load) and passed 3/3 alone (MR; BL 11:42). FIX9 fixed a third race, `wp21` `slice1_kernel_in_zone…`, which was a test race and is now 20/20 green (FIX9). The K-FLAKE lane owns bounded timeouts and 3× green suites (RP Lanes).
- Every status file reports `cargo clippy -- -D warnings` clean for its crates (BD §4.1).
- **After the release push (latest full run per workspace; the sum is this report's):**

  | Workspace | Passed | Source |
  |---|---|---|
  | kernel | 716, 0 failed, 5 ignored (51 binaries; three runs in a row: 704, 712, 716 as the tree moved) | K-FLAKE Tests, head `3008357` |
  | seed | 300 = 275 unit + 16 `tests/boot.rs` + 9 `tests/self_md.rs`, 5 ignored; 3 runs in a row; the same 300 in release mode and under the seed's own G0 | K-FLAKE; REL Tests; G0-SEED Tests |
  | root, `init-root --features test-fixtures` | 138 = 87 unit + 51 integration (fix5 1, owner_frames 3, rsafe 6, rsec 2, wp20 9, wp21 6, wp21b 2, wp22 8, wp23 4, wp24 6, wp25 4) | G0-ROOT Tests |
  | root, `init-sandbox-profiles` | 18 = 11 unit + 2 doc-drift + 4 params + 1 self-test run (240 pass / 0 fail / 11 skipped of 251 checks) | G0-ROOT Tests |
  | tui | 56 = 45 unit + 9 `proto` + 1 `tty` + 1 `e2e_root`, 1 ignored (TUI-13); green twice | WP26 Tests |
  | seed/ext | 3 | BD §4.1 |

  That is **1,231** distinct tests green, each counted once from its workspace's latest run. The kernel figure predates K-GATE's tests (`g1_child_paths_lie_inside_the_build_profile`, `promote_seatbelt.rs`) and G0-SEED's `seed_g0.rs`, which each passed in their own runs (K-GATE: `promote_seatbelt` 3 runs in a row; G0-SEED: G0 pass on `3610c17`) but no full kernel run has counted yet. The release-mode quality guard of `scripts/release.sh` counts root 105 (init-root 84 unit, rsafe's 3 source audits, sandbox-profiles 18; the fixture suites are cfg-gated off) and seed 300, 0 failed (REL).
- **Flaky tests after K-FLAKE.** `sv10` had two causes, both fixed: a test-side fd leak (macOS sets `FD_CLOEXEC` after `pipe()`, not atomically) and a product lost wakeup, the missing `HandoffDone` when B crashes while A retires (`5e50940`). `h21` now asserts cache entries, not milliseconds (`e379f4f`). Every process-spawning kernel test arms `init_testkit::watchdog` with a measured bound (`6728d78`, `cd8b8c2`); none fired in the 3 counted runs (K-FLAKE).

## 3. Measurements

### 3.1 M-1 … M-5 (spec §10 "Measurements to take before locking the stack", items 1–5)

| Id | Threshold (T §3.11) | Measured | Result | Source |
|---|---|---|---|---|
| **M-1** build time | cold build within supervise.md's 900 s timeout, warm within 300 s; raise risk R2 if cold > 10 min or the target dir > 4 GB | clean: seed 116.6 s, kernel 54.6 s, root 8.5 s (WP0 skeleton, n = 1); a cold sandboxed build of the real seed inside install: 209.9 s. `touch` rebuild, median of 5: seed 6.69 s, root 3.25 s, kernel 1.31 s. `seed/target` 1.7 GB | **pass** | P0 Results; `measurements/m1.json`; `wp0-build-times.md`; FIX4 |
| **M-2** T1 build time | warm median ≤ 5.0 s (criterion C1, ext/authoring.md §3.10) | warm `touch` rebuild: `count-lines` 0.34 s median / 0.81 s max; `semver-sort` 0.39 / 0.39 s. WP14's earlier run gave 380 ms. `.wasm` about 248 KB | **pass**; C1 does not trigger the fallback lane | P0; `measurements/m2.json`; WP14 |
| **M-3** agent writes a component (10 real-model trials) | ≥ 6/10 (ext/authoring.md §3.10) | not run | **pending** (budget) | P0; WP16 L |
| **M-4** prefix hash across a T2 restart | 5/5 identical, and the negative control differs | body level: 5 handoffs, 6/6 identical `prefix_sha256` and `prefix_hash`, simulated `cache_read` 4,032–4,992 tokens covers the ≈ 3,627-token head, and the `SELF.md` control changed both hashes. Kernel level (A3, 3 runs): 4/4 `turn` requests share one prefix across the promote and the rollback | **pass** | P0; `measurements/m4.json`; WP16 |
| **M-5** RSS over a 1,000-step session (socket mode) | excluding steps < 100: slope ≤ 2 KiB/step, and max ≤ 1.25 × RSS at step 100 | 18.5 MB at step 100 to 32.9 MB at step 1000; slope 15.2 KiB/step, max/base 1.78 (at 100 ms sampling: 14.9 and 1.91). After FIX8's fix, ps RSS: 8.11 KiB/step and 1.528, and 11.6–12.3 and 1.52–1.65 in 3 more trials | **FAIL as written** | P0; `measurements/m5.json`, `m5-100ms.json`, `m5-fix8-{before,after}.json`; FIX8 Tests |

M-4 handoff timings (no threshold beyond the timeouts): at body level, the window from pause to swap is 36 ms and candidate spawn → ready 34 ms. At kernel level (A3), G0 0.87 s, G1 0.93 s, G2 1.63 s, G3 5.98 s, handoff stage 1.52 s, promote total 12.4 s, asynchronous G4 6.9 s (P0).

**M-5 under the v0.1 rule: PASS, 3 of 3 trials** (lane M5, `3b02364`; `measurements/m5-v0.1.json`; §9.3). `init-measure m5 --trials 3` samples `phys_footprint` with the baseline at the last sample before the first mask or compaction (step 155), thresholds unchanged. Slopes −6.43, −5.02 and +0.53 KiB/step (≤ 2); max/base 1.141, 1.117 and 1.198 (≤ 1.25). The old rule (`ps` RSS from step 100) still fails all three runs, so the result rests on the provisional owner decision (§3.2).

**M-3: not run** (S-REAL; `measurements/real-v0.1.md`). It was deferred for budget. One A2-like trial (A2-real, which passed) cost $0.246 Root-metered on the input-price meter ($0.032 proxy), so 10 trials would be ≈ $2.5, over the batch's $1.00 cap. It stays pending, for a budget the owner sets.

### 3.2 M-5: FIX8's analysis and the provisional method

- **Not a leak.** The body's live heap is bounded, and every compaction frees it: the troughs stay at 1.4–2.5 MiB (FIX8 Cause). WP16 had suspected that the body keeps the whole event log in memory (WP16 N; BL 07:29). FIX8's heap measurements did not confirm that.
- **A real defect, fixed** (`a595a9b`). The view kept every masked tool output and unread per-response metadata until the next compaction, and each epoch briefly held two copies. Live heap peaked at 6.9 MiB before the fix and 5.0 MiB after it (FIX8 Cause 1, Done). Two regression tests fail on the old code and pass on the new (FIX8 Done).
- **A measurement artifact.** On macOS, `ps -o rss=` counts pages that libmalloc has freed but left resident. At step 860 after the fix, ps RSS was 29.7 MB while the physical footprint was 12.0 MB (FIX8 Cause 2).
- **A baseline in the fill phase.** At step 100 the context holds about 55k of the 114.7k-token compaction trigger, and the first epoch is at step 155, so "max ≤ 1.25 × RSS at step 100" compares a half-full context with a full one (FIX8 Cause 3; WP16 N).
- **Provisional owner decision 2026-10-20 (owner may override):** sample `phys_footprint` on macOS and take the baseline at the first context epoch; both thresholds stay at 2 KiB/step and 1.25× (RP). Under that rule, in FIX8's three manual footprint trials, the fixed body passes 3 of 3 (slope −3.0 to 1.98 KiB/step, ratio 1.02–1.13) and the old body fails 2 of 3, so the rule still discriminates (FIX8 Tests). One trial was at 1.98 against 2. Boxing `MessageEv` would cut about 40% more, and the M5 lane owns it (FIX8 D3; RP Lanes).
- `init-measure m5` now implements the footprint sampler and the first-epoch baseline, and M-5 has a tool-produced PASS (M5, `3b02364`; §9.3). The `MessageEv` boxing landed in the same commit.

### 3.3 Not measured yet

- M-1: cold builds in empty target dirs (n = 3), the semantic-edit, `check`, `clippy` and `test --no-run` variants, and the gate-path step from M5 on (P0 Details; WP16 L).
- M-2: cold builds in an empty target dir, and the latency of the first `ext` call through a live body (WP16 L).
- M-4: the proxy's `last_head_hash` at body level (socket mode has no proxy; the A3 path covers it), and the real variant E2E-3 (P0 Details; T §3.11).
- M-5: the full-stack variant and the variant that reloads a T1 component every 100 steps (P0 Details; FIX8 L).
- Selfmap S-12 and lineage RR-16 in release builds; H16 on CI hardware (BD §4.3).

### 3.4 Notable measurements (shortened from BD §4.2)

| What | Value | Source |
|---|---|---|
| Extension instantiate | p50 31 µs, p99 154 µs (300 runs) | WP14 (BL 05:41 says p99 166 µs; see §10.3) |
| Whole `ext` call | p50 385–397 µs | WP14 |
| Log `sync_data` (F_FULLFSYNC) | p50 2.81 ms, p99 3.64 ms (threshold 50 ms) | WP12 |
| Log crash durability | 500 SIGKILLs mid-append, 0 confirmed events lost | WP12 |
| Sandbox self-test | 222 pass / 0 fail / 11 skipped (disposable-only) of 233 | FIX1 |
| Loop | interrupt to idle < 100 ms; tool group dead < 200 ms | WP11 |
| Root start → ready (debug) | ≈ 25 s (S1) and 9.8 s (WP21b) → 1.5–1.7 s with sha2 at opt-level 3 | S1; WP21b; FIX4 |
| Kernel start-up | `run/kernel.sock` accepting at +10 ms, serving at +110 ms, body ready ≈ +390 ms | FIX6 |
| Cold seed install (install path, mock) | 209.9 s; peak transient disk 1,573 MB | FIX4 |
| Gate-of-gates with the real judge | 92–99 s on the 12-task stand-in S at 8 + 4; `pass:true` | BD §4.2; FIX5 |
| D14 full-rule simulation | false accepts unchanged in 28 of 30 cells; the pool-only cheat accepted 0% | FIX2; BD §3.3 |
| G-21 (G4 power) | n = 16, R = 2: 83.8% accept at 0 / 8.2% at −2δ; only n = 30, R = 3 meets the targets (91.7% / 6.5%) | WP8; BD §3.2 |
| Slice-1 real (deepseek-flash) | owner line → idle 2.39 s; Root 2,844 µUSD vs proxy 762 µUSD (3.7×); turn-2 cache 4,480 hit / 190 miss (959‰) | S1; BD §3.13 |
| Slice-1 owner line → idle ($0) | fakebody 331 ms, mock 351 ms | P0 |
| Acceptance wall time after FIX6 | A1 11.5 s, A2 43.8 s, A3 24.2 s, A4 19.8 s, A6 15 s | FIX6 |

## 4. Acceptance status

### 4.1 A1–A6 and NO-OWNER-1 (definitions: T §3.9, §3.17; results: WP15, `9e4c2a1`)

| Id | $0 variant | Real variant | State | What remains, and why |
|---|---|---|---|---|
| A1 coding task | pass: real kernel + real body under `init-fakeroot`, `--strict deepseek` mock, 10–13 s | not run | **partial** | A1-real, ≥ 2 of 3 runs (E2E-4): budget |
| A2 T1 extension, head unchanged | pass: the agent writes, builds, reloads and calls `semver-sort`; prefix and `tools` constant over 6 requests | not run | **partial** | A2b (keep and promote t1): disk, because it needs a G0 build of the real seed's `ext/` workspace. A2-real (E2E-5): budget. The REPL variant A2-repl (T §3.9) was not run. |
| A3 T2 self-edit → handoff → same conversation | pass: G0–G3, handoff on the same session file, asynchronous G4 `reject: no_gain`, rollback at idle | not run | **partial** | Runs on WP8b's fixture body, not the real seed, because a G0 build of the real seed per candidate did not fit the disk (WP15 D4). The G4 accept branch is not exercised in a scenario (`check_g4` bug, BD §2 #31). A3-real: budget. |
| A4 boot-crash candidate | pass: G2 fails, the live body runs on, the fix is promoted, exactly 2 body spawns | not run | **partial** | Fixture body, as A3. A4-real: budget. |
| A5 crash loop → demotion, safe mode, self-clear | A5-fake (kernel half) pass under `init-fakeroot` | not run | **partial** | The Root half runs only as WP21's `crash_loop_safe_mode_halt_retry_and_self_clear` against the real Root, not through `start_root`. A5-halt and A5-real: disposable environment, because they need the Root's rollback/sweep path and an external SIGKILL (WP15 L). |
| A6 invariants enforced | live half pass on the real `init-root`: 10 refusals, 3 controls, protected trees unchanged, `audit verify --full` ok | not run | **partial** | Steps 5–6 (G0 refuses `unsafe` and a missing `forbid`): disk. Step 10 (`kill -STOP` of the Root): the owner rule allows no kill on the live machine. The sandbox half is WP6a's self-test (222/0/11 since FIX1), whose 11 skips are disposable-only. |
| NO-OWNER-1 | pass: no scenario file sends an owner signal or removed command, and every scenario above passes without an owner step | — | **partial** | T §3.17 also lists the F-* faults, RT-W1, RT-W4, SA-1 and SA-2 under the runner's guard. Those did not run under the guard; RT-W* need the disposable environment. |

MR's "What is green" table marks "Acceptance A1–A6 + NO-OWNER-1 ($0)" as done, with all passing. That holds for the $0 scope WP15 defined (A5-fake, the A6 live half), not for the full T §3.9 and §3.17 definitions.

**Real-model variants (S-REAL, deepseek-flash, full stack on the real install path; `measurements/real-v0.1.md`):**
- **A1-real pass:** 7 turns, `python3 -m unittest -q` OK, re-checked outside the sandbox.
- **A2-real pass**, with relaxed matching: the agent wrote, tested, built and reloaded `semver-sort`, the prefix hash was unchanged, and the output is a valid SemVer order; the fixture's tie order differs and SemVer leaves that order free.
- **A3-real pass:** the agent's own T2 self-edit passed G0–G3, including model-mode G3; the handoff kept the same session, and the new body named its new commit; G4 made 13 real paired runs and ended `inconclusive: underpowered`, as designed for the 2/1/1 stand-in pool.
- **E2E-2 pass** 8/8, on 4 homes.
- **E2E-3 pass:** a no-op T2 handoff; the first request after the boot hit 5,888 of 6,696 tokens, ≥ 0.9 × head.
- **E2E-1:** boot half pass. The quick gate passed G0 live, then hit `gate.g1.self_json_missing`, because the seed has no committed `self.json` (S-REAL N). The G2 round trips are shown by E2E-3's and A3's gates.
- E2E-4 (A1 × 3) ran once. E2E-5 is the A2-real and A3-real runs above.

### 4.2 Root scenarios at $0

The Root's integration tests (`init-root --features test-fixtures`: wp20–wp25 and fix5, 38 tests) run on the live machine with the in-test fake kernel or the mock, and signal only their own children (WP21, WP24, WP25 Tests). They cover, among others, MS-1…MS-7, MS-C1…C3 and MS-G4 (WP24), RT-L1…RT-L7 and RT-L9 on test ledgers (WP25), RT-E1 through the real mock in strict mode (BL 03:08), and the crash-loop/safe-mode Root path (WP21). Still not run, all disposable-only: SW-1…SW-5 (SW-6 is a refusal and runs live), the full RT-P2 and RT-P5 with `live-sweep`, SV-16, S-M40/41/47a/47b and S-Z17a–f, RR-6/7/11–13 (BD §4.3).

**Owner-machine mode (R-SAFE, §9.1).** Since ADR-008 the real `init-root` runs on the live account when it is built without `live-sweep`, because it then signals only the pids it holds. Run live: RT-P1, P3, P4, P6 (non-launchd), P7, P9 and P11 (wp21); RT-P2 as built, the latch-only fallback (wp20); RT-P10, the mirror ref (wp23 `rr19`); RT-C1…C4, C7…C10, RT-E1, E2 and E5 (wp22, wp21, kernel-proxy); SW-6; and the owner-machine variants SW-1o…SW-5o, SW-7o and SW-8o (tests/rsafe.rs, 6/6). Still disposable-only: SW-1…SW-5 as written (they prove the live D1 *kill*), RT-P5's escapee kill, RT-P2 as specified, and RT-P6 with a real LaunchAgent (R-SAFE "Task 3"). R-DEC added RT-L10 (three demotions at L0 in 24 h enter safe mode) and the wp24 small-S test; R-SEC added `tests/rsec.rs` (2). All Root suites pass: 138 with `test-fixtures` (§2.6).

### 4.3 Slice-1 harness cases and real-model checks

| Check | State | Source |
|---|---|---|
| `s1.sh fakebody` on the install path | pass | FIX4 |
| `s1.sh mock` on the install path | pass | FIX4 |
| `watchdog`, `realdry` on the install path | open: disk (about 7.1 GB free needed per cold case); both passed on the old path | FIX4; S1; BD §2 #11 |
| `s1.sh real` (deepseek-flash, production egress) | pass, 1 of 1 attempts, on the old path | S1 `5aaa40b` |
| V-DS-1…6, E2E-1…5, T-PROV-1…6, live T-AW-9/10, A1–A6 real, M-3 | open: budget | BD §3.17; BD §4.3 |

Under RP, the real-model batch is deepseek-flash only, with a hard cap of $1.00 Root-metered and no new checks after $0.50 (RP §3.17 row). T §3.10 assigns deepseek-v4-pro to M-3 and the A-real runs, so a flash-only batch runs those checks on a different model than testing.md assumes.

## 5. Deviations

### 5.1 Totals (BD §1, §5)

- **BD §1: 149 rows.** 117 adopt (103 applied in docs-sync `986669f`, 14 still open, all new in v0.2), 19 fix-code (one already fixed in code by FIX6, `80afa41`), 13 decide (BD totals line).
- **BD §5: 10 docs-sync leftovers, counted separately.** 6 adopt (L1, L2, L3, L6, L7, L8) and 4 fix-code (L4, L5, L9, L10).
- **Applied since.** The DOCS lane applied the 6 §5 adopt rows on 2026-10-20 (L1, L2, L3, L6, L7, L8): body/boot.md §3.2 and ext/wit.md §3.4 (L1), kernel/supervise.md §3.15 step 5 (L2), kernel/config.md §3.2 (L3), and testing.md §2, §3.14, §9 and §3.18, plus root.md §8 (L6–L8), committed as `50c553e`. The 4 fix-code rows went to the code lanes; BD §5 assigns L4 and L5 to R, L9 to the orchestrator and L10 to B.
- **Fixed in the release push (BD v0.3).** Of BD §1's 19 fix-code rows, R-DEC fixed 6 in Root code (WP21 D9's Root half, WP21 D10/D12, WP23 D3, WP24 N in `518e68e`; WP24 D6 and D8 in `47c618f` and `518e68e`), FIX6 had fixed 1 (`80afa41`), WP24 D7 is tagged v0.1.x (a design note in R-SAFE), and 11 stay open. BD §5 L4 (`47c618f`) and L5 (`7a460a6`) are fixed; L9 and L10 stay open. The decide rows behind §3.9, §3.13 and §3.15 are built (R-DEC). The review's F1–F7 and the live blockers are in BD §6. BD §1's 14 open adopt rows were not re-checked.

### 5.2 Open cross-lane gaps (BD §2, updated by later status files)

| # | Gap | State |
|---|---|---|
| 1 | G0 not sandboxed | open: G0 runs as a direct child of the kernel inside the kernel profile. Since `d05c1e3` that profile's section 6a lets G0's children use Unix sockets under the build slot's `OUT` only, a temporary rule until G0 runs in `build` instances (G0-ROOT D1) |
| 2 | Sandboxed fetch relay | open: the fetch uses plain `git-upload-pack` |
| 3 | L1 mechanics | open (M7) |
| 4 | L2 mechanics (fleet) | open (M9) |
| 5 | L3 mechanics (kernel upgrades) | open (M10) |
| 6 | Root `proposal.file`, `sealed.*` on the link | partly closed (FIX5 `54fb956`) |
| 7 | Linux sandbox (WP6b) | open (M7) |
| 8 | Start-up hashing | closed for the Root (FIX4 `d84a321`); off the kernel's start-up path (FIX6 `80afa41`); `init-fakeroot` still spends about 10 s hashing |
| 9 | Signal refresh | open |
| 10 | Ext host in body boot | closed (FIX3 `0a5f043`) |
| 11 | Install path end to end | mostly closed (FIX4); `scripts/install.sh` → `init` → one mock turn → `uninstall.sh` passes end to end at $0 (REL `48bb3ba`); `watchdog`/`realdry` on it pending disk; `install --upgrade` not built |
| 12 | D14 floor | closed (FIX2 `35647a8`, `194ba30`) |
| 13 | Gate judge breadth | open |
| 14 | G4 extras | open |
| 15 | Proxy rest | open |
| 16 | Anthropic and OpenAI context renderers | open |
| 17 | Mock strict checks for Anthropic and OpenAI | open |
| 18 | Root R1 rest (quotas, pruning, `status --selftest`) | open |
| 19 | Root R7 rest | open |
| 20 | Sealed R6 rest (real S) | open (M7); the cumulative sealed reserve and the synced `start` line are built (R-DEC `47c618f`); sealed spawns in the broker's table are v0.1.x (R-SAFE design note) |
| 21 | Supervise rest | open |
| 22 | Config loader | open |
| 23 | Body facts from the kernel | open (fallback since FIX3) |
| 24 | Early initctl | closed (FIX6 `80afa41`) |
| 25 | Measurements | M-1, M-2, M-4, M-5 measured since (WP16 `fb5a5be`); M-5 passes 3/3 under the provisional rule (M5 `3b02364`); M-3 open |
| 26 | CI (`.github/workflows`, `rootloc`) | partly closed: `fast-linux.yml` committed (REL `b461f1f`), not yet run on GitHub; `ci/local.sh` gained the `seed G0`, `promote under Seatbelt` and `tui` steps (`3610c17`, `900feef`, `367542b`); `rootloc` not built (WP17) |
| 27 | Acceptance A1–A6, NO-OWNER-1 | closed for the $0 scope (WP15); the rest open (§4.1) |
| 28 | Live provider checks | open (S-REAL lane) |
| 29 | Disposable-environment half | open; not required for v0.1 under owner-machine mode (ADR-008, R-SAFE `27dd505`); still required before `live-sweep` is ever enabled |
| 30 | `init-kernel install --template` | open |
| 31 | Testkit runner | partly closed (FIX6); `check_g4` and the fake Root's `CONSTITUTION.md` open |
| 32 | S size floor for D14 | closed: `sealed import` refuses S < 12 plus a canary, and a gog on a smaller S records `underpowered` with no demotion (R-DEC `518e68e`, provisional decision §3.15) |
| 33 | Promote tests red under the holdout rule | closed by FIX7 (`37aef0c`); BD v0.2 shows it in progress |

## 6. Decisions taken

### 6.1 Provisional owner decisions (RP, taken 12:16)

Every row below is a **provisional owner decision 2026-10-20 (owner may override)**. The orchestrator took them because the owner said not to wait (RP; BL 12:18).

| Decision (BD §3) | Taken as (RP) | Carried out by (RP Lanes) |
|---|---|---|
| §3.1 Root size | per-module sub-budget = measured + 5%, rounded up to 10; ≈ 7,500 total (7,500 at `dd5c358`, 7,540 at `cdb49e8`); re-measured at release; core and `sandbox-profiles` on their own lines (`d4807ca`; first written as "≈ 7,000" in `9055953`) | DOCS (root.md §4.8, spec); `rootloc` is not built (WP17) |
| §3.2 G4 power | n = 30, R = 3 plus one A/A calibration per model, as the default once S has ≥ 30 tasks (M7); v0.1 keeps the 12-task stand-in S | DOCS (gate.md §3.8.3, §3.8.5; roadmap M7; `50c553e`, `781504b`). No v0.1 code change follows from it. |
| §3.4 Constitution | built as written; stays PENDING OWNER | DOCS (CONVENTIONS §6 OW-1; `781504b`) |
| M-5 method (FIX8) | `phys_footprint` on macOS, baseline at the first context epoch, thresholds unchanged | M5 |
| §3.9 Repeated demotion | 3 demotions at L0 within 24 h trigger system safe mode | R-DEC |
| §3.13 Root money meter | bill cache hits at the provider's cache-read price; keep the peak window | R-DEC |
| §3.15 Small S | S < 12 tasks gives `underpowered`, no demotion, plus an alert | R-DEC |
| §3.17 Real-model batch | deepseek-flash only; hard cap $1.00 Root-metered; no new checks after $0.50 Root-metered | S-REAL |
| Disposable account | not required for v0.1, under owner-machine mode (RP item 4, ADR-008) | R-SAFE |

### 6.2 Root budget

- **History.** The design figure is about 1,000–1,500 lines, core ≤ 995 (spec §13; root.md §4.8 v0.3). The orchestrator raised it to ≈ 5,000 provisionally at 02:35 (BL 02:35). WP24's count was ≈ 6,450 (BD §3.1). RP's first provisional value, ≈ 7,000 (`9055953`), was set from that count.
- **Re-measure (DOCS lane).** By root.md §4.8's method (non-blank, non-comment lines, excluding `#[cfg(test)]` items, `tests/` and `testfix.rs`), `init-root` at `dd5c358` is **7,053 lines**. The core subtotal (zone, sup, panic, egress money, lineage verify) is **1,650** against a budget of **1,760**. The DOCS lane last re-measured at `cdb49e8`, after R-DEC's money-meter commit. That count gives **7,085** lines, core **1,656** of **1,770**, and a table total of **7,540**. `sandbox-profiles` is unchanged. `root/sandbox-profiles` is counted on its own line: **821** profile lines (budget 870), plus a **1,454-line** self-test runner (budget 1,530). The per-module rows are in root.md §4.8 (`50c553e`). The same figures are in spec §13, RR-34, egress.md §6, sealed-eval.md §4.2, CONVENTIONS §4 and §6, and roadmap M6 and R13.
- **Rule applied.** 7,053 exceeds the ≈ 7,000 figure, so the table applies the decision's rule, measured size plus about 5% per module, and the per-module sub-budgets total **7,500** (`d4807ca`; root.md §4.8).
- **Flagged for the owner.** 7,500 is five times the design figure and above RP's first value. R-SAFE and R-DEC are still adding Root code for v0.1.0, so the count is re-measured at release (root.md §4.8). Roadmap R13's fallback is to move anything that is not control back to the kernel (RM §3.10).

### 6.3 Owner decisions of v0.3 (CV §6)

- The owner chose a minimal immutable Root, with the agent autonomous for everything else; human review is removed (ADR-007).
- ESC-1: no approval for crates. ESC-2: `/opt/homebrew` readable. Both are L0 rights.
- E1/ET-E1: the ceilings move into the charter.
- Charter template: `max_level = 4`; `/panic` demotes one level (`stop` does not); the lineage mirror goes to the private repo `INIT-Lineage`.
- Settled by the main session on 2026-10-19: probe-mode boots only make the model round trip, the G4 limit is 90 min, M0 needs ≥ 35 GB free, and the body defaults listed in CV §6.

### 6.4 Still pending

| Item | State | Source |
|---|---|---|
| OW-1 constitution text (awareness.md §3.3, 649 words) | PENDING OWNER; built as written | CV §6; MR decision 3; RP |
| OW-2 genesis prompt | listed as pending in CV §6; RM §3.9.1 says v0.3 drops the decision | CV §6; RM §3.9.1, §9 |
| E1/ET-E1 money | listed as pending in CV §6; RM §3.9.1 says D13 settled it as charter ceilings | CV §6; RM §9 |
| ET-E2 G4 model, ET-E4 CI hardware | CV §6 pending; RM calls ET-E2 a maintainer default; ET-E4 decided at M7 | CV §6; RM §3.9.1 |
| D14 ratification (both parts) | built and in the docs (`986669f`); the owner is asked to ratify | BD §3.3; MR decision 5 |
| BD §3.5 mirror URL `""` until M0; §3.6 seed release script; §3.7 batch thinning; §3.8 Purpose block in the head; §3.10 `owner` lineage kind; §3.11 drop proxy cost reconciliation; §3.12 build-profile items; §3.16 disk | recommendations only; none is in RP's table | BD §3 |
| M0 owner inputs: write the charter, create `INIT-Lineage`, `init-root keys set deepseek`, disk ≥ 35 GB, the disposable account | not done (the disposable account is not required for v0.1 under RP) | MR milestone table; RP |

CV §6 is inconsistent with itself: its v0.3 paragraph settles ESC-1 and ESC-2, but its Pending table still lists them, and body/boot.md §3.2 still marks ESC-2 "PENDING OWNER". RM §9 already flags the Pending table as stale for OW-2, E1/ET-E1, ET-E2 and ET-E4.

## 7. Open risks

| Risk | Evidence now | Mitigation or owner |
|---|---|---|
| A test of a sweep or kill rule damages the live system (RM R18). On 2026-10-20 at 01:20 a sandbox sweep test SIGKILLed 33 of the owner's processes, Docker Desktop among them (CV §1; MR Incidents). | Fixed in policy and code: the D1 three-condition sweep rule; sweep tests disposable-only; `live-sweep` off and never run (WP21 Tests); panic kills only held processes (BD §1.3, WP21 D1–D2). SW-1…SW-5 have not run, because they are disposable-only (BD §4.3). Since R-SAFE (`27dd505`): `sys::signal` refuses any pid that is not an unreaped child, `live-sweep` cannot be compiled into a release build, and the sweep dry-run over the live table selected exactly the test's instances in 3 runs (448–475 processes, 13–14 incident-class sandboxes rejected by (c)). Residual: a `setsid` escapee survives a panic and is reported, never killed (ADR-008). | Done for v0.1 (R-SAFE, §9.1). The disposable account and SW-1…SW-6 before `live-sweep` is ever enabled (ADR-008 "Plan to enable live-sweep later"). The owner restarts Docker Desktop (MR). |
| The Root outgrows its budget (RM R13) | 7,085 lines at `cdb49e8` against 1,000–1,500 (§6.2) | Provisional budget ≈ 7,500; `rootloc` in `ci/local.sh` (WP17, not built); owner review |
| The sandbox is the whole boundary, and the Root runs as the owner's user (RM R17) | self-test 240/0/11 of 251 since G0-ROOT (`d05c1e3`; 222/0/11 at FIX1), the 11 skips disposable-only; A6 live half passes (WP15); the security review found 0 critical and 1 high; all 7 findings are addressed, F5 by detection and report only, which ADR-008 keeps as a residual risk (BD §6) | Accepted residual risk (RM R17); keep the sandbox suite green |
| G0 runs unsandboxed, and cargo reads `~/.cargo/config.toml` from the build slot's parents | BD §2 #1; BD §1.6 (WP8 N, fix-code) | Root/X (broker-side G0 runner) + K1 (BD §2 #1) |
| G4 statistical power at Phase 0 sizes (spec §11 hypothesis 4; RM R11, H4) | 83.8% / 8.2% at n = 16, R = 2, against ≥ 90% / ≤ 7% (BD §3.2) | n = 30, R = 3 with A/A calibration at M7 (provisional, §6.1) |
| Too small a sealed set S (RM R16) | a leak drop to 11 tasks makes every gog fail; R7 demotes on every `mutation{pass:false}`, even for a maintainer candidate (BD §2 #32; BD §1.3 WP24 N) | Built (R-DEC `518e68e`): `underpowered` with no demotion (provisional §3.15), and R7 demotes only for `source == "agent"` |
| The Root money meter over-bills | Root 3.7× the proxy, so the $5/day ceiling stops at about $1.35 of real spend (BD §3.13) | Built (R-DEC `cdb49e8`): cache hits at the cache-read price (provisional §3.13); S1's usage now 2.00× the proxy, the peak/off-peak factor kept on purpose. S-REAL's batch ran on a Root built before it, so the ratio on real traffic is a v0.1.x re-check (RP) |
| Repeated L0 demotion becomes a restart loop | WP25 D6; BD §3.9 | Built (R-DEC `518e68e`, wp25 `rt_l10`): 3 demotions at L0 in 24 h → system safe mode (provisional) |
| A gate keeps its routable set after a demotion | it can send L1 models, which the Root refuses, which demotes again; RT-L8 not built (WP5 D11; BD §1.8) | fix-code, X (WP5) |
| M-5 margin and body memory | not a leak; M-5 passes 3/3 under the v0.1 rule, but the ratio's margin is thin (worst 1.198 against 1.25), with peaks at compaction (M5 D2) | v0.1.x: drop the loop's held messages, the provider's second render and `client.rs`'s request copy (RP v0.1.x) |
| Flaky tests | fixed (K-FLAKE): `sv10`'s fd-leak race and lost `HandoffDone` (`5e50940`), `h21` on cache counts (`e379f4f`), a watchdog in every process-spawning kernel test (`6728d78`); kernel and seed 3× green | `ci/local.sh` 3× at the release check (orchestrator) |
| Release-blocking bugs only a real install shows | the real batch hit `gate.g0.forbid_missing`, G0 `test` failures under the sandbox and G1's `manifest_error` (BD §6); no $0 test ran the gate under the real profiles | `seed G0` and `promote under Seatbelt` steps in `ci/local.sh` (G0-SEED `3610c17`, K-GATE `900feef`) |
| Disk (spec §11; RM R6) | 6.1–8.4 GB free through the night against the 20 GB floor; several $0 runs were held for disk (BD §3.16) | The owner frees disk; `install --template` (BD §2 #30) makes later cases clones |
| How well models write Wasm components is unmeasured (spec §11; RM R1) | M-3 not run (P0) | real-model trials (S-REAL, within the cap); fallback lane per ext/authoring.md §3.10 |
| Weak models get worse with self-modification (spec §11; RM R3) | one real run, on deepseek-flash only (S1) | S-REAL batch; frontier-model run later, inside the ceilings |
| No head-to-head study of restart vs in-process change (spec §11; RM R5) | 36 ms body-level handoff window, A3 and A4 at $0 lose no conversation (P0; WP15) | accepted residual risk, recorded in metrics (RM R5) |
| Novel hypotheses (spec §11, H1–H5) | H3 and H5 shown at $0 (A3, A4, M-4); H1 needs A2b; H2 (thinning) not tested; H4 as above | M5 (A2b), M7 (H2, H4) per RM §3.10 |
| The DeepSeek key appeared in chat | stored only in its key file, never printed, logged or committed; 0 occurrences in 218 artifacts (MR; S1) | The owner rotates the key (MR) |
| The owner's email reached crates.io User-Agent headers (four doc-research agents) | fixed by the CV §1 privacy rule; no later agent repeated it (MR Incidents) | CV §1 rule, binding on every lane |
| `ci/local.sh` looked for `kernel/target/debug/init-mockllm`, which did not exist, so the root step could take its "missing" branch | FIX9 N; since `9e374d2` every lane uses the shared default target dirs, and the release-push runs pass `INIT_MOCKLLM_BIN=kernel/target/debug/init-mockllm` (R-DEC, G0-ROOT Tests); not re-checked inside `ci/local.sh` | orchestrator (the `ci/local.sh` 3× release run) |

## 8. v0.1.0 scope

**Included** (RP "What v0.1.0 means"):
1. Install with one command, `scripts/install.sh`, then `init-root keys set deepseek` by the owner. The script never touches the key.
2. Chat: the `init` TUI against the running Root, on real DeepSeek, showing streaming text, tool calls, cost and level.
3. Self-evolution at L0 on the real model: a real T1 promotion (A2-real) and a real T2 promotion with the rollback path (A3-real).
4. Safety on the owner's live account (owner-machine mode): stop, `/panic`, ceilings and rollback verified; signals only to Root-held PIDs; `live-sweep` compiled out; a sweep dry-run over the real process table that sends no signal.
5. Green CI: `ci/local.sh` green 3 times in a row with no flakes, and the GitHub `fast-linux` job green.
6. Docs: README quickstart, CHANGELOG, and the tag `v0.1.0`.

**Excluded** (not in RP's definition of done; open gaps in BD §2):
- L1+ mechanics: task admission, `budget set`, `model set`, predictions (gap #3). Production builds cap at L0 (§2.2).
- The fleet, L2 (gap #4), and kernel self-upgrades, L3 (gap #5).
- The Linux sandbox (gap #7, WP6b, M7). Without it the zone launcher errors on Linux (BD §2 #7); macOS on Apple Silicon is the primary platform (CV §5).
- A real sealed S and full R6 (gap #20, M7). v0.1 keeps the 12-task stand-in S (RP §3.2 row).
- G4 at n = 30, R = 3 (RP §3.2 row), sandboxed G0 (gap #1) and the sandboxed fetch relay (gap #2).
- `live-sweep` and every disposable-environment test (RP item 4, rule list). RP still requires them before `live-sweep` is ever enabled (ADR-008).
- Anthropic and OpenAI context renderers and their strict mock checks (gaps #16, #17), even though the adapters are built (WP10).
- The lineage mirror stays off (`url = ""`) until M0 (BD §3.5).

**State of the included items at `baab414` (16:15).**

| Item | State | Source |
|---|---|---|
| 1. Install | built: `scripts/install.sh`, `release.sh`, `uninstall.sh`, the `init` launcher; the $0 end-to-end run passes (install, `init` → `init-tui`, one mock turn, uninstall to the Trash). The owner's real install is the owner's step. | REL; §9.4 |
| 2. Chat | built: `init-tui`, tested against the real `init-root` in test mode (TUI-R1…R3); the run on real DeepSeek is S-REAL's | WP26; §9.5 |
| 3. Real L0 self-evolution | A2-real passes; A3-real (and E2E-3) pending | S-REAL; §9.2 |
| 4. Owner-machine safety | built and verified live; residual risk: a `setsid` escapee is reported, not killed | R-SAFE; ADR-008; §9.1 |
| 5. Green CI | kernel and seed suites 3× green; `ci/local.sh` 3× and the first `fast-linux` run pending | K-FLAKE; REL N |
| 6. Docs and tag | README, CHANGELOG and docs/release.md committed; the tag waits for a clean re-cut at the tagged commit | REL; docs/release.md §2 |

## 9. Release-push lane results

All lanes started at 12:20 (RP Lanes; BL 12:20). §9.1 and §9.3–§9.5 are the lanes' results from their status files; §9.2 (S-REAL) is still a placeholder.

### 9.1 R-SAFE: owner-machine mode (ADR-008)

**Done** (R-SAFE; commits `27dd505`, `03f1e16`, `7a460a6`; docs `af10367`, `3ebbddc`; status `d3a559a`).
- **Held pids only.** `sys::signal`, the only `kill`/`killpg` in a default build, checks `sys::held(pid)` itself: `waitid(P_PID, pid, WEXITED|WNOHANG|WNOWAIT)` must succeed, so only an unreaped child (running, stopped or a zombie) is signalled, and the check never reaps. Broker kills, the reaper, panic, stop, the kernel-crash paths, the resume rollback and sealed runs all go through it. Cold start never signals a recorded pid.
- **`live-sweep` compiled out.** It is in no default or implied feature; a build without `debug_assertions` that names it stops at a `compile_error!`; and even a `live-sweep` build scans only where the root-owned disposable sentinel exists (testing.md §3.1a).
- **Sweep dry-run.** `init-root doctor --sweep-dry-run` applies D1 to our uid's real process table and reports the would-kill set with `signals_sent: 0`. Live, three runs over 448–475 processes selected exactly the test's own instances; rejects were 236–267 for (a), 194–199 for (b), 13–14 for (c) (the 2026-10-20 incident class), 0 "cannot tell" (ADR-008).
- **Review F5.** When the zone empties and at cold start, D1 runs on the zone mark, held pids excluded; a match on two passes 300 ms apart gets an `escapee` lineage record and an owner alert, and is never signalled. The panic result says `sweep:"held_only"`.
- **Follow-ups.** BD §5 L5: `resume` is refused while the mirror's `refs/heads/panic` exists (`7a460a6`, wp23 `rr19`). R-SEC's need: the daemon raises its soft `RLIMIT_NOFILE` to min(hard, 4096) and logs it (`7a460a6`). WP24 D7 (sealed spawns in the broker table) is a design note, tagged v0.1.x: the quota already reserves `sealed.parallel`, so the gap is bookkeeping, not safety.
- **Tests.** tests/rsafe.rs 6/6 with `test-fixtures` (3 source audits, SW-8o, plus 3 live tests covering SW-1o…SW-5o and SW-7o) and 3/3 without; wp20, wp21, wp21b, wp22, wp23, fix5 green, and wp24 and wp25 green on re-run in a build slot. The live-run and disposable-only lists are in §4.2.
- **Residual risk (ADR-008).** A zone process that calls `setsid()` survives a panic until `live-sweep` is enabled on a verified build. It keeps the zone's Seatbelt policy, its egress socket closes at panic, and it is reported by pid.

### 9.1a R-DEC and R-SEC: the provisional Root decisions and the review fixes

- **R-DEC** (`cdb49e8`, `518e68e`, `47c618f`, `3b08245`; docs `d524407`; status `29bb1ac`): §3.13 cache-read billing (S1's usage: Root/proxy 3.73× → 2.00×); §3.15 small S (`sealed import` refuses S < 12 plus a canary; a smaller S gives `underpowered`, no demotion, alert `gog_underpowered`); §3.9 (the third demotion at L0 in 24 h enters system safe mode, alert `demotion_loop`); review F1 (saturating admission, `max_out` clamped at 2^20 tokens), F4 (input held at one token per request byte), F6 (fd stash ≤ 64 for ≤ 10 s), and R-SEC's F3 bound on sealed lanes (32 connections); the Root fix-code rows of BD §1.3–§1.5 and §5 L4 (BD v0.3 marks each). `init-root --features test-fixtures`: 136 passed, 0 failed.
- **R-SEC** (`e0e0834`): F2 default-deny metering (explicit `(provider, method, path)` rows; anything else `class.unmetered`, never forwarded); F3 at most 96 egress connections, 503 `root.busy` above, timed reads and writes; F7 `/_root/health` and `/_root/ceilings` behind the in-flight guard. `tests/rsec.rs` 2/2.

### 9.2 S-REAL

**Done** (S-REAL; `measurements/real-v0.1.md`; artifacts `build/real-artifacts/real/`). The batch ran on deepseek-flash through `tests/real/real.sh`: every request went owner → init-root → init-kernel → init-body → kernel proxy → Root egress → api.deepseek.com, on homes built by the real install path.
- **Spend.** Root-metered (the cap's input-price meter) **$0.529** of the $1.00 hard cap, carried across 6 homes by charter caps of $1.00 minus the earlier spend. **Real money: $0.21** (balance $10.62 → $10.41, 13:34–17:06). Key-leak grep: 0 occurrences of the key in 38,211 files, and 0 `sk-` strings in committed paths or git history.
- **Passed live:**
  - V-DS-1, 3, 4, 5, 6. V-DS-2 contradicts provider.md: DeepSeek accepts its own tool turns without `reasoning_content`, and refuses only foreign ones.
  - A1-real; E2E-2 (8/8, ×4); A2-real.
  - **E2E-3**: real promote, G0–G3, handoff, cache 5,888/6,696 after the boot.
  - **A3-real**: the agent's T2 self-edit through G0–G3 (model-mode G3), handoff on the same session, then G4's 13 real paired runs → `inconclusive: underpowered`, as designed.
  - E2E-1's boot half, and `gate --quick` through G0 (f8c212d verified live).
- **Fixed in S-REAL's lane, each with a $0 regression test:**
  - the DeepSeek foreign-turn fallback (`669d5a1`): it adds `reasoning_content: ""` first, because the flatten made the model re-run the tool;
  - the provider and tools tests on a body checkout under G0 (`f4cde26`, `a730d10`);
  - the harness (`tests/real/`).
- **Found live, fixed by other lanes:**
  - quick-gate HEAD fetch (`f8c212d`);
  - seed `forbid_missing` (`fade67a`);
  - selfmap relative root (`40600e0`);
  - G0 `test` under the sandbox (`d05c1e3`, `eec8d5e`, `3008357`, `f76e749`);
  - G1's sandbox OUT (`70f30d3`);
  - G4 scoring zero-request runs (`c712701`).
- **Open (S-REAL N):**
  - the seed has no committed `self.json`, so a quick gate on an unchanged seed fails G1;
  - `thinking` has no config key, so E2E-2's disabled mode was checked at the provider level only;
  - every T2 attempt pays a cold ~1 GB, ~5 min `selfmap` build;
  - M-3 and the over-window half of V-DS-6 are not run.

### 9.3 M5

**Done: M-5 passes 3 of 3 trials under the v0.1 rule** (M5; `3b02364`; `measurements/m5-v0.1.json`, `measurements/phase0.md`). Mock only, $0.
- **Tool.** `init-measure m5` samples `phys_footprint` (`proc_pid_rusage`, cross-checked against `footprint -p` to the KiB) and `ps` RSS every 200 ms. The baseline is the last sample at or before the first `mask` or `compaction` event (step 155 here). `--trials N` passes only if all pass, `--metric` and `--baseline N` give the old rule, and `m5-reanalyze` re-applies the rule to stored results. 22 `init-measure` tests pass, 6 of them new.
- **Margin work in the seed** (requests byte-identical): `EventBody::Message(Box<MessageEv>)` (`EventBody` 504 → 184 B, `Loaded` 568 → 248 B), slimmer unread view data, an estimate-only projection that removes 2 of 3 content copies per request, and the byte cache freed before the compaction fork renders. Window heap per masked step: 3,800 → 2,611 B. New regression tests pin the sizes and the per-step growth (≤ 3,000 B).
- **Result.** 1,001 turns, 2 compactions and 8 masks per trial, 0 errors:

  | Trial | Slope (KiB/step, ≤ 2) | Max/base (≤ 1.25) | Old rule on the same run |
  |---|---|---|---|
  | 1 | −6.43 | 1.141 | 4.50 / 1.550, FAIL |
  | 2 | −5.02 | 1.117 | 7.11 / 1.633, FAIL |
  | 3 | +0.53 | 1.198 | 6.74 / 1.737, FAIL |

- **The rule still discriminates:** the stored pre-FIX8 runs fail it, and a 3 KiB/step leak fails the unit test.
- **Margin.** The ratio's is thin (worst 1.198 against 1.25): footprint peaks at compaction, when several context copies coexist. The remaining copies (the loop's held messages, the provider's second render, `client.rs`'s request copy) are a v0.1.x item (RP).
- **Seed suite:** 275 unit + 16 boot + 8 self_md green at `3b02364`; clippy clean.

### 9.4 REL

**Done** (REL; `371a84b` uninstall, `263de61` the `init` launcher, `6957b4e` release profiles, `b461f1f` release.sh, README, CHANGELOG, docs/release.md and the workflow, `48bb3ba` install.sh and release-e2e.sh, `baab414` the workflow's paths-ignore). The verdict file is `build/status/rel-e2e.json`.
- **`scripts/release.sh`** builds `target/dist/v0.1.0/`: `init-root`, `init-kernel`, `initctl`, `init-mockllm`, `init-body`, `init-tui`, the kernel source and seed bundles, the kernel docs, WIT, the template extension, the charter assets and `MANIFEST.json`. Its quality guard (`cargo test --release`) passed root 105 and seed 300, 0 failed. The release build took 1,709 s at load 10–30 (kernel 692 s cold, seed 336 s). Binaries: init-kernel 38.0 MB, init-body 22.7 MB, init-root 5.4 MB, init-tui 1.3 MB, init-mockllm 1.1 MB, initctl 1.0 MB; the dist is 87 MB.
- **Release profiles** state cargo's defaults. Measured on `init-root`: `lto = "thin"` gave +1.1% size for +24% build time, and thin plus `codegen-units = 1` gave −12% size for 1.4–2× the build time; neither pays for binaries built once per install and not CPU-bound (`6957b4e`).
- **`scripts/install.sh`:** preflight, release reuse or build, the charter wizard (every prompt has a default; `--yes`), `init-root install` with the seed bundle, `kernel/config.toml`, `init` and `init-tui` in `INIT_BIN_DIR`, the LaunchAgent unless `--no-service`, then the next steps. It never reads or writes a key. **`scripts/uninstall.sh`** refuses while a Root holds `run/root.lock`, asks for a typed `uninstall`, then moves the homes and commands to `~/.Trash`; it never runs `rm`.
- **End to end, $0** (`scripts/release-e2e.sh`, 16:08–16:14): install 315 s (311 s of it the cold body build); start → ready 1.6 s; `init --raw` (the TUI's line mode) and one line against `init-mockllm`: `work/hello.txt` = `INIT`, reply `done`, idle after 728 ms, 2 egress and 2 mock requests, 3,781 µUSD at mock prices, no ceiling hit, `audit verify --full` exit 0; uninstall refused while the Root ran, declined on `no`, and on `uninstall` moved 4 items to the test Trash.
- **`.github/workflows/fast-linux.yml`:** the portable kernel crates as the gate job (init-proto, initctl, kernel-gate, kernel-selfmap) and the seed plus the ext template as an informational job. It has not run on GitHub yet.
- **Left for the tag:** a clean re-cut on a clean tree at the tagged commit (today's dist is `dirty: true`, because other lanes had uncommitted edits), the first `fast-linux` run, and the owner's own install (`scripts/install.sh`, `init-root keys set deepseek`, `init`). `install --upgrade` has no end-to-end check.

### 9.5 TUI

**Landed.** `docs/tui.md` is committed (`ddce8fa`). It defines a separate client over `owner.sock` that is never linked into `init-root`, with trust rules T-1…T-5 and tests TUI-1…TUI-14 and TUI-R1…TUI-R3. The same commit added the `tui/` workspace to CONVENTIONS §2, WP26 to the roadmap, and OW-TUI to CONVENTIONS §6. REL's `scripts/init` launcher is also committed (`263de61`).

**Done: `init-tui`** (WP26; `74acc1a` the workspace and core, `4ce8dfe` Root §9 Q1, `18a7ee6` kernel §9 Q2, `147c3ce` tests, `367542b` TUI-R1…R3 and the `ci/local.sh` steps, `7a4c779` TTY mode, `36295f3` TUI-14, `b5a67d6` docs/tui.md as built, `de30764`).
- **What it is.** The fifth workspace, `tui/`: crate and binary `init-tui`, `#![forbid(unsafe_code)]`, crossterm 0.29 and a hand-written renderer, a 25-crate closure (vendor/ 265 → 277). Trust rules T-1…T-5 hold: its only endpoint is `ROOT_HOME/run/owner.sock`, it opens and writes no file, and its only exec is `init-root panic` on the `/panic` fallback.
- **Features (v0.1).** Streaming text with a Markdown subset, collapsed thinking, one row per tool call, the Ctrl-O transcript view, a multi-line input box, `/` completion, Ctrl-C interrupt and double Ctrl-C detach, `/panic` through three paths, a status bar (level and cap, model, run cost, today against the ceiling, Root state, body sha), notice classes with `src`-aware anti-forgery, reconnect with Root epochs and `attach{since}`, echo merge that never resends, a line mode, a sanitizer on every frame string, and terminal restore on every exit path.
- **Root and kernel helpers.** The Root's owner frames gained `rseq`, `src`, `attach{since, epoch}`, alert `code` and a throttled ceiling notice (`4ce8dfe`); the kernel's `state` frame gained `model` and `task_musd` (`18a7ee6`).
- **Tests** (green twice): 45 unit (TUI-1 goldens at 80×24, 120×40 and 40×12, TUI-2, TUI-3, TUI-4 with a 10⁵-frame fuzz), 9 `proto` (TUI-5…TUI-10, TUI-14 under `sandbox-exec`), 1 `tty` (TUI-12 through a pty) and 1 `e2e_root` (TUI-R1…R3 against the real `init-root` in test mode, owner-machine variant); TUI-13 ignored in the fast tier. Clippy clean.
- **Measured.** `/panic` → PANICKED in the status bar in 50 ms; owner line → echo p50 13.9 ms, p99 23.9 ms against the real Root (debug builds); 20 pings and 3 lines queued across a Root restart each reached the kernel exactly once; keystrokes render at once and frames coalesce per 8 ms (docs/tui.md §10).
- **Found on the way:** a body display text near 1 MiB overflowed the kernel → Root line and crashed the kernel. K-LINE fixed it (`15dbf74`: text split into 128 KiB frames, other kinds cut with a marker, owner.out paced at 100/s), and the Root now refuses owner lines over 900 KiB (`44b9c6e`).
- **Left:** TUI-11 proper (real kernel and body under `init-fakeroot`), TUI-13's 60 s flood on an idle machine, and the run on real DeepSeek (S-REAL).

## 10. References

### 10.1 Files

- `INIT Agent spec.md` (v0.3; §10, §11, §13, §19)
- `docs/roadmap.md` (§3.2, §3.3, §3.9.1, §3.10, §4.3, §9), `docs/testing.md` (§3.1a, §3.9, §3.10, §3.11, §3.17), `docs/CONVENTIONS.md` (§1, §5, §6), `docs/root/root.md` (§4.8, RR-34)
- `docs/reconciliation/build-deviations.md` v0.2 (§1 totals, §2, §3, §4.1–§4.3, §5) and v0.3 (§6)
- `RELEASE_PLAN.md`, `BUILD_LOG.md`, `MORNING_REPORT.md`, `build/ORCHESTRATOR_STATE.md`, `docs/adr/008-owner-machine-safety-v0.1.md`, `build/review/root-security-2026-10-20.md`
- `measurements/phase0.md`, `measurements/wp0-build-times.md`, `measurements/{m1,m2,m4,m5,m5-100ms,m5-fix8-before,m5-fix8-after,m5-v0.1}.json`, `measurements/real-v0.1.md`
- `build/status/{WP0…WP25, WP7b, WP8b, WP8c, WP21b, S1, FIX1…FIX9}.md`; release push: `build/status/{R-SAFE, R-DEC, R-SEC, K-FLAKE, K-LINE, K-GATE, M5, REL, WP26, G0-ROOT, G0-SEED, S-REAL}.md`, `build/status/rel-e2e.json`
- `build/ci/local-full-2026-10-20.log`, `local-full-2026-10-20-final.log`, `kernel-rerun-2026-10-20.log`
- `root/crates/init-root/src/ledger.rs` (`SUPPORTED_MAX_LEVEL`)

### 10.2 Commits

| Commit | What |
|---|---|
| `37032de` | spec v0.3: immutable Root and autonomy ladder (ADR-007) |
| `9a71966` | WP0 skeleton, vendoring, `wasm32-wasip2` |
| `5aaa40b` | S1: Slice-1 green on the real DeepSeek |
| `738553c`, `05a9473` | build-deviations v0.1, v0.2 |
| `9e4c2a1` | WP15 acceptance at $0 |
| `986669f` | docs-sync: the adopt rows, D14 and the holdout rule in all docs |
| `35647a8`, `194ba30` | FIX2: D14 floor and holdout rule |
| `54fb956`, `2f35667` | FIX5: `proposal.file`, 12-task S, real-judge gog re-run |
| `d84a321` | FIX4: Slice-1 on the real install path; sha2 opt-level |
| `80afa41` | FIX6: early `kernel.sock`, runner socket waits |
| `37aef0c` | FIX7: promote tests on a 4-task holdout |
| `fb5a5be` | WP16: Phase 0 measurements |
| `3297b2e` | CI log 07:39 (2 root steps red) |
| `6c97b4c`, `8c47549` | FIX9: sealed-block drift test; wp21 test race |
| `a595a9b` | FIX8: bounded view; M-5 analysis |
| `3dcefbc` | MORNING_REPORT and final CI logs |
| `9e374d2` | `ci/env.sh` disk flags |
| `9055953` | RELEASE_PLAN v0.1.0, provisional decisions, lanes |
| `dd5c358` | orchestrator state; the first Root re-measure |
| `cdb49e8` | R-DEC money meter; the DOCS lane's last Root re-measure |
| `d4807ca` | RELEASE_PLAN: Root budget rule (measured + 5% per module) |
| `50c553e` | DOCS: provisional decisions in the docs (Root budget, G4 power); build-deviations v0.3; §5 adopt rows L1–L3, L6–L8 |
| `781504b` | DOCS: CONVENTIONS §6 decision table and registry; roadmap M6, M7, R13 |
| `ddce8fa` | TUI: `docs/tui.md` |
| `263de61` | REL: `scripts/init` launcher |
| `27dd505`, `7a460a6` | R-SAFE: owner-machine mode; resume vs the mirror's panic ref, `RLIMIT_NOFILE` |
| `cdb49e8`, `518e68e`, `47c618f`, `3b08245` | R-DEC: the provisional Root decisions, review F1/F4/F6, the Root fix-code rows |
| `e0e0834` | R-SEC: review F2, F3, F7 |
| `5e50940`, `6728d78`, `28f204d`, `2794924`, `f8c212d`, `e379f4f` | K-FLAKE: lost `HandoffDone`, watchdogs, build timeouts, `gate --quick` rev, `h21` |
| `15dbf74`, `44b9c6e` | K-LINE and the Root: oversized owner frames |
| `fade67a`, `d05c1e3`, `eec8d5e`, `3008357`, `f76e749`, `3610c17` | the seed passes its own G0 (seed forbid, sockets under `OUT`, `init-mockllm` shipped, seed test homes, the `seed G0` check) |
| `70f30d3`, `900feef`, `c7a1319` | K-GATE: G1's `OUT`; promote under the real Seatbelt profiles; a kept extension across a T2 |
| `40600e0` | selfmap: absolute root and target |
| `3b02364` | M5: M-5 PASS under the v0.1 rule |
| `74acc1a` … `de30764` | WP26: `init-tui` |
| `b461f1f`, `48bb3ba`, `baab414` | REL: release, install, CHANGELOG, `fast-linux` |

Per-crate commits are in §2.

### 10.3 Source disagreements noted in this report

- **Instantiate p99:** 166 µs in BL 05:41, 154 µs in WP14's table. BD §4.2 already notes this; this report uses WP14.
- **M-5 cause:** BL 07:29 and WP16 N suspected that the body keeps the whole session log in memory; FIX8 (BL 08:04) found the live heap bounded and no leak. The later finding is used.
- **Test totals:** BD §4.1 sums 1,079 green and 3 red (06:55); MR reports 1,137 passes from the final CI run; this report sums 1,100 distinct tests by latest status file. §2.6 explains the different bases.
- **Gap #33:** BD v0.2 shows it in progress; FIX7 (`37aef0c`) closed it.
- **Real-model batch stop:** BD §3.17 and MR recommend a stop at $0.20; RP, the later decision, uses no new checks after $0.50 within the $1.00 cap.
- **Root budget:** RP said "≈ 7,000" at `9055953` and "≈ 7,500, measured + 5%" at `d4807ca`. The second follows the DOCS lane's re-measure (§6.2).
- **Acceptance wording:** MR says A1–A6 pass; WP15 scopes that to A5-fake (kernel half) and the A6 live half (§4.1).
- **Cold install time:** WP21b measured 172 s on its run and FIX4 209.9 s on a later run. Both are cold sandboxed seed builds; this report quotes FIX4. WP21b's "about 1 GB" of transient disk per cold build was measured at 1,573 MB by FIX4 (BD §1.3).
- **CV §6:** the v0.3 paragraph settles ESC-1 and ESC-2, but the Pending table still lists them, and boot.md §3.2 marks ESC-2 "PENDING OWNER" (§6.4).
- **Root fixture count:** G0-ROOT's Tests line says "87 unit + 50 integration", but its per-suite list sums to 51 (owner_frames has 3 since `44b9c6e`); §2.6 uses the list, 138. R-DEC's earlier run counted 136 (86 + 50, owner_frames 2).
- **Seed `self_md` count:** M5 reports 8 `self_md` tests; K-FLAKE, REL and G0-SEED count 9 (`fade67a` added one that checks every target root). §2.6 uses 9.
