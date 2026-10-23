# WP8b: gate G2, G3, the G4 driver, the gate record and notes, and the T2 promote pipeline (lane K1)

Status: **done for the T2 path at L0, partial overall** (2026-10-20). The items under "Left" are mostly L1+ features, `judge`, and wiring into `init-kernel`.

## Done
**`kernel-gate`** (pure rules, unit-tested):
- **`g2`** covers gate.md §3.5 criteria 1–8 on `ArmObs` (probe outcome plus `ScopeSummary`):
  - `probe_timeout`, `probe_exit`, `probe_no_clean_stop`, `probe_no_round_trip`;
  - `tools_mismatch`, with a per-tool `+/-/~` schema diff;
  - `prefix_hash`, with the first differing byte of the ICJ head objects;
  - `boot_event_missing`, `boot_prefix_hash_mismatch`, `continuity` (element index and byte offset, `n/a`, `not_observed`, `waived`) and `budget`.
  - The warnings `head_bytes_changed`, `prefix_change_declared_but_unchanged` and `ttft_regression`.
  - A failed arm p is `gate.g2.parent_probe` (infra).
- **`g3`**:
  - the nonce;
  - the §3.6 instruction verbatim; the mock's G3 responder parses it;
  - verification steps 1–4 (one parent, agent identity, exactly `{self.json, src/main.rs}`, the `mod g3_<n>` and assert lines).
- **`g4`**: the run driver behind a `RunExecutor` trait.
  - `task.json` v2 loading and the workspace digest check.
  - The check score rule and the check env.
  - Scheduling: canaries first (R = 1, arm c; a 1000 stops G4), then the seeded (task, repeat) pairs with both arms run together, up to `parallel` runs at once.
  - The futility look at ⌈n/2⌉ tasks, with a `mid` hook for `gate.g4.mid`.
  - An `infra` run is re-run once; a second `infra` gives `error`.
  - Agent `task_flaky` tasks are dropped.
  - The one top-up, the `wall_s` stop (`inconclusive: timeout`), and then `accept::judge`.
  - RunRecord v2 with the agent-visible `redacted()` (holdout → `"holdout"`), and the permille `stages.G4` record.
- **`record`**:
  - the Root-lineage subset (the X9 fields plus `level`, `scope`, `escalated`, `audit_required`, `charter_sha256`, `n`, `capability`; no paths, build hash or per-task values);
  - the redacted note line;
  - the agent report (failure text ≤ 4 KiB);
  - the stage notice;
  - `append_note`/`read_notes` on `refs/notes/gate` (G-30 on a bare repo, git 2.39.5).

**`kernel-supervise::promote`** (new module) drives those rules with supervise's primitives:
- **`promote()`**: the synchronous part runs in the caller and returns `{sha}` or the error.
  - Steps: `evo/<id>` check, then `E_BUSY` (handoff busy, or one pipeline at a time), fetch into `refs/init/candidates/<job>` through `Env.upload_pack`, archive ref, `check_candidate`, `evo::validate` (dotted code plus every problem in `details[]`), scope.
  - The job thread then runs: export and build in the slot (deps cache when warm), G0 (kernel-gate runner), and G1 (`init-kernel selfmap … --out` as a `build`-profile child through the Root broker, then drift, markers and the WIT copy).
  - Next, G2: two probes in parallel on copies of the live log, each with its own proxy lane. The live R_p and R_c come from the trace store. arm p gets one retry.
  - Next, G3: a third probe gets the instruction and the agent part ends on a quiet `stop` finish. The kernel then fetches `evo/g3-<n>` from the probe's `body/` before stop, verifies it with `$G`, runs `evo::validate`, and runs the inner G0/G1/G2 with the candidate as parent.
  - Synchronous G4 for `dep`/`t0_escalated`, with the §3.12 escalation.
  - Staging `bin/init-body.staged`, then `awaiting_boundary`, then supervise's overlapped handoff.
  - Then the asynchronous G4 job on its own thread, so it can overlap the next pipeline.
- **`run_g4_job`** (candidate `bin/init-body` vs `bin/init-body.lkg`):
  - Accept: gate record to Root lineage, and only after the ack `Cmd::G4Pass` (LKG commit, `promotion{lkg:true}`) and the note.
  - Reject: `Cmd::G4Reject` (rollback at idle, `update-failed.json`).
  - Canary: `refs/init/quarantine/<sha>` and an alert.
  - Inconclusive: one escalated job (R = `g4_escalate_repeats`), then `inconclusive_final`.
  - Error: retried up to 2 times (F12).
  - Runs go to `vault/gate/jobs/<job>/runs.jsonl`, `vault/gate/results/<sha12>.jsonl`, and `noise.json` is updated.
- **`provisional_watch(age)`**:
  - G-46: resubmits the pending accept records (`vault/gate/pending/`) and commits LKG only after the ack.
  - F16: rolls back a provisional version that has had no verdict for longer than `provisional_max_age_s`.
