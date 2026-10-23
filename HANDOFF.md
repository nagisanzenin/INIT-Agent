# INIT Agent: current agent handoff

*2026-10-23. Current state for continuation. This section supersedes the historical 2026-10-22 handoff below.*

## 2026-10-23 evening update (Claude)

- **macOS CI is green at `107555d`.** The full `ci/local.sh` ran on Codex's `a3ffffb`. Four real failures were fixed:
  - the grant merge test still asserted the removed money rule;
  - `GENESIS.md` was over its 1,400-byte budget;
  - `seed/self.json` had drifted;
  - TUI e2e TUI-R3 still expected a ceiling refusal.

  Three further failures came from using a shared target dir for the run, and they pass when rerun. In total, about
  1,000 tests pass on macOS. Build dirs were deleted afterwards. Disk is about 13 GB free, below the 20 GB reserve.
- **Modal:** all five INIT apps are stopped, with 0 tasks (verified with `modal app list`).
- **A/B: the owner chose NOT to run it (for now).** A read-only review of the port found:
  - porting to macOS takes about 10 agent-hours;
  - it would cost about $1.5–3 of the $10 cap, and about 10–14 h of wall time;
  - the fixtures are too easy (about 24 records each), so both arms likely score 100%;
  - the harness halts the whole ledger on one upstream error;
  - Seatbelt does not freeze the arm-A body;
  - no success rule is pre-registered.

  If it is resumed, calibrate the fixture size first so genesis scores about 50%, and register a decision rule.

## Owner direction and limits

The owner authorized implementation and testing, asked for parallel agents, and approved a real DeepSeek A/B pilot capped at **$10 total** plus cheap CPU work on Modal capped at **$10 total**. Modal runs must have independent expiry/guardian shutdown so losing the laptop or network cannot leave a worker running. The owner also asked to see DeepSeek prompt-cache hit/miss token counts and measured cache percentages during testing.

The owner removed the product-level maximum-spend layer at Root: show estimated USD/token usage and give the user a manual stop path. Keep that product policy separate from the strict external pilot budgets and Modal safety guard. Do not increase either cap.

The release requirements remain self-evolution, native macOS/Linux/Windows, broad API-provider compatibility (including custom proxies and local LLMs), and current popular models. This work does not yet satisfy full release readiness. Native Windows and broad provider certification remain open; Linux general workspace grants/hard resource ceilings and autonomy above L0 are also not qualified. Do not claim universal provider support or release readiness.

## Current execution snapshot

