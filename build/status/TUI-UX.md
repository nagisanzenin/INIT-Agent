# TUI-UX: compact tool activity, token/cache/cost segment, heartbeat (2026-10-24)

## Done
- **Activity block** (docs/tui.md §3.6): consecutive tool calls between assistant messages are one block
  updated in place: spinner, current action, `8 tool calls · 1 running · 1 failed · 1 min 16 s`; the
  current call's running time, newest output line (dim) and `still working…` after 10 s of silence; the
  last 3 finished calls, older ones folded into `… +N more`; failures and `no result` never folded.
  Thinking between calls folds in. Freezes on the reply's text / run end / owner line / error / body
  restart to `▸ ran 8 tools in 1 min 22 s · bash ×4 · read ×2 · edit ×2   ✗ 1 failed` plus failure rows.
- **Details:** Ctrl-O opens the transcript on the last activity, expanded (calls, per-request token lines,
  thoughts); `/details` prints that expansion into the scrollback. Nothing is dropped from the session log.
- **Status bar** (§3.9): `turn in 238k out 1.0k · cache 99.6% · $0.004` and `session cache 97.1%`.
  Cache = Σcached/Σinput (token-weighted); missing cache field → `?` / `~`, never 0%. Cost = Root ledger
  delta since the turn began (observe-only, no caps). Per-request `tokens:` lines left the main screen.
- **Line mode / screen readers** (§3.12): no animation; `[tools] ran 3 calls (2 bash, 1 read), 1 failed`,
  `[turn] 3 requests: … (session cache …)` at the run's end, a 30 s heartbeat, `/details`.
- `tool_progress` display frames are rendered if a body sends them (proposed, §9 Q4).

## Tests ($0, mock/fixtures only; test homes under tui/target/test-homes)
- `cargo test --offline --bin init-tui --test proto --test tty` (in tui/): 73 + 14 + 2 passed (1 ignored, as before).
  New: goldens `tuiux_activity`, `tuiux_failed`, `tuiux_status`; model tests for the activity, progress,
  token parsing / weighted cache / unknown cache, turn+session tallies and cost; line-mode test
  `tuiux_line_mode_activity_and_turn_summaries`. Updated goldens: tui1_conversation, tui1_notices,
  tui1_thinking_batch (hint text), tui10_plain (+ the `[tools]` line).
- `cargo clippy --offline --all-targets -- -D warnings`: clean.
- `cargo test --offline --test e2e_root`: compiles and passes, but SKIPS its body: the test-fixtures
  init-root, init-root-fakekernel and init-mockllm bins are not built (root/target and kernel/target are
  absent) and disk was 5.4 GB free (floor 5 GB), so I did not build them. The fixture pty test (tests/tty.rs) passes.

## Deviations
- The hint line drops "(twice: quit)" to fit "Ctrl-O details" in 80 columns (the overlay and the first
  Ctrl-C's hint still say it).
- Line mode prints the activity summary without durations (deterministic goldens; TTY mode has them).

## Needs from others
- Body/kernel (§9 Q4): `display{kind:"tool_progress", call_id, name, text}` for live output lines. Until
  then the heartbeat is all the owner sees during a long call.
- REL/LINUX: run the e2e (`ci/local.sh` tui e2e step) once disk allows building the test-fixtures Root bins.

## Left
- Nothing in tui/**. Optional: a usage field on `turn_end` (structured, instead of parsing the text).
