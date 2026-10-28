<p align="center"><img src="assets/init-rebuild.svg" alt="INIT: it rewrites its own harness and grows with every model. Everything is writable except the Root." width="100%"></p>


INIT is a coding agent that can rewrite any part of itself (its prompts, its tools, its compiled Rust core) while it works. One part sits outside its reach: the **Root**, which holds your keys, the sandbox, a signed history of every change, and the power to roll it all back.

```sh
curl -fsSL https://github.com/nagisanzenin/INIT-Agent/releases/latest/download/get.sh | sh
init-agent  # first run: pick a provider, paste a key, pick a model
init-agent upgrade  # later: a new release; keeps the body INIT evolved, its lineage, your keys and charter
```

macOS 14+ on Apple silicon · Linux x86_64, experimental ([bubblewrap ≥ 0.13](docs/platforms.md) built from source) · about 1–2 min; get.sh also installs the Rust toolchain INIT rebuilds itself with · Apache-2.0
Upgrading from 0.4.1, which has no `init-agent upgrade`: `curl -fsSL https://github.com/nagisanzenin/INIT-Agent/releases/latest/download/get.sh | sh -s -- --upgrade` ([upgrade](docs/guide.md#upgrade)).
**Spend is metered, not capped.** Set a limit at your provider. Details: [user guide](docs/guide.md).
**Full access by default:** the agent reads and writes your files and uses the network, like your own shell; only the Root (`~/.init-root`, its service, INIT's launchers) is off-limits. **The Root protects INIT, not your files, in full mode:** its rollback undoes INIT's own changes, and `/undo` restores only the folder you started in. `init-agent --workspace` confines it to that folder ([ADR-012](docs/adr/012-full-access-by-default.md)).

## Why

**Harnesses are the hand-written features of this era.** The loop, the tools, the memory and the planner were designed by people, for models weaker than those people. Sutton's *bitter lesson* says hand-crafted structure loses to learning and scale. When the model outgrows its designers, every scaffold becomes a ceiling.

**You cannot write the reward for what you did not foresee.** Research self-improvers (Darwin Gödel Machine and its kin) optimize a score someone fixed in advance; product agents that learn (Hermes Agent's skills, for instance) change only their notes and skills, never the loop or the code that runs them. The improvements that matter most (a new kind of memory, a tool for a task nobody anticipated, a different loop) have no score until they exist. And a fixed score gets gamed.

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

**This happened (2026-10-24, on a small model).** INIT v0.2 had no sub-agents, so we asked for them:
- **It built them:** a `delegate` tool, plus a nested loop with a fresh context, capped at 8 turns and 24 tool calls, with no recursion. Only a summary returns to the main context, and the prompt-cache prefix stays intact.
- **It verified them:** it regenerated its own self-map, re-blessed its golden tests and added one; 304 tests passed.
- **Then its own Root stopped it:** free disk fell below the Root's floor, so the Root refused every new process, even `git commit`. The agent explained what was blocked and asked how to proceed.
- **v0.3 ships it:** the built-in `delegate` sub-agents are based on the tool INIT wrote for itself.

## How it works

```text
                                  you
                                   │  init-agent · /stop · /panic · /undo
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
  L0 ┃  its whole body: prompts · tools · core  ◀ v0.3 │   writes itself count)
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
| **The sandbox** | Seatbelt profiles composed by the Root. By default the agent reads and writes anything you can and uses the network, but never the Root's files, its service or INIT's launchers; workspace mode (`init-agent --workspace`) adds no IP, a secrets deny-list and writes only in the start folder. The agent never holds your API key. ([ADR-012](docs/adr/012-full-access-by-default.md)) |
| **Undo and history** | APFS clones before every change; every promotion, rollback, grant and panic goes into an Ed25519-signed, append-only lineage. |
| **Watchdog** | Every workload is polled each second; a runaway process tree is killed by PID; work stops below a disk floor. |

About 200k lines of Rust in four workspaces (Root, kernel, body, TUI), on `std` threads with no async runtime. The Root is not small: 20,334 lines without tests in v0.5.0 (counted as in [root.md §4.8](docs/root/root.md); 3,474 more for its sandbox profiles), against the 7,540-line budget set for it on 2026-10-20.

## Status: v0.5 research preview

- **Works:** the edit → gate → hot-swap → rollback loop has run end to end on a real model. The Root holds against our red-team suite. On macOS, 2,022 CI tests pass (v0.5.0); known load flakes pass when rerun alone.
- **New in v0.5: upgrade in place, and self-changes on screen.** `init-agent upgrade` moves INIT to a new release and keeps the body it evolved, its lineage, your keys and your charter (stop, snapshot, replace, a health probe, then commit or roll back; bundles are checked by SHA-256, not signed). The TUI shows each self-change as a gate track that ends kept, refused at a gate, or rolled back, gives the Root its own colour, and puts the signed lineage in Ctrl-O; `init-root audit verify --file` checks a published lineage offline. ([release notes](docs/release-notes/v0.5.0-preview.md))
- **New in v0.4: goals.** `/goal "<what you want>" done=card:requests_turn<=2 usd=0.40 guard=cost_turn:300` gives the agent a direction for its self-changes. The Root, which the agent cannot change, measures each change on your turns (mean, σ, z against the turns before), counts a reading only if the agent predicted it before the change went live, scores the forecasts, rolls back a change that breaks a guardrail, and accepts `met` only when a predicted change moved the card beyond noise. On deepseek-flash ($0.25 over four runs) the record, the guardrail rollback (3.8 s, to the parent) and the scoring worked; the two registered goal changes read as noise, and the two that read better (z +3.5 and +6.6) were unregistered, so they count for nothing. ([release notes](docs/release-notes/v0.4.0-preview.md), [ADR-013](docs/adr/013-goal-based-agentic-loop.md))
- **New in v0.3:** **steering** (type while the agent works: a pending tray shows your line until the agent reads it; Esc interrupts and continues with it; Up takes back an unread line) and **built-in `delegate` sub-agents**, based on the tool INIT wrote for itself (parallel read-only scouts, one writer at a time, bounded per sub-task, no recursion). Linux aarch64 is experimental.
- **Self-rewrite out of the box:** get.sh installs Rust 1.97.1 with wasm32-wasip2 and clippy (`--no-rust` skips it), so T1 tools and T2 core changes work on a fresh install. The first T2 promote on a fresh install builds cold: 10–16 min for build and tests (G0), 17–26 min end to end with G3's self-edit check, measured on an M2. Later ones reuse the dependency cache: about 2 min end to end for a small change.
- **Not shown yet:** that self-evolution makes today's models better. On a small model, our one real self-change was neither kept nor shown harmful. In a missing-mechanism test, wherever INIT failed, a well-known agent on the same model failed too. A harness built to be outgrown only shows its value once the model can outgrow it. If you run frontier models, we would love your results. ([technical report](reports/INIT%20technical%20report.md))
- **Limits:**
  - **Linux is experimental:** x86_64, tested on a GitHub-hosted Ubuntu 24.04 VM on the mock model (install, pause, a self-change of the compiled core through the gates); aarch64 was tested in Docker (Debian 12, Ubuntu 24.04) for v0.4 and has no 0.5.0 bundle yet. It needs bubblewrap ≥ 0.13 at `/usr/bin/bwrap`, built from source because no LTS distro ships it ([docs/platforms.md §3.4.0](docs/platforms.md)). On Ubuntu 24.04, whose AppArmor restricts user namespaces, the installer prints a one-time sudo step: root-owned copies of `unshare` and `bwrap` and a profile that lets only those two create user namespaces ([§3.4.6](docs/platforms.md)). Full access works as on macOS; workspace mode on Linux keeps the agent in its own home (it cannot read or edit your files).
  - Autonomy is capped at L0. DeepSeek is tested live; OpenAI-compatible, Anthropic, Responses and local servers are experimental. Windows is unsupported (WSL2 untested).

**More:** [user guide](docs/guide.md) · [design overview](docs/overview.md) · [ADRs](docs/adr/) · [changelog](CHANGELOG.md) · [docs conventions](docs/CONVENTIONS.md) · [testing](docs/testing.md).

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
