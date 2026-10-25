---
name: gbal
description: Pursue an owner goal about how you work ("fewer tokens on big repos", "stop re-asking") by changing your own Body. Choose mode A (Root card), B (judged) or hybrid with three instrument tests, file a prediction before each change, keep a change only when it beats noise on the goal and no guardrail regresses, record the outcome, stop with a reason. Use when `initctl goal status` shows an active goal.
---

# GBAL: goal-directed self-change

The loop is yours to rewrite. The Root keeps what stays true for any optimiser: the owner's goal and
budget, the prediction record, the card readings and outcomes, your calibration, and the rollback.
You can never set a goal (`initctl goal propose intent=…` asks the owner) and nothing here approves
a change: admission is unchanged, and owner guardrails roll a change back by themselves.

## 1. Read the goal
`initctl goal status`: intent, done definition (`card:<id> <=|>= N`, `exemplar`, or `owner`), budget
(usd, wall, rounds), guardrail cards, `rounds_before_checkin`, your calibration, the cards.

## 2. Choose the mode with three tests, not with confidence
For each candidate instrument (a Root card first, then a cheap deterministic check, then judges):
1. **Noise floor**: at least 5 baseline runs on the unchanged version. For a card,
   `initctl goal read card=<id>` (needs `noise_floor: true`, n ≥ 5 owner turns on the current version).
2. **Sensitivity**: a change you know is good must move it beyond 2σ.
3. **Regression probe**: a deliberate small regression (revert a known fix) must move it the other way.
Mode **A** if the goal's own instrument passes all three; mode **B** (skills/goal-loop's judging, on
your work before and after the change) if none does; **hybrid** (the usual case) when the measurable
parts (cost, requests, cache reads, errors, tests) are guardrails and the goal itself is judged.
Mode A needs a Root card; without one the goal is mode B. Write the choice and the probe results in
`work/goals/<goal-id>/ledger.jsonl`.

## 3. Each round: check, predict, change, measure
1. Commit the change, then run **`initctl promote --check evo/<id>`**: the kernel's commit rules
   (subject, trailers, `Repro:` forms) exactly as promote runs them, nothing promoted. Fix and amend
   until it says `"ok": true`. Only then file the prediction **on that final sha, before the change
   goes live** (before `initctl promote`; an amend makes a new sha, and a prediction for an old one
   never matches):
   `initctl goal predict sha=<full 40-hex sha> mode=A|B|hybrid card=<id> expect=better|worse|same p_permille=<0..1000> note=…`.
   Check its answer (`"ok"` or an error) before you promote: a refused prediction (`goal.checkin`,
   `goal.budget`) is not filed, and promoting anyway leaves the change unregistered.
   If you still had to amend after predicting, predict again on the new sha before promoting: a
   re-file that replaces your newest prediction while its change never went live is the same round
   (it spends no round and no check-in step; the old one closes `superseded`).
   Once the change is live it is too late: the Root refuses (`goal.late`) and the readings stay
   unregistered. `p_permille` is your probability that the card moves in the `expect` direction
   beyond noise (|z| ≥ 2; for `same`, that it stays within noise). Be honest: the Root scores it
   (Brier) against what the card did, and your score sets how many rounds you may run between owner
   check-ins (1 to 12). A guardrail rollback is recorded on the outcome but is not by itself a miss:
   a regression you predicted, with the guardrail catching it, scores as a hit.
2. Promote it as usual; add `Measure: card:<id>` (or `card:<id> gate` to have the Root roll it back
   unless it is better) to the commit, or ask later: `initctl goal measure card=<id> sha=<commit> n=5`.
   A `Predicts:` trailer may restate the forecast as `card:<id> < parent`; the kernel does not score it.
3. Work normally; the Root reads the next owner turns. The reading (`delta`, `sd`, `se`, `z`, verdict
   better | worse | noise | no_noise_floor) is in the lineage and in `initctl goal status` once done.
4. **Keep** when the goal card's z ≥ 2 (Δ beyond 2σ of the difference) and no guardrail regressed beyond
   noise. Confirm a winner once on fresh turns before building on it. A change measured without an
   earlier prediction is recorded `registered: false` and does not count for calibration.
5. A reading 5σ or more beyond anything before it is suspicious: look for a measurement artefact
   (fewer turns, less work) before you keep it. Cost per turn falls if you simply do less.

## 4. Stop with one reason
`initctl goal stop reason=met|budget|plateau|instrument_gap|owner_stop|harm note=…`. For a card goal
the Root checks `met` (`verified`): the newest kept reading must reach the threshold, and a
registered change of yours must have moved the card beyond noise in the direction you predicted
(`attributed`). Reaching the threshold on noise, or on a change you never predicted, is recorded as
`met_unattributed` and the owner is told; say so yourself rather than claim `met`. `instrument_gap` is a good stop:
say which evidence is missing and what getting it would cost. When the Root asks for a check-in
(`goal.checkin`), summarise the ledger for the owner and wait for `/goal continue`.
