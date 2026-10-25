# GBAL: goal-directed self-modification (ADR-013), and the demo's bugs

Status: **built and tested at $0** on branch `gbal` (worktree agent-a9ff9ed9e096a0361), 2026-10-25. Not pushed, not
released. The real-model proof **ran once on deepseek-flash** ($0.065, 2026-10-25 20:33): see build/status/GBAL-REAL.md
(findings G1 and G3 fixed then; G2, G4, G5, G6 fixed in 21d2ab7; the rerun, 2026-10-25 22:26–22:57, three real runs, $0.188,
confirms G2 and G4 and found G7: see GBAL-REAL.md "Rerun"). Merged into `main` locally after the rerun.

## What was built

| Part | Where | What |
|---|---|---|
| Decision | `docs/adr/013-goal-based-agentic-loop.md` | the approved architecture, record schemas, the empty-instrument rule, alternatives |
| Root (fixed) | `root/crates/init-root/src/goal.rs` (+ `goal/tests.rs`) | goal record (owner-only), per-goal meter, prediction record, 6 observation cards and readings, outcomes, Brier calibration → check-in interval, guardrail/gate rollback, `met` verification |
| Root wiring | `lineage/kinds.rs` (+5 kinds), `sup.rs` (`goal.*`/`card.*` on the control link), `link.rs` (`/goal` lines, help), `main.rs` (turn hook), `sentinel/{mod,engine}.rs` (turn input/cache-read tokens; `change.open` → `goal::opened`), `egress.rs` (tap usage → turn) | |
| Kernel | `init-proto` (`CommandName::Goal`, `GoalArgs`, `KernelRequest::Goal`), `kernel-serve` + `init-kernel` (relay), `initctl` (`initctl goal <op> key=value…`), `kernel-gate` (`Measure: card:<id>[ gate]`), `kernel-supervise/promote/admit.rs` (change.open carries card measures), `init-testkit` fake Root | |
| Seed (T0, rewritable) | `seed/skills/goal-loop/SKILL.md`, `seed/skills/gbal/SKILL.md`, `seed/self.json` (re-blessed) | layer 1 and layer 2 methods |
| TUI | `tui/crates/init-tui/src/proto.rs` | `/goal` is a Root line; in the completion list |
| Docs | ADR-013; root.md §3.10.5 note; gate.md §3.12 no_instrument; guide.md (daily-use `/rollback`, `/goal`; disk floor + 2 GiB); CHANGELOG "Unreleased" | |
| Acceptance | `scenarios/root/gbal.json`, `root/crates/init-root/tests/gbal.rs` | mock-model e2e; records in `build/status/GBAL-e2e-records.json`, summary `GBAL-e2e.txt` |
| Real-model plan | `tests/real/gbal_proof.sh` | guarded, unrun |

### Commands

- Owner: `/goal "<intent>" [done=card:<id><=N|>=N|exemplar:<sha256>|owner] [usd=1.00] [wall=24h] [rounds=10] [guard=<card>[:<tol‰>],…]`;
  `/goal` (status), `/goal pause|resume|stop|continue`, `/goal accept|reject [P-id]`, `/goal adopt gp-NNNNNN [options]`.
- Agent: `initctl goal status | propose intent=… | predict sha=… mode=A|B|hybrid card=… expect=better|worse|same p_permille=N note=… |
  read card=… | measure card=… sha=… n=5 gate=true | stop reason=met|budget|plateau|instrument_gap|owner_stop|harm note=…`;
  commit trailers `Measure: card:<id>` / `Measure: card:<id> gate`.
- Cards: `cost_turn` (µUSD), `wall_turn` (ms), `requests_turn`, `cache_read` (‰ input), `provider_errors`, `corrections` (‰).

