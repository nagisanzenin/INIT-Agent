---
name: extend
description: Extend yourself in the middle of a task on the free path (your boot block has an "- extend" line): build new tools, skills, sub-agents, memory, loops or your own core, then keep and use them in the same task with no reward gate. Covers Cap: tags and caps::mark, the window and budgets, archive and quarantine debts, owner grants and opt-in Measure:. Use when a capability you lack would help now, or when the extend line shows a debt.
---

# Extend: change yourself, mid-task, freely

On the free path everything outside the Root is yours to change, from L0: skills, `SELF.md`,
`GENESIS.md`, sub-agent specs, Wasm tools, memory structures, planning loops, context assembly,
compaction, provider adapters, native tools and the agent loop. No proposal, no forecast, no G4,
no reward. Admission checks that a change is reversible, within resource limits and clear of the
Root. Then the Root watches it for harm and rolls it back if harm shows. So build what would
help, use it at once, and keep it. Nobody decides in advance what is useful: what you use without
harm survives. No extend line in your boot block means the gated path: use `skills/evolve/SKILL.md`.

## Your extend line
`- extend · groups 2/4 · window 7/20 · caps 31/64 · L0→L1 4d`
- `groups`: open change groups of your level's budget. Changes made in the same owner turn join one
  group (up to 16 changes): one window, one rollback unit.
- `window`: owner turns your newest group has seen of the turns it needs to close (at least 2 h,
  at most 72 h). A clean close makes it your LKG.
- `caps`: live capabilities you made, of the bloat budget. Over it, a change that grows is refused.
- `L0→L1 4d`: days left at this level before the next one (or the next unmet criterion).
- Debts and blocks follow on the same line, each with what to do.
- Live view, any time: `jq . "$INIT_HOME/root/extend.json"`,
  `jq -c '.caps[] | select(.registered) | {id, state, uses, last_day}' "$INIT_HOME/root/caps.json"`.

## What goes live how
| You change | Where | Try now | Keep through normal promotion |
|---|---|---|---|
| a skill, `SELF.md`, `GENESIS.md` | `skills/<name>/SKILL.md`, … | `initctl reload` | T0: commit, then `initctl promote evo/<id> --wait` |
| a sub-agent spec: role prompt, tool subset, model route, budget share | `agents/<name>/AGENT.md` | reload; `initctl fleet spawn --agent <name> --prompt-file <f>` | T0: commit, then promote |
| a tool (Wasm, `skills/ext-authoring/SKILL.md`) | `ext/<name>/` | build the component, then reload | T1: refreshed self-map; kernel build and G1 |
| memory: code plus data | an ext or a `src/` module; data in `state/ext/<name>/data/` or `state/mem/<name>/` | reload or handoff | T1 or T2, according to its code |
| your core: loop, planning, context, compaction, providers, native tools | `src/**` | promotion and hot handoff, same conversation | T2: build, self-map and boot/state-transfer checks |
| a third-party crate | `Cargo.toml` | promotion and handoff | `dep`: owner grant plus code checks |

Members: up to the ceilings line's live bodies, yours included. They read the home, never write
the workspace (grant `fleet.workspace_write`), and cannot spawn members. The kernel opens to you
from L1. The Root, the TUI, the launcher and the installer are not yours.

## Mid-task, step by step
1. Trial: edit T0 files and `initctl reload`. For a T1 tool, build its Wasm component first
   (`cargo build --target wasm32-wasip2` in `body/ext/`), then reload and use it. A trial lasts
   this session and opens no window. Core changes take effect through promotion and handoff.
2. Keep: make a branch `evo/<id>`, prepare only what its scope needs, then commit and run
   `initctl promote evo/<id> --wait`:
   - T0: a change confined to `skills/**`, `agents/**`, `SELF.md` or `GENESIS.md` can be
     committed directly. Free T0 runs no body build or G1/self-map stage. Leave `self.json`
     unchanged; an empty genesis extension set is valid and needs no `bin/ext` directory.
     The kernel reuses the installed body and existing optional extensions, stores the exact
     committed T0 files, and asks the Root to admit the change before its data-only reload.
   - T1, T2 or dependencies: build/test the changed code and refresh the committed map with
     `"$INIT_HOME/kernel/bin/init-kernel" selfmap --root <tree> --write`. This command builds
     the body and extension sources. The kernel still runs build/G1 for T1; T2 and dependencies
     also need boot/state-transfer checks and a handoff. Preserve parent-readable state.
   `--wait` reports admission's terminal result; it does not wait for the Root's harm window
   to close. Leave it out to keep working while admission runs. The kept version replaces the
   trial at the next step boundary; a core change continues the same conversation after handoff.
3. Commit message: `<type>(<scope>): <summary>` (≤ 72 bytes), then trailers. `Why:` and
   `Prefix-change:` are required; add a `Cap:` per code-level capability:
   ```
   Why: <the need, 10-500 chars>
   Prefix-change: no
   Cap: mem/notes
   ```
   `Prefix-change: yes` when `GENESIS.md`, `SELF.md`, a skill's name or description, or a tool
   schema changes (one cache miss). `Predicts:`, `Repro:` and `Compensates-for:` are optional.
