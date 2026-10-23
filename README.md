```text
 ██╗███╗   ██╗██╗████████╗
 ██║████╗  ██║██║╚══██╔══╝
 ██║██╔██╗ ██║██║   ██║
 ██║██║╚██╗██║██║   ██║
 ██║██║ ╚████║██║   ██║
 ╚═╝╚═╝  ╚═══╝╚═╝   ╚═╝
```

**An agent harness that rebuilds itself mid-task.**

INIT is a coding agent that can rewrite any part of itself (its prompts, its tools, its compiled Rust core) while it works. One small thing sits outside its reach: the **Root**, which holds your keys, the sandbox, a signed history of every change, and the power to roll it all back.

```sh
curl -fsSL https://github.com/nagisanzenin/INIT-Agent/releases/latest/download/get.sh | sh
init        # first run: pick a provider, paste a key, pick a model
```

macOS 14+ on Apple silicon · about 30 s · no Rust needed (install it later to unlock core rewrites) · Apache-2.0
**Spend is metered, not capped.** Set a limit at your provider. Details: [user guide](docs/guide.md).

## Why

**Harnesses are the hand-written features of this era.** The loop, the tools, the memory and the planner were designed by people, for models weaker than those people. Sutton's *bitter lesson* says hand-crafted structure loses to learning and scale. When the model outgrows its designers, every scaffold becomes a ceiling.

**You cannot write the reward for what you did not foresee.** Today's self-improving agents optimize a score someone fixed in advance. The improvements that matter most (a new kind of memory, a tool for a task nobody anticipated, a different loop) have no score until they exist. And a fixed score gets gamed.

**So INIT trusts the model the way we trust engineers: with an undo button.** Nobody makes an engineer prove a commit is better before merging. We give them version control, rollback, an audit trail and a stop button. INIT asks only two questions of every self-change: *can we undo it, and did it cause harm?*

> Govern the recovery path and the measuring stick, never the model's choices.

## Same model, two harnesses

A frontier model gets three weeks to migrate a 2-million-line monorepo off an in-house build DSL.

