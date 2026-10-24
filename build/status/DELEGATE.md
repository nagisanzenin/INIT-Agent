# DELEGATE — sub-agents built in (Phase 1: in-process `delegate`, Hybrid C default lane)

Branch `worktree-agent-ab288ac11fa8d1f9a` (isolated worktree; not pushed; merges after v0.2.1).

## Done
1. **Port** (commit 1, credit): INIT's own `delegate` (`evo/delegate` 4258c2e, amended in place as
   48b6de1 in `~/.init/work/wt-delegate`, read-only) copied unchanged into `seed/`. Review found:
   a delegate's `tool_start` was sent twice, sub-requests had no task label, and the repo-side
   tools.md §4.1 block was stale (a body checkout has no `docs/`, so INIT could not see that test).
2. **On top** (commit 2, `seed/`):
   - parallel fan-out: a batch's delegates run on worker threads, `INIT_DELEGATE_PARALLEL` (4,
     ceiling 8) at once, results in call order; writers form one unit (one at a time). Workers ask
     the batch thread to render (it owns `&mut` context) over a channel.
   - `mode`: `read` (default) / `write` (see "Read mode" below).
   - bounds (non-dollar, configurable, per-call lowerable): turns 8, tool calls 24, requests 12
     (retries count), tokens 1,000,000 (input+output), wall 600 s. Last-turn note; calls on the
     last turn do not run. Wall time is hard: child `Abort` per sub-task + watchdog; an owner
     abort reaches every child's stream and bash groups (`Abort::child`, `groups()` includes them).
   - no recursion (nested `delegate` → `not_available`).
   - durable record `state/spill/<session>/delegate-<call_id>.json` on every exit path (end
     reason, bounds, counts, tokens, wall, summary, transcript); result ≤ 8 KiB incl. record path.
   - attribution: sub-requests carry the parent's `x-init-task` and `x-init-subtask:
     delegate-<call id>` (kernel proxy already records `subtask` in every trace record).
   - display: inner calls send `tool_start`/`tool_end` with `parent_call_id`,
     `call_id = <delegate>/<inner>`.
   - `skills/delegate/SKILL.md`; docs/body/tools.md §3.7 + §4.1 block (registry 4,986 B).
     GENESIS.md unchanged (1,390/1,400 B; the skill is in the head's skills index).
3. **TUI + relay** (commit 3): nested, collapsed `↳` line per delegate row (calls, names,
   failures, running inner call); expanded view one level deeper; line mode indents. init-proto
   test proves `parent_call_id` survives the relay (D7 was already closed by WP26's pass-through,
   so no kernel code change was needed).
4. **self.json** regenerated through the G1 bless path (commit 4).

## Read mode — what it actually enforces
- Hard (body-enforced): `write` and `edit` calls inside a read sub-task are refused
  (`not_available`), and `delegate` is refused at any mode.
- Not enforced: `bash` and `ext` run with the body's own Seatbelt rights. Seatbelt cannot nest
  (kernel/sandbox.md §3.11) and no read-only lane is requestable from the body, so a shell
  command or an ext (which mounts `/work` read-write) can still write. The tool description, the
  opening text and the skill say so. Hard read-only scouts need a Root-spawned member (Phase 2).

## Tests ($0, mock model; test homes under the worktree's `seed/target/`, `tui/target/`)
- `seed`: `cargo test --offline --no-fail-fast` with `INIT_MOCKLLM_BIN` → 311 + 19 + 9 + 9 passed,
  0 failed. New: t99 (ported+extended), t100 fan-out (peak ≤ cap, writers peak 1, call order,
  wall < sequential), t101 read mode, t102 bounds (turn cap, call cap, 1 s wall cap on a stalled
  request), t103 owner abort (stalled stream + sub-task bash group killed < 500 ms), 7 unit tests.
- `seed` clippy `--all-targets -D warnings`: clean.
- `tui`: `cargo test --bin init-tui` 74 passed (incl. golden `delegate_nested`); clippy clean.
  The e2e binaries (`tests/proto.rs`, `tests/tty.rs`) fail in this worktree only: their socket
  path under the long worktree path exceeds SUN_LEN (`path must be shorter than SUN_LEN`); not
  related to this change — rerun on main after merge.
- `kernel`: `cargo test -p init-proto` 42 passed; clippy `-p init-proto` clean.
- seed G0 + G1 (`kernel-gate --test seed_g0 -- --ignored`, real Seatbelt `kernel` profile,
  shipped init-mockllm): G1 bless regenerated `seed/self.json` (write mode, then gate mode agrees);
  the check run passed: `pass=true`, check/clippy/ext/test ok, G1 compare ok (646 s, cold build).
  It needs `INIT_SEED_G0_DIR` on a short path: under the worktree path the seed's boot tests hit
  their own socket-path length guard (tests/boot.rs MAX_SOCK). Used
  `/Users/quanduong/Documents/INIT Agent/seed/target/g0d`, removed afterwards (exact path).

## Deviations
- The brief's "fix D7 for parent_call_id": D7 was already closed (supervise.md §4.6 extras pass
  through); added a test instead of code.
- GENESIS.md got no line: 10 bytes of budget left; the skills index already lists the skill.
- Commit 1 is the exact port (index plumbing, not re-tested in isolation); commits 2–4 are tested.

## Proposed for the Root (not built)
The Root's per-turn signals (H10 requests/turn, H5b cost/turn) cannot tell sub-task traffic apart
yet: the kernel forwards only `x-egress-caps`. Proposal (tools.md §3.7): the kernel forwards
`x-init-subtask` as `x-egress-subtask` (the Root strips it before the provider); the sentinel's
`Turn` counts `sub_requests`/`sub_cost_musd`; H10/H5b test the main-loop share or normalize by
delegate calls, so adopting `delegate` is not read as a per-turn regression. Sealed-lane M7 caps
count sub-task requests as turns (`x-init-purpose: turn`), which is correct but uses the budget.

## Needs from others
- Root/kernel owners: decide the attribution proposal above.
- TUI-UX: `Activity::tools()` now excludes sub-task calls (`all_tools()` has them); merge note.

## Left (Phase 2)
- Root-carved member lane (hard read-only scouts, per-child Root bounds, crash containment).
- Optional `cwd`/worktree argument for writer sub-tasks.