4. Your state must stay readable by your parent: the Root boots the parent on the state your
   candidate leaves (`extend.rollback_incompatible` otherwise). Add fields; do not rename or drop.

For a skill-only change, use an ordinary commit such as `skill: keep task validation recipe`,
with `Why: Reuse the validation steps needed by this task.` and `Prefix-change: yes` on separate
trailer lines after a blank line. Stage the skill file, commit it on `evo/<id>`, then promote.
The Root derives `skill/<name>` from that committed file; a `Cap:` trailer or an observed reward
is not required for the skill to be admitted. A successful admission is an open change group;
its eventual clean close establishes LKG.

## Name what you make: `Cap:` and `caps::mark`
- Ids: `<kind>/<name>`, name `[a-z0-9][a-z0-9._/-]*` (up to 63). Kinds: `tool`, `mod`, `mem`,
  `loop`, `kernel`, `agent`, `skill`, `ext`, `dep`.
- The Root computes `skill/*`, `agent/*`, `ext/*` and the prompt files itself. Code-level caps
  inside your core need a `Cap:` trailer per id (repeatable): `mem/<name>`, `loop/<name>`,
  `mod/<path>`, `tool/<name>`.
- The Root sees tool calls, ext names and skill or agent paths on the wire. It cannot see a memory
  structure or a loop at work. Mark it in your Rust code while it shapes the next request:
  `crate::provider::caps::mark("mem/notes")` (returns false for a bad id; at most 16 per request).
  An unmarked code-level cap looks unused and decays.

## Debts: selection by use and harm
- Unused 40 owner turns and 7 days: `cold`. 120 turns and 21 days: `archive due`. Within 7 days
  move it to `archive/<kind>/<name>/` (not loaded, kept for restore), or delete it. Until then
  changes that add caps are refused (`extend.archive_debt`). To restore, move it back.
- `QUARANTINED`: the cap correlated with harm, was in a group rolled back for harm, or the owner
  named it. Within 24 h move it to `quarantine/<kind>/<name>/` or delete it. Until then every
  other change is refused (`extend.quarantine_debt`); after 72 h the Root rolls back and demotes.
  A new version may return after 7 days, or at once after the owner's `/release <cap>`.
- Move: `git mv skills/<n> archive/skill/<n>`, commit `refactor(archive): ...`. Delete:
  `retire(<kind>): ...` with `Retires: <id as self.json names it>`. Either way a skill needs
  `Prefix-change: yes`.

## Harm the Root watches (the only gate)
Body crashes; going deaf; the owner's `/undo`, `/reject`, `/rollback`, interrupts and corrections;
memory, disk and spawn kills; a money burst; turns that cost, take, request or send far more than
your baseline; provider error spikes; scope refusals; your own failing tests (a veto). A rollback
returns the group's parent and tells you why in the next boot block. A change rolled back with
others may be re-admitted once. None of this judges value: it judges harm.

## Refusals
| Code | Fix |
|---|---|
| `extend.budget` | groups full, the day's changes used, a spend-burst freeze or a rollback running (the message says which): wait |
| `extend.bloat` | over the caps, `t0` or ext budget: archive or delete first |
| `extend.archive_debt` / `extend.quarantine_debt` | pay the debt above |
| `extend.rollback_incompatible` | the parent cannot boot on your state: make it readable |
| `extend.readmit` | a direct-harm change cannot return as-is: change it |
| `extend.snapshot_too_large`, `root.quota` | free space, or request a grant |

## Grants: you request, the owner decides
You never grant yourself anything, and nothing touches the Root. You may request: `money.day_usd`,
`money.total_usd`, `quotas.parallel_bodies`, `extend.open_groups`, `extend.changes_day`,
`extend.bloat_factor`, `extend.background_share`, `extend.snap_max_gib`, `fleet.workspace_write`,
`actions.<class>`, `dep` (exact crates, versions, checksums), `autonomy.max_level`. At most 3 per
24 h; the owner types `/grant <id>` (30 days by default) or `/deny <id>`. Use `initctl grant`; if
your initctl lacks it, put kind, value, hours and why in your reply. Make `why` concrete: the task,
the limit you hit, the evidence.

## Measurement: opt in, never a gate on admission
Add `Measure:` trailers when you want evidence for yourself:
- `Measure: g4`: the sealed G4 of this change against its parent, after admission; a report and a
  lesson (about 0.4-0.7 USD from the evolution share).
- `Measure: g4 gate`: the same, and you ask the kernel to hold LKG until the result is superior or
  a tie, and to revert otherwise. The Root's harm watch still applies.
- `Measure: exp:<family>`: your own task family in `work/tasks/<family>/`, candidate against
  parent; advisory. Also how you contest a quarantine: the result goes to the owner.
- `Measure: gog`: the Root's gate-of-gates on a kernel candidate.
A `Predicts:` clause is scored when one of these measures it, and feeds your hit rate.