### Record schemas (Root kinds)
See ADR-013 §Records: `goal`, `goal_proposal`, `goal_predict`, `card_reading`, `goal_outcome`. State in
`ROOT_HOME/state/goals/{state.json,<id>.md}` (0600). Readings: n, mean, sample sd, Welch se of Δ, z in ‰ of se (positive =
better), verdict better|worse|noise|no_noise_floor (< 5 baseline turns).

## Bugs fixed (each with a test and a CHANGELOG line)

| Id | Bug (demo 2026-10-25) | Fix | Test |
|---|---|---|---|
| F1 | fresh gated install: G4 with no S and an empty pool → inconclusive → escalated → `inconclusive_final` → rollback 1.3 s after a good T2 swap | `inconclusive: no_instrument` before any run, resolved at once to `accept: no_instrument` (LKG on G0–G3 alone, like T0), `g4_no_instrument` alert; one task anywhere keeps the old path | kernel-gate `no_instrument_is_no_evidence_not_a_rollback`; kernel-supervise `f7_f11_f13_f14_g4_branches` (new fresh-install step: accept, LKG moves, no rollback, no escalation, alert, Root lineage record) |
| F3 (serious) | owner `/rollback` quarantined GENESIS.md, SELF.md and 4 seed skills | a parent tuple with no t0 blob makes T0 caps `baseline` (learned, never attributed to a group); excluded in the sentinel fold, `change.open`, `select::without`, `snapshot` settled | `extend::tests::f3_seed_caps_on_a_parent_without_t0_are_baseline_never_attributed` |
| F2 | mode switch gated→free mid-session: kernel kept gated, handoff refused (`unwatched_change`), L0→L0 "demotion", whole-system restart | `charter.apply` pushes a `ceilings` event with `extend` when the mode changes; the kernel's ceilings refresh re-reads `extend.mode`; an L0 incident is `ladder{direction:"floor"}` + `incident` alert (still opens the incident, answers the trigger, counts toward 3-in-24-h safe mode, holds the L1 climb) | root `tests/gbal.rs::f2_a_charter_mode_switch_reaches_the_running_kernel`; `ledger::tests::an_incident_at_l0_is_floor_not_a_demotion`; wp25/x8 assertions updated |
| F4 | gate notices were `[kernel] gate` with no reason | `supervisor::notice_text`: stage, verdict, sha, reason and what happens next | `notice_tests::f4_gate_notices_say_why` |
| F5 (docs) | `/rollback` missing from guide; free-path disk need undocumented | guide.md daily-use rows, the disk floor + 2 GiB note | — |
| H2 ack race (e2e flake, not the demo) | `link.rs::deliver` sends `owner.in`, drops the pending lock, then reports the line to the sentinel; a fast kernel's ack in between found no line and was lost, so the line went deaf at 30 s and two in a row fired H2 (no group: kernel restart; groups open: `rollback_all`). `gbal.rs` failed ~2/6 in step 4 (`a rollback is in progress` at B's open, or no H12 veto) | the sentinel keeps an ack with no line (last 64, not persisted); the line, when it comes, is born acked | sentinel `an_ack_before_its_line_is_not_deafness`; `tests/gbal.rs` 10 consecutive passes |

**Empty-pool fix, reasoning.** Two options were on the table: treat an empty instrument as "no G4 evidence", or default
fresh installs to `free`. The default flip is an owner-level change whose preconditions ADR-011 states (definition of done:
the real-model red team, the clustering bound measured on 14 days of real turns; HANDOFF §9) and which are not met. The
empty-instrument rule is local and honest: escalating zero tasks gives zero tasks, so "inconclusive → incumbent wins" was
a rollback on no evidence at all. Keeping the version on G0–G3 is exactly what gated mode already does for T0. Gate records
never enter the ledger fold, so it unlocks nothing; an L3 kernel that wanted to fake a pass could already fake any verdict,
which is why the Root's sealed S exists.

## Test results (this worktree, macOS, $0)

