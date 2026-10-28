# INIT user guide

Install, upgrade, daily use, goals, spending, the safety model, and a status record from v0.2.0-preview. The [README](../README.md) has the overview and the current status; each release's notes are in [release-notes/](release-notes/).

## Install

### Requirements

- macOS 14 or newer on Apple silicon (tested on an M2), with the Command Line Tools (`xcode-select --install`), which provide `git` and `/usr/bin/python3`. Linux (next release): Debian 12 or Ubuntu 24.04 class, aarch64 verified (x86_64 compiled, not run), glibc ≥ 2.36, bubblewrap ≥ 0.13.0 at `/usr/bin/bwrap`, unprivileged user namespaces, python3 and git; full access works as on macOS, while workspace mode keeps the agent in its own home. See docs/platforms.md §3.4.0. Windows is unsupported.
- An API key. DeepSeek is the only provider used against a live API. OpenAI-compatible gateways, Anthropic Messages, OpenAI Responses and local servers are offered as **experimental** (offline tests only).
- Disk: about 0.5 GB for the install, plus the free-space floor you choose (default 20 GB).
- The Rust toolchain INIT rebuilds itself with (1.97.1 plus the `wasm32-wasip2` target). `get.sh` installs it for you through rustup, leaving your shell files and your default toolchain alone. Self-rewrite is the point of INIT; `get.sh --no-rust` skips it, and INIT then changes only its skills and prompts (tier T0).

### Install

```sh
curl -fsSL https://github.com/nagisanzenin/INIT-Agent/releases/latest/download/get.sh | sh
init-agent
```

`get.sh` downloads the prebuilt bundle, checks its SHA-256, makes sure the Rust toolchain is present, and installs with the default settings (no sudo, no key). The first `init-agent` opens a short setup: choose a provider and a model with the arrow keys, and paste your key (shown only as `•`; it goes to the Root, never to the agent or a log). Later, `/model`, `/key` and `/setup` change them. `get.sh --from <bundle.tar.gz>` installs a downloaded bundle.

To build from source instead (about 15 minutes on an M2, Rust required): `scripts/install.sh`, then `init-agent`.

The installer asks one question. It prints the settings it writes into your charter (`~/.init-root/charter.toml`) and asks `Install with these settings? [Y/n]`; `--yes` (which `get.sh` passes) skips it. Each setting has a flag, which `get.sh` passes on after `--` (for example `sh -s -- -- --access workspace`):

