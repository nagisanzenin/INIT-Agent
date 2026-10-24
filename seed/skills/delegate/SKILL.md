---
name: delegate
description: When a sub-task in a fresh context helps (long searches, reviews, parallel read-only scouts, one writer) and how to brief it.
kind: inv
---

# Delegate: fresh context, parallel scouts, one writer

`delegate` runs a sub-agent with your head and tools but none of this conversation, and returns
only its final text (at most 8 KiB). The full record is in `state/spill/<session>/delegate-*.json`.

## When it helps
- **Output you would only skim.** Reading through a tree, a long grep, a test log, comparing many
  files: the sub-agent pays for the output, you pay for the answer.
- **A clean second look.** A review of a diff or a plan by a context that did not write it.
- **Independent questions at once.** Several `delegate` calls in one reply run in parallel (up to
  4 by default) and come back in call order. Split by question, not by file count.
- **One writer.** `mode: "write"` for a self-contained change. Writers run one at a time; keep
  the integration (merge, build, commit) here.

## When it does not
- The next step needs the detail anyway, or the task is two tool calls: just do it.
- The sub-task needs this conversation: it cannot ask you questions.
- Two writers on the same files: serialize them yourself or give each its own worktree.

## Brief it well
- Self-contained: goal, paths, commands, constraints (`context` for facts it cannot discover).
- Say exactly what to return: paths with line numbers, a yes/no with evidence, a diff summary.
- Bound it to the work: `max_turns`, `max_calls`, `max_seconds` lower the defaults
  (8 turns, 24 calls, 600 s; set by `INIT_DELEGATE_*`).

## Read mode is advisory for bash and ext
`mode: "read"` (default) refuses `write` and `edit`, but its `bash` and `ext` run with your own
rights: the body cannot sandbox them further. Do not hand a reader a task whose shell commands
change files; use `mode: "write"`.

## Check the answer
Treat a summary as a lead, not a fact, when it matters: re-read the cited lines, or open the
record. An answer marked `[init:delegate] … cap` stopped early; the record shows how far it got.
