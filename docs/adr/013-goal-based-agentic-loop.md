# ADR-013: Goal-based agentic loop (GBAL): the model owns the loop, the Root keeps the goal, the meter and the record

Status: accepted by the owner on 2026-10-25 (architecture); built on branch `gbal` (build/status/GBAL.md); released in
v0.4.0-preview with the three post-rerun fixes (G7 re-files and `promote --check`, attributed `met`, the egress-level
meter; build/status/REL-040.md) · Area: overview
Depends on: ADR-006, ADR-007, ADR-010 (superseded), ADR-011, ADR-012; the owner's design notes (proposal and report, not published)
Defines: the goal record, the per-goal budget meter, the prediction and outcome records, observation cards and their readings, calibration and what it may scale, the `/goal` owner lines, `initctl goal`, `Measure: card:<id>[ gate]`; the empty-instrument G4 rule (§6)

## Context

The owner wants INIT to pursue a stated goal across many changes and to stop when the goal is met or when it can say why
it cannot get there. Two layers (proposal, scope): **layer 1**, goals about external artifacts (a website); **layer 2**,
goals about how INIT itself works ("fewer tokens on big repos"), pursued by changing its own Body. ADR-011's free mode asks
only "undoable? harmful?"; GBAL gives it a direction without making gain an admission rule.

The design assumes the model becomes near- or super-intelligent (proposal §4). Today's judge noise, pairwise accuracy and
over-claiming describe weak models and must not be hard-coded. Goodhart is a property of optimisation, so what decides
"what the number is" must stay out of the optimiser's reach, while what to do with the number is the model's.

## Decision

1. **The model owns the loop (T0, rewritable).** Two seed skills: `goal-loop` (layer 1: frozen yes/no checklist,
   verbatim capture, pairwise judging against the champion with position swap and ≥ 3 runs per judge, a ledger, six stop
   reasons) and `gbal` (layer 2: choose mode A card / B judged / hybrid by three mechanical instrument tests — a noise floor
   from ≥ 5 baseline runs, sensitivity to a known-good change, a deliberate-regression probe; file a prediction before
   measuring; keep a change at Δ > 2σ on the goal with no guardrail regressing beyond noise; confirm a winner once; stop
   with a reason). Nothing in the Root or the kernel runs a loop, counts rounds for the model or judges merit.
