# K-LINE: owner display lines that never break the Root link

Lane K-LINE (2026-10-20). Reported by WP26: a body display text near 1 MiB made the kernel's `owner.out` line too long for the Root.

## What happened before the fix (traced in code)

1. A body may send a `display` line of up to 1 MiB (`init-proto` `stdio::MAX_LINE`; a longer line is cut by `bodyio` and becomes a 4 KiB `info` frame). The kernel relayed it as one `owner.out` event. That event is the frame plus `boot_seq` plus the event envelope, about 40 to 70 bytes more than the body sent. A body line within that margin of 1 MiB therefore became a kernel line over 1 MiB. Before the fix, `OwnerDisplay::new` dropped only unknown fields that did not fit and never bounded `text`. The unit test `one_frame_relay_of_a_full_body_line_passes_the_limit` reproduces this.
2. **The Root closes the link.** `root/crates/init-root/src/sup.rs` `read_loop` reads at most 1 MiB + 1 bytes per line. A longer line ends the loop and sends `Ev::Lost`, and the supervisor then SIGKILLs the kernel (`Ev::Lost => zone.kill(SIGKILL)`).
3. **The Root counts a crash.** The kernel's exit while the slot is `Running` runs `zone_exit(code, "crash")` and `crash()`. `kill_all` also kills the body in the middle of its turn. root.md §3.6.3 lists "a line over 1 MiB" as a crash. More than 3 crashes in 600 s roll the tuple back to LKG, and in system safe mode more than 2 halt it (`crash_loop_safe`). A body or extension that keeps producing a huge summary could therefore force a rollback.
4. **The owner channel is not wedged, but the frame is lost.** Attached clients stay connected to the Root and see the kernel-exit notice and the restart. The turn's output is gone, and lines typed during the backoff wait in the Root's queue.
5. **A sibling limit had the same effect.** The Root also ends the link past 10,000 control frames a minute. The body sends one `display` frame per model delta, and before the fix the kernel relayed each one 1:1. A long fast stream (about 170 deltas/s for a minute) would end the link, SIGKILL the kernel and count a crash, exactly like case 2.

## Done

- `kernel/crates/kernel-supervise/src/ownerline.rs` (new):
  - `display_frames` keeps each relayed frame within `LINE_BUDGET` = 1 MiB − 1 KiB, which leaves room for the Root's `rseq` and `src`.
    - **`text` and `thinking`** are split into frames of at most 128 KiB, cut on char boundaries. Clients append the deltas, so every byte arrives in order. A piece fits even if escaping grows each byte sixfold.
    - **Other kinds** (`tool_start`, `tool_end`, `turn_end`, `info`, `error`) keep their first 64 KiB plus the marker `[kernel: showing <n> of <total> bytes, cut to fit the owner line; the session log has the full record]`.
    - `call_id` and `name` keep at most 256 bytes in every frame, so both ends of a tool call are cut alike and the TUI still pairs them.
    - **Known fields win.** Unknown fields (`detail`, …) that do not fit are dropped first and named in a marker: inline for tool kinds, as a separate `info` frame for text kinds. The text is split or cut only when it does not fit on its own. This keeps the old `OwnerDisplay::new` rule of dropping extras before anything else.
    - A frame that fits is relayed unchanged. Small deltas cost one arithmetic size estimate and no extra encoding.
  - `bounded` applies the same bound to the kernel's own `echo` and `notice` (an owner line near 1 MiB from `init-root attach`).
  - `Pacer` paces `owner.out` at 100 frames/s with bursts of 300, so at most 6,300 frames go out in any minute against the Root's limit of 10,000.
    - Under the rate, every frame goes out at once.
    - Over the rate, frames wait in order, consecutive deltas merge (up to 128 KiB), and a newer `state` replaces a waiting one.
    - Past 1,000 waiting body frames, a delta joins the newest waiting frame of its kind, so no text is lost. A body frame of another kind is dropped and counted, with a `[kernel] <n> display frames dropped` notice.
    - The kernel's own frames are never dropped. `periodic()` drains the queue.