- **`gate_quick`** (kind `quick`; G0–G2 on a probe lane; result is `GateResult`) and **`rollback`** (`E_ALREADY_LKG`, `E_BUSY`, `{from,to}`, the boundary, then supervise's rollback).
- Gate records go to `vault/gate/jobs/<job>/result.json` and `vault/lineage.jsonl` (full copy), to Root lineage as the subset (waiting for the ack), to `state/kernel/gate/<job>/report.json`, and to `refs/notes/gate` on the candidate. Stage notices go to the body.
- **Seams:**
  - `Job`: kernel-serve's `JobCtl`. The adapter is 7 one-line methods, proven in `tests/serve_seam.rs`.
  - `Jobs` (ids for internal G4 jobs).
  - `GateProxy` (kernel-proxy's `ProxyHandle`; it is not linked).
  - `Supervise` (implemented by `supervisor::Handle`).
  - `GateLineage` (implemented by `Outbox`).
  - `GateCfg` reads `[gate]`, `[budget]` and `[evolution]`.

**Other kernel-supervise edits, approved by main:**
- `probe.rs`: `ProbeScope` plus `probe_scoped()`. The lane is opened on `<root>/run/proxy.sock` before spawn and closed in `stop()` (fills `ProbeOutcome.scope`). `scope_summary()`, `exited()`, `sha()`. `probe()` is unchanged.
- `supervisor.rs`: `Cmd::Services` (broker, outbox, cfg, level), `Cmd::G4Reject` (as `G4Fail` minus its minimal record), `Cmd::Notice`.
- `Cargo.toml`: dependency `kernel-gate`; dev-dependencies `kernel-proxy`, `init-testkit` and `kernel-serve`; fixture bin `sv-agentbody`.
- `tests/fixtures/agentbody.rs`: a minimal model-driven body.
  - It speaks stdio-jsonl/1 and keeps a locked session log.
  - Probe mode: the `[init:probe]` round trip before `ready`.
  - It uses DeepSeek SSE over `INIT_PROXY_SOCK` and runs `bash`.
  - `--manifest` with `init-tools-v1`.
  - Pause, resume, exit and stop.
  - Per-candidate behaviour comes from the committed `agent-plan.json`.

## Tests (all $0: mock model only)
- `cd kernel && CARGO_TARGET_DIR=target/k1 cargo test --offline -p kernel-gate`: lib **177 passed** (20 new: g2 6, g3 3, g4 6, record 5), 1 ignored (`g21_full`); `tests/g0.rs` 12 passed.
- `cd kernel && CARGO_TARGET_DIR=target/k1 cargo test --offline -p kernel-supervise`: **all green**.
  - lib 65 (6 new in `promote::tests`: F16, the G-46 control path, rollback, argument refusals, `GateCfg`, `MemJob`);
  - `promote` 5 (about 150 s);
  - `serve_seam` 1;
  - WP4's suites unchanged (slice1 10, process 3, handoff 8, recovery 5, probe 3, install 1, fakeroot_xcheck 1);
  - `--features fault-injection --test faults`: 2.
- `tests/promote.rs` builds a real world per test:
  - real cargo builds (G0) of a fixture body, and the real `kernel-selfmap` (G1 and the agent's `--write` in G3; built into `target/k1` if missing);
  - the real kernel proxy, forwarding to **init-testkit's fake-Root egress (WP3)**, which forwards to **init-mockllm** (probe, G3 and task responders);
  - WP4's in-test fake Root on the control link;
  - the in-process supervisor doing the real handoff and LKG commit.
- What each test covers:

  | Test | Covers |
  |---|---|
  | `g31_a3_full_t2_path_to_lkg` | **G-31/A3**: promote → G0–G3 → `awaiting_boundary` → handoff (main moves) → provisional → async G4 (2 pool + 1 holdout + 1 canary, R=2) → `accept: better` → Root ack → LKG → notes `[quick, promote, g4]`. G2 `continuity: pass`, equal prefix hashes, `tools_sha256` equal to G1's, a 40-hex G3 commit, lineage before LKG (record order). **G-32**: no holdout id in the note, report or Root subset |
  | `g46_lineage_down_at_accept_then_lkg` | **G-46**: `gate.lineage.unavailable`, provisional kept, pending record; then the watch resubmits the subset and LKG follows |
  | `f1_f2_f3_f4_f9_failures_before_and_at_the_handoff` | F1 (dotted validator code with `details`, `E_NOT_DESCENDANT`, `E_ARGS`), F2 `E_BUSY`, F3 `gate.g0.clippy` (report ≤ 4 KiB), F4 `gate.g1.self_json_drift`, F9 `E_HANDOFF_FAILED` with main and the binary unchanged, `E_ALREADY_LKG` |
  | `g10_g16_probe_and_self_edit_failures` | **G-10** (crash and hang before ready), **G-11** `tools_mismatch`, **G-12** `prefix_hash` with the byte offset, **G-13** `continuity` with the element, **G-14** `boot_prefix_hash_mismatch`, **G-16** `gate.g3.no_branch`; every probe root deleted |
  | `f7_f11_f13_f14_g4_branches` | F7 (dep scope: sync G4 `score_inferior`, nothing ran), F11 (async reject → rollback, `update-failed` stage G4; with F7's noise history the futility look may stop it as `score_inferior_early`), **G-24**/F14 (canary leak → reject `canary`, quarantine ref, alert, Root record), F13 (`insufficient` → escalated re-run → `inconclusive_final` → rollback, record `escalated:true`) |

- `cargo clippy --offline -p kernel-gate --all-targets -- -D warnings` and the same for `-p kernel-supervise` (also with `--features fault-injection`): clean. The new kernel-gate files are `rustfmt --edition 2024`. `cargo check -p init-kernel`: clean.
- Test homes are `kernel/target/test-homes/w8b/<c>`. They are short, because a probe lane socket must fit in 103 bytes. Each is removed by exact path and none are left. Every process is a test child or a fake-Root child, stopped by its held handle.

## Deviations
1. **G0's commands use kernel-gate's `g0::Spawner`**, which hands back a `Child`, so they cannot go through the Root broker yet. `Env.g0` is that seam. Tests use `DirectSpawner`, unsandboxed. Builds, selfmap, probes and checks do go through the broker (`Env.spawner`).
2. **The sandboxed fetch is a seam.** `Env.upload_pack` is `init-kernel sandboxed-upload-pack` in production. Its `run/spawn.sock` relay is not built (WP4 "Left"), so tests use plain `git-upload-pack`.
3. **One record per gate job.**
   - The promote job (G0–G3 plus the handoff) records `verdict: accept, reason: g0_g3` for asynchronous scopes. Its asynchronous G4 is its own job with its own record (`gate_kind: g4`).
   - `dep` carries G4 inside the promote record.
   - `Cmd::G4Reject` exists so supervise does not post a second, minimal `gate` record.
4. **T0/T1 data-only promotions** (no handoff) end with `E_INTERNAL`, because supervise.md §3.6.8 is not built (WP4 "Left").
5. **G2 details.**
   - A probe window with no exit but fewer than two heartbeats is reported as `probe_timeout`.
   - The `prefix_hash` offset is the first differing byte of the proxy's `head_object` (ICJ), reconstructed through `GateProxy::request`.
6. **Where the tests run.** "Under init-fakeroot" applies to the money path (egress → mock). The control link is WP4's in-test fake Root with the supervisor in process, because a process-level promote trigger needs init-kernel's `Backend` wiring.
7. **The G3 mode is recorded, not routed.** In tests the mock's built-in G3 responder serves the instruction, whatever the mode. Routing `scripted` to the mock provider is proxy configuration.
8. **`cost_guard` uses `tasks_per_day = 10`** until a 30-day task-rate source exists.
9. **L0 selection only.** Selection covers pool and holdout and has no capability set A or repro tasks, and the eligible-family set is empty. kernel-gate has the rules; feeding them R4 records is L1 work.

## Needs from others
- **`init-kernel` Backend (whoever wires WP7 into init-kernel; K1 by default):**
  - `promote`/`gate`/`rollback`: wrap `JobCtl` as in `tests/serve_seam.rs` (`ServeJob`), then call `promote::{promote, gate_quick, rollback}` and return `Begun::started().with("sha"|"from"|"to", …)`;
  - build `promote::Env` from `Handle::services()`, a `GateProxy` over `ProxyHandle` (the test `TestProxy` is the reference), `Outbox` as `GateLineage`, and the `Handle` as `Supervise`;
  - call `provisional_watch` every 60 s;
  - `Jobs` from serve's `JobStore`.
- **WP6a / Root:** a sandboxed G0 command spawner, either a process handle from the `build` profile or a kernel-gate runner over the broker.
- **WP4 rest:** the `run/spawn.sock` relay for `sandboxed-upload-pack`; data-only promotions (§3.6.8).
- **Proxy (WP5):** a `quick`/`G3` budget routing hook for `gate.g3.mode = "scripted"`.

## Left
- **`gate judge`, owner records and the K stages:** `gate judge` (G-45, G-50); owner `bless`/`reject` records (G-44); K0–K3 (G-47).
- **L1+ inputs to G4:** admission jobs (G-40); feeding eligibility and the capability set from R4 records (G-41, G-42).
- **G4 extras:** the budget pre-flight (`budget_insufficient`), the disk wait, the off-peak wait, result reuse within 3,600 s, the `noise_spike`/`task_drift` alerts, and `evo::predict` with `refs/notes/predict`.
- **Recovery and supervise-side branches not tested here:** F15 (kernel restart mid-pipeline) and F10 (probation crash; supervise's own handling, not re-tested in this WP).
- **Housekeeping:** filling the deps cache from the stub workspace after a cold build, and the 30-day expiry of `vault/gate/jobs/`.