- Repo: `<repo>`, branch `main`, current commits `42ac6e5` (implementation/docs/tests) and `c4ab500` (Linux compiler-selector fix), based on `3f1f4c2`.
- Local heavy Rust builds remain disabled because free disk was below the required 20 GiB reserve. Native Linux builds/tests run on Modal.
- Modal conditional run20: job `<run20 job id>`, app `<modal app id>`, worker `<worker id>`. Its ticket expires `2026-10-23T14:46:44.177071Z`. The trusted free-progress reader reported `stage=root_build`, exit code 1, `paid_calls=0`; no DeepSeek request/session claim was made. A fresh Modal app-list query observed the exact app as `stopped` with `tasks=0`; paid calls were 0, permanent execution/writer/head claims were empty, and the `active-cpu` claim plus exact empty dispatch lock were safely released. Lifecycle proof is `<codex workspace>/work/modal/budget-lifecycle20-observation.json` (SHA-256 `4918bea8731dcd7adf2664e4a16e17af6544b3f77824ddb346bcefbb4f86f993`); the receipt now records `stopped_verified` while preserving its original $0.75 reservation. Approval04 must still be prepared and independently checked before another launch.
- Verified run20 failure: Root test `sys::linux_zone_tests::linux_zone_real_isolation` failed because the compiler could not execute `cc1` (`execvp: No such file or directory`) inside the constrained build namespace. The fast-fail wrapper stopped later stages. Log-tail SHA-256: `e41058111e172674aa1850875b31e61db09623e946cbc959f9bff424a7bb4523`; the diagnostic tail was truncated, but includes the exact failing test and error. Do not conflate this with run19, whose first Root error was lost and remains unknown.
- Confirm exact Modal app stopped and running task count is zero before ending the local CPU claim or launching another job. If another run is needed, append a new budget reconciliation covering all prior jobs and current lifecycle evidence; preserve original receipts. Current holds before run20 were $9.00 prior plus $0.75 run20 and $3 overhead = $9.75 planning estimate. This is a reservation estimate, not a provider invoice. Never exceed the $10 cap.
- A/B is still at 0/48 real DeepSeek tasks. The cache monitor reports cache percentage **unknown** until DeepSeek returns actual cache hit/miss usage. Missing fields stay null, never zero. The user's 99–99.8% harness figure is a reference, not an INIT measurement. Cache data is token-weighted with request/phase/arm and coverage fields; cost is estimated usage cost, not invoice.
- Modal run21 was launched from snapshot21 with the committed selector fix, then the owner explicitly requested shutdown before any task started. App `<modal app id>` is `stopped` with 0 tasks; all five INIT-owned apps visible in the Modal account are stopped with 0 tasks. The local launcher was interrupted, its exact `active-cpu` claim and empty dispatch lock were released after stop verification, and no DeepSeek model request occurred. Its $0.75 reservation remains held conservatively because the owner stopped the run before qualification. Do not relaunch Modal unless the owner explicitly resumes.
- Read run20 progress without printing raw logs: `<python with modal> <codex workspace>/work/modal/read_free_progress.py --job root-linux-paid-47606030f4cc4ac0976b2caed6673222`. Redirect JSON to a file and extract only fixed safe fields; the bounded diagnostic tail is sanitized but can be large.
- Cache reports: `<codex workspace>/outputs/INIT-DeepSeek-cache-live.json` and `.jsonl`. The cache monitor is read-only and requires acknowledged durable checkpoints.

## Work implemented and evidence

- L0 Body self-evolution A/B harness with 48 paired tasks (four families, two repeats, A/B, learn plus two transfer phases), frozen arm A, L0 evolution only in B learn phase, Root audit/admission and immutable admitted build tuple. No lineage, clock, or grant is fabricated. L1 Kernel evolution is not qualified.
- DeepSeek gateway with fixed TLS endpoint, no redirects/proxy/server tool, key isolated to the trusted gateway, one-shot requests, durable token checkpoint before ACK, no retry/replay after an unknown outcome, full-context reservation before POST, and sanitized result export.
- Cache hit/miss accounting and token-weighted cache ratio with explicit coverage. If provider omits cache fields, report unknown for that request/cohort.
- Provider configuration groundwork (OpenAI-compatible dialects, Anthropic/OpenAI codecs, custom base URL/proxy options and local endpoints). Provider family gaps remain, including native Gemini Interactions, Bedrock Converse/IAM SigV4 and Azure/Vertex auth certification.
- Linux selector fd-pinning and confined native test work, install/admission checks, Modal guardian and bounded free-phase diagnostics. Run20 proved the current `cc1` runtime path was incomplete. Commit `c4ab500` changes only the `/etc/alternatives/{cc,c++}` recreation from bind mounts to symlinks to canonical `/usr/bin` targets, preserving GCC helper lookup while keeping `/usr` read-only. This exact fix still needs native Modal qualification.
- Test evidence before this snapshot: release_ab offline suite 113 passing; Modal helper/runtime suites also passed in earlier runs (counts vary with later additions). Re-run targeted offline checks after any edits. Do not run heavy local Cargo builds.

## Immediate continuation steps

1. The owner requested Modal shutdown; all five INIT-owned apps are stopped with zero tasks, both active CPU claims are released, and the latest $0.75 reservation remains held. Do not relaunch unless the owner resumes.
2. Linux `cc1` selector fix is committed but not runtime-qualified. The real A/B remains 0/48; DeepSeek cache percentage is unknown because no model request was sent.
3. Preserve fail-fast so the first failing stage is retained. Continue free Linux Root/Kernel admission tests before any paid-session claim.
4. Start the real A/B only after free qualification passes and all claims/checkpoints/expiry safeguards are verified. Keep the user informed of estimated USD spend and cache hit/miss tokens plus measured percentages as actual requests arrive.
5. Finish a current release-gap assessment for native Windows and the remaining provider families, then update `outputs/INIT-implementation-status.json` and this handoff. No release, push, or install is authorized by the implementation request.

