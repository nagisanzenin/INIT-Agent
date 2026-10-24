# STEER: fluid, visible steering (v0.2.1 lane)

Branch `worktree-agent-aa4872305e1b5470b`. Not pushed; main merges after the v0.2.1 release. Everything ran at $0: mock models, fakes, no real model calls.

## Done
- **Kernel** (`c28f93f`)
  - The `state` frame gains `steer_pending`, `steer_seen` and `steer_retracted`.
    - All three are optional and absent from older kernels.
    - They are cumulative, so the pacer can replace a waiting `state` without losing anything.
  - A `state` frame is published after every body `ack` and every accepted retract.
  - `owner.in` now accepts `interrupt{then:"continue"}` and forwards it to the body.
  - `owner.in` accepts a new `retract{seq}`.
    - If no body holds the line, the kernel closes it itself.
    - Otherwise the body decides and answers `retracted{seq, ok}`.
    - A retracted line is closed with an inbox `ack` line, so no kernel, old or new, redelivers it.
- **Body** (`b315a8b`, `4b7343b`)
  - A retract gate shared between the input thread and the loop.
    - The answer to a retract is immediate, not at the next turn boundary.
    - It is atomic against the append.
  - Interrupt-and-steer: abort exactly as today, append the aborted results first, then the steer, then continue the run.
  - With no steer to answer, interrupt-and-steer is a plain stop.
- **Root** (`cfb4f51`, `290bbc8`, `3d06573`)
  - Forwards `then:"continue"` and `retract`. A retract is never queued.
  - `init-root attach` sends a bracketed paste, or `\` continuation, as one `line` frame.
- **TUI** (`c9f6f9b`)
  - A pending tray above the input box shows when each line will be read. Each line moves into the transcript as `· seen` when the body acks it.
  - Fixed the false `? no result` on a call that is still running. The test at `model.rs` that pinned the bug now asserts the call keeps running and finishes ok.
  - Tab queues the box as `/later`, shown as "next task".
  - Up on an empty box retracts the newest unread line.
  - Esc with a steer pending sends `interrupt{then:"continue"}`.
  - Line mode prints `[steer queued]`, `[steer seen]` and `[steer retracted]`.
  - Older kernels fall back to marking a line delivered at its echo.
- **Docs**: `docs/tui.md` §3.6, §3.7, §3.7.1 (new), §3.11 and §3.12; `docs/kernel/supervise.md` §3.10 and §4.5–4.6; `docs/body/loop.md` §3.4–3.5; `docs/root/root.md` §4.2.

## Tests
Command: `cargo test --offline` / `cargo clippy --offline --all-targets -- -D warnings` per workspace, run in a build slot.

| Area | Result |
|---|---|
| init-proto | 43 + 6 pass |
| kernel-supervise | lib 124 pass. `owner_state` 4 pass, including the new `state_reports_steers_pending_seen_and_retracted`. handoff 8, slice1 11, recovery 5, access 2, install 2, probe 3, process 3 pass |
| seed (body) | 309 lib + 19 boot + 9 self_md pass, including 10 new STEER tests. Clippy clean |
| root | 208 unit pass + 7 new pass |
| TUI | 77 unit pass, including the tray tests and the new golden `steer_tray`. `tty` 4 pass, including two new tests: the pty e2e `steer_tray_seen_tab_retract_and_esc_through_a_pty` and `steer_lines_in_line_mode`. Clippy clean |

Kernel clippy is clean for every file I touched. It needed three pre-existing lints allowed, in files I did not touch: `kernel-proxy/config.rs` (collapsible_if) and `kstate.rs` (is_multiple_of, byte_char_slices).

### Not verifiable in this worktree
The worktree's path is 43 bytes longer than main's, so many Unix socket paths go past SUN_LEN. These all fail with "path must be shorter than SUN_LEN" or "socket path longer than 103 bytes":
- kernel `extend` (4), `promote` (8), `fakeroot_xcheck` (1)
- most of init-testkit's acceptance, fakes and scenarios tests
- 2 root unit tests (`lane_connections_are_capped`, `rf31_rf33…`) and the Root daemon tests

For the TUI tests I added an `INIT_TUI_TEST_HOMES` override in `tests/common` and pointed it at `/tmp/stw-steer/target/test-homes/tui`. That path is a symlink into this worktree's `tui/target/test-homes/tui`, so the data stays in the worktree. Under the symlink, 3 TUI `proto` tests fail: `tui14` (its sandbox profile matches real paths) and two `wire9` workspace-path tests. Without the symlink all 14 fail on SUN_LEN.

Re-run all of the above on main after the merge.

## Deviations
- The tray item for a follow-up shows as a plain owner line once read, not `· seen`, because it starts the next task.
- `init-root attach` enables bracketed paste only when stdin and stdout are both terminals.
- The kernel's retract answers arrive as notices (`[kernel] taken back: …` / `[kernel] not taken back: …`). The TUI turns them into hints instead of transcript lines.

## Root sentinel: does smoother steering inflate "corrections"? Yes. A fix is proposed below, not implemented.
How it is counted today:
- `sentinel/engine.rs` `line()` sets `turn.correction = true` for any owner line that arrives during an owner turn, as a "steer".
- `interrupt()` also marks a correction.

The flag is a boolean per turn, so several steers in one turn still count once. Smoother steering raises the share of turns that carry at least one line:
- (a) Tab follow-ups (`/later`, new work) arrive mid-turn and are counted.
- (b) "and also …" additive notes now cost nothing to send, and each is counted.
- (c) A retracted line was already counted when the Root relayed it.
- (d) Esc interrupt-and-steer counts. That is a true correction.

`corrections_trend` (`ledger.rs`, X10 item 5) compares a 14-day window with the reference span. A rise in the base rate caused by the UX alone, right after v0.2.1, could fire the trend and block level-ups (R7) even though nothing got worse. In the other direction, attach pastes no longer turn into N steers (fix C), so attach users see fewer false corrections.

Proposed fix (Root, not done here):
1. In `sentinel::relayed`, classify the line. `/later …` is never a correction. Plain steers go into a separate `steered` count on the turn record, a weak signal. A correction stays an interrupt (including `then:"continue"`), a CORRECTIONS word, or a steer followed by an interrupt within the same turn.
2. On a relayed `retract{seq}`, undo that line's contribution when it was the turn's only cause. This needs a `qid → cause` map plus the kernel echo's seq.
3. Record a `ux_epoch` in the ledger at upgrade, so the trend's reference span restarts after a UX change instead of comparing across it.

## Needs from others
- Main: re-run the socket-path-limited suites listed above after the merge.
- The `twin` and sentinel owners: decide on the proposal above.

## Left
- A real cross-process body+kernel test of interrupt-and-steer. Each side is tested against the same JSON, but there is no harness that runs the real seed body under the kernel.
- An optional notice in `sup.rs` when an old kernel rejects `retract`. Today it fails silently, and the TUI shows the line as seen later.
- Cleanup: the `/tmp/stw-steer` symlink, removed by exact path when the lane finishes.