2. **The Root keeps five things, mechanically (no LLM in the Root, ADR-011 decision 6):**
   1. **The goal record.** Only an owner line sets it: `/goal "<intent>" [done=card:<id><=N|card:<id>>=N|exemplar:<sha256>|owner]
      [usd=…] [wall=…h|m] [rounds=…] [guard=<card>[:<tol‰>],…]`; `/goal pause|resume|stop|continue`. The agent may only
      propose (`goal.propose`, adopted by `/goal adopt <gp-id>`). One goal is active at a time. The intent text stays off
      the mirrored lineage (sha256 only), as `proposal` does.
   2. **Artifact capture** (phase 3): the bytes an evaluator is shown. *Design note only, not built*: a Root-run capture
      recipe over the Root's workspace snapshot (ADR-011 §2.3), judges on sealed lanes, a bounded read-back, judge spend on
      `Pool::Agent`. It is needed only when panel verdicts start deciding what is kept.
   3. **Per-goal budget metering.** The zone spend of every egress request the Root settles while the goal is active
      (owner and background turns alike), charged at settlement with the same `cost_musd` as the egress ledger's `end`
      record, so the goal's meter reconciles with the ledger to the µUSD over the goal's active time (v0.4: it used to
      add each closed turn's cost and missed the turn in which the agent stopped the goal, and requests that settled
      after their turn closed; GBAL rerun: 51,290 against the ledger's 59,718 µUSD). Also active wall time, and rounds
      (= predictions, less re-files: 2.4). At a spent money or wall budget the Root stops the goal itself
      (`goal{op:stop, reason:budget, by:root}`); a spent rounds budget refuses the next prediction. It stops the goal, never
      egress: the product has no Root money cap (owner decision 2026-10-23).
   4. **The prediction record** (`goal.predict{sha, mode, card?, expect, p_permille}`), filed by the agent through initctl
      and timestamped by its lineage seq. A card reading is `registered` only when a prediction for that sha is older than
      the `tuple` record that made the change live; anything else is `registered: false` (unregistered). A prediction for
      a change already live is refused (`goal.late`). **Re-files (v0.4, GBAL rerun G7):** the kernel's commit rules may
      force an amend after the prediction, which changes the sha. A prediction that replaces the goal's newest prediction
      while that one's change has never gone live is the same round: it spends no round and no check-in step (neither
      budget refuses it), and the old prediction closes `superseded` (`goal_predict{supersedes}`; no outcome, its pending
      readings dropped, and it never registers a reading even if that commit goes live later). To avoid the amend in the
      first place, `initctl promote --check evo/<id>` runs promote's commit checks without promoting (kernel/initctl.md),
      and skills/gbal runs it before every prediction.
   5. **The outcome record and calibration.** Outcomes come from Root observations only: the prediction's own card
      reading, or the owner's `/goal accept|reject` (mode B, or a prediction with no card). Agent- or panel-reported
      verdicts never enter it. **What is scored is the forecast** (GBAL real-model proof, G4): `p_permille` is the agent's
      probability that its card moves in the `expect` direction beyond noise (`better`/`worse`: |z| ≥ 2 that way; `same`:
      within noise), and o = 1 exactly when it did. A guardrail or gate rollback of the change (`harm`) is recorded on the
      outcome but is not by itself o = 0: a regression the agent predicted, which the owner's guardrail then caught, is a
      hit. A reading with no noise floor scores nothing (the prediction resolves with no outcome). A change rolled back
      before its card reading had `n` turns is read on the turns it had (≥ 3). The Brier score per goal kind
      (`card:<id>`, `exemplar`, `owner`) is folded from `goal_outcome` records.
      **Calibration scales one freedom**: the number of rounds between owner check-ins (3 by default; 6 at n ≥ 5 and
      Brier ≤ 0.15; 12 at n ≥ 10 and Brier ≤ 0.08; 1 at n ≥ 5 and Brier > 0.25), always under the owner's budget caps.
      **It never feeds the L0–L4 ledger**: ledger.rs folds none of the five new kinds.
3. **Mode A instruments, phase 1: Root observation cards** computed from data the Root already sees per owner turn:
   `cost_turn` (µUSD), `wall_turn` (ms), `requests_turn`, `cache_read` (‰ of input tokens), `provider_errors`,
   `corrections` (‰ of turns; `/undo`, `/rollback`, interrupts, `/stop` are corrections, ADR-011 §3.1). Tool error rate is
   not a card: tool errors happen inside the body and the Root cannot see them. A reading compares the change's first `n`
   owner turns (default 5) with up to 40 turns of the incumbent before it: n, mean, sample sd (σ), Welch se of Δ, z = Δ/se
   in the better direction, verdict `better | worse | noise | no_noise_floor` (fewer than 5 baseline turns). Exposed as
   `Measure: card:<id>` and `Measure: card:<id> gate` (alongside `Measure: g4`), `initctl goal measure`, and `initctl goal
   read` (the noise floor). Phase 2 (owner-imported task cards in `ROOT_HOME/sealed/cards/`) is not built.
4. **Guardrails and gates roll back, never approve.** An owner guardrail card (`guard=`) is read on every change that goes
   live while the goal is active; a `worse` reading beyond the owner's tolerance rolls the change back. The agent's own
   `card:<id> gate` rolls back unless the reading is `better`. Both use H12's path for an open group (honoured, never
   counted either way) and `extend.rollback` to the parent tuple otherwise. The kernel applies either at once, also when
   the parent is not its LKG (an older group still open): a data-only rollback when every change after the parent is
   t0 (G2; kernel/supervise.md §3.6.7), otherwise the Root's whole-system rollback. A one-sided z ≤ −2 has a ≈ 2.3% false-rollback
   rate per guardrail per measured change; the tolerance and n trade that off and are the owner's.