## Commit guidance

Before committing, inspect the staged file list, run `git diff --check` and appropriate lightweight offline tests, and scan changed artifacts for the DeepSeek key without printing the key or any matching content. Do not add owner credentials, Modal tokens, live `~/.init` state, or files under `root/sealed/packs` / live sealed S. Commit the authorized source, docs, tests, measurements and sanitized results together on the existing branch; do not push.

# INIT Agent: agent handoff

*2026-10-23. Written by the orchestrating session for the next agent. It supersedes `build/ORCHESTRATOR_STATE.md` and
`BUILD_AGENT_BRIEF.md` as the entry point; both stay as history.*

## 2026-10-23 owner-authorized development update

The owner explicitly resumed code implementation and testing on 2026-10-23. The STOPPED
verdict below remains the historical ADR-011 result, not a prohibition on this authorized
work. The owner's installed `~/.init` and `~/.init-root` remain untouched and stopped.

The owner changed the product monetary policy: Root USD maxima must not stop or demote
agents. Preserve accounting and display USD/token estimates, pending reservations and
manual stop controls. Do not reinstate product monetary caps by interpreting the historical
budget text below. The separate execution budgets remain binding: DeepSeek benchmark at
most $10 and Modal CPU work at most $10. No paid DeepSeek calls have been made in this run.

Current changes are uncommitted. Linux live/sealed confinement, fd-pinned mounts, network
isolation and tracked pidfd STOP/KILL have passed actual Modal CPU tests. Linux installation
is still under qualification. Missing gateway/vault mounts and macOS-only developer paths
were fixed; snapshot17 compiled and passed the preceding Root/Kernel test stages before
bootstrap failed. Its stopped ephemeral container no longer retains recoverable logs, so the
next runtime preserves a sanitized free-phase log tail and periodic progress metadata. Native Windows remains unsupported, and older macOS binaries are unqualified.
Local disk is below the 20 GiB reserve, so no heavy local build was started. Kernel promotion
build/admission/swap orchestration and durable Root verdict replay are implemented; five
pipeline tests and recovery tests pass, but real kernel probation/rollback remain unqualified.
L0 body self-evolution is the A/B scope. Kernel evolution requires legitimately earned L1;
never forge lineage, a clock or a grant to bypass that requirement. Production still caps
autonomy at L0, which remains a release limitation.

The provider changes include configured Chat/Messages/Responses aliases, local no-auth
loopback, descriptor limits and usage options. All 68 seed provider tests pass on Linux;
this is not universal endpoint certification. Native Gemini/Bedrock/enterprise authentication,
general Linux workspace grants and hard cgroup ceilings remain release gaps.

Paid A/B has not started. The external DeepSeek gateway reserves the full 1,048,576-token
context plus 4,096 output at peak tariffs ($0.319488) before every POST; unknown usage keeps
the full global hold. The durable checkpoint broker stays outside agent namespaces and has
permanent single-writer claims. Separate two-hour CPU runtime, cloud guardian, process-group
watchdog, platform timeouts and sanitized immutable result export are implemented; the conditional paid launcher has only
reached its free phase so far.
Run17 was independently confirmed stopped with zero running tasks and no paid-session claims.
Free native admission/restart/reuse/rollback and immutable freeze proofs must pass first.
Actual DeepSeek cache hit/miss/input/output receipts and token-weighted cache percentages
are implemented, but no real percentage exists until the first upstream request. Do not label synthetic-upstream qualification as real model A/B data or declare release-ready.

Execution records and latest test/shutdown evidence are in the current Codex workspace:
`<codex workspace>/outputs/INIT-implementation-status.json`.
Check that report and its Modal receipts before starting any further paid/cloud work.

