# Changelog

All notable changes to INIT Agent. Versions follow [semver](https://semver.org). The design is `INIT Agent spec.md`; each line below points at the build status file that records it (`build/status/`).

## v0.4.1-preview (2026-10-27)

Workspaces are at version 0.4.1 (seed tag `seed-v0.4.1`). Release notes: docs/release-notes/v0.4.1-preview.md.

### The command is `init-agent` (nagisanzenin/INIT-Agent#1, build/status/REL-041.md)
- The launcher `init` is renamed `init-agent` on all platforms: on Linux `~/.local/bin` often precedes `/sbin` in
  PATH, so `init` shadowed `/sbin/init` in user shells. `scripts/init` is now `scripts/init-agent`; `init-root`,
  `init-tui` and `initctl` keep their names. Every owner-facing string that named the command says `init-agent` (the
  Root's no-workspace notice and access status line, the guided-setup hint, the TUI's hints, the charter template
  comment, the body's access line), as do README and the docs.
- Linux: install.sh never installs a file named `init` (its `put_bin` refuses one), and removes INIT's old launcher at
  `INIT_BIN_DIR/init` and `~/.local/bin/init` when it finds one: exact path, a regular file, the old launcher's header
  line and its `INIT_ROOT_HOME_DEFAULT=` line. Any other file named `init` is left as it is, with a note.
- macOS, this release only: `init` is installed as a shim (`scripts/init-shim`, marker `INIT_AGENT_SHIM=1`) that prints
  the new name on stderr and runs `init-agent` with the same arguments; it is written only where no `init` exists or
  over INIT's own old launcher or shim. The next release drops it.
- install.sh refuses a relative `INIT_BIN_DIR` and one that is `/sbin`, `/usr/sbin`, `/bin` or `/usr/bin`, compared by
  device and inode, so a symlink, `..`, doubled or trailing slashes and case variants match too. It is the first
  preflight check, before anything is written (tests/install/test_install_guards.py; a new ci/local.sh step).
- uninstall.sh moves `init-agent`, `init-tui` and INIT's `init` (the old launcher or the shim, by marker) to the Trash;
  a foreign `init` stays.
- The Root's full-access write deny list (ADR-012, Z3a) now names `<bin_dir>/init-agent` besides `<bin_dir>/init` and
  `<bin_dir>/init-tui` (`access::LAUNCHERS`), so the agent cannot replace the owner's launcher under its new name.
  Tests: `full_lists_keep_only_the_root_protected_set`, Seatbelt selftest S-A23a, the Linux full-isolation test
  (write, unlink and rename refused for all three), `full_body_writes_the_home_not_the_root` (both names).
- Harnesses: release-e2e.sh plants a foreign `init` that install and uninstall must leave untouched; bundle-e2e.sh
  plants INIT's old launcher, which the Linux install must remove and the macOS install must turn into the shim
  (`init --which` = `init-agent --which`, the notice on stderr).

### Release (lane REL-041, build/status/REL-041.md)
- Bundles for macOS arm64 and Linux arm64 from 8539ee8 (the Linux body bit for bit the one built from 7c15772). On
  both: a fresh install with INIT's old launcher planted (removed on Linux, the shim on macOS) and the pause check; on
  macOS also the source install with a foreign `init` kept (`release-e2e.sh`) and a no-op T2 promote (G0–G3). Full
  `ci/local.sh` on macOS: 1,925 passed, 4 failed, 38 ignored, the two failing steps rerun green at 96008ac
  (`kernel test` 887 / 0 / 12, `tui test` 98 / 0 / 1): 1,929 passed, 0 failed, 38 ignored. clippy `-D warnings` clean in
  the changed workspaces.
- Two failures found on the way and fixed: root.md §3.3.2a's verbatim copy of the full tail's Z3a comment said
  init-agent while `zone-tail-full.sb` did not (`doc_drift`; the profile comment now names it, 8539ee8), and the TUI's
  launcher test still ran `scripts/init` (now `scripts/init-agent`, plus a check of the macOS shim, 96008ac).

## v0.4.0-preview (2026-10-26)

Workspaces are at version 0.4.0 (seed tag `seed-v0.4.0`). Release notes: docs/release-notes/v0.4.0-preview.md.

### Release (lane REL-040, build/status/REL-040.md)
- Bundles for macOS arm64 and Linux arm64 from 4a76b12; verified on both (fresh install, the pause check), a no-op T2
  promote on macOS, the GBAL mock flow on the macOS bundle (`tests/real/gbal_proof.sh --mock`, new knob `GBAL_BUNDLE`),
  and one live DeepSeek turn. Full `ci/local.sh` on macOS: 1,929 passed, 0 failed, 38 ignored.
- `cargo clippy --all-targets -- -D warnings` is clean in all four workspaces: the pre-existing lints in the Root
  (`sealed/`, `zone.rs`, `sys.rs`, `egress/money.rs`, `tests/wp22.rs`) and the kernel (`kernel-proxy/config.rs`,
  `kupgrade/kstate.rs`, `init-kernel/upgrade.rs`, `supervisor.rs`) are fixed or allowed with a reason, and one in the
  seed (`boot/gitsync.rs`, a needless borrow from the G2 fix) is fixed. `ci/local.sh` has no clippy step; this was run
  by hand.

### GBAL: goal-directed self-modification (ADR-013, build/status/GBAL.md)
- The owner sets a goal with `/goal "<intent>" [done=…] [usd=…] [wall=…] [rounds=…] [guard=…]` (TUI, `init-root attach`);
  the agent can only propose one. The Root meters the goal's spend, wall time and rounds and stops it at its budget.
- The agent files a prediction before each change (`initctl goal predict …`); the Root timestamps it and marks any
  reading of a change without an earlier prediction `registered: false`.
- Six Root observation cards (`cost_turn`, `wall_turn`, `requests_turn`, `cache_read`, `provider_errors`, `corrections`)
  read from the Root's own egress and owner-channel data; readings carry n, mean, σ, se, z and a verdict.
  `Measure: card:<id>` and `card:<id> gate` join `Measure: g4`; `initctl goal read|measure`.
- Outcomes (card reading, owner accept/reject, guardrail harm) and a Brier score per goal kind; calibration sets only
  the rounds between owner check-ins (1–12), never the owner's budget and never the autonomy ledger.
- Owner guardrail cards (and the agent's own card gate) roll a change back on H12's path.
- Seed skills `goal-loop` (artifact goals: frozen checklist, pairwise judging with position swap, ≥ 3 runs, stop reasons)
  and `gbal` (mode A/B/hybrid by three instrument tests, predict → change → measure → keep at 2σ → stop with a reason).
- Tests: Root `goal::tests` (7), the mock-model acceptance `root tests/gbal.rs` driven by `scenarios/root/gbal.json`.

### Fixes from the real-model demo of 2026-10-25 (the owner's private demo records, F1–F6)
- **F1** A fresh gated install (no sealed S, empty pool) rolled every T1/T2 change back ~1.3 s after its swap (G4
  inconclusive → escalated → `inconclusive_final`). G4 with no instrument is now `accept: no_instrument` at once: kept on
  G0–G3, one `g4_no_instrument` alert (kernel-gate `no_instrument_is_no_evidence_not_a_rollback`, kernel-supervise
  `f7_f11_f13_f14_g4_branches`).
- **F3** An owner `/rollback` of the first free-path change quarantined GENESIS.md, SELF.md and four seed skills: a parent
  tuple without a t0 blob made every T0 cap an `add`. Such caps are now `baseline`: learned, never attributed to a group,
  never quarantined (`f3_seed_caps_on_a_parent_without_t0_are_baseline_never_attributed`).
- **F2** Switching `extend.mode` while the kernel ran was not picked up (the next handoff was refused as
  `unwatched_change`, logged as an L0→L0 demotion, whole-system restart). `charter.apply` now pushes the mode to the
  kernel and the kernel's ceilings refresh re-reads it (`f2_a_charter_mode_switch_reaches_the_running_kernel`). An
  incident at L0 is recorded as `ladder{direction:"floor"}` with an `incident` alert, not a demotion; it still counts
  toward the 3-in-24-h safe-mode rule (`an_incident_at_l0_is_floor_not_a_demotion`).
- **F4** Gate notices said only `[kernel] gate`; they now name stage, verdict, sha and reason with what happens next
  (`f4_gate_notices_say_why`).
- **Docs** docs/guide.md: `/rollback` and `/goal` in the daily-use table; the free path needs `disk_floor_gib` + 2 GiB free.

### Fixes from the GBAL real-model proof (build/status/GBAL-REAL.md)
- **G1** The Root closed an owner turn only after 30 s with no model request, so an owner line sent sooner after the
  reply counted as a steer of the finished turn: on the $0 dry run five questions 2 s apart made one turn and the GBAL
  cards saw no turns at all (`goal read` n = 0, no reading, `met` unverifiable). The kernel's `state{busy:false}` now
  closes the owner turn once its requests have ended (sentinel `kernel_idle_closes_the_owner_turn`); a line typed while
  the body works is still a steer.
- **G3** `initctl goal predict` accepted a prediction for a change already live (it can never be registered) and spent
  one of the owner's rounds on it; on the real run the model amended its commit, promoted, then re-filed, and its goal
  change stayed uncalibrated. It is now refused (`goal.late`, naming the tuple and saying to predict the final commit
  before `initctl promote`), and an abbreviated sha is refused (it never matched the tuple's full sha)
  (`goal::tests::predict_measure_outcome_guardrail_budget_and_stop`).
- `tests/real/gbal_proof.sh` runs: `--mock` ($0, scripted init-mockllm) and the real mode, with the key copied into the
  test Root (`keys set` reads only a TTY), owner lines through `init --raw` (`tests/real/gbal_drive.py`; there is no
  `attach --line`), the spend from `egress stats` `money.total_spent_musd`, a workspace-mode charter generated from the
  template, a spend and disk watchdog, and a verdict that sets the exit status.

- **G2** (serious) A guardrail or gate rollback of a free-path change whose parent was not the kernel's LKG (every round
  after the first while a group stays open) was not applied by the kernel: the bad change stayed live until the Root's
  120 s whole-system fallback, and the agent's own `initctl rollback` meanwhile went all the way to LKG (genesis),
  dropping the good change too. The kernel now rolls back to any parent it reaches through admitted t0 changes only as a
  data-only rollback (the running binary, a new body instance; lineage `main` moves before it boots, and the body's
  `body/` resync now also leaves a rolled-back `evo/<id>` checkout, so the target's T0 data is what the head reads),
  confirmed to the Root by `body_rollback` at once. `initctl rollback` on the free path undoes only the group holding the
  running change: an open group is vetoed (H12), a closed one rolled back to its parent (kernel-supervise
  `g2_rollback_to_an_open_groups_parent_is_data_only_and_prompt`, `g2_data_only_rollback_to_a_non_lkg_target`; seed
  `g2_head_on_a_rolled_back_commit_is_resynced_to_main`).
- **G4** Calibration scored a guardrail rollback as o = 0 even when the prediction forecast that very regression
  (P-000003: `expect=worse`, p 0.9, read worse at z −2.7, Brier 0.81). The outcome now scores the forecast on its own
  card: o = 1 when the card moved in the predicted direction beyond noise; a guardrail or gate rollback is recorded on
  the outcome (`harm`) but is not by itself a miss; a reading with no noise floor scores nothing; a change rolled back
  before its card reading had n turns is read on the turns it had (≥ 3) (`goal::tests::g4_a_predicted_regression_caught_by_a_guardrail_is_a_hit`).
- **G5** The kernel's `Predicts:` grammar refused card ids (`requests_turn`: "unknown metric"), which cost the model two
  amends. `card:<id>` clauses now parse (unscored by the kernel: the Root reads the card and `initctl goal predict` is
  the registered forecast), and a bare card id is refused with the `card:<id>` spelling (`g5_card_clauses_parse`).
- **G6** skills/gbal said to predict "before it goes live", which the model read as "before the next owner turn". It now
  says: on the final sha (after any amend), before `initctl promote`; it also states G4's scoring. seed/self.json
  re-blessed.

- **H2 ack race** An owner line's `owner.in` ack that the kernel sent before link.rs reported the line to the sentinel
  was lost, so the line counted as deaf; two such lines in a row fired H2 (a kernel restart with no group open, else
  every open group rolled back). The GBAL e2e failed about 2 runs in 6 on it. The sentinel now keeps an ack that comes
  before its line (last 64) and the line is born acked (sentinel `an_ack_before_its_line_is_not_deafness`).

### Fixes after the GBAL rerun (build/status/GBAL-REAL.md "Rerun", build/status/REL-040.md)
- **G7** The kernel's commit rules refused the promote *after* the prediction (8 refusals in 3 real runs: an unknown
  subject type, a missing `Why:`, prose in `Repro:`), each amend made the filed prediction stale, and every re-file spent
  one of the owner's rounds and one check-in step: run A used up `rounds=3` before anything went live. Three changes:
  `initctl promote --check evo/<id>` runs promote's synchronous commit checks (fetch, candidate checks, the validator under
  the current `extend.mode`, the scope) and answers at once, with no job, no quota ticket, no gate and no record
  (kernel-supervise `promote::check`, kernel-serve `Backend::promote_check`; `f1_f2_f3_f4_f9_failures_before_and_at_the_handoff`
  asserts the check and promote report the same codes, and that a good commit passes with nothing left behind; initctl
  golden rows); a re-file that replaces the goal's newest prediction while its change never went live keeps the round,
  spends no round and no check-in step, and closes the old prediction `superseded` with its pending readings, so it never
  registers (`goal::tests::g7_a_refile_before_live_supersedes_and_spends_no_round`); skills/gbal runs `promote --check`
  before predicting and checks the prediction's answer before promoting, and skills/extend lists the subject types and
  the exact `Repro:` forms. `initctl goal status` lists the open predictions.
- **`met` requires attribution** (owner decision). A card goal used to be `met`, verified, whenever the newest kept
  reading reached the threshold, also on noise (runs B and C: 2.4 → 2.0 requests per turn, z +1.64, threshold ≤ 2). It is
  now `met` only when, in addition, a registered change moved the goal card beyond noise in its prediction's direction
  toward the threshold (`attributed` on the stop record); reaching the threshold otherwise is recorded as
  `met_unattributed` (`claimed: "met"`, `verified: false`) with a `goal_met_unattributed` owner alert
  (`goal::tests::met_requires_a_registered_change_beyond_noise`; ADR-013 decision 5; skills/gbal).
- **Goal meter = egress ledger.** The per-goal money meter added each closed turn's cost, so it missed requests that
  settled after their turn closed and the turn in which the agent stopped the goal (rerun C: 51,290 against the ledger's
  59,718 µUSD). The Root now charges every egress request it settles while the goal is active, at settlement, with the
  `end` record's own `cost_musd`; the money budget is checked there too (`the_budget_meter_stops_the_goal`: paused spend
  is not the goal's, no double count; the mock-model e2e `tests/gbal.rs` asserts goal meter = egress ledger to the µUSD).
- seed/self.json re-blessed (skills/gbal, skills/extend).

## v0.3.3-preview (2026-10-25)

Workspaces are at version 0.3.3 (seed tag `seed-v0.3.3`). Release notes: docs/release-notes/v0.3.3-preview.md.

### DeepSeek: no refused request after a pause (f3f8dbf, docs/body/provider.md §3.8.1, build/status/REL-032.md "Observation")
- Every assistant message on the DeepSeek wire now carries `reasoning_content`, `""` when it has no DeepSeek
  thinking: a turn paused before its first token (`[no visible output]`), a tool call whose reasoning came back
  empty, and a foreign turn after a provider switch. Before, each such resume was refused once (400 `reasoning_content
  missing`) and the `deepseek_empty_reasoning_foreign_turns` fallback retried; the fallback stays as a safety net.
  Both request builders (the context renderer the loop sends, and `Provider::build`) changed together.
- Tests: `pause_before_first_token_resumes_without_400` (seed/tests/boot.rs, strict DeepSeek mock: the resume is one
  accepted request, no 400, no fallback); the strict-mock foreign-turn test now expects an accepted first request and
  checks the fallback on a body without the field.

### Release (lane REL-033, build/status/REL-033.md)
- Bundles for macOS arm64 and Linux arm64 from cb83a41; verified on both (fresh install, the pause with the strict
  DeepSeek mock: two requests and no 400), a no-op T2 promote on macOS, and one live DeepSeek turn.

## v0.3.2-preview (2026-10-25)

Workspaces are at version 0.3.2 (seed tag `seed-v0.3.2`). Release notes: docs/release-notes/v0.3.2-preview.md.

### An owner interrupt pauses the task (67cf3c1, docs/body/loop.md §3.5.1)
- Esc or Ctrl-C while busy used to end the task; an interrupt before the first token left an empty assistant
  message, and the next run read the owner's words as a new request. The input thread now records owner interrupts
  (`Abort::trigger_owner`, `trigger_owner_continue`), and a run that ends aborted with that record appends one harness
  message `[init:paused] …` after the results: finished work stands, calls marked `aborted: not executed` did not
  run, and the owner's next message resumes the task unless it changes or drops it. The loop shows `paused · type to
  continue, or say what to do instead` instead of `aborted`.
- The `stop` grace, EOF and faults still abort, show `aborted` and add no note; interrupt-and-steer that continues
  the run adds no note. The note is appended once and never rewritten (the prefix cache is unaffected).
- TUI: the Esc hint says `pausing · type to continue, or say what to do instead`; the Esc help line says pause.

### Release (lane REL-032, build/status/REL-032.md)
- Two prebuilt bundles, `init-v0.3.2-preview-macos-arm64.tar.gz` and `init-v0.3.2-preview-linux-arm64.tar.gz`, each
  with a `.sha256`, and one `latest.json` for both, built from 37be317 (`seed/self.json` re-blessed for the seed
  change). Verified on these exact bundles: fresh macOS and Ubuntu 24.04 installs, a no-op T2 promote (G0–G3
  and handoff, 13 min cold on an M2), the pause on both bundles (a stalled turn paused by SIGINT, then resumed with
  the `[init:paused]` note in the next request) and one live DeepSeek turn.
- Docs: docs/guide.md and docs/tui.md say Ctrl-C and Esc pause the turn.

## v0.3.1-preview (2026-10-24)

Workspaces are at version 0.3.1 (seed tag `seed-v0.3.1`). Release notes: docs/release-notes/v0.3.1-preview.md.

### Release (lane FULLACCESS-release, build/status/FULLACCESS.md "Release")
- Two prebuilt bundles, `init-v0.3.1-preview-macos-arm64.tar.gz` and `init-v0.3.1-preview-linux-arm64.tar.gz`, each
  with a `.sha256`, and one `latest.json` for both, built from e4215ed. Verified on these exact bundles: fresh macOS
  and Ubuntu 24.04 installs (access FULL by default), a no-op T2 promote (G0–G3 and handoff, 17 min cold on an M2) and
  one live DeepSeek turn.

### Full access by default (ADR-012, lane FULLACCESS, build/status/FULLACCESS.md)
- **The live body now has full access by default:** it reads, writes and runs anything the owner can and uses the IP
  network, except the Root-protected set: `ROOT_HOME` (files and its sockets), the Root's service file and directory
  (`~/Library/LaunchAgents`, `~/.config/systemd/user`), INIT's launchers (`<bin_dir>/init`, `<bin_dir>/init-tui`),
  `INIT_HOME`'s Root-written parts and marks, `~/.config/init-agent` and the charter's `sandbox.read_deny`; no signals,
  XPC, LaunchServices or AppleEvents outside its sandbox. Fleet members and probes never get it.
- ADR-009's behaviour is kept, unchanged, as **workspace mode**: charter `[sandbox] mode = "workspace"` (new key,
  default `"full"`), `init --workspace` for one session, or `/full off`. `/full on` needs a typed confirmation only
  when the charter says `"workspace"`. `allow_full = false` still forbids full access.
- A full grant sets the body's `HOME` to the owner's home (so `~/Downloads` is the owner's), drops
  `CARGO_NET_OFFLINE` and adds `~/.local/bin` and `~/.cargo/bin` to `PATH`. The body's git sync now runs with
  `GIT_CONFIG_GLOBAL=/dev/null`, so the owner's `~/.gitconfig` does not affect it. The boot block's access line says
  what full access allows and denies.
- macOS: a full ROOT_ALLOWS group and a deny-only tail `zone-tail-full.sb`. Linux: bubblewrap without a network
  namespace, the host root bound read-write with the protected set hidden or read-only, and a seccomp program without
  the socket-domain rule. Full access works on Linux; workspace mode there stays the agent's own home.
- TUI: `--workspace` (excludes `--full`), a `FULL` (red) or `WORKSPACE` (yellow) badge that is never dropped, new
  `/full` help; `init-root status` shows the access mode. `install.sh` states the default and how to confine it.
- **Residuals** (ADR-012, build/status/RED-TEAM.md §6): writes outside the start folder have no `/undo`; files the
  owner's tools run later (shell startup files, `PATH` directories, editor configs, git hooks) are writable, so full
  access is trusted like the owner's shell; anything readable may reach the provider or the network;
  `launchctl remove dev.init.root` from inside any sandbox stops the Root (fail-safe: it ends INIT, nothing escapes);
  on Linux, abstract Unix sockets and the system D-Bus (logind) stay reachable.

## v0.3.0-preview (2026-10-24)

A research preview for macOS (Apple silicon) and, experimentally, Linux aarch64. Workspaces are at version 0.3.0
(seed tag `seed-v0.3.0`). Release notes: docs/release-notes/v0.3.0-preview.md. Autonomy stays capped at L0.

### Release and install (lane REL-030, build/status/REL-030.md)
- Two prebuilt bundles: `init-v0.3.0-preview-macos-arm64.tar.gz` and `init-v0.3.0-preview-linux-arm64.tar.gz`
  (built on Debian 12, glibc ≥ 2.36), each with a `.sha256`, and one `latest.json` for both.
- `get.sh` installs Rust 1.97.1 with wasm32-wasip2 and clippy through rustup (no shell-file or default-toolchain
  changes; `--no-rust` opts out), so T1/T2 self-rewrite works on a fresh install. Clippy was missing from the
  minimal profile, and the first no-op T2 promote on a fresh bundle install failed G0 (`gate.g0.clippy`); install.sh
  now records a toolchain only when it has clippy. The first T2 promote on a fresh install
  builds cold (measured on an M2: G0 16 min, 26 min end to end with G3); later ones reuse `cache/deps` (about 2 min
  for a small change).
- `latest.json` carries `sha256_<target>` for every bundle; `get.sh` reads its own target's checksum from it (the
  `.sha256` next to the bundle stays the fallback).
- `scripts/bundle-e2e.sh` follows get.sh's default: Rust is installed into the test home (`RUSTUP_HOME`/`CARGO_HOME`
  there), so the plain run checks a T2-capable install and `--t2` no longer needs the maintainer's rustup.

### Steering: fluid and visible (lane STEER, build/status/STEER.md)
- A pending tray above the input box shows each owner line until the body reads it; it then moves into the
  transcript as `· seen` (the kernel's `state` frame carries `steer_pending`/`steer_seen`/`steer_retracted`).
- Esc with a steer pending interrupts and continues with the note (`interrupt{then:"continue"}`); Up on an empty
  box takes back the newest unread line (`retract{seq}`); Tab queues the box as `/later`.
- `init-root attach` sends a bracketed paste (or `\` continuation) as one line. Line mode prints
  `[steer queued]`, `[steer seen]` and `[steer retracted]`.

### Sub-agents: the built-in `delegate` tool (lane DELEGATE, build/status/DELEGATE.md)
- Ported from the `delegate` tool INIT wrote for itself. A batch's delegates run in parallel (default 4, at most 8),
  results in call order; writers run one at a time. Modes `read` (default; write/edit refused, bash and ext are not
  confined) and `write`. Bounds per sub-task: 8 turns, 24 tool calls, 12 requests, 1M tokens, 600 s wall; no
  recursion; an owner abort reaches every sub-task. A durable record per sub-task under `state/spill/`.
- Sub-requests carry `x-init-subtask: delegate-<call id>`; the TUI nests sub-task calls under the delegate row.

### Root sentinel: steering and sub-tasks are not harm (lane MERGE, build/status/MERGE.md)
- An owner correction is now only an interrupt (Esc/Ctrl-C, also `then:"continue"`), `/stop`, `/panic`, `/undo`,
  `/reject` or `/rollback`. Plain steers go to a weak `steered` count; `/later` counts as nothing; a retracted line
  takes its steer back. The `v2_fix` long-session behaviour is unchanged.
- The kernel proxy forwards a well-formed `x-init-subtask` to the Root as `x-egress-subtask` (never upstream). H10
  (requests/turn) and H5b (cost/turn) count only the main loop's share, so `delegate` is not read as a harm spike.

### Warm T2 builds (lane MERGE, build/status/MERGE-T2CACHE.md)
- The kernel's `cache/deps` is now filled and reused: on a miss a stub workspace builds only the third-party
  dependencies before the candidate's code runs; the entry is published only after the candidate's G0 passes, and a
  hit is used only when its tree digest matches. Vendor mtimes are pinned and slot builds use a fixed
  `SOURCE_DATE_EPOCH`, which forced rebuilds before. Measured on the real seed under the real Seatbelt profiles
  (mock, load 10–23): first promote 1029 s (build 925 s), second promote of a small change 126 s (build 32 s).

### Linux: native support (lane LINUX, build/status/LINUX.md)
- **Verified:** Debian 12 (build and tests) and Ubuntu 24.04 (no-Rust bundle e2e), Linux 6.10, aarch64, in Docker at
  $0 on the mock model. **Not verified:** x86_64 (the scripts build it, but nothing ran there), real hardware or a VM,
  Ubuntu's AppArmor user-namespace restriction, SELinux, kernels before 6.1, WSL2, and a live provider on Linux.
- **The zone:** bubblewrap ≥ 0.13.0 at `/usr/bin/bwrap`, which you build from the upstream release (docs/platforms.md
  §3.4.0), plus seccomp and an empty network namespace. The zone now keeps the owner's uid and gid. The cc1 fix
  (c4ab500) is qualified natively, and the install-mode kernel builds the genesis body inside the zone.
- **Memory and disk:** the macOS watchdog semantics now hold behind bwrap; one tool call is killed, not the whole
  instance. A cgroup v2 `memory.max` backstop applies per workload where the systemd unit delegates memory. The disk
  floor is unchanged.
- **Install:**
  - `get.sh` picks `init-v<version>-linux-arm64` or `-linux-x86_64`.
  - `install.sh` checks bwrap, user namespaces, python3, git and a systemd user session, then enables
    `~/.config/systemd/user/init-root.service` (skipped with `--no-service`).
  - `uninstall.sh` disables the service and moves everything to the freedesktop Trash.
- **Limits on Linux:** ADR-009 home reads, workspace writes and full mode are capped off, so the agent works in its
  own home only. Keys are 0600 files. No Landlock yet.
- **Root fixes found by the native suite:**
  - the kernel's own exit code (78 config, 75 swap) is recorded behind bwrap;
  - the probe kernel keeps fds 3 and 4;
  - unbound lane sockets no longer refuse a body;
  - Linux test portability;
  - `ci/local.sh` builds root and seed before the kernel tests, and gets a native Linux zone step.

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