| Day 2, the model notices… | A fixed harness | INIT |
|---|---|---|
| `grep` can't parse the DSL | lives with the noise | writes a Wasm tool that does, uses it the same session |
| compaction drops type signatures | re-reads 40 files every session | rewrites its own compaction; the new core is hot-swapped mid-task |
| a sub-task wants a fresh context | waits for the vendor to ship sub-agents | writes a delegation tool |
| a new, smarter model is released | same scaffolds, same ceiling | rewrites the scaffolds |
| one of its changes makes things worse | (it can't make changes) | the Root rolls the whole system back |

A fixed harness caps the agent at the imagination of the people who built it. INIT caps it at what can be undone.

**This happened (2026-10-24, on a small model).** INIT v0.2 has no sub-agents, so we asked for them:
- **It built them:** a `delegate` tool, plus a nested loop with a fresh context, capped at 8 turns and 24 tool calls, with no recursion. Only a summary returns to the main context, and the prompt-cache prefix stays intact.
- **It verified them:** it regenerated its own self-map, re-blessed its golden tests and added one; 304 tests passed.
- **Then its own Root stopped it:** free disk fell below the Root's floor, so the Root refused every new process, even `git commit`. The agent explained what was blocked and asked how to proceed.

## How it works

```text
                                  you
                                   │  init · /stop · /panic · /undo
                                   ▼
╔══════════════════════════════════════════════════════════════════╗
║  ROOT                         the agent can never write here     ║
║                                                                  ║
║   keys              egress + metering       sandbox composer     ║
║   signed lineage    harm sentinel H1–H12    snapshots + rollback ║
║   stop · panic      autonomy ledger         memory watchdog      ║
╚══════════════════════════════════╤═══════════════════════▲═══════╝
                                   │ one Unix socket       │
                                   │ no IP, no TCP         │ every model call
                                   ▼                       │ is metered here
┌──────────────────────────────────────────────────────────┼───────┐
│  AGENT ZONE · sandboxed · the model may change all of it │       │
│                                                          │       │
│    ┌─────────────────────────────────────────────────┐   │       │
│    │  KERNEL   admits changes · gates · hot-swaps    │   │       │
│    └──────────────────────────┬──────────────────────┘   │       │
│                               ▼                          │       │
│    ┌─────────────────────────────────────────────────┐   │       │
│    │  BODY     T0  prompts + skills                  ├───┘       │
│    │           T1  new tools (Wasm components)       │           │
│    │           T2  its own compiled core and loop    │           │
│    └─────────────────────────────────────────────────┘           │
└──────────────────────────────────────────────────────────────────┘
```

**The life of a self-change.** If the new body crashes at boot, nothing happens, because the old one never stopped. The code changes; the memory does not, since the conversation is an append-only log with one writer.

```text
 ┌──────────┐    ┌──────────┐    ┌──────────┐    ┌──────────┐    ┌──────────┐
 │  edit    │───▶│  build   │───▶│  boot    │───▶│ hot-swap │───▶│ watched  │
 │  itself  │    │  + test  │    │  probe   │    │ mid-chat │    │ by Root  │
 │  evo/ID  │    │  G0 · G1 │    │  G2 · G3 │    │ same log │    │ H1 – H12 │
 └──────────┘    └──────────┘    └────┬─────┘    └──────────┘    └────┬─────┘
                                      │ crashes?                      │ harm?
                                      ▼                               ▼
                           the old body never stopped:     roll the whole system
                           nothing happened                back to last known-good
```

**Autonomy is earned from records the Root writes itself, never from what the agent reports.** This release caps it at L0, which already allows the whole body.

```text
        what it may change                               how it gets there
  L4 ┃  + the model itself, as a component             ▲  days at level
  L3 ┃  + more external actions                        │  + your turns
  L2 ┃  + a wider blast radius                         │  + clean change groups
  L1 ┃  + its own kernel                               │  (only records the Root
  L0 ┃  its whole body: prompts · tools · core  ◀ v0.2 │   writes itself count)
     ┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┷━━━━━━━━━━━━━━━━━━━━━━━━
        ▼ one level down on: harm · /panic · crash loop
```

## Under the hood

| | |
|---|---|
| **Hot-swapping a compiled core** | Rust has no stable ABI, so a fresh process is started: it is probed first, runs overlapped with the old one, takes over at a step boundary, and receives the same conversation. The swapper is never the swapped. ([ADR-001](docs/adr/001-overlapped-handoff.md)) |
| **Five gates** | G0 build, test and clippy · G1 self-map regenerated byte for byte · G2 boot probe · **G3 the new version must itself complete a self-edit**, so a change can't break the ability to change · G4 sealed regression. ([gate.md](docs/kernel/gate.md)) |
| **A test set it never sees** | G4 draws from Root-held task templates, plus canaries for tamper, leak and impossible tasks, using paired cluster-bootstrap non-inferiority. |
| **Reward-free admission** | In `free` mode a change needs only checks A0–A5: it builds, it boots, it can be rolled back. No benchmark decides what the model may build. ([ADR-011](docs/adr/011-reward-free-self-extension.md)) |
| **A harm sentinel that doesn't know your goals** | 12 generic signals (crashes, refusals, your stops and undos, cost and error spikes), counted per session against a 40-session baseline at α = 0.005. |
| **Frozen twin** | A pre-change copy replays your sessions nightly; an exact signed-rank test runs over 14 nights. It only reports; it never blocks. |
| **Selection by use** | Unused capabilities decay into an archive; ones that keep coinciding with trouble are quarantined by a Fisher exact test. |
| **Knows itself from evidence** | Models are poor at introspection, so INIT gets verified files instead: `SELF.md` (its own map), `self.json` (inventory, checked at G1), a boot block saying why it woke up, and live signals for context, cache, spend and level. ([awareness.md](docs/awareness.md)) |
| **The sandbox** | Seatbelt profiles composed by the Root: no IP at all, one Unix socket to the Root's egress, secrets on a deny-list. The agent never holds your API key. |
| **Undo and history** | APFS clones before every change; every promotion, rollback, grant and panic goes into an Ed25519-signed, append-only lineage. |
| **Watchdog** | Every workload is polled each second; a runaway process tree is killed by PID; work stops below a disk floor. |

About 200k lines of Rust in four workspaces (Root, kernel, body, TUI), on `std` threads with no async runtime.

## Status: v0.2 research preview

- **Works:** the edit → gate → hot-swap → rollback loop has run end to end on a real model. The Root holds against our red-team suite. CI is green on macOS (1,807 tests).
- **Not shown yet:** that self-evolution makes today's models better. On a small model, our one real self-change was neither kept nor shown harmful. In a missing-mechanism test, wherever INIT failed, a well-known agent on the same model failed too. A harness built to be outgrown only shows its value once the model can outgrow it. If you run frontier models, we would love your results. ([technical report](reports/INIT%20technical%20report.md))
- **Limits:** macOS only (Linux in progress), autonomy capped at L0, and no built-in sub-agents. DeepSeek is tested live; OpenAI-compatible, Anthropic, Responses and local servers are experimental.

**More:** [user guide](docs/guide.md) · [design overview](docs/overview.md) · [ADRs](docs/adr/) · [changelog](CHANGELOG.md) · contributors and agents start at [HANDOFF.md](HANDOFF.md).

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