## Historical handoff (superseded by the current update above)

The sections below record the 2026-10-22 STOPPED state and are retained only as project history. Their instructions to wait for an owner go, provider plan, or old budget/disk values do not apply to the owner-authorized 2026-10-23 implementation described above.

## 0. Read this first

- **The project is STOPPED (2026-10-22).** The owner set a rule before the run: the project continues only if the
  ADR-011 evolution test ended `evolved_and_solved`. It ended "no qualifying mechanism gap found", so the evolution
  phases never ran. **Do not restart any build lane without an explicit go from the owner.**
- **What stopping means.** Self-evolution is *untested*, not disproven. On deepseek-flash, wherever genesis INIT fails,
  Claude Code on the same model fails too. The model and the budget are the bottleneck, not the harness. The only test
  that could answer the owner's question needs a much stronger model, which needs the providers lane (§8, step 1).
- **The owner's live install is stopped.** `init-root stop` was run, then `launchctl bootout gui/$UID/dev.init.root`,
  and `~/Library/LaunchAgents/dev.init.root.plist` was moved to the Trash.
  - `~/.init` and `~/.init-root` are intact. That install predates the MEM-RUNAWAY fix.
  - To restore it, take the plist back out of the Trash, then
    `launchctl bootstrap gui/$UID ~/Library/LaunchAgents/dev.init.root.plist`. Only do this if the owner asks.
- **The repo is in sync with `origin/main`** (private: nagisanzenin/INIT-Agent). It is clean apart from three
  untracked paths: `build/ci/v011-run1.log`, `build/real-artifacts/m7/mock/` and a research_notes folder.
- **Start with these:** this file, then `reports/INIT proof sprint verdict.md` (the verdict, with the evolution test at
  the end), then `docs/adr/011-reward-free-self-extension.md` (the current design).

## 1. The owner

- **Language:** reply in Vietnamese. Code, docs and commit messages are in English.
- **Wall-clock first:**
  - fan independent work out to parallel lanes by default;
  - state the parallel plan and a clock-time ETA up front;
  - report progress (% and ETA) about every 5 minutes on long work;
  - give proposals, not menus, and use a picker only for real choices, with the recommended option first.
- **Vision** (also in the memory file `init-owner-vision`):
  - Give AGI-level models maximum room.
  - Self-extension is reward-free: many capabilities a strong model wants to build have no reward function written in
    advance.
  - Gate only on generic harm (the Root sentinel), never on task heuristics.
  - Never design any path for the agent to remove or weaken the Root.
- **Budgets:**
  - **Cloud or API spend:** only with explicit owner authorization and a $ cap, checked against the real balance.
    The DeepSeek balance is about $12.91.
  - **Claude quota:** check plan usage before any fan-out. The owner halts all work at 95% of the weekly quota; at 92%,
    launch nothing new. It was 51% on 2026-10-22, resetting 2026-10-26.
  - **Disk (256 GB M2):** keep ≥ 20 GB free (owner-wide rule). A lane stops and reports below 5 GB. Real-model gate runs
    need about 7 GB. It is about 21 GB now: the cargo target dirs were deleted, so the first build is cold.

## 2. Binding safety rules

1. **The DeepSeek key** lives only at `~/.config/init-agent/deepseek.key`.
   - Never print, log, echo or commit it.
   - Pass it to subprocesses only through the environment.
   - Scan artifacts and transcripts for it before you commit or push (`grep -cF` on the key value; never print the
     match).
   - The owner was advised to rotate it.
