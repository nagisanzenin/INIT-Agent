---
name: evolve
description: Turn mined failure evidence into one self-change through the engine. Read the friction index, file one proposal with attested evidence, submit up to 3 variant patches, screen them and promote the chosen one. Use when you run as the evolution worker, when your boot block shows eligible frictions, or when you want variants compared before you keep one. On the free path the engine is optional; skills/extend covers changing yourself directly.
kind: inv
---

# Evolve: frictions → proposal → variants → screen → promote

One proposal per session. KEEP_BASE (no change) is a valid outcome: `initctl evo abandon <p> keep_base`,
then report why. `initctl evo status [<p>]` shows where a proposal is.

## Free path (your boot block has an `- extend` line)
- The engine is one way to find what to change and to compare variants, never a requirement. You
  may change anything outside the Root directly, mid-task: `skills/extend/SKILL.md`.
- No reward gate. Promote runs no G4 and waits for no gain: admission checks only reversibility
  and budget, then the Root's harm window decides whether the change stays (a clean close is the
  verdict). The screen still drops variants that break; its `est` and `go` are advice.
- `predicts` is a forecast that gates nothing (a baseline clause such as `pass_rate >= parent`
  satisfies the schema); in your own commits `Predicts:` is optional. For a measured verdict add
  `Measure: g4` (a report) or `Measure: g4 gate` (a gate you impose on yourself).
- No `go` but you still want a variant: commit its patch yourself on `evo/<p>` with `Why:`,
  `Prefix-change:` and `Cap:` trailers, and promote it like any change.
- Gated path (no extend line): everything below applies as written, and the gate decides.

## 1. Pick a friction
- `initctl evo frictions`: the top 10. Full index: `initctl evo frictions --json --all` (the same lines
  as `$INIT_HOME/state/kernel/evo/frictions.jsonl`).
- Per cluster: `id` (`f-` + 6 digits), `signature` (the signal, e.g. `path.home_relative`), `counts`,
  `prio` (harm score, higher first), `s_c_permille` (share of failing pool runs this cluster killed),
  `eligible` (enough attested evidence to propose on), `tried`, `refs` (the evidence).
- Take the eligible cluster with the highest `s_c_permille`, then `prio`. None eligible → KEEP_BASE.
- Read the evidence raw (`$INIT_HOME/work/evidence/` first, when it exists):
  - `trace:<id>`: `curl -s --unix-socket "$INIT_HOME/run/proxy.sock" http://x/_init/traces/<id>/request`
    (the messages and tool results the model saw); `.../<id>/response` is its answer.
  - `run:<gate>/<run>`: a failing pool run (score < 1000); its log reaches you only via the evidence dir.
  - `friction:f-…`: a cluster; follow its own `refs`.
- Find the step where it went wrong and why. That is your hypothesis. No clear cause → KEEP_BASE.

## 2. Choose the change
- **INV** (`"scope":"inv"`): how you work: tools, loop, paths, retries, scripts, SELF.md working rules,
  INV skills. No domain facts. Tool, path, bash, loop, retry, cap, finish and owner signals are INV.
- **DOM** (`"scope":"dom"`): knowledge of one kind of work, in `kind: dom` skills only. Only for
  `task.fail` on one label with no tool-level signal.
- Gated path: only T2 changes (the body's Rust source, `"tier_hint":"t2"`) are measured on the
  sealed suite, so aim there: deterministic tool and loop behaviour in `src/tools/` or `src/loop/`
  (e.g. finishing with tests failing or untested → the loop hands back the failing output for one
  more turn). T0 (skills, SELF.md) and T1 (ext/) proposals are filed as `s_measurable: false`: no
  sealed verdict. On the free path every tier is kept the same way.
- Prefer, in order: delete what causes the failure; make a tool deterministic (a clear error, a hint,
  a resolved path); only then add prompt text. Prompt text rarely moves the pass rate.

## 3. File one proposal
Write the JSON to `$INIT_HOME/work/evo-proposal.json`, then `initctl evo propose "$INIT_HOME/work/evo-proposal.json"`:
```json
{"v":1,"scope":"inv","frictions":["f-000003"],"hypothesis":"<cause, one sentence>",
 "change":"<what changes, one sentence>","repro":["trace:<16 hex>","run:j-000031/<run_id>","friction:f-000003"],
 "predicts":["pass_rate@sealed >= parent+15%","cost_usd <= parent+30%"],
 "subject":"fix(tools): <at most 72 bytes>","compensates_for":"none","prefix_change":false,"k":3,"tier_hint":"t2"}
```
- `repro`: ≥ 2 refs of kinds `trace:`, `run:`, `friction:`, at least one not `friction:`, each showing
  a failure (an `error:` tool result, a non-zero exit, a cap refusal, an owner abort; a run with
  score < 1000; an eligible cluster). `event:` refs only add context.
- `predicts` (1 to 5 clauses): gated, keep the `pass_rate@sealed` clause; the cost clause bounds
  the price. Free: a forecast only.
- `compensates_for`: `none`, or `<slug>: <note>` when the code works around a model weakness; then
  mark the added code `/// @init compensates-for: <same value>`.
- `prefix_change`: true when GENESIS.md, SELF.md or a skill's name or description changes.
- Answer: `{id, state}`, or every problem at once (`evo.schema`, `evo.evidence_missing`,
  `evo.evidence_unattested`, `evo.not_failure`, `evo.quota`). Fix all of them once and retry. A second
  refusal of any step ends the proposal: `initctl evo abandon <p> keep_base` (or stop if it has no id).

## 4. Variants (up to K = 3, really different)
For each k, in a fresh worktree of lineage `main`; never commit there:
```sh
p=<p>; k=1; wt="$INIT_HOME/work/evo-$p-v$k"
git -C "$INIT_HOME/body" fetch --quiet origin && git -C "$INIT_HOME/body" worktree add --detach "$wt" origin/main
# edit; for code: cd "$wt" && cargo test --offline   (must pass)
"$INIT_HOME/kernel/bin/init-kernel" selfmap --root "$wt" --write   # after any code or skill change
git -C "$wt" add -A && git -C "$wt" diff --cached origin/main > "$INIT_HOME/work/evo-$p-v$k.patch"
initctl evo variant "$p" "$k" "$INIT_HOME/work/evo-$p-v$k.patch"
git -C "$INIT_HOME/body" worktree remove --force "$wt"
```
- The patch holds the change plus the refreshed `self.json`. Answer: `{ref, sha, diff_stat}`. Refusals:
  `evo.patch_apply` (rebuild against origin/main), `evo.duplicate_variant` (too close to an earlier
  one: make a real alternative or submit fewer), `evo.scope_paths` (a path outside the scope), `evo.dep`.
- The kernel writes the commit message from the proposal; you write none.

## 5. Screen, then promote
- `initctl evo screen <p> --wait` (bash timeout 3600 s). It only rejects. Per variant: stage, pass,
  reason; then `est` (permille) and `go` (est ≥ 80). No `go` → gated: stop and report the reasons;
  free: see the free-path section.
- Promote the variant the screen chose (k):
  `git -C "$INIT_HOME/body" fetch --quiet origin && git -C "$INIT_HOME/body" branch -f evo/<p> origin/init/evo/<p>/v<k>`
  then `initctl promote evo/<p> --wait` (bash timeout 3600 s). The validator takes this commit only
  while the proposal is `screened` and it is the chosen variant.
- Report: the cluster, the hypothesis, each variant's screen result, `est`, and the promote result.
