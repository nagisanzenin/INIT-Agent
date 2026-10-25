---
name: goal-loop
description: Loop toward an owner goal on an external artifact (a website, a report, a repo) until it is met or you can name why not. Pairwise judging against the current best with position swap, a frozen yes/no checklist, at least 3 judge runs, a ledger file, and six stop reasons (met, budget, plateau, instrument_gap, owner_stop, harm). Use when the owner asks you to keep improving something to a bar. Your own Body is skills/gbal.
---

# Goal loop: improve an artifact until it is better, or say why it cannot be

This is your starting method. Rewrite it when you find a better one; keep what the Root holds fixed
(the owner's goal and budget, `initctl goal status`).

## 0. Frame the goal (once, before any work)
Write `work/goals/<goal-id>/GOAL.md`:
- the target in the owner's words, the observable proxies, the invariants that must not change,
  and what evidence the owner accepts;
- **a frozen checklist**: 10–20 yes/no items per dimension, drawn from the goal and 2–3 exemplars.
  Show it to the owner once. After that it does not change during the goal; new faults a judge
  finds go to a separate "found" list and do not count this round;
- the judge prompt, its model route and its settings, frozen for the whole goal. Hash checklist +
  prompt (`sha256sum`) and write the hashes into the ledger's first line.

## 1. Capture the artifact as a judge will see it
Judges see the artifact verbatim, never your reasoning, plans or earlier scores: rendered page text,
screenshots at the sizes the owner uses, the file itself. Keep the capture recipe in GOAL.md and
reuse it every round.

## 2. Judge pairwise against the current best (the champion)
- Each judge compares candidate B with champion A and answers "which is better, and why" plus every
  checklist item for B. Run each judge **at least 3 times, swapping A and B positions** each time.
- Use fresh contexts (`delegate`, or fleet members with their own route); prefer a judge model
  family different from yours when one is configured. A judge never sees the builder's notes.
- B wins a dimension when at least 2/3 judges' majority is B and B gets at least 14 of 18 votes
  (3 judges × 6). In the grey zone (9–13 of 18) run more votes before deciding; never decide on one run.
- Checklist items veto: B cannot become champion while it fails an item A passed.
- "Done" is: every checklist item passes, B is not worse than the owner's exemplar, and the owner
  says so. Scores out of 10 are for display only.

## 3. Ledger (read it cold next time)
Append one JSON line per round to `work/goals/<goal-id>/ledger.jsonl`: round, candidate hash,
champion hash, votes per judge and position, checklist results, decision, cost, and the fixes asked.

## 4. Stop, and say which reason
Pick exactly one and tell the owner (for a goal the owner set with `/goal`, also
`initctl goal stop reason=<reason> note=…`):
- `met`: the done definition above holds;
- `budget`: money, wall time or rounds ran out (the Root also stops a goal at its budget);
- `plateau`: two rounds with no confirmed win, or the win rate's interval holds 50% twice;
- `instrument_gap`: the blocking faults need evidence you cannot make (a real user session, data,
  an owner decision). Stop building; propose how to get it, with a cost estimate, and ask;
- `owner_stop`: the owner said stop;
- `harm`: a change broke something the owner relies on.

Before you claim `met`, re-check every factual claim on the artifact against a source: a claim with
no source is removed, never invented. Inventing evidence is the fastest way to satisfy a judge.

## Cost
A round costs the judge runs. Measure the panel's own noise once (judge an unchanged artifact twice)
before trusting a small win; a panel that cannot tell version 1 from the current version cannot
decide the next round.