| Setting | Default | Flag | Meaning |
|---|---|---|---|
| disk floor | 20 GiB | `--disk-floor N` | INIT pauses new work when less is free |
| spending | $5 a day, $25 in total | `--day-usd X`, `--total-usd X` | what INIT shows its spend against; tracked, not a cap ([Spending](#spending)) |
| autonomy | up to L4 | `--max-level N` | the highest level INIT may earn; it starts at L0 |
| access | full | `--access full\|workspace` | full: anything you can, never INIT's Root files; workspace: writes only the folder `init-agent` starts in |

Change any of them later: edit the charter, then `init-root charter check --apply`. `--purpose TEXT` sets the charter's one-line purpose, and `--charter FILE` uses a charter of your own instead.

The installer never touches your key. It writes `~/.init-root/` (the Root: your charter, keys, signed lineage) and `~/.init/` (the agent's world). It puts the `init-agent` command and the TUI `init-tui` in `~/.local/bin/`: `init-agent` opens the TUI, or the plain `init-root attach` terminal when the TUI is not installed, and `init-agent --which` shows which. It also loads the LaunchAgent `dev.init.root`, which keeps the Root running and starts it at login. Use `--no-service` to skip the service and run `init-root start --foreground` yourself.

Up to v0.4.0 the command was `init`. It is `init-agent` from v0.4.1, because `~/.local/bin/init` shadows Linux's `/sbin/init` ([issue #1](https://github.com/nagisanzenin/INIT-Agent/issues/1)). v0.4.1 kept `init` on macOS as a shim that printed a rename notice; v0.5.0 drops it. The installer and the upgrade install no `init` and remove INIT's own old one (the pre-v0.4.1 launcher or the shim, recognised by content), and leave any other file named `init`. The installer refuses to install into `/sbin`, `/usr/sbin`, `/bin` or `/usr/bin`.

## Upgrade

```sh
init-agent upgrade                       # the latest release
init-agent upgrade --version 0.5.1       # a given release
init-agent upgrade --bundle init-v0.5.1-macos-arm64.tar.gz   # a downloaded bundle (its .sha256 next to it, or --sha256 HEX)
```

INIT 0.4.1 has no `init-agent upgrade`. Upgrade it once with `curl -fsSL https://github.com/nagisanzenin/INIT-Agent/releases/latest/download/get.sh | sh -s -- --upgrade`; after that, `init-agent upgrade` works. A release before 0.4.1 cannot be upgraded: uninstall it (`scripts/uninstall.sh` moves it, its lineage and your key to the Trash), then install. get.sh names the installed release and prints the uninstall command.

The upgrade replaces the Root, the kernel and the `init-agent` and `init-tui` commands, and keeps what INIT has become: the body it evolved, its extensions, skills and sessions, the signed lineage, your keys and your charter. Run it from your own shell. In order:

1. **Download and check.** The bundle and its SHA-256 come from the GitHub release over HTTPS (only when you run the command). The download, the check and the unpacked copy live in `~/.init-root/cache/upgrade/`, which the agent cannot write in any access mode, so it cannot swap the code between the check and its run.
2. **Stop.** With the LaunchAgent (or the systemd user unit), the upgrade stops the Root. A Root you started in a terminal is never signalled: stop it with Ctrl-C, then run the upgrade.
3. **Check.** Nothing changes if the release is older than the one installed, if it is the same release, or if its kernel cannot run your evolved body. For the body check the Root runs `--manifest` of your body and of the release's own body, each in a sealed sandbox, and compares the control protocol, the manifest format and the extension WIT majors. INIT never migrates or rewrites an evolved body: an incompatible release is refused with the reason.
4. **Snapshot and replace.** The Root takes a snapshot of everything it will replace (APFS clones on macOS; reflinks or copies on Linux) into `~/.init-root/state/upgrade/`, then installs the new Root, kernel and kernel source, and forms a new system tuple with the new kernel and your current body and extensions.
5. **Health probe.** The new Root starts, the kernel must be ready on the new tuple and your body must boot, within 120 s (`-- --probe-timeout S` changes it), and the whole lineage must verify.
6. **Commit or roll back.** If the probe passes, the new tuple becomes last-known-good and the snapshot goes. If it fails, the old Root, kernel and commands come back from the snapshot, a `rollback` record returns the system to the tuple from before, and INIT runs its old version unchanged. The service is started again either way.

`init-root upgrade --status` shows the last upgrade. Limits of this release, plainly:

- **Bundles are checked by SHA-256 only.** The checksum comes from the same GitHub release as the bundle, so it detects a broken download, not a compromised release. Releases are not signed yet.
- **The new seed is not offered to INIT.** A release's new seed body (its prompts, skills, tools) is not merged into the body INIT evolved; INIT keeps running its own. Merging is planned for a later release.
- **No live swap.** The upgrade stops INIT for its duration (seconds); the running conversation continues after it.
- **An open self-change refuses the upgrade.** In `free` mode a change group in its watch window (2-72 h) must close first, so that a later rollback never returns to a tuple of the old kernel. Older tuples (audited-good, genesis) keep their old kernel; a rollback to one of them runs that kernel under the new Root.

## Daily use

| You want to | Do |
|---|---|
| chat or give a task | `init-agent`, type a line, press Enter. Ctrl-C or Esc pauses the current turn (your next message continues the task, or says what to do instead); Ctrl-D detaches and INIT keeps running. |
| work on a project | `cd` into it and run `init-agent`. By default (v0.3.1, [ADR-012](adr/012-full-access-by-default.md)) the agent has **full access**: it reads and writes anything you can, `~/.ssh` included, and uses the network, like your own shell. Only the Root is off-limits: `~/.init-root`, its service (`~/Library/LaunchAgents`), the `init-agent` and `init-tui` launchers, `~/.config/init-agent` and your `read_deny` list. `FULL` shows in the status bar. The folder you started in is the workspace, the only place `/undo` covers. |
| confine the agent to the project | `init-agent --workspace` for one session, or `/full off` in the chat (`/full on` lifts it). `WORKSPACE` shows in the status bar. The agent then reads your home except secrets (`~/.ssh`, cloud credentials, keychains, browser and mail data), edits only that folder and its own `~/.init`, and has no network except the model provider. Run from `~`, it can read but not edit. To make this the default, set `mode = "workspace"` under `[sandbox]` in `~/.init-root/charter.toml`, then `init-root charter check --apply`; `/full on` then asks you to type `/full on confirm`. |
| undo the agent's edits | when the agent is idle, `/undo`, then `/undo confirm`: the folder goes back to the snapshot the Root took when you ran `init-agent` there. The state it replaces is saved first, so an undo can be undone. Folders over 2 GiB or not on APFS get no snapshot. |
| keep the agent out of a folder | add it to `read_deny` under `[sandbox]` in `~/.init-root/charter.toml`, then `init-root charter check --apply`; it holds in full access too. `read_home = false` turns home reads off in workspace mode; `allow_full = false` forbids full access altogether. |
| see what it has spent | `/usage` in the chat, or the status bar (estimated USD and tokens). `init-root egress stats` prints the Root's full money block; it says `"enforced": false`. Every request is logged in `~/.init-root/egress/usage/`. |
| stop the current run | `/stop` in the chat, with no demotion. `/resume` starts it again. |
| stop everything, now | `/panic` in the chat, or `init-root panic` from any terminal. Every agent process stops, the panic is latched, and the agent drops one autonomy level (when it is above L0). `/resume` (or `init-root resume`) when you are ready. |
| stop for the day (no demotion) | `init-root stop`; later `init-root start` |
| see state and level | `init-root status` (slot, mode, level and cap, panic, charter), `init-root ledger` (level history and progress); `/status` in the chat for the kernel's view |
| take back the agent's last self-change | `/rollback` in the chat: the Root rolls back the newest open change group (free mode) to its parent version and records it as your reversal (harm signal H3). `/rollback <sha\|cap>` names the group. Only that group's own changes are undone; the seed's skills and prompt files are never quarantined by it. `/rollback settled`, `/rollback lkg` or `/rollback twin` returns the whole system to that tuple in either mode (`settled`: the newest tuple that has been last-known-good for 7 days with no rollback since). `/root help` lists the forms. |
| give the agent a goal to pursue | `/goal "<what you want>" [done=card:cost_turn<=600\|exemplar:<sha256>\|owner] [usd=1.00] [wall=24h] [rounds=10] [guard=requests_turn,…]`. Only you can set a goal; the agent may propose one (`/goal adopt <id>`). `/goal` shows it; `/goal pause\|resume\|stop`; `/goal continue` after the agent checks in; `/goal accept\|reject` judges its last change. The Root meters the goal's spend, records the agent's predictions and the measured outcomes, and rolls back a change that makes a `guard` card worse ([Goals](#goals-goal-v04), [ADR-013](adr/013-goal-based-agentic-loop.md)). |
| answer the agent's request for more room | `/grants` lists open requests; `/grant <id> [for <n>d\|<n>h]` approves one with a typed confirmation; `/deny <id>` refuses it. |
| see the frozen twin | `/twin` shows the nightly shadow-replay report (report only; it changes nothing). |
| change the charter | edit `~/.init-root/charter.toml` (autonomy cap, sandbox, memory cap, disk floor, `[extend]`), then `init-root charter check --apply` |
| change the model | edit `~/.init/kernel/config.toml`, then `init-root stop && init-root start`. The `[budget]` dollar values there are not enforced while the Root reports observe-only money. |

`init-root` lives in `~/.init-root/bin/`; add it to your PATH or use the full path.

Where things are:
- Root logs: `~/.init-root/logs/` (`launchd.log` for the service).
- Kernel and body logs: `~/.init/state/kernel/logs/`; conversations: `~/.init/state/sessions/`.
- Every metered request with its usage and estimated cost: `~/.init-root/egress/usage/`. Every promotion, rollback, panic, grant and charter change: `~/.init-root/lineage/lineage.jsonl`, signed; check it with `init-root audit verify --full`.
- The agent's own source and history: `~/.init/body/` and `~/.init/lineage/`.

## Goals (`/goal`, v0.4)

A goal asks INIT to change its own Body toward something you can state: "fewer model requests per turn on short
questions about this folder", "cheaper turns on big repos". You set it; the agent works on it in rounds; the Root
keeps the books ([ADR-013](adr/013-goal-based-agentic-loop.md)). It works best in free mode (`[extend] mode = "free"`),
where a T0 change (a skill or a prompt file) goes live in about a second.

```
/goal "answer short questions about this folder with fewer model requests per turn" done=card:requests_turn<=2 usd=0.40 wall=3h rounds=8 guard=cost_turn:300,provider_errors
```

- **`done=`** says when it is done. `card:<id><=N` or `>=N` uses one of the Root's own observation cards, measured on
  your turns: `cost_turn` (µUSD per turn), `wall_turn` (ms), `requests_turn`, `cache_read` (‰ of input tokens),
  `provider_errors`, `corrections` (‰ of turns you corrected, undid or stopped). `exemplar:<sha256>` or `owner`
  (the default) leave the verdict to you (`/goal accept` or `/goal reject` on the agent's last change).
- **The budget** is yours: `usd=` (the Root charges the goal every metered request while it is active, the same
  amount as the egress ledger), `wall=`, `rounds=` (one round per prediction). At a spent budget the Root stops the
  goal itself.
- **`guard=`** lists cards that must not get worse; `cost_turn:300` allows 30 % before it counts. A change that makes a
  guard card worse beyond noise is rolled back by the Root (a T0 change in about 4 s in the real runs), and the agent's earlier good changes stay.
- **How a round runs.** The agent reads the card's noise floor (at least 5 of your turns on the current version),
  commits a change, checks it against the commit rules (`initctl promote --check`), files a prediction (which card,
  which direction, how sure), then promotes it. The Root reads your next turns (5 by default) against the turns
  before, and records Δ, σ, z and a verdict (`better`, `worse`, `noise`). Only a prediction filed before the change went
  live counts ("registered"). The agent's forecasts are scored (Brier), and that score sets one thing: how many rounds
  it may run before it checks in with you (3 by default; 1 to 12). At a check-in, read what it did, then `/goal
  continue` or `/goal stop`.
- **How it ends.** The agent stops with a reason: `met`, `budget`, `plateau`, `instrument_gap` (it says what evidence it
  lacks), `owner_stop` or `harm`. For a card goal the Root checks `met`: the card must have reached your threshold *and*
  one of the agent's registered changes must have moved it beyond noise in the predicted direction. If the threshold is
  reached some other way (noise, drift, a change it never predicted), the Root records `met_unattributed` and tells
  you: the goal may be done, but not by anything the agent measured.
- **What you see.** `/goal` prints the goal, its spend and rounds, the calibration and the pending readings. Every
  goal, prediction, reading and outcome is a record in `~/.init-root/lineage/lineage.jsonl`.
- **Limits.** Cards read your turns, so a reading takes as many turns as `n`; at a few turns a day that is slow. An agent
  can lower `cost_turn` by doing less; `corrections` as a guard is the counterweight. One goal is active at a time (a
  new `/goal` pauses the old one).

## Spending

Since 2026-10-23 (owner decision) the Root **observes** spending and does not cap it:

- The Root records every model request with the provider's reported token usage and an estimated cost from the charter's tariffs. When a provider reports no usage, the Root keeps the reservation estimate and marks token totals incomplete; it never counts unknown usage as zero.
- You see the estimates in the status bar, with `/usage` and with `init-root egress stats`. They are estimates, not invoices.
- The Root does **not** refuse, stop or demote anything when a dollar amount is reached. The charter's `day_usd`, `total_usd` and the twin's dollar keys are kept as metadata. The kernel's dollar soft budgets are switched off as well.
- Non-dollar limits are still enforced: per-class request counts, bytes per day and rate per minute, a bounded output size on every request, and the time and token limits of runs.
- You stop spending with `/stop`, `/panic` or `init-root stop`.

**The risk, plainly:** a self-modifying agent can keep making model requests, and so keep spending, until you stop it. **Set a spend limit at your provider.** For DeepSeek, which bills against a prepaid balance, keep that balance small. Check `/usage` while it works.

### Spending FAQ

- **Does INIT cap what I spend?** No. The Root tracks every request's tokens and estimated cost, but no dollar amount
  stops INIT on its own. The charter's `day_usd` and `total_usd` are records, not limits.
- **Is there any dollar limit the Root enforces?** One: a goal's budget. `/goal … usd=0.40` makes the Root charge the
  goal for every metered request while it is active, and stop the goal when the budget is spent ([Goals](#goals-goal-v04)).
  It stops the goal, not INIT: after that the agent still answers you, and those requests are not capped.
- **What is the hard limit, then?** The one at your provider: a monthly or per-key spend limit, or a small prepaid
  balance. It holds even if INIT, its estimates or your machine misbehave.
- **How do I see what it spent?** The status bar, `/usage`, and `init-root egress stats`. They are estimates from the
  charter's prices and the provider's reported usage, not invoices; compare them with your provider's console.
- **How do I stop it now?** `/stop` ends the turn, `/panic` stops every agent process, `init-root stop` stops INIT.

## Safety model

1. **The Root** (`init-root`) is a separate process the agent can never write. It holds your key, your charter and the signed lineage, and it runs the agent only inside a macOS sandbox (Seatbelt) it composes. Model requests always go through the Root, which meters and logs them. In full access (the default) the agent's tools also use the network directly, and INIT does not log that traffic; in workspace mode the sandbox has no network at all.
2. **Spending is observed, not capped** (see [Spending](#spending)). The Root meters and logs every request; you stop it.
3. **Rollback.** In the default `gated` mode every self-change passes the gate (G0–G4) first. The Root keeps the last known-good system and rolls back the whole tuple when a change fails, and a panic demotes.
4. **Self-extension (ADR-011).** The charter key `extend.mode` chooses `gated` (the default) or `free`. In `free` mode a change is admitted after mechanical checks only (A0–A5), and the Root then watches it: a harm sentinel (signals H1–H12, such as crashes, refusals, owner stops and undos, cost and error spikes) rolls back the change group that caused harm, using Root-held snapshots and a rollback probe. **Admission takes a snapshot, so it needs free disk of at least `sandbox.disk_floor_gib` plus 2 GiB** (22 GiB with the default 20 GiB floor); below that, every free-path change is refused. Switching `extend.mode` with `init-root charter check --apply` reaches the running kernel at once. In `gated` mode with no sealed S and an empty gate pool (a fresh install), G4 has nothing to measure: a T1/T2 change is kept on G0–G3 alone and you get one `g4_no_instrument` alert (until this fix it was rolled back about a second after the swap). Autonomy levels L0–L4 are earned from records and owner grants, but **this build caps autonomy at L0**.
5. **Memory and disk.** The Root polls every workload each second. A tool call's process tree that passes `sandbox.mem_cap_mib` (default 4096 MiB) is killed alone, by pid. Below `sandbox.disk_floor_gib` (default 5 GiB) free, the Root refuses new workloads and kills the running ones. The Root and the kernel are never killed.
6. **Owner-machine mode (ADR-008).** On your live account the Root signals only the processes it holds, by PID; the process sweep (`live-sweep`) is compiled out. Enabling it still requires the disposable-account tests first ([docs/adr/](adr/)).
7. **Permissions (ADR-012, ADR-009).** By default the agent has full access: it reads and writes anything you can and uses the network, except the Root's files (`~/.init-root`, its service, the `init-agent` and `init-tui` launchers, `~/.config/init-agent`, your `read_deny` list). It can never touch your keys or the lineage, signal the Root, or start apps outside its sandbox. Anything it reads, secrets included, may be sent to the model provider or, in full access, anywhere else on the network. The Root snapshots the folder you started in before the agent can edit it, and `/undo` restores that folder only: changes elsewhere have no undo, and the Root's rollback covers INIT, not your files. Files your own tools run later (shell startup files, `PATH` directories, editor configs, git hooks) are writable and then run with your full rights, so full access is trusted like your own shell. Workspace mode (`init-agent --workspace`, `/full off` or `sandbox.mode = "workspace"`) restores ADR-009: home reads minus secrets, writes only in the folder and `~/.init`, no network but the provider, and shell startup files, git config and hooks, `~/.local/bin`, `~/bin` and login items unwritable. The agent can end INIT itself (`launchctl remove dev.init.root` stops the Root and every agent process; log in again, or `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/dev.init.root.plist`), but it cannot escape it.

### Checking a published lineage

`init-root audit verify --full` checks your own install's lineage. To check a lineage file someone published (for
example a demo's records), without installing INIT, run the `init-root` binary of v0.5.0 or later:

```sh
init-root audit verify --file lineage.jsonl --pub <public key> --head <head hash>
```

It checks every record's hash chain and Ed25519 signature from genesis, and follows key rotations signed by the old
key. Take the key and the head hash from a place other than the records file, such as the release notes; a file
checked against a key shipped next to it proves nothing. Without `--head`, a file cut at a record boundary still
passes as a shorter chain. A valid result shows the records were not changed after the Root signed them. It does not
show that the binaries you run were built from the published source.

## Uninstall

```sh
scripts/uninstall.sh
```

It stops INIT through `init-root stop`, unloads the LaunchAgent, then, after you type `uninstall`, **moves** `~/.init-root`, `~/.init`, the `init-agent` and `init-tui` commands and INIT's old launcher or shim at `~/.local/bin/init` to the Trash (with a timestamp in the name). Nothing is deleted: empty the Trash yourself. Your key file goes to the Trash with `~/.init-root`.

## Status (v0.2.0-preview)

This section is the v0.2.0-preview record and is kept as it was. Later releases and what each one verified are in
[release-notes/](release-notes/) and the [README](../README.md).

**What works and how it was checked:**
- **macOS CI.** The full `ci/local.sh` passes on macOS: 1,807 tests at $0 on the mock model. Three `kernel-selfmap` tests can time out when the machine is heavily loaded, and they pass when run alone.
- **Install and a live turn.** The prebuilt bundle installs without Rust in about 30 s. A real DeepSeek turn through it cost about $0.002, and the key stayed in one 0600 file the agent cannot read.
- **The gated self-edit loop ran end to end on a real model** (deepseek-flash, $2.67, 2026-10-21): the evolution engine found a failure pattern in the agent's own runs, the agent wrote a change, passed gates G0–G3, was hot-swapped into the running session, and the sealed gate G4 sent it back with an automatic rollback. That run used the code of 2026-10-21, before ADR-011 and the spending change.
- **The Root held every attack tried** in a $0 red-team suite against the real Root (in test mode): self-declared prices, unknown providers, forged lineage, forged owner commands, a forged L3 command. The one gap found, RT-GAP-1 (self-reported promotions could unlock L1), was latent under the L0 cap and is now closed.
- **MEM-RUNAWAY is fixed on macOS**: the memory watchdog and disk floor above. It was found when a sandboxed process reached 43.9 GB of compressed memory in a test run.
- **ADR-011 self-extension is built**: free and gated modes (default gated), the harm sentinel, snapshots and the rollback probe, per-capability attribution, selection by use (decay, archive, quarantine), records-based levels with `/grant` and `/deny`, and the frozen twin. It is tested at $0, including end to end against the real Root with the mock model. The free path has not run on a real model.

**What is not shown:**
- **There is no evidence yet that self-evolution improves results.** The one real self-change was not kept, and its sealed result (−5 points) was inside the measurement noise, so it shows neither help nor harm. The ADR-011 evolution test on deepseek-flash found no task where INIT failed but a missing mechanism was to blame (where INIT failed, Claude Code on the same model failed too), so its evolution phases never ran. See the [verdict](../reports/INIT%20proof%20sprint%20verdict.md) (in Vietnamese; erratum and evolution test at the end), the [technical report](../reports/INIT%20technical%20report.md) and the [evolution test protocol](../measurements/evo-test-protocol.md) §10–§11.
- **Real-model coverage of this release is one smoke turn on DeepSeek.** Everything else since the spending and provider changes is checked at $0.
- **Linux is verified for the next release.** Verified natively, in Docker, on Debian 12 and Ubuntu 24.04 (aarch64, Linux 6.10). Covered: the zone with a real compiler run (the cc1 fix), the memory watchdog plus a cgroup v2 `memory.max` per workload, the bundle installed with no Rust and run end to end on the mock, and the systemd user service. Not verified: x86_64, real hardware, Ubuntu's AppArmor user-namespace restriction, WSL2. Workspace mode is not available on Linux yet (docs/platforms.md §3.4.0). **Windows is unsupported.**

**Providers:**
- **DeepSeek** (`deepseek-flash`, `deepseek-v4-pro`) is the only provider used live and the only one the installer offers.
- **Experimental:** OpenAI-compatible Chat Completions gateways, OpenAI Responses, Anthropic Messages and local servers on loopback without a key. They exist as code with offline tests, configured by hand ([docs/provider-configuration.md](provider-configuration.md)), and none has been tested against a live endpoint. Native Gemini, Ollama's native API, Bedrock, Azure and Vertex authentication are not built.

**Other limits:**
- Autonomy is capped at **L0** in this build, so kernel self-evolution (L1 and up) is not available.
- The `free` mode's false-rollback rate under strongly clustered sessions measured 3.09% in simulation, above its 2.5% bound; measure it on real use before making `free` the default.
- A kernel could bypass the Root's `root.unwatched_change` check through a wrapper or an interpreter (relevant only above L0).
- There is no sub-agent runtime: `fleet_spawn` answers "not built".
- The off-machine lineage mirror is off until you create its repository.

Measurements are in [measurements/](../measurements/) and the [technical report](../reports/INIT%20technical%20report.md); each release's verification record is in [build/status/](../build/status/) (`REL-*.md`).