5. **Admission is unchanged.** `extend.mode` gated/free decides admission as before. GBAL never approves a change; panels
   are advisory (ADR-006 decision 8); the Root never judges merit (ADR-011). `met` is checked by the Root only where it can
   be (a card goal); other goals are the owner's. **`met` requires attribution** (v0.4, owner decision after the GBAL
   rerun, where runs B and C stopped `met` on a reading that was noise, z +1.64): the newest reading of the goal card on a
   change that was not rolled back reaches the threshold, *and* a registered change (kept, in this goal) moved that card
   beyond noise in the direction its prediction named and toward the threshold (`verified: true`, `attributed:
   {prediction, sha, verdict, z_permille}`). Reaching the threshold otherwise (noise, drift, an unpredicted change) is
   recorded as `met_unattributed` (`verified: false`, `claimed: "met"`) with a `goal_met_unattributed` owner alert: the
   goal may be done, but not by the agent's measured doing. A threshold not reached stays `met`, `verified: false`.
6. **No G4 instrument is no G4 evidence, not a rollback** (fix of the fresh-install bug). With no sealed S drawn and an
   empty frozen pool and holdout, G4 is `inconclusive: no_instrument`, resolved at once (no escalation: zero tasks again)
   as `accept: no_instrument`: the version becomes LKG on G0–G3 alone, as a T0 change does (ADR-006 decision 3), with a
   `g4_no_instrument` alert. One task anywhere keeps the old path. We did not flip fresh installs to `free` instead: ADR-011's
   definition of done for that default (the real-model red team, the clustering bound measured on real turns) is not met.

## Records (Root kinds, root.md §3.10.5)

| Kind | Data |
|---|---|
| `goal` | `{id, op: set|pause|resume|checkin|stop, by: owner|agent|root, kind, done, budget{usd_musd, wall_s, rounds}, guard[{card, tol_permille}], spent{musd, wall_s, rounds}, intent_sha256?, intent_bytes?, reason?, verified?, claimed?, attributed?, note_sha256?}` (`reason` adds `met_unattributed`, Root-recorded, v0.4) |
| `goal_proposal` | `{id: gp-NNNNNN, sha256, bytes}` (text in `state/goals/<id>.md`, 0600) |
| `goal_predict` | `{id: P-NNNNNN, goal, goal_kind, sha, mode, card, expect, p_permille, round, note_sha256, supersedes?: P-id}` (a re-file keeps the superseded prediction's `round`) |
| `card_reading` | `{card, unit, sha, tuple, parent_tuple, goal, measure, n_before, n_after, mean_before_milli, mean_after_milli, delta_milli, sd_milli, se_milli, z_permille, verdict, registered, prediction, gate, guard, action: none|rollback}` |
| `goal_outcome` | `{prediction, goal, goal_kind, sha, p_permille, o: 0|1, source: card|owner_accept|owner_reject, reading, harm?: ["guard:<card>"|"gate:<card>"], n, brier_ppm}` (records before G4 may carry `source: harm`, o = 0) |

## Consequences

- Root growth: `src/goal.rs` is 644 counted lines (root.md §4.8 method, approximate), plus about 29 elsewhere. That is
  against ADR-007's ≈ 7,500 budget, which the Root already exceeds at this commit by the same approximate count
  (17,513 → 18,186); the release measurement must set a `goal` sub-budget line, as for SBX-009 and M7.
- Phase 0 panels stay tamper-evident, not tamper-proof: the builder writes the judge's task and chooses the capture. That is
  acceptable while verdicts only steer the agent's own choices (they never reach calibration).
- A goal about INIT itself opens change groups; at L0 the blast-radius budget (4 open groups, 48 changes a day) bounds the
  round rate.

## Alternatives considered

| Alternative | Why not |
|---|---|
| A GBAL engine in the Root or kernel | It would govern the model's choices; loop methods go stale as models improve |
| Panels that approve | ADR-006 decision 8; Goodhart; the agent shapes what the judge sees |
| Agent- or panel-reported outcomes in calibration | The agent would grade its own forecasts |
| Calibration feeding L0–L4 | It would make forecasting an autonomy lever (ADR-006 decision 7: predictions are scored, never gated on) |
| Default fresh installs to `free` | ADR-011's definition of done is not met |
| Tool error rate as a card | Not Root-observable |
