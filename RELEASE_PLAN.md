# Release plan: v0.1.0, owner-usable (2026-10-20)

*The owner's instruction (12:16): "chủ động làm đi đừng chờ tôi, cần đạt release sớm nhất có thể, dùng subagents parallel". The orchestrator runs this plan without waiting for the owner. Progress goes in `BUILD_LOG.md`.*

## What v0.1.0 means (definition of done)

1. **Install.** One command, `scripts/install.sh`, builds the release binaries, writes a charter from a short wizard, and runs `init-root install`. The owner then runs `init-root keys set deepseek` themselves; the script never touches the key.
2. **Chat.** `init` opens the TUI against the running Root. It works on real DeepSeek: streaming text, tool calls, cost and level shown.
3. **Self-evolution at L0 on the real model.** A real T1 promotion (an extension the agent writes, A2-real) and a real T2 promotion with the rollback path (A3-real) both work end to end.
4. **Safety on the owner's live account (owner-machine mode).** Stop, `/panic`, ceilings and rollback are verified. Signals go only to processes the Root holds (by PID), and live-sweep is compiled out. A sweep dry-run over the real process table proves the D1 predicate selects only INIT's own instances, without sending a single signal. The disposable-account tests remain required before live-sweep is ever enabled (ADR-008).
5. **Green CI.** `ci/local.sh` is green 3 times in a row with no flakes, and the GitHub `fast-linux` job is green.
6. **Docs.** README quickstart, CHANGELOG, and a git tag `v0.1.0` on the private repo.

## Release status: v0.1.0 tagged at 2ea6ce2 (2026-10-20 ~20:50)

| DoD item | Evidence |
|---|---|
| 1 Install | `scripts/install.sh` E2E at the release commit: install 204 s, ready 1.3 s, `init` opens init-tui, mock turn wrote hello.txt = INIT, audit verify OK, uninstall to Trash (build/status/rel-e2e.json) |
| 2 Chat | TUI e2e on the real Root in CI; real DeepSeek A1/E2E-2 (measurements/real-v0.1.md) |
| 3 L0 self-evolution, real model | A2-real (T1 extension) and A3-real (T2 self-edit → G0–G3 → handoff on the same session → real G4) both PASS; E2E-3 PASS |
| 4 Owner-machine safety | ADR-008: held-pid signals only, live-sweep compiled out, sweep dry-run over the live table, escapee alerts; review F1–F7 fixed |
| 5 Green CI | `ci/local.sh` green 3× in a row (1,333 passed, 0 failed each, 19:08–20:19), GitHub fast-linux green |
| 6 Docs | README, CHANGELOG v0.1.0, docs/release.md, phase0-report v0.2, spec v0.4 proposal |

Real-model spend for the whole push: $0.21 (balance $10.62 → $10.41), Root-metered $0.529 of the $1.00 hard cap; 0 key leaks.

## Provisional owner decisions (taken 12:16 because the owner said not to wait; the owner may override any of them)

| Decision (build-deviations §3) | Taken as |
|---|---|
| §3.1 Root size | Per-module sub-budget = measured + 5%, rounded up to 10; ≈ 7,500 total at dd5c358 (root.md §4.8 method), re-measured at release. Core and sandbox-profiles are counted on their own lines. |
| §3.2 G4 power | n = 30, R = 3 plus one A/A calibration per model, as the default once S has ≥ 30 tasks (M7). v0.1 keeps the 12-task stand-in S. |
| §3.4 Constitution | Built as written. It stays PENDING OWNER; the owner should still read it. |
| M-5 method (FIX8) | `phys_footprint` on macOS, baseline at the first context epoch; thresholds unchanged |
| §3.9 Repeated demotion | 3 demotions at L0 within 24 h trigger system safe mode |
| §3.13 Root money meter | Bill cache hits at the provider's cache-read price; keep the peak window |
| §3.15 Small S | S < 12 tasks gives `underpowered`, with no demotion, plus an alert |
| §3.17 Real-model batch | deepseek-flash only. Hard cap $1.00 Root-metered; no new checks after $0.50 Root-metered. |
| Disposable account | Not required for v0.1, under owner-machine mode (item 4 above, ADR-008) |

