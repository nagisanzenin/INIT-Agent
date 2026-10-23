# K-FLAKE: bounded tests, no hangs; sv10 root causes; h21 deterministic; build timeouts

Status: **done** (2026-10-20, lane K-FLAKE, kernel + seed workspaces, default target dirs, `source ci/env.sh`).

## Done

### 1. Why sv10 hung for 3 h 25 min (two causes, both fixed)
- **Test-side deadlock (the hang).** On macOS, std creates pipes and socketpairs with `pipe()`/`socketpair()` and sets `FD_CLOEXEC` afterwards. macOS has no `pipe2`, so this is not atomic. A child that another thread spawns inside that window inherits the pipe. `Command::output()` reads to EOF, so it then waits until that unrelated child exits. In sv10 the in-test fake Root spawns the rollback body at the same moment the test, woken by `body_rollback`, runs `git rev-parse` (`main_of`). The body inherits git's stdout pipe. `output()` then waits for a body that only the test's own `stop()` ends, and nothing in that wait has a timeout. Proven with a scratch program: while one thread loops on `pipe()`, 751 of 2000 spawned children inherit fds (1633 of 2000 with 4 threads; 0 of 2000 with none). A sleeping Mac only changes the timing. The fake Root's spawns are the window.
- **Product lost wakeup.** `machine.rs`: when B (already committed) crashed while A was still retiring, the machine set `handoff = None` without `HandoffDone`. So `promote::Supervise::handoff` (`rx.recv()`, no timeout) waited forever. In the test this showed up as `handoff finished: Timeout` under load: sv10 failed 4 of 4 runs at load 40-270. Fixed: answer `ok` (the handoff committed), then apply the crash rules. Commit 5e50940, with unit test `sv10_b_crashes_while_a_retires_answers_the_handoff_then_rolls_back`.

### 2. `init_testkit::watchdog` (new, `kernel/crates/init-testkit/src/watchdog.rs`), commit 6728d78
- `arm(name, bound)` is the first line of every process-spawning integration test. On timeout it prints a clear message, both into the test's captured output and straight to stderr. It then SIGKILLs the children the test registered, by pid. Each kill happens only after `waitid(WNOHANG|WNOWAIT)` confirms the pid is still an unreaped child of this process, plus the process group that child leads (a live or zombie leader keeps the group id reserved). Then it fails the test: the guard's drop panics. If the test thread is still blocked 30 s after the kills, it kills every registered child of the binary and exits 101, so CI fails instead of hanging. Never a sweep.
- Bounds are monotonic (`recv_timeout`/`Instant` = `CLOCK_UPTIME_RAW`, paused while the Mac sleeps), so sleep neither fires a watchdog nor eats a bound.
- `spawn`/`spawn_keeping(cmd, 5)`: `close_leaked_fds` in the child (`pre_exec`). It closes every fd ≥ `from` that lacks `FD_CLOEXEC`, up to the highest open fd + 256 (the soft limit here is 1,048,576), and registers the pid. `output`: stdout and stderr go to unlinked files, and it waits for the child's **exit**, never for EOF on a pipe. `status`: tracked spawn + wait. `INIT_WATCHDOG_LOG=<file>` logs one line per test (ms, bound) to size the bounds.
- Unit tests: own-child check (running, zombie, reaped, pid 1); `output` returns at exit although a grandchild holds stdout for 30 s; a child does not inherit an unflagged fd; a hung test fails and its child is SIGKILLed; a quick test does not fire.

### 3. Where it is used
- Armed, each test with its own measured bound: 10x its slowest run in four full kernel runs when under 60 s, 5x above that, floor 30 s (commits 6728d78 and cd8b8c2). The tightest margin now is about 5x (`real_seed_and_kernel_trees` 26-176 s, bound 900 s). Files: kernel-supervise `handoff`, `promote`, `process`, `recovery`, `probe`, `slice1`, `install`, `serve_seam`, `fakeroot_xcheck`, `faults`, `owner_state`; init-kernel `e2e`, `judge_cli`; init-testkit `acceptance`, `acceptance_root`, `scenarios`, `fakes`, `mockllm_server`; kernel-serve `ic`, `sandboxed_lane`; kernel-gate `g0`, `judge`; kernel-selfmap `selfmap`; initctl `cli`; kernel-proxy `proxy`.
- Leak-safe spawns: the supervise tests' fake Root (bodies; `spawn_kernel` keeps 3 and 4) and its `git()` helper (`output`, which removes the sv10 deadlock outright). init-testkit's `fakeroot` (kernel keeps 3 and 4; bodies), `fakebody` `hang_before_ready` child, and the scenario runner (fakeroot, body, init-root start, every `output()`). Every `Command::{spawn,output,status}` in the armed test files.
- Dev-dependency `init-testkit` added to init-kernel, kernel-serve, kernel-gate, kernel-selfmap and initctl (`Cargo.lock`: dev edges only).

