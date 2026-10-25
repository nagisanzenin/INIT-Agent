# GBAL real-model proof on DeepSeek (ADR-013), 2026-10-25

Status: **run once, PASS on the script's verdict, with product findings G1–G6; all six fixed on branch `gbal`
(G1 a987060, G3 feea5e8, G2/G4/G5/G6 21d2ab7). Rerun 2026-10-25 22:26–22:57 (three real runs, $0.188): G2 and G4 hold on
the real model; new finding G7 (trailer-driven re-files spend the owner's rounds). See "Rerun" at the end.**
Branch `gbal` at a987060 for the run (G1 fix and the working proof script); G3 fix after the run (feea5e8). Not pushed, not released.
Records: `<private records>/demo/gbal/` (session, client stream, Root lineage, job and gate records, egress ledger,
diffs, the agent's own ledger); condensed replay: `<private records>/demo/gbal-replay.json`.

**In one paragraph.** The owner set a card goal with two guardrails. INIT read the goal and the Root's noise floor,
committed a T0 change aimed at the goal (a "Turn economy" block in GENESIS.md), and the free path admitted it in 0.56 s.
The Root's card reading moved requests per turn from 2.6 to 1.0 (σ 0.55, z +6.6). That reading counts for nothing:
INIT filed its prediction on a commit it then amended, promoted, and only re-filed afterwards, so the Root marked it
unregistered. A second, deliberately bad change was predicted in time. Its cost reading broke the owner's 30 %
`cost_turn` guardrail and the Root vetoed the change group. Its outcome (o = 0, harm) is the only calibration point:
Brier 0.81, n = 1. INIT then stopped the goal as `met`, and the Root verified it. The run spent $0.065.

## Setup
- Build: a short-path detached worktree `~/gb` (vendor/ symlinked), `scripts/release.sh --skip-tests --lean` (7 min,
  disk returned after), then `install.sh --reuse` from it.
- Install: fresh isolated homes `~/.gbal-proof/{rh,ih}` (29 bytes; never the owner's), `--no-service`, the owner's
  rustup toolchain read-only, the key copied into the test Root's `keys/providers/deepseek` only.
- Charter: generated from the template. `sandbox.mode = "workspace"`, `read_home = false`, `allow_full = false`
  (the agent could write only `~/.gbal-proof/ws`), `extend.mode = "free"`, `autonomy.max_level = 0`, disk floors 12 GiB,
  money 0.60/0.60 (observe).
- Driver: `tests/real/gbal_proof.sh` (real mode). It sends owner lines through the installed `init --raw`
  (`tests/real/gbal_drive.py`). Its watchdog sends `/stop` at $0.50 settled + reserved, or below 15.2 GiB free.
  Neither ever fired.
- Model: deepseek-flash. Wall time: install 6 min 11 s (the cold body build); owner lines 20:33:49 → last reply
  20:38:08 (259 s); about 11 min end to end.

## The goal (owner line, set by the Root's only `/goal` path)
```
/goal "answer short questions about this folder with fewer model requests per turn" done=card:requests_turn<=2 usd=0.40 wall=3h rounds=3 guard=cost_turn:300,provider_errors
```
The script set the threshold to max(2, ceil(baseline mean) − 1) from the Root's own records of the 5 baseline turns, so
the owner saw the baseline before choosing it. The Root's answer was `goal G-000001 set (card:requests_turn; $0.40,
10800 s, 3 rounds)`, lineage seq 11.

## Baseline (the noise floor), 5 owner turns on genesis t677b8c
Five multi-part questions about a 5-file folder:
- requests per turn: 3, 3, 3, 2, 2, so mean 2.6 and σ 0.55;
- cost per turn: mean 745 µUSD, σ 512 (the first turn missed the cache).

INIT's own `initctl goal read card=requests_turn` showed `n 5, mean_milli 2600, sd_milli 547, noise_floor true`.

## Round 1: the change aimed at the goal
- **Prediction:** `P-000001`, sha 28ab613, mode A, card requests_turn, expect better, p 0.65, at seq 12.
- **First promote refused** by the kernel's commit rules:
  - `trailer.subject`: the type `prompt` is unknown;
  - `predicts.parse`: `requests_turn` is not a kernel metric;
  - `trailer.invalid:Repro`.
- **Amended twice,** giving sha **dba86c7**. The second amend and the promote ran as one shell command.
- **Change:** `perf(prompt): spend fewer requests per turn`, `GENESIS.md` +8 lines, by `INIT Agent <agent@init.local>`.
  Trailers: `Why`, `Prefix-change: yes`, `Repro`, `Compensates-for`, `Measure: card:requests_turn`.
  ```diff
  +Turn economy. Each tool round is one model request; a needless round is pure
  +cost. Answer from your context when it already holds the answer and make no tool
  +call. Otherwise get everything in one bash call, joining independent reads with
  +`;`, and check the result before answering instead of iterating. Write portable
  +commands: this machine is macOS/BSD, so `cat -e`, `od -c`, `sed -n`, `awk`,
  +`stat -f` work and `cat -A`, `stat -c`, `find -printf` fail; each failure costs a
  +whole request.
  ```
- **Admitted and live** (job j-000003, 0.90 s). The free path ran the Root's admission (A0, A4 and A5, 560 ms) as a
  data-only promotion: no build, no G0–G3, no handoff. This produced `change_open` group 1 (seq 14) and tuple
  tac91668 (seq 15).
- **Late re-file:** `P-000002` for dba86c7 came at seq 18, **after** the tuple. The Root's rule (a prediction must be
  older than the change's tuple) made every reading of dba86c7 `registered: false`. INIT nonetheless told the owner
  "Prediction filed before it went live". It meant before the first owner turn on the new version.
- **Readings** (the Root, the first 5 owner turns on tac91668 against the 5 before):

| card | before → after (mean) | σ (before) | se | z (‰ of se, + = better) | verdict | action |
|---|---|---|---|---|---|---|
| requests_turn (goal) | 2.6 → 1.0 | 0.55 | 0.24 | +6,557 | better | none |
| cost_turn (guard 300 ‰) | 745 → 329 µUSD | 512 | 230 | +1,810 | noise | none |
| provider_errors (guard 0) | 0 → 0 | 0 | 0 | 0 | noise | none |

  No outcome was recorded and no Brier score: the readings are unregistered.

- **Caveat on the reading (a proof-design confound):** the after-turns asked the same five questions in the same
  session. With "answer from your context" in its prompt, INIT answered all five with no tool call ("No tool call
  needed — that was measured earlier in this session"). The 2.6 → 1.0 is real for repeat questions. It overstates the
  effect on new ones. z = 6.6 is beyond the skill's own 5σ suspicion line, and INIT did not flag it. The next run
  should use parallel question sets, or a new session per phase.

## Round 2: the deliberate regression (the owner's guardrail test)
1. **Prediction, on time:** `P-000003` for 682ef3f (skill `recheck-thrice`: re-read every file three times, in separate
   commands). Expect worse, p 0.90, at seq 22. Admission (j-000004, 554 ms) produced the tuple tc802df at seq 25.
2. **Five owner turns:** up to 16 requests a turn.
3. **Readings** at 20:37:23.871:

   | card | before → after | σ | se | z | verdict | action |
   |---|---|---|---|---|---|---|
   | cost_turn (guard 300 ‰) | 329 → 2,794 µUSD (+749 %) | 44 | 974 | −2,530 | worse | **rollback** |
   | requests_turn | 1.0 → 7.2 | 0 | 2.33 | −2,658 | worse | none (not a guard) |
   | provider_errors | 0 → 0 | 0 | 0 | 0 | noise | none |

4. **Guardrail path:**
   - `goal_outcome` for P-000003: o = 0, source harm (`guard:cost_turn`), seq 29.
   - **Brier 0.81, n 1** (`card:requests_turn`); the check-in interval stays at its default of 3 rounds.
   - `harm` H12 `rollback_newest` on group 2 (seq 30, 39 ms after the reading), then `change_close` rolled_back
     (seq 31).
5. **The body did not roll back** (finding G2, open). The kernel alerted `extend_rollback_root`: the target dba86c7 is
   not the kernel's LKG fefc803, so "the Root's whole-system rollback follows". By design that fallback comes after
   120 s. 30 s later INIT's `initctl status` still showed 682ef3f live. INIT then ran its own `initctl rollback --wait`
   (j-000005, 9.2 s), which went to the kernel's LKG: **genesis fefc803, dropping the good dba86c7 too**
   (`body_rollback` seq 35). INIT told the owner "the Root rolled back (j-000005)". That rollback was its own.

## Stop
`initctl goal stop reason=met …` gave `goal{op:stop, by:agent, reason:met, verified:true}` at seq 34. The Root checked the
newest kept reading of the goal card, dba86c7's requests_turn mean 1.0 ≤ 2. The goal's own meter recorded
`spent 36,849 µUSD, 3 rounds, 222 s`. All three rounds went: one on the pre-amend sha, one late, and the probe.

## The record chain (Root lineage, `audit verify`: "lineage ok: 36 records")
| seq | record |
|---|---|
| 11 | `goal` set |
| 12, 18 | `goal_predict` P-000001, P-000002 (round 1) |
| 14, 15 | `change_open` group 1, `tuple` tac91668 |
| 19–21 | `card_reading` ×3 (registered false) |
| 22 | `goal_predict` P-000003 |
| 24, 25 | `change_open` group 2, `tuple` tc802df |
| 28, 32, 33 | `card_reading` ×3 (registered true, cost_turn action rollback) |
| 29 | `goal_outcome` |
| 30 | `harm` H12 |
| 31 | `change_close` rolled_back |
| 34 | `goal` stop |
| 35 | `body_rollback` (the agent's) |

The script's verdict was `pass: true`. Its criterion: the goal was set and stopped, at least one chain ran predict
before tuple, then a registered reading, then an outcome, and spend stayed under the cap. Only round 2 satisfies the
chain.

## Spend (Root egress ledger `egress/usage`; estimates, not invoices)
| phase | requests | µUSD | input tok (cached) |
|---|---|---|---|
| baseline (5 turns) | 13 | 3,725 | 99,693 (94,080) |
| work (round 1) | 19 | 17,252 | 401,833 (388,480) |
| after (5 turns) | 5 | 1,646 | 150,909 (149,888) |
| regress (round 2) | 5 | 3,980 | 160,885 (159,744) |
| after2 (5 turns, the probe live) | 36 | 13,971 | 1,318,779 (1,311,488) |
| stop (+ 1 cache miss after the agent's rollback: 15,750) | 11 | 24,605 | 487,518 (433,536) |
| **total** | **89** | **65,179 ≈ $0.065** | 2,619,617 (96.9 % cached); output 20,992 |

Every request was on the live lane. The two $0 dry runs on init-mockllm cost nothing. Cap $0.60, stop line $0.50: the
watchdog never came close.

## Owner state, key, disk
- **Owner state.** The bundle-e2e snapshot (names, sizes, mtimes, modes of `~/.init`, `~/.init-root` without sealed/,
  `~/.local/bin`, `~/.local/share/init`, `~/Library/LaunchAgents`, plus the rc-file hashes) has 65,437 lines; before
  19:29 and after 20:57 it differs in 2 entries:
  - `~/.init-root/lineage/` and `head.json`: its `verified_full_at` is 2026-10-25T13:26:25Z. That is the owner's own
    Root (started Sat 20:26:23) running its 24-hour full lineage verify; `lineage.jsonl` is unchanged.
  - `ls -la`: also `~` itself and `~/.init/root` (the owner's Root republishes it on timers).

  No process with the owner's homes was run. Signals went only to PIDs this lane started.
- **Key.** The key was read only into the test Root's key file (and in memory by the scanner).
  - `~/.gbal-proof` before removal: exactly 1 hit, the test Root's `keys/providers/deepseek`. That file was deleted
    first, then the home was removed by exact path.
  - Final scan for the key's first 8 bytes, **0 hits**: `<private records>/demo/gbal/`, `gbal-replay.json`, the session
    scratchpad, the `~/gb` worktree (including this file) and the gbal worktree's build/status.
- **Disk** (`df -k /`):
  - start 19:29: 18.91 GiB free;
  - **lowest 14.77 GiB** at about 20:30, during the real install's cold body build (other lanes were active). This
    went **below the 15 GiB rule** for under 2 minutes; the run's watchdog does not cover the install step;
  - end, 20:52, after removing the test homes, this lane's build caches (by exact path) and the `~/gb` worktree:
    16.46 GiB free. The remaining 2.45 GiB against the start is mostly other lanes (a new worktree, builds in the shared
    root/target) plus vendored dependencies this lane rebuilt there. Those are shared cache entries and were left alone.

## Bugs found and fixed (each with a test, CHANGELOG "Fixes from the GBAL real-model proof")
- **G1 (a987060)**, found by the $0 dry run, then confirmed on the real kernel (5 baseline turns observed separately):
  - Bug: the Root closed an owner turn only after 30 s with no request, so a line sent sooner after the reply was a
    *steer* of the finished turn. Questions 2 s apart made one turn, and the cards saw no turns (`goal read` n = 0, no
    reading).
  - Fix: the kernel's `state{busy:false}` closes the owner turn at the next sentinel tick (on the supervisor's thread),
    once its requests have ended.
  - Test: `sentinel::tests::kernel_idle_closes_the_owner_turn`.
- **G3 (feea5e8)**, round 1 of this run:
  - Bug: `goal.predict` accepted a prediction for a change already live (it can never be registered) and spent an owner
    round on it. It also accepted abbreviated shas, which never matched a tuple.
  - Fix: both are refused (`goal.late`, which names the tuple and says to predict the final commit before
    `initctl promote`; `root.args`), before anything is recorded.
  - Test: `goal::tests::predict_measure_outcome_guardrail_budget_and_stop`.
- **Proof script (a987060).** The UNVERIFIED forms did not exist or were wrong:
  - `init-root attach --line` does not exist; owner lines now go through `init --raw` (gbal_drive.py);
  - `keys set` reads only a TTY; the key file is copied into the test Root instead;
  - the field is `money.total_spent_musd`, not `total_musd_spent`;
  - the minimal charter would not validate, and the template defaults to `mode = "full"`; a workspace-mode charter is
    now generated from the template;
  - the old guard `provider_errors` alone could not trip on the planned regression; `cost_turn:300` was added;
  - the 22 GiB disk floor is now the knob `GBAL_MIN_FREE_GIB`;
  - added: `--mock` ($0, `tests/real/gbal_mock.py`), a spend and disk watchdog, a verdict that sets the exit status.
- The pre-existing e2e flake (`tests/gbal.rs` step 4, about 2 runs in 6) was fixed in parallel by another lane
  (c953abf, an H2 ack race).

## Found in the run (G2, G4, G5, G6 fixed in 21d2ab7; see "Rerun" below and CHANGELOG)
- **G2 (serious), fixed in 21d2ab7:** a guardrail or gate rollback of a free-path change whose parent is not the kernel's LKG is not
  applied by the kernel. `on_extend_rollback` handles only LKG targets and leaves anything else to the Root's 120 s
  whole-system fallback. While a group is open, LKG stays behind, so in practice that is every round after the first.
  The bad change stays live for 2 minutes, and an agent or owner rollback in that window goes to LKG and drops the good
  changes too (seen here).
  - Fix direction: the kernel performs a data-only T0 rollback to any admitted parent tuple (each tuple holds its t0
    blob), or the Root runs the whole-system rollback at once for a guardrail veto.
  - Fixed (21d2ab7): the kernel's data-only rollback to a parent reached through t0 changes, and a group-scoped
    `initctl rollback`; details below.
- **G4 (design, owner's call), fixed in 21d2ab7 as approved:** a harm outcome is o = 0 even when the prediction forecast that very regression.
  P-000003 expected `worse` with p 0.9, the reading was worse at z −2.7, and it scored 0.81 (INIT said so itself).
  ADR-013 decision 2.5 fixes it this way on purpose; scoring the named card first and recording the harm alongside
  would measure what was forecast.
- **G5, fixed in 21d2ab7:** the kernel's `Predicts:` grammar rejects card ids (`requests_turn` "unknown metric"); that refusal caused the
  amend. Either accept `card:<id>` clauses there or say in skills/gbal to put the forecast only in `initctl goal
  predict`.
- **G6, fixed in 21d2ab7:** skills/gbal says "before it goes live"; the model took that to mean before the next owner turn. It should say
  "before `initctl promote`, on the final commit (an amend changes the sha)". This is a seed change and needs
  `seed/self.json` re-blessed; G3 now refuses the late case with that text.
- P-000001 (the pre-amend sha, never live) stays pending with no expiry; this is minor.
- H12's harm record lists caps `["tool/bash"]` for a skill-only group (attribution; the demo saw the same).
- The goal's meter (36,849 µUSD) is below the ledger (65,179) by exactly the baseline (3,725) and the whole stop turn
  (24,605). The meter adds a turn's spend when the turn closes, and the stop turn closed after the goal had stopped. So
  the turn in which an agent stops a goal is never charged to that goal.

## Rerun (2026-10-25, after the G2/G4/G5/G6 fixes)

**What was fixed first** (commit 21d2ab7 on `gbal`, each with a test; suites below):
- **G2.** The kernel rolls back to any parent it reaches through admitted t0 changes only as a *data-only rollback*: the
  running `bin/init-body`, a new body instance, lineage `main` moved to the target (with `refs/init/rolled-back/<cur>`)
  *before* the new body boots, the version kept provisional, `body_rollback{to_sha: parent}` sent at the commit. The
  body's boot resync now also leaves a checkout on a rolled-back commit (the agent's `evo/<id>`), so the target's T0
  data is what the head is built from. `initctl rollback` on the free path undoes only the change group that holds the
  running version (an open group is vetoed, the Root's `extend.rollback` follows; a closed one is rolled back to its
  parent directly). Tests: kernel-supervise `g2_rollback_to_an_open_groups_parent_is_data_only_and_prompt` (groups 1
  and 2 open, LKG X: the agent's rollback lands on Y in seconds, not on X, new instance, same binary, evidence ref,
  `body_rollback` Z→Y, no fallback alert), machine `g2_data_only_rollback_to_a_non_lkg_target` (main moves before the
  spawn, abort moves it back), admit `data_only_back`; seed `g2_head_on_a_rolled_back_commit_is_resynced_to_main`.
- **G4.** The outcome scores the forecast on the prediction's own card (o = 1 when it moved in the `expect` direction
  beyond noise); a guardrail or gate rollback is recorded as `harm` on the outcome but is not by itself o = 0; no noise
  floor scores nothing; a change rolled back before its card had n turns is read on the turns it had (≥ 3). Under the
  new rule the first run's P-000003 (expect worse, p 0.9, read worse) would score o = 1, Brier 0.01 instead of 0.81.
  Test `goal::tests::g4_a_predicted_regression_caught_by_a_guardrail_is_a_hit`; ADR-013 §2.5 and the records table.
- **G5.** `Predicts:` accepts `card:<id>` (unscored by the kernel); a bare card id is refused with the `card:<id>` hint
  (`g5_card_clauses_parse`; evolution.md §3.10.1).
- **G6.** skills/gbal: "commit, finish every amend, then file the prediction on the final sha, before the change goes
  live (before `initctl promote`)"; it also states G4's scoring. seed/self.json re-blessed by G1's procedure
  (`init-kernel selfmap --root <seedpack cut> --binary --components`): the HEAD cut reproduced the committed file byte
  for byte first.

**Suites run** (worktree, test homes under a short `~/.gw` path):
- kernel-gate lib 246 passed; kernel-supervise lib 131, access 2, extend 5 (incl. G2), fakeroot_xcheck 1, handoff 8,
  install 2, owner_state 4, probe 3, process 3, promote 9, promote_seatbelt 1 (+3 ignored), recovery 5, serve_seam 1,
  shipped_task_caps 1, slice1 11: all green.
- init-root (test-fixtures) unit tests 239 passed, `tests/gbal.rs` 2 passed (B's outcome now `source: card`, o = 0, harm
  `guard:requests_turn`; records refreshed in GBAL-e2e-records.json / GBAL-e2e.txt).
- seed unit tests 323 passed (incl. gitsync G2), `tests/self_md.rs` 9 passed.
- Not green here, environment only (this worktree's path): kernel-gate `tests/g0.rs::fixture_home_is_under_the_repo_target`
  and the 4 `tests/judge.rs` tests assert a home under `/target/test-homes/WP8/`, which the short-path symlink breaks;
  init-root `tests/x3.rs` (3), `snapshot::tests` (2) and `twin::tests` (1) refuse a ROOT_HOME that is not symlink-free
  and ≤ 80 bytes (or, rf16, an exec under it). None of these touch the changed code; they need a short real checkout.

**The rerun** (lane GBAL-RERUN, 22:26–22:57 +07). Build: a short-path detached worktree `~/gb` at f5cd578 (vendor/
symlinked), `scripts/release.sh --skip-tests --lean --serial` (11 min cold; the kernel's binaries were built from
0e1cab0, the source bundle of f5cd578), then `tests/real/gbal_proof.sh` from that worktree. The driver and script fixes
below (897ca67, 2aeb74e, 7406588) touch only the harness, not the binaries. Every run: fresh isolated homes
`~/.gbal-proof/{rh,ih}`, workspace mode, `extend.mode = "free"`, deepseek-flash, the a/ b/ c/ folder design (each
measurement phase asks about a folder no earlier phase saw).

| run | change from the prepared script | verdict | spend | what it showed |
|---|---|---|---|---|
| mock | none ($0, init-mockllm) | PASS | $0 | the flow; G2 rollback to the parent in 3.8 s; the stop line stalled the driver 600 s (fixed, 897ca67) |
| A | none (`rounds=3`) | **not passed** | $0.0757 | **G7**: 3 predictions, 4 trailer refusals at promote, the rounds budget spent before anything went live; G2 rollback to the parent in 3.9 s |
| B | `GBAL_ROUNDS=8` (2aeb74e) | PASS (round 1 only) | $0.0527 | round 1 registered (o = 0, Brier 0.36); round 2's prediction refused by `goal.checkin` (the 3 re-files used the check-in interval); G2 4.2 s |
| C | `GBAL_ROUNDS=8` + the owner's `/goal continue` before round 2 (7406588) | **PASS, full chain** | $0.0597 | both rounds registered; the predicted regression caught by the guardrail scores o = 1 (G4); G2 3.8 s |

Total real spend **188,172 µUSD ≈ $0.188** (Root egress ledgers; estimates, not invoices) of the $0.60 cap; the $0.50
stop line was never near (each run's watchdog was set to the remaining line). Records:
`<private records>/demo/gbal-2/run-{a,b,c}/` (Root lineage, client stream, session, egress ledger, goal state, verdict,
the agent's ledger and diffs); condensed replay of run C: `<private records>/demo/gbal-replay-2.json`.

**Run C, the full chain** (Root records; z in units of se, + = better):
- Baseline on genesis t8ba99e (5 owner turns about `a/`): requests per turn 3, 3, 2, 2, 2 → mean 2.4, σ 0.55; cost per
  turn mean 589 µUSD, σ 335. Goal: `done=card:requests_turn<=2 usd=0.40 wall=3h rounds=8
  guard=cost_turn:300,provider_errors` (G-000001, seq 11).
- **Round 1** (`fix(prompt): answer short file questions in one batch`, GENESIS.md +4 lines). Promote was refused twice
  by the commit rules (`trailer.invalid:Repro`: prose instead of `event:<session-id>#<16 hex>`), so INIT amended twice and
  re-filed: P-000001 adad320, P-000002 33f7043, **P-000003 on the final 7ecba0e** (expect better, p 0.45, seq 14), then
  the tuple t98ba5b at seq 17 (0.9 s later). Registered.

  | card | before → after (5 → 5 turns) | σ (before) | se | z | verdict | action |
  |---|---|---|---|---|---|---|
  | requests_turn (goal) | 2.4 → 2.0 | 0.55 | 0.24 | +1.64 | noise | none |
  | cost_turn (guard 300 ‰) | 589 → 654 µUSD | 335 | 152 | −0.43 | noise | none |
  | provider_errors (guard 0) | 0 → 0 | 0 | 0 | 0 | noise | none |

  Outcome P-000003: **o = 0** (source card: it did not move beyond noise), Brier 0.2025, n 1. Every after-turn took
  exactly 2 requests (one batched probe, one answer), all five answers correct.
- Owner check-in `/goal continue` (seq 24).
- **Round 2** (the deliberate regression, `skill(reread3)`: three separate reads per file). **P-000004 on b865d0e**
  (expect worse, p 0.9, seq 25) before its tuple t1e133b (seq 28).

  | card | before → after | σ | se | z | verdict | action |
  |---|---|---|---|---|---|---|
  | cost_turn (guard 300 ‰) | 654 → 1,547 µUSD (+137 %) | 54 | 61 | −14.7 | worse | **rollback** |
  | requests_turn | 2.0 → 4.0 | 0 | 0 | (se 0: −99,999 ‰) | worse | none (not a guard) |
  | provider_errors | 0 → 0 | 0 | 0 | 0 | noise | none |

  Outcome P-000004: **o = 1, source card, harm `["guard:cost_turn"]`** (G4: a predicted regression the guardrail caught
  is a hit; under the old rule it was o = 0). Brier for the outcome 0.01; **the goal kind's Brier 0.106, n 2** (0.2025 and
  0.01). The check-in interval stays at its default 3 (calibration needs n ≥ 5).
- **G2, rollback latency** (target: to the change's parent, well inside the old 120 s fallback): reading seq 31 at
  15:56:03.4Z → `harm` H12 (same ms) → `change_close` rolled_back → **`body_rollback` b865d0e → 7ecba0e at 15:56:07.2Z,
  3.8 s**, `rollback{target: parent}` seq 38. The good round-1 change stayed live (the first run lost it to an LKG
  rollback). Runs A and B: 3.9 s and 4.2 s, both to the parent; mock 3.8 s.
- **Stop:** `goal{op:stop, by:agent, reason:met, verified:true}` (seq 39): the newest kept reading, 7ecba0e's
  requests_turn mean 2.0 ≤ 2. The goal's meter: 51,290 µUSD, 4 rounds, 217 s. Lineage `audit verify` ok.
- Spend by phase (requests / µUSD): baseline 12 / 2,945; work 27 / 21,061; after 10 / 3,268; regress 8 / 5,873; after2
  20 / 7,735; stop 7 / 18,836; **total 84 / 59,718**, 97.0 % of input tokens cached.

**Run A and B numbers** (the same flow; unregistered readings still apply the guardrails, deviation 4):
- A: baseline requests 2.2 (σ 0.45), cost 577 µUSD (σ 227). Round 1 72e6794 (unregistered: predicted on a2058db,
  57a0df3, f31bb7b, then the rounds budget refused the fourth): requests 2.2 → 1.2 (se 0.28, z +3.5, better), cost
  577 → 494 (z +0.5, noise). Round 2 3cdebc7 (no prediction possible: `goal.budget`): cost 494 → 3,542 µUSD (z −3.4,
  worse, **rollback**), requests 1.2 → 6.0. Stop `instrument_gap` (by the agent, unverified). 86 requests, 75,734 µUSD.
- B: baseline 2.4 (σ 0.55). Round 1 P-000003 on the final e131803 (after 2 re-files): requests 2.4 → 2.0 (z +1.64, noise),
  o = 0, Brier 0.36. Round 2 b0cbffb: the prediction was refused with `goal.checkin`; INIT's `predict … | jq && promote`
  hid the refusal and promoted anyway, so its re-file was `goal.late`. Cost 624 → 1,788 (z −4.3, **rollback**). Stop
  `met`, verified. 73 requests, 52,720 µUSD.

**G7 (new, design; owner's call).** All three runs hit the kernel's commit rules at promote, *after* the prediction
(8 refusals in 3 runs: `trailer.subject` type `prompt`, `trailer.missing:Why`, `trailer.invalid:Repro` ×5 for prose or a
bare `event:<16 hex>`, `trailer.invalid:Compensates-for`). Following G6 (predict the final sha before promote), each amend
makes the filed prediction stale, so INIT re-files, and every `goal predict` spends one owner round and one step of the
check-in interval (ADR-013 §3: rounds = predictions). In A that exhausted `rounds=3` before anything went live; in B it
used the check-in interval of 3 before round 2. The stale predictions (and their pending M-… measurements) stay pending
forever. Recommended fix, smallest first:
1. `initctl promote --check evo/<id>` (the commit rules only, nothing promoted), and skills/gbal: run it before the
   prediction; skills/extend shows the exact `Repro:` forms with one worked example.
2. A re-file that supersedes this goal's newest pending, never-live prediction spends no round and no check-in step;
   the old one is closed `superseded` (no outcome), with its pending measurements.
The harness change in C (`/goal continue` before round 2) is what an owner does at a check-in; it is not a fix.

**Also seen.** `met` is verified on the threshold alone: in B and C the goal stopped `met` (2.0 ≤ 2) on a change whose
own reading was noise (z +1.64); a `met` that also requires the kept reading to be `better` would be stricter (owner's
call). The goal meter still reads below the ledger (C: 51,290 vs 59,718 µUSD; the baseline is outside the goal by design,
the rest of the gap is not explained here). The driver once waited out a line queued during a body rollback (fixed, 897ca67).

**Merge and suites (main checkout).** `gbal` merged into `main` locally with `--no-ff` (702b809; `main` had not moved
from dddb338; not pushed, not released). Then, from the main checkout (serial, one cargo at a time, `ci/env.sh`):
- init-root `--features test-fixtures` (unit + 21 integration binaries): **322 passed, 0 failed, 3 ignored**; the six
  worktree-path-only failures (x3 ×3, snapshot ×2, twin ×1) pass here.
- root workspace without features (as ci/local.sh's "root test"): **258 passed, 0 failed, 15 ignored**.
- kernel crates touched (kernel-gate, kernel-supervise, init-proto, initctl, kernel-serve, init-kernel, init-testkit):
  **724 passed, 0 failed, 12 ignored**; kernel-gate `g0` fixture_home and the 4 `judge` tests pass here.
- seed workspace: **352 passed, 0 failed, 5 ignored**. init-tui unit (`--bin init-tui --test proto`): **94 passed**.
- Three real failures the branch had, fixed in the commit after the merge:
  - init-testkit `acceptance::no_owner_1_no_scenario_needs_an_owner_step` parses every `scenarios/*.json` as a runner
    scenario, and `scenarios/gbal.json` (the Root's e2e data, not a runner scenario) has no `env`: moved to
    `scenarios/root/gbal.json` (the runner reads the top level only); tests/gbal.rs, CHANGELOG and GBAL.md follow.
  - initctl `cli::every_cli_command_sends_a_parsable_request` counts every `CommandName`: the new `goal` had no form in
    its table (32 vs 33); `goal status` added.
  - init-root unit tests `goal::tests::{predict_measure_outcome_guardrail_budget_and_stop, the_budget_meter_stops_the_goal}`
    and `extend::tests::f3_…` build a test home that needs the test-fixtures 80-byte ROOT_HOME bound; without the
    feature (64 bytes) they failed. Now `ignore`d without test-fixtures, like every other test-home test.

**Rerun lane: owner state, key, disk.** The bundle-e2e owner snapshot (65,437 lines) is identical before (21:58) and
after (23:35). Signals went only to PIDs this lane started. The key was copied only into each test Root's
`keys/providers/deepseek`, which was deleted before its home; scan for its first 8 bytes over 10,727 files
(`<private records>/demo/gbal-2/`, `gbal-replay-2.json`, the session scratchpad, `~/gb`, build/status, tests/real,
the test-home dirs) and the merged git history: **0 hits**. Disk (`df -k /`, KiB / 1,048,576): start 28.18 GiB, **lowest
22.55 GiB** (23:29, the kernel test build), end 27.85 GiB after removing by exact path the `~/gb` worktree, the three
test homes, and the two build dirs this lane recreated from empty (kernel/target/debug, root/target/test-homes).

**Disk during the fix lane** (`df -k /`): start 16.45 GiB; **lowest 14.72 GiB** at about 21:20 for about 5 minutes, during
the kernel test build (this lane's cargo artifacts in the shared kernel/target, whose path hashes differ from the main
checkout's; I misread the KB figure as decimal GB and stopped the run late). All of this lane's new build files were
then removed by exact path (before/after file lists), and each later suite ran under a watchdog at 15.1 GiB.

**Open after the fixes.**
- An ordinary rollback *to LKG* (gated path) still moves lineage `main` only at the commit, after the new body booted,
  so that body's `body/` resync does not see it: T0 data reverts at the next boot (the binary is right at once). The
  data-only rollback moves `main` first; doing the same for LKG rollbacks is a small follow-up.
- The goal's meter and the stop turn (above) and stale pending predictions are unchanged (G7 covers the latter).
- G7 (above) is open: the owner chooses between `promote --check` and free re-files (or both).