## Lanes (all started 12:20)

| Lane | Owns | Output |
|---|---|---|
| R-SAFE | init-root zone/sweep/panic/doctor + tests; ADR-008 | owner-machine mode, sweep dry-run, live-safe RT/SW tests |
| R-DEC | init-root egress money, sealed, ledger, sup halts, logs | §3.9, §3.13, §3.15 and the Root fix-code rows |
| K-FLAKE | test helpers and integration tests (kernel), seed `h21` | bounded timeouts everywhere; 3× green kernel and seed suites |
| S-REAL | scripts/slice1, tests/real, seed provider/loop/tools (bug fixes only) | real-model flow batch within the cap; bugs fixed |
| M5 | init-measure, seed log/context | footprint sampler, first-epoch baseline, `MessageEv` boxing, M-5 PASS |
| REL | scripts/{release,install,uninstall}.sh, README, CHANGELOG, .github/workflows, release profiles | install path tested in a repo test HOME |
| TUI | `docs/tui.md`, then the TUI crate | `init` TUI (WP26) |
| DOCS | docs/** (except the files other lanes own), spec v0.4 proposal | provisional decisions in docs, leftovers, Phase 0 report |
| REVIEW | read-only | Root and proxy-boundary security review → fixes routed to the R lanes |

## Rules

- `BUILD_AGENT_BRIEF.md` binds every lane. Shared default target dirs; `source ci/env.sh`; disk floor 5 GB.
- No kills of processes a lane did not spawn. No `live-sweep`. Tests never touch `~/.init` or `~/.init-root`.
- The real model is used only by S-REAL, within the cap above. The key is read from `~/.config/init-agent/deepseek.key` and never printed, logged or committed.
- No cloud spend.

## Backlog after v0.1 (from the Harness² research, reports/Google harness2 cho INIT.md)

None of this blocks v0.1.
1. Require a `Repro:` line on any `skill` commit that adds lines. Today such commits pass with only G0–G1. 2–3 agent-hours; a candidate for v0.1.x.
2. A lessons index distilled from gate verdicts, including rejected changes. 4–8 h.
3. Tag proposals as INV (how the agent works) or DOM (domain knowledge). 1–2 h.
4. A deferred G4 run over batches of T0 changes. 4–6 h.
5. Assert that the full run grid is present in G4 before any statistics are computed. 1–2 h.
Later: separate editor and executor models (L1); a compose step for the fleet (L2).

## v0.1.x items found during the release push
- T2 cost: every agent T2 attempt runs a cold selfmap `cargo build --frozen` of the body into `<root>/target`, about 1 GB and 5 min inside the zone. Reuse the gate's G0 target or keep a warm target dir (S-REAL).
- WP24 D7: register sealed spawns in the broker table. v0.1 stays within `max_instances` through the zone quota; the design note is in build/status/R-SAFE.md.
- Real-batch meter: S-REAL's batch ran on the input-price meter (frozen Root built before cdb49e8). Re-measure the Root/proxy ratio on the next real run.
- M-5 ratio margin is thin (worst 1.198 against 1.25). Peaks come at compaction, when several context copies exist. Drop the loop's held messages, the provider's second render and client.rs's request copy.
- T2 gate cost: the build stage's environment differs from G0's (RUSTC_WRAPPER), so wasmtime probably recompiles for about 2 min on every T2 gate. Align the environments (G0-SEED).
- Under the `build` profile, `tools::testutil::homes_root` writes into the read-only checkout (49 tests). This does not affect production G0, which runs under the kernel profile; fix it before G0 moves into `build` instances (G0-SEED).
- Docs still describe init-mockllm as testkit-only (CONVENTIONS, testing.md, supervise.md); since eec8d5e it ships in every install.
- Ext UX: after a body restart, `initctl reload` says "not built" until the ext source is touched, because a no-op cargo build keeps the old mtime. This can waste agent turns. Fix the freshness check, or document it in the ext skill (K-GATE).
- Probes always get an empty ext set, and a T2 that also changes ext/ hands off with the old set (supervise.md §3.6.5). A kept T1 set does survive a T2 handoff, proven in c7a1319 (K-GATE).
- Seed body size: the seed's `body_tokens` is 359,557, against the spec's 25k body budget. Nothing enforces that budget yet. Decide between enforcing it and raising the budget (SELFJSON).
- The docs name `init-kernel doctor --sandbox-selftest`, which doesn't exist; the self-test runs as `init-sbx-selftest run`. Add the subcommand or fix the docs.
- **RT-GAP-1 (RED-TEAM 0052df5, medium, latent):** the L1 unlock counts kernel-self-reported promotion records, and the Root only checks that the body blob is in its store, which the kernel can fill itself. In a test build, 1 genuine clean audit plus 20 self-reported promotions unlocked L1 (`redteam.rs`, ignored test). Release builds cap the level at L0, so nothing ships exploitable. Fix: count only promotions backed by Root-held `sealed_g4` accept records (m7: "≥ 3 passed an S-backed G4"), in `ledger.rs`/`tuple.rs`. Must land before any build allows L1.
- **HIGH, MEM-RUNAWAY (M7 D, 2026-10-21 06:48):** a single sandboxed python3.9 inside a sealed run grew to 43.9 GB of compressed memory. macOS swap grew on the system volume until free disk hit 0.24 GB, and then jetsam killed it. Seatbelt has no memory limit, so the agent's bash tool on the owner's machine can do the same in daily use. Fix: a per-process memory watchdog (phys_footprint polling; kill a workload over a charter cap, e.g. 4 GiB) in the kernel's tool runner and in the Root's sealed run supervisor, plus a Root-side disk floor that refuses new runs. tests/real/m7watch.py (e51f39b) is the harness-level stopgap. Linux: cgroups memory.max. **Fixed (MEM-RUNAWAY lane, 2026-10-22; build/status/MEM-RUNAWAY.md):** the Root's `memwatch.rs` polls every held zone instance and sealed run each second, kills a workload tree over `sandbox.mem_cap_mib` (default 4096, 512–65536) by pid (never the Root or the kernel), refuses and kills workloads below `sandbox.disk_floor_gib` (default 5), records each kill for the body (`INIT_MEM_EVENTS`; bash reports "process exceeded memory cap N MiB, killed") and alerts the owner (root/root.md §3.14.1, tools.md §3.5.8, ADR-008 amendment). Kernel probes and gate instances are covered because the Root spawns them. Linux cgroups: TODO. The live install needs the new `init-root` binary (no `install --upgrade` yet).
- Kernel probe roots leak (about 1.4 GB per G4 screen phase): each clones INIT_HOME/kernel including the read-only imported pool, remove_dir_all stops at EACCES, and the kernel ignores the error. Make the copies writable before removing them, and surface the error. **Fixed (K1-FIXES, ADR-011 X0d, a3fcae5; build/status/K1.md):** on EACCES the delete gives every directory in the root owner rwx without following a link and removes it again; a failure is logged, alerted (`probe_root_leak`) and returned as `ProbeOutcome.delete_error` (RF-05).
- The Root has no kernel-upgrade path, so a fixed kernel needed a full home reinstall in M7.
- **G3-STAGE-TIMEOUT (M7 D, 2026-10-21 12:05):** the G3 stage defaults (`gate.g3.agent_timeout_s`, `inner_timeout_s`, both 300 s) are shorter than a cold build slot's body build (~280 s) plus the model's turns. A promote after the slot is cleared fails G3 with `timeout` (j-000010, 589.6 s). `74a14a1` raised only the model's bash timeout. Fix: derive the stage timeout from the build timeout (cold 3600 / warm 1800), or start the clock after the inner build. M7 ran with 1800 s via config (DV-5c). **Fixed (K1-FIXES, ADR-011 X0c, bd9732c):** the agent part is bounded by `agent_timeout_s` + `build_timeout_cold_s`, and the inner gate's clock starts after its build (RF-03); on the free path G3 runs after the change goes live and vetoes through `change.veto` (A3, RF-04).
- **STRANDED-PROPOSAL (M7 D, 2026-10-21 11:03):** an interrupted G4 job leaves its proposal stuck at `promoted`. A kernel restart during G4 turns the job into `kernel_restart`, and the provisional then expires only after 24 h. The validator (`kernel-gate` evo/validate.rs, `proposal.state`) accepts an evo commit only while its proposal is `screened`, so the same chosen sha can never be gated again, and no verb re-runs G4 on the current provisional. Fix: on a G4 job recovered as `kernel_restart`, or on a rollback without a G4 verdict, return the proposal to `screened` with a history entry (or add `promote --g4-only`). The M7 stopgap was a harness edit of the test-home proposal file (DV-5b, `checks_m7.py dv5-reopen`, 099684a). **Fixed (K1-FIXES, ADR-011 X0b):** every path without a verdict returns the proposal to `screened` with a history entry: a stale provisional, `initctl rollback`, a failed promote job, a kernel restart (resumed from the Root's records at start), and a non-direct rollback once. G4's final verdict sets `verdict` (RF-01, RF-02).
- **RT-GAP-4:** symlink write-through from the zone has not been tested yet (sandbox lane).
- kernel-selfmap s08/s09/s12 time out under load: s08 waits on a shared fixture build inside its 30 s watchdog. Arm the watchdog after the fixture is ready, or give fixture-building tests the build bound (v0.1.1 CI run 1).
- Lanes running the full init-root suite at the same time collide on shared test homes and on the shared `init-root` binary (RED-TEAM: wp24 4/6, wp25 0/4, both green on re-run). Use a test home per test binary run (pid-suffixed) and a frozen binary copy, as `redteam.rs` does.

## v0.1.1: workspace permissions (owner decision 2026-10-20, V1_PLAN phase A)

| Item | Status |
|---|---|
| **Workspace (ADR-009, Codex-style permissions):** home read minus a secrets deny list; write `INIT_HOME` plus `init`'s cwd; `--full`; APFS snapshots and `/undo`; charter keys `sandbox.read_home`, `read_deny`, `allow_full` | **docs done** (DOC-009: docs/adr/009-codex-style-permissions.md, kernel/sandbox.md §3.1.1, root/root.md §3.17, tui.md §3.8.1, body/boot.md, awareness.md, CHANGELOG "Unreleased (v0.1.1)"). Code in progress: SBX-009 (Root and sandbox), WIRE-009 (TUI, kernel, body). The SBPL blocks in root/root.md §3.3.1–§3.3.2 are copied verbatim from SBX-009's 8570638 (kernel/sandbox.md's are unchanged). |
| Write-deny git hooks and config, shell startup files, git user config, `~/.local/bin`, `~/bin`, login items and `bin_dir` in every mode (ADR-009 residual risk; root/root.md §9.3 Q5) | **done** (SBX-009 cc16577: tails Z3a and Z3b, self-test S-A20–S-A26b). Left open: workspace build files and the full-mode vectors listed in ADR-009. `scripts/install.sh` should pass `--bin-dir` for a custom `INIT_BIN_DIR`. |
| **Dev checkout in `read_deny`** (provisional owner decision 2026-10-20, owner may override; CONVENTIONS §6; m7-design.md §3.1.2). With ADR-009's `read_home` the live body can read the owner's home, which includes the dev checkout `~/Documents/INIT Agent` and, from M7, the S pack generators in `root/sealed/packs/` and their `.git` objects. The checkout's absolute path therefore goes into the charter's `sandbox.read_deny`, and an install from a dev checkout (`--seed-source`) writes it into the template. Side effect: the Root refuses the checkout as an `init` workspace (`deny_list`), which is fine because INIT is developed with Claude Code | **docs done** (DOC-M7: root/root.md §3.12.2, CONVENTIONS §6). Open: the installer's template code (no lane assigned yet), and the owner's existing charter, which needs the entry through the owner-only edit flow (`init-root charter check --apply`, root/root.md §3.12.4) before any pack file lands in the checkout |