| Suite | Result |
|---|---|
| **root, full suite** (`cargo test -p init-root --features test-fixtures`, run from a short-path APFS clone `~/.gw/src` because this worktree's path breaks SUN_LEN/ROOT_HOME limits) | **319 passed, 0 failed, 3 ignored** across the unit binary (239) and 21 integration binaries (access, fix5, gbal 2/2, m7, memwatch, owner_frames, redteam, rsafe, rsec, wp20–wp25, x3, x5, x67, x8). The one failure of the first run (`select::tests::rf22_rf23…`, expected `direction:"down"` at L0) was the F2 change and is fixed; wp25's alert count likewise |
| kernel-gate lib | 245 passed (+1 new) |
| kernel-gate evo (RF-06 Measure table, card rows) | 65 passed |
| kernel-supervise lib | 130 passed (+1 new F4) |
| kernel-supervise `promote` G4 branches + sv26 | 2/2 (test homes need a short path: `kernel/target/test-homes` → `~/.gw/k`) |
| init-proto | 43 + golden 6/6 (goal row) |
| initctl | unit 7/7 (goal golden row); `tests/cli.rs::client_timeout_exits_2` fails on SUN_LEN in this worktree path (environment) |
| kernel-serve | 20 passed |
| init-tui unit | 79/79 (line_kinds + `/goal`); its e2e_root/tty integration tests refuse to run outside `…/target/test-homes/tui/` (a safety assert) — not runnable from this worktree layout |
| clippy `-D warnings` | goal.rs and every touched Root file clean. The Root workspace has pre-existing clippy errors at dddb338 (`sealed/slice.rs`, `zone.rs`, `egress/money.rs`, `tests/wp22.rs`); the kernel has pre-existing `kernel-proxy/config.rs` collapsible_if and `kupgrade/kstate.rs`; none in changed code |
| seed | no seed code changed; `self.json` regenerated in G1 gate mode (the same procedure reproduced HEAD's file byte for byte first) |


## The e2e scenario output (`scenarios/root/gbal.json`, mock model, fake clock)

```
requests_turn aaaa noise  n 6→5  mean 2000→2000         sd 0     se 0    z 0       registered  none
cost_turn     aaaa better n 6→5  mean 840000→540000 m‰  sd 16970 se 8123 z 36932   registered  none
wall_turn     aaaa noise  n 6→5  (fake clock: wall 0)                              registered  none
requests_turn bbbb worse  n 5→5  mean 2000→4000         sd 0     se 0    z -99999  registered  ROLLBACK
cost_turn     bbbb worse  n 5→5  mean 540000→601600     sd 9486  se 4706 z -13089  registered  none
outcome P-000001 (A) p 0.80 o 1 card  brier 0.04  n 1
outcome P-000002 (B) p 0.60 o 0 harm  brier 0.20  n 2
harm H12 groups [2]; change_close group 2 rolled_back trigger H12; extend.rollback → A's tuple; no cap quarantined
goal set by owner → goal stop by agent, reason met, verified true (A's kept reading 540 ≤ 600 µUSD/turn), spent 10,748 µUSD, 2 rounds
```
(Before the H2 ack-race fix an `H2` harm with no group sometimes appeared here, and in about 2 runs of 6 an H2 with
groups open rolled A and B back and the test failed in step 4. It was not the fake clock: the kernel's `owner.in` ack
could reach the sentinel before link.rs reported the line, the line stayed unacked and counted as deaf. See
"H2 ack race" in the bugs table above.)

## Deviations from the approved architecture

1. **Guardrail rollbacks use H12's path** (honoured, never counted toward the ledger) rather than counting as a direct harm
   rollback: the architecture says calibration must not feed L0–L4, and a 2σ card is far weaker evidence than H-signals at
   α = 0.002, so letting it cost the agent clean groups would make GBAL an autonomy lever.
2. **Calibration scales one freedom, the check-in interval** (rounds between owner check-ins, 1–12), enforced by refusing
   `goal.predict` with `goal.checkin`. Rounds and budget stay the owner's caps; calibration never raises them.
3. **Registration is anchored to the change's `tuple` record** (the moment it went live), not to a "change group" id that
   does not exist yet when the agent predicts; the prediction names the commit sha.
4. **Owner guardrails apply to every change that goes live during the goal**, predicted or not (a new tuple seen in an owner
   turn registers the readings), so skipping a prediction does not skip a guardrail.
5. **Outcomes for mode B** come only from the owner (`/goal accept|reject`) or from harm; panel verdicts never reach the
   Root (they would let the agent grade its own forecasts).
6. `met` is Root-verified only for card goals (the newest reading of a change not rolled back); other kinds are `verified: null`.
7. Phase 2 task cards and phase 3 capture are not built (phase 3 is a design note in ADR-013 §2.2).
8. One active goal at a time (a new `/goal` pauses the old one): it keeps per-goal spend attribution exact.

## Root line count vs ADR-007

Approximate root.md §4.8 count (non-blank, non-`//` lines of `src/**/*.rs`, test files and trailing `#[cfg(test)] mod`
excluded; script in the session scratchpad): **17,513 at dddb338 → 18,186 at the branch head, +673** (`goal.rs` 644, the
rest wiring and F2/F3). The ≈ 7,500 budget table in root.md §4.8 was last measured at cdb49e8 and the Root has grown past it
since (SBX-009, M7, ADR-011 lanes); the release measurement must add a `goal` sub-budget line (measured + 5%) and the owner
should decide whether the total budget is re-set.

## Open issues

- **Disk/worktree environment:** this worktree's absolute path is ~45 bytes longer than the main checkout's, which pushes
  several tests over SUN_LEN/ROOT_HOME limits; I ran them with `target/test-homes` symlinked to `~/.gw/{k,r,t}` (removed at
  the end). The TUI e2e tests cannot run here at all (their path assert).
- **Pre-existing clippy errors** in root and kernel (listed above) make `ci/local.sh`'s clippy steps red independent of this
  branch; a separate small cleanup is due.
- **F6/F7 of the demo** (G0 sensitive to `INIT_HOME` length; G3 keeps building after a free-path rollback) and **F4's boot
  block stale cause** (demo "F4": the boot block after an owner rollback names the earlier ceiling_hit) are not fixed here.
- The fake kernel does not confirm `extend.rollback`, so the e2e asserts the Root's actions (veto, close, event), not the
  kernel's body swap; the existing x3 tests cover the confirmation path.
- The cards read owner turns; at low turn rates a reading takes many turns. Background-turn cards were left out on purpose
  (they are not comparable work).
- An agent could lower `cost_turn` by doing less; the skill warns, and `corrections` as a guardrail is the counterweight.

## Real-model proof plan (as prepared; it ran: build/status/GBAL-REAL.md)

`tests/real/gbal_proof.sh` (guarded by `GBAL_I_MEAN_IT=1` and `GBAL_CAP_USD ≤ 1.00`; isolated homes under `~/.gbal-proof`;
refuses below 22 GiB free; parts marked UNVERIFIED must be dry-run on the mock first). deepseek-flash, free mode:
owner goal with `guard=provider_errors` → 5 baseline turns → the model runs skills/gbal (noise floor, predict, one T0
change) → 5 turns → a deliberately regressing change → 5 turns → stop. Pass = the record chain in the lineage and spend ≤ cap.

**Cost estimate.** ~18 short owner turns + 3 working turns. At deepseek-flash prices ($0.30/M input, $1.20/M output, cache
hits billed lower) and the measured ~$0.002 for a short live turn (HANDOFF), short turns ≈ $0.04 total; working turns
(skill reading, a T0 change, ~10 requests on a ~25k-token head) ≈ $0.03–0.10 each. **Expected ≈ $0.15–0.30, worst case
≈ $0.60; recommended cap $0.60.** Wall ≈ 30–40 min. Off-peak (UTC 16:30–00:30) halves it.
