# K-RACE: the crash-loop spawn stall in final CI run 2

Status: **done** (2026-10-20, lane K-RACE, kernel workspace, default target dir, `source ci/env.sh`, `ci/slot.sh`).

## Done

### Root cause (product: the kernel-supervise driver)
`f_crash_boot_c_lkg_crash_loop_enters_body_safe_mode_then_halts` failed once in final CI run 2: two `crash_before_ready` exits, then no 3rd spawn for 20 s, and lineage held only `kernel_start`. The cause is a lost `exited` event in `supervisor.rs`, not a timing margin.

- A body that dies at once can be reported (`exited`) before its spawn answer reaches the supervisor loop. There are two ways this happens:
  1. The answer goes link reader → spawn thread (it wakes, takes the 3 fds from fd 4, sends `DEv::Spawned`). The event goes link reader → forwarder → loop. Under load the spawn thread can lose that race even when the Root sent the answer first.
  2. The Root's per-instance reaper (root `zone.rs` `reaper`) does not wait for its own spawn reply, so the event can also come first on the wire. The fake Root's reaper behaves the same way.
- The loop then saw the exit of an instance it did not know yet: `bodies` had no record, and the machine was in `Starting { inst: None }`, so it took nothing. The exit was dropped. The spawn landed next, giving `Starting { inst: Some, deadline: +ready_timeout }`. At the 10 s ready timeout the machine killed the instance (the Root answered `not_found`, which `kill` treats as done), added it to `zombies`, and waited for an `exited` that had already come. It stayed in `starting` with no further spawn: the CI signature exactly.
- Not the harness: the fake Root faithfully models the Root's independent reaper. Not the watchdog (6728d78, cd8b8c2): it only changes how children are spawned and bounded.

### Fix (commit a505475)
- `Supervisor.early_exits`: while a spawn of ours is in flight (`pending_inst` not empty), an `exited` for an instance with no body record is held instead of handled. `on_spawned` replays the held exit whose instance just landed, through the normal path (`on_exited`, factored out unchanged). Once no spawn is in flight, it handles any remaining held exits as before, only later. Unrelated exits (builds, probes, cold-start kills) see no change apart from that delay. Builds and probes wait on the `ExitBoard`, which records every exit on the link reader, so they were never affected.
- The crash keeps its stderr. A replayed exit first waits (at most 500 ms, `EARLY_EXIT_TAIL_WAIT`) for the dead body's stderr reader to reach EOF: `BodyIo::wait_stderr_eof`. The reader has only just started, and its pipe is already at EOF.
- This covers every spawn through the supervisor, including a handoff's B (the same race would have sent a B that dies at once to its ready timeout).

### Tests
- New deterministic integration test `slice1::k_race_exit_reported_before_the_spawn_answer_is_still_the_crash`. The fake Root's new `Opts::exited_first` waits until the body has died (up to 1 s), sends `exited`, and answers the spawn 100 ms later. The test runs F-crash-boot (c): 5 crashes, then the safe spawn, then halted after 8 spawns. It checks every respawn's reason is `crash`, no ready-timeout kill happens, and boot.json's crash `log_tail` holds the body's stderr.
  - Before the fix: **6/6 fail**, each with the CI signature (`timed out waiting for safe spawn; spawns=1 … lineage=["kernel_start"]`).
  - After the fix: **43/43 pass** (3 single runs plus a 40-iteration loop under load).
- New unit test `bodyio::stderr_eof_wait_is_bounded_and_ends_at_eof`. The old `frames_raw_lines_eof_and_stderr` now waits for EOF instead of sleeping 50 ms.

## Failure rate (target test `f_crash_boot_c…`, test binary run in a loop, `--exact`)
| Build | Load | Iterations | Failures |
|---|---|---|---|
| before | 2 parallel loops + 5 CPU burners (1-min load 6-15) | 240 | 0 |
| before | 2 parallel loops + 6-11 burners + the full crate suite (load 22-91) | 100 | 0 |
| after | 2 parallel loops + 10 burners (load 12-50) | 300 | 0 |
| after | 2 parallel loops + 6-11 burners + the full crate suite (load 22-91) | 100 | 0 |
| before, forced order (`exited_first`) | idle and loaded | 6 | **6** |
| after, forced order (`exited_first`) | loaded | 43 | 0 |

Natural rate: before 0/340, after 0/400. The natural race needs the spawn thread to be descheduled for about 10 ms at the wrong moment, so a plain loop at this machine's load rarely hits it. CI hit it once in the final runs (whole workspace in parallel). The forced-order knob reproduces it every time, which is why the regression test uses the knob.

Whole crate, `cargo test --offline -p kernel-supervise` (in a slot, under load): **run 1 green** (18:41-18:47, 129 passed, 2 ignored), **run 2 green** (18:55-19:00, 129 passed, 2 ignored, 1-min load up to 91). `cargo clippy --offline -p kernel-supervise --tests -- -D warnings`: clean.

## Deviations
- None from the spec. kernel/supervise.md says nothing about how `exited` is ordered against the spawn answer. The kernel now tolerates either order.

## Needs from others
- Docs owner (optional): one line in kernel/supervise.md §3.4 or §3.13. Proposed wording: "an instance's `exited` may arrive before its `spawn` answer; the kernel holds it until the spawn lands".
- init-testkit's `fakeroot` (lane T) needs no change. The fix is in the product, so every Root model benefits.

## Left
- Nothing for v0.1.0. Not pushed (the orchestrator pushes).
- Cleanup: my 6 failed-run homes (`target/test-homes/wp4/s1-race-*`) were removed by exact path, and my CPU burners were killed by PID. `s1-safe-30ff1d` (18:08) is the CI run-2 failure's home. I did not create it, so I left it.