- `supervisor.rs`: `relay_display` goes through `display_frames`, and every `owner.out` goes through `owner_send` (bound, then pace). The display relay no longer uses `OwnerDisplay::new`.
- `link.rs`: `write_line` never sends a line over 1 MiB. It logs and returns `LinkError::Other` instead. `respond` replaces an over-long response with an `E_INTERNAL` error, so the Root still gets an answer.
- `tests/fixtures/testbody.rs` gains three commands: `!big <kind> <bytes>` (frames packed to exactly the 1 MiB body line), `!bigdetail <bytes>` and `!flood <n>`.
- `tests/common/mod.rs`: `Seen.max_line` records the longest line the kernel sent.
- docs/kernel/supervise.md §4.6 has an as-built paragraph, "Line and rate bounds".

## Tests

- `ci/slot.sh cargo test --offline -p kernel-supervise --lib ownerline`: 10 passed. They cover the old relay passing the limit; split text and thinking arriving whole (nasty escapes, 2- and 4-byte chars, up to 2 MiB + 3); other kinds cut with the marker and ids capped alike on small and large frames; extras dropped and named, with a text that fits alone kept whole; `echo` and `notice` bounded; and three pacer tests (under the rate unchanged; 20,000 deltas merged with no text lost and ≤ 6,300 frames/min; an unmergeable flood dropped with a notice while `state` merges and kernel frames stay).
- `ci/slot.sh cargo test --offline -p kernel-supervise --test owner_state`: 3 passed (5.9 s).
  - `oversized_displays_keep_the_link_up` is the regression test. The body sends 2 MiB of text in about 1 MiB frames. The owner gets every byte (more than 8 frames), and a 1 MiB `tool_end` arrives cut with the marker and the session-log pointer. A 1 MB `detail` is dropped and named. No kernel line passes `LINE_BUDGET`. There are no exits, kills or respawns, and a later line round-trips. Then a clean stop with exit 0.
  - `a_delta_flood_is_paced_without_losing_text`: 20,000 deltas arrive whole and in order, in fewer than 6,300 frames, and the link stays up.
- `ci/slot.sh cargo test --offline -p kernel-supervise`: all green (unit 81; fakeroot_xcheck 1, handoff 8, install 2, owner_state 3, probe 3, process 3, promote 6, recovery 5, serve_seam 1, slice1 10).
- `cargo clippy --offline -p kernel-supervise --all-targets -- -D warnings`: clean. `cargo check -p init-kernel`: clean.

## Deviations

- The wire format is unchanged and init-proto is untouched. `ownerline.rs` keeps a local copy of `RELAY_KEYS` (`boot_seq rseq src t v`, tui.md §9 Q1/Q2), because init-proto's copy is private.
- Splitting text into several `display` frames relies on clients appending consecutive deltas. tui.md §3.5 already says so ("appended to the streaming paragraph"; line mode prints as it arrives).
- A `text` frame whose unknown fields are dropped gets the drop marker as a separate `info` frame, never inside the streamed text.
- Pacing beyond the rate coarsens streaming to about one merged frame per 10 ms. That is invisible next to the TUI's 8 ms coalescing.

## Needs from others

- **Root (R lane), owner lines near 1 MiB.** `init-root attach` (cli.rs:241) sends any line, and the Root's `conn` accepts a client frame of up to 1 MiB. Its `owner.in` request adds about 95 bytes. A line in that last 95-byte window becomes a Root → kernel line over 1 MiB. The kernel's link reader then drops the link, the kernel exits 69, and the Root counts a crash. The Root's queue redelivers the line at the next ready, which repeats the crash until LKG rollback or a halt. Suggested fix, about 5 lines in `link.rs` `line()`: refuse a line whose ICJ `owner.in` would pass 900 KiB, with the TUI's notice "too long to send; split it". The kernel already bounds the echo.
- **init-testkit `fakekernel.rs` (test double).** It still relays one frame per display with no bound or pacing, so the TUI and Root e2e tests do not exercise these bounds. It is optional to mirror. init-testkit cannot depend on kernel-supervise.

## Left

- Nothing for this lane. For later: tool output (`detail`) larger than one line could be streamed as several frames, if a body ever sends it; today it is dropped with a marker.