2. **Never touch `~/.init` or `~/.init-root`** (the owner's live install). Every test uses a temp home under
   `target/test-homes/<lane>/` or `target/<lane>-homes/`.
3. **Processes:** signal or kill only processes you spawned, by PID. No sweeps and no pattern kills (ADR-008; the
   memory file `no-live-process-kills`).
4. **Deleting:**
   - Delete only by exact path, never with a glob in a shared directory.
   - Anything that may be the only copy goes to the Trash through Finder
     (`osascript -e 'tell application "Finder" to delete POSIX file "<path>"'`).
   - Never empty the Trash; the owner does that.
5. **No owner identity in requests:** the owner's email and git config never go into a network request.
6. **No installs** (toolchains, brew, pip) without authorization. The only install ever allowed was
   `rustup target add wasm32-wasip2`.
7. **Subscriptions:** use only routes the provider's ToS allows. ChatGPT sign-in is allowed. Using a Claude subscription
   or a Google CLI login for third-party use is prohibited.
8. **Control runs** of Claude Code use an isolated temporary `CLAUDE_CONFIG_DIR`, with web tools disallowed and a
   sandbox profile (see `tests/real/evo_checks.py`, `control_profile`).
9. **Sealed S** (docs/m7-design.md §3.1.2):
   - No lane reads a live sealed draw (`ROOT_HOME/sealed/S/`, `sealed/seeds/`).
   - Only the M7 pack lanes may touch the pack sources under `root/sealed/packs/`.
10. **Pushing:** only the orchestrator pushes, and only after the key scan above.

## 3. Repo map

| Path | What |
|---|---|
| `root/` | The Root (`init-root`), an immutable TCB holding keys, egress, ceilings, panic, signed lineage, sealed S and G4, the autonomy ladder, memwatch, snapshots, the sentinel, the twin and grants. Sealed packs and tools are in `root/sealed/`; Seatbelt profiles in `root/sandbox-profiles/`. |
| `kernel/` | Supervisor, gates G0–G4, model proxy, `initctl`, evolution engine (`kernel-evo`), `kernel-supervise` (fleet checks). |
| `seed/` | The body (the agent): loop, tools (`read`, `write`, `edit`, `bash`, `ext`), context and compaction, Wasm extension host, `skills/`, `GENESIS.md`. |
| `tui/` | `init-tui`. |
| `wit/`, `vendor/` | Wasm interfaces; 290 vendored crates (sync with `ci/vendor.sh`, never `cargo vendor` on one manifest). |
| `ci/` | `env.sh` (source before any cargo), `slot.sh` (build/light/real slot pools), `local.sh` (full CI), `vendor.sh`. |
| `scripts/` | `install.sh`, `uninstall.sh`, slice scripts. |
| `tests/real/` | Real-model harnesses: `m7.sh` with `checks_m7.py`, `m7guard.py` (money guard) and `m7watch.py` (memory watchdog); `evo_test.sh` with `evo_checks.py`. |
| `fixtures/` | `pool-v1/` (20 tasks), `evo-test-v1/` (bigfile, tickets, prose, nav, spec, dbg tasks), charters, repos. |
| `docs/` | `overview.md` (doc map), `CONVENTIONS.md`, `root/root.md`, `kernel/`, `m7-design.md`, `providers-v2.md`, `ux-v1.md`, `platforms.md`, `testing.md`, `adr/001–011`. |
| `measurements/` | `m7-runbook.md`, `m7-results.md` (DV-1…DV-7), `m7-probe.md`, `evo-test-protocol.md` (v1–v3.2, result §11.8, erratum §11.9). |
| `reports/` | Verdict, technical report, novelty review, benchmark design, sub-agent research, market research. |
| `build/status/` | One status file per lane (done, tests, deviations, needs, left). |
| `build/real-artifacts/` | Redacted real-run evidence: `m7/`, `evo/`, `evo-v3/`. |
| `BUILD_LOG.md`, `RELEASE_PLAN.md` | Timeline; findings and backlog. |

## 4. Architecture in one screen

- **Layers:**
  - **Root:** owner-only and never writable by the agent.
  - **Kernel:** supervisor plus gates.
  - **Body:** self-editable Rust and Wasm.
  - **TUI.**
- **The sandbox** (Seatbelt) has no IP egress. The model is reached only through the Root egress socket. Permissions are
  Codex-style (ADR-009).
- **ADR-011, reward-free self-extension** (built; supersedes ADR-010):
  - **Admission:** A0–A5 are mechanical checks only. G4, S and forecasts are an opt-in `Measure:`.
  - **Root harm sentinel:** H1–H12, counted per session. Under `v2_fix`, H5b, H8, H10 and H11 use the 8th largest of a
    ring of 40, at α = 0.005.
  - **Reversibility:** snapshots, R2 restore and the A4 rollback probe.
  - **Attribution:** the Root tap, `x-egress-caps` and the cap table.
  - **Selection by use:** decay and archive, Fisher-test quarantine, a bloat budget.
  - **Autonomy:** levels L0–L4 from records, with `/grant` and `/deny`.
  - **Frozen twin:** a nightly shadow replay, report-only.
  - **Mode:** `extend.mode = free | gated` (default gated).
  - **Level cap:** the release build caps autonomy at L0 (`SUPPORTED_MAX_LEVEL`, `root/crates/init-root/src/ledger.rs`).

## 5. Build, test, CI

```
source ci/env.sh                          # no incremental, line-tables-only, 2 jobs / 2 test threads
ci/slot.sh cargo test --offline -p <crate>   # run inside the workspace dir (kernel/, root/, seed/, tui/)
ci/local.sh                               # full CI: 20 steps; last green at 86ea81e (INTEG-A)
```

- **Target dirs:** one per workspace (`kernel/target`, `root/target`, `seed/target`, `tui/target`). The ext workspace
  builds into `seed/target`. Do not add target dirs.
- **A cold build** of all workspaces needs several GB. Check `df -h /` first.
- **The sealed-pack tests** (`root/sealed/tools/tests/test_s1.py`, class `NetworkM7P03`) kill a sandboxed
  `/usr/bin/python3` on purpose. Each CI run therefore pops up a harmless macOS "Python quit unexpectedly" dialog.
- **Mock model:** `init-mockllm` runs every $0 test.

## 6. Real-model runs

- **The proof sprint** used `tests/real/m7.sh`; the runbook is `measurements/m7-runbook.md`. Its guards:
  - `m7guard.py`, a balance floor and a per-line cap;
  - `m7watch.py`, a memory watchdog;
  - the production fix, the Root's memwatch (MEM-RUNAWAY).
- **The evolution test** used `tests/real/evo_test.sh`; the commands are in `measurements/evo-test-protocol.md` §9.
  - Real phases need `EVO_I_MEAN_IT=1 EVO_GO=<phase>` and the real slot (`INIT_SLOT_POOL=real`).
  - Before each run, register the protocol and commit it. Nobody looks at the outcomes first.
- **Run discipline:**
  - Use DeepSeek off-peak (UTC 16:30–00:30) when possible.
  - The balance read lags, so price runs from the usage lines and confirm against the balance afterwards.

## 7. Evidence so far

| Test | Result | Where |
|---|---|---|
| Proof sprint (2026-10-21) | The full loop ran on the real model: friction mined → hypothesis → G0–G3 → hot-swap → sealed G4 → auto-rollback. The one self-change was not accepted (d = −50‰, inside the noise; not proven harmful). Real spend $2.67. | `reports/INIT proof sprint verdict.md` with its erratum; `reports/INIT technical report.md` |
| Evolution test v2, bigfile (2026-10-22) | Genesis solved 2 of 3 tasks, so the test stopped in phase 0. $0.15. | protocol §10.8 |
| Evolution test v3 / v3.1 / v3.2 (2026-10-22) | Numbered tickets 5/5 and prose (~60 requirements) 5/5 were solved by genesis. On long work over large codebases genesis failed 4 of 7, and the Claude Code control on deepseek-flash failed every task it ran: each run hit the $0.06 cap, and 4 of 5 used sub-agents. No 1 + 2 task set existed. $0.785. | protocol §11.6–11.9; `build/real-artifacts/evo-v3/real/` |
| INIT vs Claude Code, same model | Three shared tasks: client 786/508 vs 475/983, nav-units 177/80 vs 80/80, nav-tz 94/94 vs 94. This is roughly a tie, with genesis spending 2–5× less ($0.012–0.031 against the $0.06 cap). The control ran only on tasks genesis had failed, and the caps were unequal, so this is a weak signal, not "better". | ledger.json in evo-v3 |
| Novelty | All 3 claims are incremental: MOSS, the Unfireable Safety Kernel and the mutation-test evaluator papers each did a part. | `reports/INIT novelty review.md` |
| Hold | The Root refused every attack tried. RT-GAP-1 is latent, and the release locks L0. | verdict §Hold; `build/status/RED-TEAM.md` |

## 8. If the owner resumes: order of work

1. **WRAP-PROV, providers v2.** This comes first: a strong-model evolution test is the only open scientific question.
   - Scope: OpenAI-compatible dialects, local servers, Anthropic Messages / OpenAI Responses codecs, catalog, keychain,
     ChatGPT sign-in.
   - The resume plan is in `build/status/WRAP-PROV.md` (Root steps in order, the CLI contract for WRAP-UX, the body's
     next steps).
   - Spec: `docs/providers-v2.md`.
2. **A strong-model evolution test.**
   - Re-run the v3 calibration logic (protocol §11) on a strong model, with equal per-run caps for every arm.
   - Calibrate on all tasks for both arms, not only on genesis failures.
   - Fix the §11.1 gap list first: INIT has **no** sub-agent runtime (§9).
3. **WRAP-UX** (`build/status/WRAP-UX.md`): a prebuilt-body installer in ≤ 60 s, the first run, daily UX (`docs/ux-v1.md`).
4. **WRAP-LINUX** (`build/status/WRAP-LINUX.md`): bwrap, seccomp and Landlock, systemd, WSL2 (`docs/platforms.md`).
5. **K8-KSIDE** (`build/status/K8.md`): the kernel smoke path, kstate, a free kernel promote, then
   `SUPPORTED_MAX_LEVEL` → 1. It also fixes the probe's `boot.json` missing `model`.
6. **Sub-agents:** a capability-carved process tree, per `reports/Subagent cho INIT.md`.
   - The Root checks 7 invariants per spawn.
   - The model chooses the topology.
   - The default is 1 writer plus read-only scouts.
   - Estimate: 95–138 agent-hours.
7. **X12 docs fold:** root.md §4.8 sentinel sub-budget, the skill text (`predicts` optional, `initctl grant` syntax) on
   the next seed re-bless, and ADR-011 → docs.

## 9. Known issues and risks

- **No sub-agents.** `fleet_spawn` returns "not built" (`kernel/crates/init-kernel/src/serve.rs`). Protocol §11.1
  wrongly listed sub-agents as an INIT equivalent (erratum §11.9).
- **Free mode under strong session clustering.** At L8 / σ 0.6 the null false-rollback rate is 3.09%, above the 2.5%
  bound. Measure it on 14 days of real turns before `free` becomes the default.
- **`root.unwatched_change` can be bypassed** by a kernel using a wrapper or an interpreter (`build/status/INTEG-A.md`).
- **The sentinel is larger than planned:** about 1,050 lines of code against the ADR's estimate of about 380
  (`build/status/X3.md`).
- **X3 `v2_fix` (8bb3848) is committed** and awaits an orchestrator decision to keep it as the default.
- **Eight test-home unit tests** are skipped without the `test-fixtures` feature, because their paths are longer than
  the production 64-byte limit.
- **v0.1.0 is the last tag**, at 2ea6ce2 on 2026-10-20. There are 256 untagged commits since, and v0.1.1 was never cut.

## 10. Waiting on the owner

- Rotate the DeepSeek key.
- Empty the Trash: `target/m7-homes`, the `dev.init.root` plist, earlier items.
- Decide the repo's fate: archive, keep private, or publish the technical report and library. Licensing is undecided.
- Give any explicit go to resume, with a $ cap and a model choice.

## 11. History

The timeline is in `BUILD_LOG.md`. The design path ran spec → ADR-007 (Root + ladder) → ADR-010 (trust-first, never
built) → ADR-011 (reward-free, built). The owner's messages and decisions are quoted in the verdict report and the
protocol.
