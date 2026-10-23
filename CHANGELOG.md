# Changelog

All notable changes to INIT Agent. Versions follow [semver](https://semver.org). The design is `INIT Agent spec.md`; each line below points at the build status file that records it (`build/status/`).

## v0.2.0-preview (2026-10-23)

A research preview for macOS. It folds in v0.1.1, which was never cut, and everything since: reward-free self-extension (ADR-011), the memory watchdog, observe-only spending, provider and Linux groundwork, and the CI fixes. There is no evidence yet that self-evolution improves results (see "Research results" below). Autonomy stays capped at L0.

### Self-extension: ADR-011 (docs/adr/011-reward-free-self-extension.md)
- **Two modes.** The charter's `[extend]` section (rule C17) sets `extend.mode = "gated"` (the default: every change passes G0–G4) or `"free"`. On the free path a change is admitted by mechanical checks A0–A5 only; G4, the sealed set and forecasts become an opt-in `Measure:` trailer, and `Predicts:` is optional (X1, X2, INTEG-A).
- **The Root harm sentinel** (`root/crates/init-root/src/sentinel/`): generic harm signals H1–H12 counted per session, with exact exceedance and binomial tests, change groups and their windows. It rolls back the group that caused harm and never needs a task-specific reward. Under `v2_fix`, H5b, H8, H10 and H11 use the 8th largest of a ring of 40 at α = 0.005, so they stay live in long sessions (X3, X10).
- **Reversibility.** Root system snapshots, R2 restore, the A4 rollback probe in Root-owned probe roots, the body's `check` mode and the `settled` pointer (X4).
- **Attribution.** The Root's egress tap, the body's `x-init-caps` forwarded as `x-egress-caps`, the usage ledger, `cap_use` digests and `caps.json` (X5).
- **Selection by use, levels and grants.** Decay and archive of unused capabilities, Fisher-test quarantine, a bloat budget; levels L0–L4 from records; owner grants with expiry through `/grants`, `/grant <id> [for <n>d|<n>h]` (typed confirmation) and `/deny <id>`; an owner-corrections trend that blocks level-ups. RT-GAP-1 (self-reported promotions could unlock L1) is closed (X6, X7).
- **The frozen twin.** A nightly shadow replay of a frozen earlier version on sealed lanes, with an exact signed-rank cost test; report only, re-frozen only by the owner (`/twin`, `/twin refreeze`) (X9).
- **The agent knows.** An awareness line and the `extend` skill tell the body which mode it runs in (X11).
- The broker's `root.unwatched_change` check: on the free path a live body must run an admitted body binary (INTEG-A).

### Evolution engine and the measuring stick (proof sprint, 2026-10-20 to 10-21)
- `kernel-evo`: a friction miner over the agent's own traces, proposals, kernel-built variants, a screen with a go/no-go estimate, and `initctl evo frictions|propose|variant|screen|status|abandon` (EVO-LITE).
- G4 v2: an exact sign-flip test with a non-inferiority margin and A/A calibration, per-run caps, a pool cache and failing-run log retention (GATE-V2a, GATE-V2b, ROOT-S1b).
- Sealed set S v1: pack install and draw, the G4 slice, canaries and tamper checks, calibrate, probe and milestone commands; the `s1` pack has 56 templates. A visible pool `pool-v1` of 20 tasks (ROOT-S1a, S1-*, POOL-LITE).
- Real-model harnesses with money and memory guards: `tests/real/m7.sh` and `tests/real/evo_test.sh` (REAL-M7, EVO-TEST-PREP).

### Memory watchdog and workload disk floor (MEM-RUNAWAY, 2026-10-22)
In M7 one sandboxed process grew to 43.9 GB of compressed memory, and swap filled the disk to 0.24 GB free. Seatbelt has no memory limit, so the agent's bash tool had the same exposure.
- **The Root polls every workload each second** (zone instances and sealed runs). A tool call's or build step's process tree whose memory (resident + compressed) passes `sandbox.mem_cap_mib` (charter, default 4096 MiB, 512–65536) is killed alone, by pid. The Root and the kernel are never killed (MEM-RUNAWAY).
- **Disk floor.** Below `sandbox.disk_floor_gib` (default 5 GiB) of free disk, the Root refuses new workloads and kills the running ones; the agent and the kernel stay up (MEM-RUNAWAY).
- **The agent is told.** bash reports `process exceeded memory cap N MiB, killed` as a tool error, and refuses new commands while the disk is below the floor. The owner gets an alert, `mem_cap` or `disk_floor` (MEM-RUNAWAY).
- Linux (cgroups `memory.max`) is not built yet.

### Permissions: Codex-style (owner decision 2026-10-20; docs/adr/009-codex-style-permissions.md)
The owner found v0.1.0's sandbox too strict for daily work: the agent could not even read a project outside `~/.init`. It now works like Claude Code or Codex, while the Root, the keys, the signed lineage, the gate and the network stay as they were.
- **Read your home, except secrets.** The live body reads the owner's home, minus a deny list that holds in every mode: `~/.init-root`, `~/.config/init-agent`, `~/.ssh`, `~/.gnupg`, cloud and registry credentials (`~/.aws`, `~/.azure`, `~/.config/gcloud`, `~/.kube`, `~/.docker/config.json`, `~/.netrc`, `~/.npmrc`, `~/.pypirc`), keychains, cookies, Mail, Messages, Safari, browser profiles, 1Password and `~/Library/Group Containers`. The charter adds entries with `sandbox.read_deny` (up to 32) and turns the home read off with `sandbox.read_home = false` (SBX-009).
- **Write the workspace.** `init`'s cwd is the workspace: the TUI sends it once per launch (`workspace.set`), and the Root validates it (inside the home, not the home itself, not a Root path, not on the deny list). The body then writes `~/.init` plus that folder. Launched from `~`, the agent can read but not edit (SBX-009, WIRE-009).
- **`init --full` and `/full on|off`.** Full mode makes the whole home writable except the deny list and the Root's paths, after a typed confirmation (`/full on confirm`) the first time in a session. The charter can forbid it (`sandbox.allow_full = false`), and the status bar shows `FULL` (SBX-009, WIRE-009).
- **Snapshots and `/undo`.** The Root takes an APFS clone of the workspace into `~/.init-root/snapshots/` when the workspace is set and when full mode turns on. `/undo` (then `/undo confirm`) restores the latest one, after saving the current state, so an undo can be undone. The Root keeps 5 snapshots per workspace; a folder over 2 GiB, off APFS or on another volume gets none, with a notice (SBX-009).
- **Enforced by the Root's tail.** Two Root-generated grants (`Z_HOME` read, `Z_WRITE` write, both excluding `~/.init`), per-entry deny lines in tail Z2 plus a write deny on `~/Library/LaunchAgents`, and `Z_WRITE` in tail Z3. The kernel's `spawn` parameters are unchanged, and the Root sets `INIT_ACCESS` and `INIT_WORKSPACE` on every body spawn. Probes stay hermetic; fleet members read but do not write the home (SBX-009).
- **The agent knows its access.** The body reads `INIT_ACCESS` (`none`, `read_home`, `workspace`, `full`) and `INIT_WORKSPACE`, starts its tools in the workspace, and states its access in one line of the boot block, so the cached prompt head does not change. A workspace change restarts the body at an idle boundary on the same conversation (WIRE-009).
- **No unsandboxed execution through the obvious paths.** In every mode, tail Z3a write-denies the shell startup files, `~/.gitconfig` and `~/.config/git`, `~/.local/bin`, `~/bin`, login items (LaunchAgents, autostart, systemd user units) and the installer's `bin_dir` (`init-root install --bin-dir`). Tail Z3b denies `.git/hooks`, `.git/config` and the same under `.git/modules/` in every repository outside `~/.init`; commits still work (SBX-009).
- **`/undo` waits for an idle agent.** It answers `root.busy` while the body is busy, before asking for the confirmation. Every access change is a path-free `access` lineage record (SBX-009).
- **Known risk.** Workspace build and task files (Makefile, package scripts, `.envrc`) can still run with the owner's rights later, and in full mode so can editor configs, language tool directories and other PATH directories (ADR-009, Threat model delta).

### Spending: observe only (owner decision 2026-10-23)
- **The Root no longer enforces dollar limits.** Live, sealed, audit and twin requests are never refused, stopped or demoted because a dollar amount is reached. The Root's money block reports `"enforced": false, "policy": "observe"`. `money.day_usd`, `money.total_usd`, `sealed.reserve_usd` and the twin's dollar keys are kept as metadata; the charter no longer checks them against each other (C5, C12, C17).
- **What stays enforced:** request validation, a bounded output size on every request, per-class request counts, bytes per day and rate per minute, and run time, request and token limits.
- **Accounting.** Every metered request is logged with the provider's token usage and a tariff-based cost estimate. Missing usage keeps the reservation estimate and marks token totals incomplete, never zero. `total_reserved_musd` shows in-flight estimates.
- **The kernel** turns its dollar soft budgets off when the Root reports observe-only money; an older Root without the marker keeps the old behaviour.
- **The TUI** shows the estimated live total, live tokens and today's evolution-check cost in the status bar, with a `/usage · /stop` hint, and `/usage` prints the full estimate locally.
- **Risk:** a self-modifying agent can spend until the owner stops it (`/stop`, `/panic`). Set a spend limit at the provider.

### Providers (groundwork, experimental; docs/provider-configuration.md)
- Owner-configured provider aliases with `api = "chat" | "responses" | "messages"`, a literal `base_path`, exact destinations and explicit tariffs. Redirects and ambient HTTP proxy settings are disabled.
- Local servers on `127.0.0.1` or `localhost` without a key (`local = true`, `auth = "none"`).
- Model descriptors in the kernel config: context window, output limits, `request_max_tokens`, `chat_stream_usage`, `native_extensions`.
- Offline tests only. DeepSeek is still the only provider used against a live API and the only one in the installer.

### Linux (groundwork, unverified)
- A fail-closed Linux mount plan (`root/sandbox-profiles/src/linux.rs`) and a Linux zone in the Root: fd-pinned mounts, network isolation and pidfd-tracked STOP/KILL, which passed CPU tests on Modal. Broad home and workspace grants are refused on Linux until their deny overlays exist.
- A native Linux build test failed (the compiler could not run `cc1` inside the build namespace); the fix (`c4ab500`) is not yet qualified. No cgroup memory limit yet.

### Kernel upgrade and release qualification (groundwork)
- Kernel promotion build, admission and swap orchestration, the Root's conformance probe and durable verdict replay (K8, `init-kernel` `upgrade.rs`). Real kernel probation and rollback are not qualified, and the L0 cap keeps kernel evolution off.
- A release A/B harness (`tests/real/release_ab/`, 48 paired tasks, a DeepSeek gateway with reservations and cache accounting) and a platform release gate (`tests/platform_release/`). The A/B has not been run (0 of 48 tasks).

### Fixed
- A proposal interrupted during G4 could be stranded; the kernel now resumes proposals from the Root's records at start (STRANDED-PROPOSAL).
- G3 stage bounds are now derived from the build timeouts (G3-STAGE-TIMEOUT, found when a cold build slot timed out G3).
- Probe roots made from the read-only pool clone could not be deleted (probe-root leak).
- macOS CI after `42ac6e5` (`107555d`): the grant merge test still asserted the removed money rule; `GENESIS.md` was over its 1,400-byte budget; `seed/self.json` had drifted; the TUI end-to-end test still expected a ceiling refusal.

### Research results
- **Proof sprint** (2026-10-21, deepseek-flash, $2.67): the loop ran end to end on a real model and one self-change reached G4, which did not establish non-inferiority (d = −50‰, inside the A/A noise of 57‰); the Root rolled it back. Per the erratum, this does not show the change was harmful. `reports/INIT proof sprint verdict.md`, `reports/INIT technical report.md`.
- **ADR-011 evolution test** (2026-10-22, $0.94): on deepseek-flash no task was found where INIT failed and a missing mechanism was to blame; where INIT failed, Claude Code on the same model failed too. The evolution phases never ran, so self-evolution is untested, not disproven. `measurements/evo-test-protocol.md` §10–§11.

### Known limitations
- macOS only. Linux is unverified; Windows is unsupported.
- Autonomy is capped at L0 (`SUPPORTED_MAX_LEVEL = 0` in release builds).
- No evidence yet that self-evolution improves results. The free path of ADR-011 has not run on a real model.
- No real-model request has been made on the code since `42ac6e5`; the changes since are checked at $0 on the mock model and in macOS CI.
- Spending is not capped by INIT (above).
- The install wizard still asks for a daily and a total "spending ceiling"; the answers are stored as metadata and not enforced.
- Experimental providers have offline tests only. Native Gemini, Ollama's native API, Bedrock, Azure and Vertex authentication are not built.
- In `free` mode under strongly clustered sessions the simulated null false-rollback rate is 3.09%, above the 2.5% bound; `gated` stays the default.
- `root.unwatched_change` can be bypassed by a kernel using a wrapper or an interpreter (only relevant above L0).
- There is no sub-agent runtime (`fleet_spawn` answers "not built").
- The sentinel is about 1,050 lines, against an ADR estimate of about 380.
- Eight test-home unit tests are skipped without the `test-fixtures` feature (their paths exceed the production 64-byte limit).

## v0.1.0 (2026-10-20)

The first owner-usable release: one command installs INIT on a Mac, and it chats on DeepSeek through the Root. Autonomy is capped at L0 in this build.

### Install and use
- `scripts/install.sh`: one command. It builds the release binaries, asks a short charter wizard (purpose, daily and total ceilings, max level, provider, model, disk floor; `--yes` takes the defaults), runs `init-root install` with the seed bundle, writes `kernel/config.toml`, installs the `init` command and loads the LaunchAgent. It never touches the provider key (REL).
- `scripts/release.sh`: the release directory (`target/dist/v0.1.0/`): `init-root`, `init-kernel`, `initctl`, `init-mockllm`, `init-body`, `init-tui`, the kernel source and seed bundles, the kernel docs, WIT and the template extension, with a `MANIFEST.json` of hashes. It runs the release-mode tests of root and seed as its quality guard (REL).
- `scripts/uninstall.sh`: stops INIT, unloads the service and moves both homes to the Trash after a typed confirmation; it never deletes (REL).
- `init`: opens the TUI when it is installed, else `init-root attach` (REL).
- **The `init` TUI** (`init-tui`, a new `tui/` workspace): a terminal client of the Root's owner socket and nothing else. It holds no keys, writes no files and is never linked into `init-root`. It streams text, shows one row per tool call, a transcript view (Ctrl-O), a multi-line input box with history and `/` completion, and a status bar with level and cap, model, run cost, today against the ceiling and the Root's state. Ctrl-C interrupts, a second Ctrl-C detaches, and `/panic` falls back to a fresh connection and then to `init-root panic` when the attached session cannot deliver it. It reconnects across Root restarts without resending a line, and `--plain`/`--raw` give a line mode for scripts (WP26; docs/tui.md).
- The release ships `init-mockllm`, the $0 mock used by the gate's G0 tests; it makes no network calls. Every install copies it into `kernel/bin/`, and `install` refuses a release directory without it (G0-ROOT).

### The system
- **Root** (`init-root`): charter (R5) and its validation, owner socket and CLI, cold start, kernel supervision with crash-loop escalation and system safe mode (R1), whole-system tuples, LKG and rollback, panic and stop, the spawn broker with the macOS zone profiles, the egress gateway with keys and integer micro-USD ceilings (R2), signed lineage with verification and a mirror (R4), the minimal sealed evaluation (R6) and the autonomy ledger (R7). The install path builds the seed in the kernel's install mode and forms the genesis tuple (WP20–WP25, WP21b).
- **Kernel** (`init-kernel`): supervise with overlapped handoff and rollback (WP4), the model proxy with lanes, soft budgets and traces (WP5), the gate G0–G4 and the gate-of-gates judge (WP8, WP8b, WP8c, FIX2), the self-map (WP9), the `initctl` server (WP7, WP7b) and the macOS sandbox (WP6a).
- **Body** (`init-body`, the seed): DeepSeek, Anthropic and OpenAI providers (WP10), the loop and five tools (WP11), the event log and context assembly with a bounded view (WP12, FIX8), boot and awareness files (WP13), and the Wasmtime 48 extension host (WP14, FIX3).
- **Test kit**: the $0 deterministic model `init-mockllm`, fake body, kernel and Root, the scenario runner (WP2, WP3); acceptance A1–A6 and NO-OWNER-1 green at $0 (WP15); Phase 0 measurements M-1, M-2, M-4 (WP16) and M-5 (M5 lane) pass.
- **CI**: `ci/local.sh` (the full macOS suite) and the GitHub `fast-linux` workflow (the portable kernel crates; the seed as an informational job). `ci/local.sh` gains three steps: **seed G0**, the seed passing its own G0 (build, check, clippy, ext and `cargo test` under the real Seatbelt texts with the shipped mock; G0-SEED); **promote under Seatbelt**, the quick gate and a promote G0 → G1 → G2 → G3 → handoff → G4 → LKG with every instance checked by the Root's own spawn rules and run under `sandbox-exec` (K-GATE); and the TUI's build, tests and end-to-end run against the real Root in test mode (WP26).
- **Tests that cannot hang**: every process-spawning kernel test arms a watchdog with a measured bound and spawns its children so they cannot inherit its pipes (K-FLAKE).

### Safety: owner-machine mode (ADR-008)
- The Root signals only processes it holds: `sys::signal` refuses any pid that is not its own unreaped child, so panic, stop, rollback and crash handling reach only INIT's instances and their own process groups. Cold start never signals a pid a previous Root recorded (R-SAFE).
- The process sweep (`live-sweep`) is compiled out: no feature implies it, a release build that names it fails to compile, and even a debug build with it scans only on a disposable account marked by a root-owned sentinel (R-SAFE).
- `init-root doctor --sweep-dry-run` applies the sweep rule to the live process table and lists what it would kill, sending no signal. On the owner's account it selected exactly the test's own instances across 448–475 processes (R-SAFE).
- A process that leaves its group (`setsid`) and survives a panic is reported in an `escapee` lineage record and an owner alert, never killed (R-SAFE; review F5).
- `resume` is refused while the lineage mirror's `refs/heads/panic` branch exists, and the Root raises its open-file soft limit to min(hard, 4096) at start (R-SAFE).

### Security review fixes (build/review/root-security-2026-10-20.md: 0 critical, 1 high, 3 medium, 4 low)
- F1 (high): money admission cannot overflow; a request's `max_tokens` is clamped, and a `u64::MAX` request is refused at the ceiling (R-DEC).
- F2: the egress meters by default-deny: every billable route is an explicit `(provider, method, path)` row, and anything else is refused before a key is attached (R-SEC).
- F3: at most 96 egress connections (32 per sealed lane) with timed reads and writes; beyond that, `root.busy` (R-SEC, R-DEC).
- F4: the held reservation counts input at one token per request byte, so the money ceiling is a hard wall (R-DEC).
- F5: escapee detection (above). F6: the fd stash keeps at most 64 descriptors for at most 10 s (R-DEC). F7: the health and ceiling endpoints sit behind the in-flight guard (R-SEC).

### Provisional owner decisions (2026-10-20; the owner may override any of them)
The owner said not to wait, so the orchestrator took these for v0.1.0 (RELEASE_PLAN.md; docs/CONVENTIONS.md §6):
- The Root's size budget is ≈ 7,500 lines as per-module sub-budgets (measured + 5%), re-measured at release.
- G4 moves to n = 30, R = 3 plus an A/A calibration per model once the sealed set S has 30 tasks (M7); v0.1 keeps the 12-task stand-in S.
- The constitution is built as written and stays PENDING OWNER.
- M-5 is measured with `phys_footprint` on macOS, baselined at the first context epoch, thresholds unchanged. Under it M-5 passes 3 of 3 (M5).
- Three demotions at L0 within 24 h put the system in safe mode (R-DEC).
- The Root bills cache hits at the provider's cache-read price and keeps the peak window: S1's traffic went from 3.73× to 2.00× the proxy's meter (R-DEC).
- A sealed set S below 12 tasks is refused at import, and a gate-of-gates on a smaller S records `underpowered`, alerts and does not demote (R-DEC).
- The real-model batch uses deepseek-flash only, with a hard cap of $1.00 Root-metered (S-REAL).
- No disposable account is required for v0.1, under owner-machine mode (ADR-008).

### Fixed during the release push (build-deviations.md §6)
- The seed failed its own G0: integration-test roots lacked `#![forbid(unsafe_code)]`, so every T2 promotion and quick gate was rejected (`fade67a`).
- G0's `cargo test` failed the seed in a real install: the `build` profile now allows Unix sockets under the build's `OUT` only (and the kernel profile the same under the build slot's `OUT`, while G0 runs as kernel children); `init-mockllm` ships in every install; the seed's tests keep their homes and sockets in the target dir (`d05c1e3`, `eec8d5e`, `3008357`, `f76e749`).
- G1 rejected every candidate with `gate.g1.manifest_error`: its sandbox `OUT` did not hold the candidate's `bin/` and `ext/` (`70f30d3`).
- A body display line near 1 MiB, or a fast stream, broke the kernel's link to the Root and counted as a crash: the kernel now splits, cuts and paces owner frames (`15dbf74`), and the Root refuses owner lines over 900 KiB (`44b9c6e`).
- A handoff could wait forever when the new body crashed while the old one retired (`5e50940`).
- Build timeouts from `config.toml` were ignored, and a build could wait on a descendant holding its pipes (`28f204d`, `2794924`).
- `initctl gate --quick` could not fetch `HEAD` or a snapshot commit (`f8c212d`), and `selfmap --root .` broke extension builds (`40600e0`).

### Known limitations
- L0 only (`SUPPORTED_MAX_LEVEL = 0` in release builds); macOS only (the Linux sandbox is unverified); DeepSeek only for the installer's wizard. The off-machine lineage mirror is off until its repository exists.
- **Panic and escapees** (ADR-008 residual risk): a zone process that calls `setsid()` leaves its process group and survives a panic until `live-sweep` is enabled on a build verified on a disposable account. It keeps the zone's sandbox (no IP network, no access to the Root's files or keys), its egress socket closes at panic, and it is reported by pid so the owner can end it.
- The process sweep (`live-sweep`) stays compiled out on the owner's machine until the disposable-account tests run (ADR-008).
- G0 runs the candidate's tests as the kernel's children in the kernel's sandbox profile, not in separate `build` instances; the socket rule for G0 in the kernel profile is temporary.
- M-5 passes on the provisional rule only (it fails on `ps` RSS from step 100), and the ratio's margin is thin (worst 1.198 against 1.25), with peaks at compaction. M-3 (agents writing extensions) is not measured.
- Every agent T2 attempt runs a cold `selfmap` build of the body (about 1 GB and 5 min inside the zone), and the build stage's environment differs from G0's, so wasmtime is probably recompiled for about 2 min on every T2 gate.
- Sealed evaluation runs are not in the Root's spawn-broker table. They stay within `max_instances` through the zone's quota, so this is bookkeeping, not a safety gap.
- The Root/proxy money ratio on real traffic is unmeasured since cache-read billing: the real-model batch ran on a Root built before it.
- Extensions: after a body restart, `initctl reload` reports "not built" until the extension's source is touched (a no-op cargo build keeps the old mtime). Probes get an empty extension set, and a T2 that also changes `ext/` hands off with the old set; a kept T1 set does survive a T2 handoff.
- `install --upgrade` is not built, and the GitHub `fast-linux` workflow has not run yet.