### 4. Build timeouts (from S-REAL via the orchestrator)
- `BuildSlot::new` used the 900/300 s defaults, so `build_timeout_cold_s`/`_warm_s` were ignored. `BuildTimeouts::from_cfg` and `BuildSlot::configured` now feed both `run_build` sites (install.rs, promote/stages.rs). Commit 28f204d, unit test `configured_build_timeouts_are_honoured`, integration test `install_honours_the_configured_build_timeout`.
- (commit 2794924) `run_to_completion` no longer waits for descendants that hold a build's pipes. After the exit it waits at most `PUMP_DRAIN` (5 s) for EOF, then stops logging and abandons the pumps. `LocalSpawner` and the test fake Root signal the instance's process group before the leader, only while the leader is unreaped (the real Root already does this, zone.rs `kill`). Tests: `timeout_kills_the_group_so_a_grandchild_cannot_hold_the_pipes`, `pump_drain_is_bounded_when_a_descendant_escapes_the_group`, and the install test, whose build script leaves a 30 s grandchild on stdout.

### 5. h21 (seed `ext_host::tests::h21_incremental_cache_warm_vs_cold_recompile`), commit e379f4f
- It asserts cache entries, not milliseconds. An entry is inserted only on a miss, so the entries a compile adds equal the functions it compiled. The warm recompile of the edited v2 (on the engine that compiled v1) must compile fewer than half of the functions a cold engine compiles for v2. It also checks that nothing was evicted during the test. compile_ms stays as a measurement print only. Measured: v1 cold 887 entries; v2 warm compiled 297 and reused 586; v2 cold 883 (compile_ms 110 warm vs 219 cold, not asserted).

### 6. Quick-gate fetch (from S-REAL via the orchestrator), commit f8c212d
- `gate --quick` sent `HEAD` (clean worktree) or a snapshot commit id (dirty tree), but the kernel fetched `refs/heads/<rev>`. `Git::fetch_rev`: `HEAD` means the remote HEAD, a full id is fetched by id (protocol v2 upload-pack serves any object, including an unreferenced snapshot; checked on git 2.39.5), `refs/...` is used as given, and anything else becomes `refs/heads/<rev>`. Refspec metacharacters are refused. Test `fetch_rev_takes_head_a_branch_a_ref_and_a_raw_commit_id`.

### 7. A silent skip found on the way
- kernel-gate `tests/judge.rs` looked for init-mockllm in `kernel/target/*/debug/` only (the old `target/k1` layout). With the default target dir it printed SKIP and "passed" in 0.8 s. It now looks in its own target dir first and runs: 4 tests, 253 s.

## Tests
Every run used `source ci/env.sh` (`RUST_TEST_THREADS=2`, `CARGO_BUILD_JOBS=2`), `ci/slot.sh`, the default target dirs, `cargo test --offline` in the workspace root, one after another, and started only once the 1-minute load was under 20. The tree kept moving under other lanes' commits; the head of each run is listed.

| Run | Start-end | Load at start | Head | Result |
|---|---|---|---|---|
| seed 1 | 14:26-14:27 | 19.5 | 59b7f5d | **pass**: 275 + 16 + 9 passed, 5 ignored |
| seed 2 | 14:28-14:29 | 19.3 | 59b7f5d | **pass**: 275 + 16 + 9 passed, 5 ignored |
| seed 3 | 14:29-14:30 | 10.2 | 29bb1ac | **pass**: 275 + 16 + 9 passed, 5 ignored |
| kernel 1 | 14:30-14:40 | 7.2 | 03178be | **pass**: 704 passed, 0 failed, 5 ignored (51 binaries) |
| kernel 2 | 14:40-14:54 | 5.7 | 1162d3b | **pass**: 712 passed, 0 failed, 5 ignored |
| kernel 3 | 14:54-15:07 | 8.7 (47 mid-run) | 3008357 | **pass**: 716 passed, 0 failed, 5 ignored |

- Before those: a validation run (13:44-14:07, load 9-35) also passed, and gave the first timings. Not counted.
- No watchdog fired in any run. The only `watchdog:` line is the unit test `a_hung_test_fails_and_its_child_is_killed`, which fires one on purpose.
- Targeted: `kernel-supervise --lib` 71/71 (machine, build, instance, git unit tests above); handoff 8/8 (sv10 included), install 2/2, promote 6/6, judge 4/4 (253 s; skipped before).
- `cargo clippy --offline --workspace --tests -- -D warnings` in kernel/: clean.
- Before the fixes, under load 40-270: sv10 failed 4 of 4 runs (`handoff finished: Timeout`), which is the lost-reply bug, and sv5 took 18 s alone.

## Deviations
- Watchdogs are also armed in kernel-gate, kernel-selfmap, initctl and kernel-proxy tests (not in the task's list). They spawn processes too, and "no test may ever hang" covers them.
- `run_to_completion` and `LocalSpawner`/fake-Root group kills are product/test changes beyond the original list, asked for by the orchestrator.

## Needs from others
- REL: `kernel/Cargo.toml` and `seed/Cargo.toml` have your uncommitted release profiles. I left them alone.
- Anyone adding a process-spawning test: use `init_testkit::watchdog::arm` as the first line and `watchdog::{spawn,output,status}` for children (kernel-gate `seed_g0.rs` and supervise `owner_state.rs` already do).
- The real Root needs nothing: `exec-clean` already closes inherited fds and `zone.rs kill` already signals the group. The fd-leak race only bit test processes that spawn bodies themselves.

## Left
- Nothing for this lane. `ci/local.sh` 3x (root + ext steps too) belongs to the orchestrator's release check.
