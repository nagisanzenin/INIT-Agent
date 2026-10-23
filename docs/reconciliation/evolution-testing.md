# Reconciliation round 1: evolution-testing

```
Status: R1 · 2026-10-19 · Area: evolution
Docs owned: evolution.md, testing.md, roadmap.md, adr/005-rust-and-wasm.md, adr/006-evolution-gate-policy.md
Sources searched: the "Open questions" and "Spec deltas" sections of all 26 docs; every line elsewhere
that names evolution.md, testing.md, roadmap.md, ADR-005 or ADR-006; the four other R1 ledgers
(body-core, body-log-awareness, kernel-proxy-gate, kernel-supervise) as they stood at 21:50 +07.
```

**Totals:** 73 incoming items: **70 ACCEPT** (some modified, as noted), **1 REJECT**, **1 ESCALATE-OWNER** (CF-4 → E1), and 1 ACCEPT whose hardware part is escalated (SB-2 → E4). 6 more rows are self-raised follow-ups (RAISED), not counted. **Escalations to the owner: 4** (E1–E4); E2 and E3 come from this area's own budget and disk numbers, which the brief routes to the owner.

Two things changed under me during the round and are reflected here. supervise.md R1 adopted evolution's original ref names (`refs/init/candidates/<job>`, `refs/init/archive/<sha>`) and the `lineage/seed.git` mirror, so evolution.md keeps them; my first R1 pass had moved toward supervise's draft names and was reverted. supervise.md also decided the kernel assigns no task ids, so evolution.md now defines the task id as body-created (the id of the owner input that started the run).

## Ledger

| id | from doc § | question | decision | change made (my doc §) | follow-up that ANOTHER doc must make |
|---|---|---|---|---|---|
| SV-1 | kernel/supervise.md §9 item 4, §3.11.3 (K-F3) | the kernel must not run `git stash`/`switch` in `body/`; move it to the body's boot | ACCEPT | evolution §3.6: the resync script is body-run; it is reason-agnostic (ancestry test), branches from `main`, and only moves the ref when `body/` is not on `main` | body/boot.md §3.6: run the updated script (not the earlier HEAD-based version) |
| SV-2 | supervise §9 item 4, §3.6.7 | rollback uses the overlapped protocol, not stop-then-start | ACCEPT | evolution §3.6 trigger table and text | — |
| SV-3 | supervise §3.11.1 (its ledger EV-5) | refs: `refs/init/candidates/<job>`, `refs/init/archive/<sha>`, `refs/init/quick/<job>`, `lineage/seed.git`, `refs/upstream/<tag>` | ACCEPT | evolution §3.1, §3.5.1, §3.5.2 F1, §3.9 steps 1–3, §3.13.2, §3.15, §4.3, §7 | — |
| SV-4 | supervise §3.6.4–§3.6.5; adr/001 delta | spawn from `.staged`, rename only at commit after ready and settle | ACCEPT | evolution §3.5.1 diagram, F9 (`bin/init-body` never changes on abort) | — |
| SV-5 | supervise §3.7 | safe mode thresholds; Safe then Halted; `reload` refused in safe mode; `/clear-safe`, `/resume` | ACCEPT | evolution §3.7; testing §3.8.2 F-crash-boot (c), §3.9 A5 | — |
| SV-6 | supervise §9 item 9, §4.10 (its ledger TS-2) | testing's fault points must use the implemented names | ACCEPT | testing §3.8.1 (table of owners and names), §3.8.2, §8 | — |
| SV-7 | supervise §9 item 9, §4.9 | use the `[supervise]` key names | ACCEPT | testing §3.4.6 table, §3.6 JSON | — |
| SV-8 | supervise §3.10 item 9 (its ledger TS-1) | scriptable owner channel; owner alerts as a file | ACCEPT | testing §3.6 rules (`line` frames on `run/owner.sock`, `alerts.jsonl`), §3.9 A5 | — |
| SV-9 | supervise ledger TS-4 | S9 must use a socket outside `work/` (`$TMPDIR` is `work/tmp/`) | ACCEPT | testing §3.7 T-S2 | — |
| SV-10 | supervise ledger TS-5 | S14 is verified: `EPERM` on macOS, `ESRCH` on Linux | ACCEPT | testing §3.7 T-S3 | — |
| SV-11 | supervise §9 item 5 (its ledger PV-1) | task ids are created by the body; the kernel assigns none | ACCEPT (my first R1 pass had a kernel-assigned id; aligned) | evolution §3.1 "Task" row, §3.10.3, §10 | body/loop.md: set `Request.task` = id of the owner input that started the current run, for every request of that run |
| SV-12 | supervise ledger EV-1, §3.10 item 8 | `/accept <task_id>` recorded by the kernel | ACCEPT | evolution §3.10.3 (online pass rate from `accept` records; unaccepted = unknown), §7 | — |
| SV-13 | supervise ledger EV-2, §3.6.5 | kept T1 artifacts as immutable sets `bin/ext.d/<set>/` with symlinks | ACCEPT | evolution §3.4 diagram | — |
| SV-14 | supervise ledger EV-3, EV-4 | `state/kernel/usage.json`, `thinning.json`, `vault/usage.jsonl`; `lkg.json.model` | ACCEPT | evolution §3.11.2, §3.12 | — |
| SV-15 | supervise ledger EV-6 | REJECT of the kernel fast-forwarding `body/` | ACCEPT (the rejection) | evolution §3.6 (body-run), §9 | — |
| SV-16 | supervise ledger RM-1, §4.8 | `install --template`; there is no `cache/target`; use `cache/deps/` | ACCEPT | roadmap §3.2, §3.8; testing §3.3, §3.6, §3.11 M-1, §5 | — |
| SV-17 | supervise ledger TS-3, §4.8 | `install --with-testkit` places the mock and fake body (opt-in) | ACCEPT | testing §3.4.1, §3.4.6 | — |
| SV-18 | supervise §3.7 (disk) | `E_DISK`, `DiskFull`, GiB thresholds | ACCEPT | testing §3.8.2 F-disk-full | supervise.md / config.md: MiB-granular thresholds when `proxy.test_mode = true` (see RS-2) |
| SV-19 | supervise §3.6.1 | identities are checked by supervise before the validator | ACCEPT | evolution §3.9 step 3; `promote.identity` removed from §4.2 | — |
| SV-20 | supervise §4.3 | `update-failed.json` stage list | ACCEPT | evolution §3.5.2, §4.5 | — |
| GA-1 | kernel/gate.md Q-EV1, §3.11 | `budget.gate_usd` default $3.00 | ACCEPT (already $3.00 in my draft) | evolution §3.14 | — (kernel-proxy-gate E1 is about the day cap and share, see E1 here) |
| GA-2 | gate Q-EV1, §9 item 3 (their E2) | G4 wall 90 min instead of 60 | ACCEPT | evolution §3.14 (`gate.g4.wall_s` 5,400 s); ADR-006 alternatives row | kernel-proxy-gate ledger: E2 can close, evolution no longer proposes 60 min |
| GA-3 | gate Q-EV1 | `change_type` in `Validated` | ACCEPT (already present) | evolution §3.9 | — |
| GA-4 | gate Q-EV1 | `turns` counts only `x-init-purpose: turn` requests | ACCEPT | evolution §3.10.1 | — |
| GA-5 | gate §3.1 | scopes `t0_escalated` and `dep` with synchronous G4 | ACCEPT | evolution §3.2 (table, diagram, rules), §3.3, §3.13.3; ADR-006 decision 3 | — |
| GA-6 | gate §3.9, §3.12 | accept rule details; inconclusive keeps the version provisional; owner `bless`/`reject` | ACCEPT | evolution §3.2, §3.5.2 F7, F11–F13; ADR-006 decisions 4–5 and consequences | — |
| GA-7 | gate §3.7.4 | comparisons use `cost_ref` | ACCEPT | evolution §3.10.1 (`cost_usd` = `cost_ref`), §3.14 | — |
| GA-8 | gate Q-TS1 (their TS-8a) | G4 fixture tasks in task.json v1 format | ACCEPT | testing §3.3 | — |
| GA-9 | gate ledger TS-8b, §3.6 | G3 default stays `model`; tests set `gate.g3.mode = "scripted"` | ACCEPT | testing §3.4.4 (G3 responder), §3.4.6, §3.6 | — |
| GA-10 | gate §3.5 | G2 probes candidate and parent in parallel on the same session id | ACCEPT | evolution §3.5.1, §3.5.3; testing §3.4.4 (stateless probe responder) | — |
| GA-11 | gate §3.8.5 | default sizing (K 12, H 8, R 2) and its resolution (≈ 17–20 points at n = 16) | ACCEPT | evolution §3.14 rationale; ADR-006 consequences; roadmap R11 | — |
| GA-12 | gate §4.1 | gate fault-point names | ACCEPT | testing §3.8.1 | — |
| PX-1 | kernel/proxy.md Q-E1 | `Repro: trace:` takes a 16-hex `req_id` | ACCEPT | evolution §3.8 ABNF, §3.9 step 10 | — |
| PX-2 | proxy §3.4; adr/002 alternative G; kernel-proxy-gate EV-1b; body-core EV-1 | drop the `X-Init-Gate` header proposal; gate traffic is attributed by lane socket | ACCEPT | evolution §9 (proposal removed) | — |
| PX-3 | proxy Q-P5 and open Q2; provider Q-P7; kernel-proxy-gate PV-2 | how the body learns the task id for `x-init-task` | ACCEPT (defined here) | evolution §3.1 | body/loop.md (as SV-11) |
| PX-4 | proxy §3.3; kernel-proxy-gate RM-1 | the DeepSeek key goes in through `init-kernel keys set deepseek`, never `config.toml` | ACCEPT | roadmap §3.9; testing §3.10 | — |
| PX-5 | proxy §4.1; kernel-proxy-gate TS-2 | loopback test upstream only with `proxy.test_mode = true` | ACCEPT | testing §3.4.1, §3.4.6 | — |
| PX-6 | proxy §3.10.2 | trusted tool-call records are the usage source | ACCEPT | evolution §3.11.1 | — |
| CF-1 | kernel/config.md Q-C1 | timing keys only in `[supervise]` | ACCEPT | evolution §4.6 (removed from `[evolution]`) | kernel/config.md §4.7: drop the four duplicated keys |
| CF-2 | config Q-C2 | money keys are strings | ACCEPT | evolution §3.14 table, §4.6 | — |
| CF-3 | config Q-C4 | `proxy.test_mode`; `supervise.ready_timeout_s` | ACCEPT | testing §3.4.6, §3.6 | — |
| CF-4 | config §4.3 defaults vs evolution's day cap $10 and offset +07:00 | which day cap, day boundary and Phase 0 total | **ESCALATE-OWNER (E1)** | evolution §3.14 now quotes config.md's defaults; roadmap §3.9 carries the recommended Phase 0 overrides | — |
| IC-1 | kernel/initctl.md §9 item 1 (evolution Q8) | `wait`, `error.details[]`, dotted codes; `wait` returns at `awaiting_boundary` | ACCEPT | evolution §4.1 (no "wait until provisional": it would deadlock); testing §3.4.4 (`promote --wait`) | — |
| IC-2 | initctl §9 item 2 | `E_PROVISIONAL` vs `state.provisional` | ACCEPT (modified: all kernel-state refusals use `E_*`; evolution keeps only validator dotted codes; count quotas use `E_QUOTA`) | evolution §3.5.2, §3.12 step 0, §3.14, §4.2 | kernel/initctl.md: document `E_QUOTA.data.quota` (`promote_attempts_task`, `promotes_day`, `quick_gates_day`) and `E_BUSY.data.reason = "thinning_baseline"`; drop the note that `state.provisional` stays an evolution code |
| IC-3 | initctl §3.7 | `serve.promote.after_fetch` placement | ACCEPT | testing §3.8.1 | — |
| SM-1 | kernel/selfmap.md Q-E1 | component kind `prompt/<file>/<slug>`; `compensates_for` is one string; `hook/*` empty | ACCEPT | evolution §3.1 table, §3.8 ABNF, §3.11.1, §3.12 note | — |
| SM-2 | selfmap §3.8 check 3 | the trailer/marker cross-check runs in G1 | ACCEPT | evolution §3.9 step 10, §4.2 | — |
| WT-1 | ext/wit.md §9.2 | hooks deferred; mark the hook example hypothetical | ACCEPT | evolution §3.1, §3.11.1 | — |
| WT-2 | ext/wit.md §9.2 (testing Q6) | A2-pty → A2-repl over `init:ext/process` | ACCEPT | testing §3.9 A2, §10 | — |
| HO-1 | ext/host.md §9.2 | `wasmtime`/`wasmtime-wasi` =48.0.6 (LTS), not 49.0.3 | ACCEPT | testing §3.11 M-1; roadmap §3.2 M0, §11; ADR-005 decision and references | — |
| HO-2 | ext/host.md §9.2 | add H0, H1, H23 to L1 | ACCEPT | testing §3.2 | — |
| HO-3 | ext/host.md §9.2 | A2 event order per host §3.9 rule 3 | ACCEPT (already matched) | testing §3.9 A2 | — |
| HO-4 | ext/host.md, evolution Q3 | kept artifacts are kernel-published | ACCEPT | evolution §3.4 | — |
| AU-1 | ext/authoring.md §9.2 → testing | A2 precondition, build cwd `body/ext/<name>`, M-2 per §3.6, M-3 and the lane per §3.10 (C1–C3, JavaScript inside Wasm) | ACCEPT | testing §3.9 A2, §3.11 M-2, M-3; roadmap R1; ADR-005 | — |
| AU-2 | ext/authoring.md §9.2 → evolution | keep flow commits `ext/<name>/` and the lockfile entry, never the `.wasm` | ACCEPT | evolution §3.4 | — |
| AU-3 | ext/authoring.md E10 | a lockfile diff touching only source-less entries is `t1`, not `dep` | ACCEPT (evolution depends on it) | evolution §3.4 note; roadmap R12 | kernel/gate.md §3.1: adopt the rule (also raised by ext/authoring.md) |
| SB-1 | kernel/sandbox.md §9 item 1 | adopt the S-* ids and `doctor --sandbox-selftest` as the A6 sandbox half | ACCEPT | testing §3.7 (own list replaced), §3.9 A6, §7, §8 | — |
| SB-2 | sandbox §9 item 1 | CI needs macOS 14/15/26 runners and a Linux ≥ 7.1 runner for S-L8 | ACCEPT for the plan; hardware **ESCALATE-OWNER (E4)** | testing §3.12 plan; roadmap §3.9, R9 | — |
| BT-1 | body/boot.md Q-B2, §10 item 1; body-core TS-1 | only probe-mode boots make the model round trip | ACCEPT (current boot.md behaviour); the owner decision is already body-core E1, not re-escalated | testing §3.4.4, §3.10 E2E-1, §10 | — |
| BT-2 | body/boot.md §4.1 | `INIT_TEST_PROBE_WATCHDOG_MS` | ACCEPT | testing §3.4.6 | — |
| BT-3 | body/boot.md §3.4; provider model table | mock model id `mock-frontier-1` | ACCEPT | testing §3.4.6, §3.6 | — |
| BT-4 | body-core SV-2b | say the script runs in the body's boot | ACCEPT | evolution §3.6 | — |
| BT-5 | body-core TS-6 | E2E-1 if boots do not probe | ACCEPT | testing §3.10 E2E-1 (via `gate --quick`) | — |
| TL-1 | body-core TS-5; body/tools.md delta 1 | bash truncation is head 4 KB + tail 26 KB | ACCEPT | testing §3.2 | — |
| LC-1 | body/log-context.md X6 | `GENESIS.md` is T0 data | ACCEPT | evolution §3.1 T0 paths, §3.8 `skill` paths; ADR-005 | — |
| LC-2 | body/log-context.md X6 | add `state/blobs/` to the runtime layout | REJECT (wrong owner: not an evolution contract) | — | CONVENTIONS §3 (main session) |
| LC-3 | log-context X10; body-log-awareness TS-1 | torn tail: `boot.log_repair` and `error{code:"corrupt_line"}` | ACCEPT | testing §3.8.2 F-corrupt-tail | — |
| LC-4 | body-log-awareness EV-1–EV-3 | event item objects, stage values, permille | ACCEPT (done on their side) | evolution §4.5 | — |
| LC-5 | log-context D2 | tool uses live in the assistant `message`; `tool_call` is an execution-start marker | ACCEPT | testing §3.9 A1 expected events | — |
| PV-1 | body/provider.md Q-P4 | harness additions accepted | ACCEPT (no change needed) | — | — |
| LP-1 | body/loop.md §8 item 21 | M-5 adopted | ACCEPT (no change needed) | — | — |
| AW-1 | awareness.md boot-block lines | they render evolution's objects | ACCEPT (no change needed) | — | — |
| RM-2 | body/*, ext/* test plans (roadmap Q2) | component test plans name testing.md ids | ACCEPT (done on their side) | roadmap §8, §9 | — |
| RS-1 | self-raised | when the kernel syncs the seed mirror; install from a tag | RAISED | evolution §3.13.2 | kernel/supervise.md: `init-kernel upstream sync` (§4.8); §3.11.4 install imports a `seed-v*` tag from `lineage/seed.git` (the published branch is `refs/heads/seed`, not `main`) |
| RS-2 | self-raised | F-disk-full cannot observe recovery with GiB thresholds on a 256 MiB volume | RAISED | testing §3.8.2 | kernel/supervise.md, kernel/config.md: MiB-granular disk thresholds in test mode |
| RS-3 | self-raised | parallel G2 probes share one mock cursor | RAISED | testing §3.4.4 (stateless built-in responders) | body/provider.md §3.12: list the probe, G3 and per-task responders as accepted harness behaviour |
| RS-4 | self-raised | G4 verdicts on mock data flip with `wall_ms` jitter | RAISED | testing §3.6 (`verdict_consistent`), §5 | kernel/gate.md (optional): a test-mode switch excluding `wall_ms` from `cost_x_wall` |
| RS-5 | self-raised | the R1 contracts enlarge the kernel | RAISED | roadmap §3.2–§3.6: ≈ 150 agent-hours, critical path ≈ 51 AH, ≈ 5 calendar days | — |
| RS-6 | self-raised | `GENESIS.md` changes count as T0 for the `skill` type | RAISED | evolution §3.8 | — |

## Escalations to the owner

**E1. Phase 0 money: day cap, day boundary, hard total** (joins kernel-proxy-gate E1, which covers `gate_usd`, `day_usd` and `evolution_share`).
- **Recommended:** keep config.md's shipped defaults (`day_usd = "5.00"`, `day_starts_at_utc_offset = "+00:00"`). For Phase 0, set `day_usd = "10.00"` on real-model test days, `day_starts_at_utc_offset = "+07:00"`, `task_usd = "0.50"`, `gate_usd = "3.00"`. As the hard total, top up the DeepSeek account with $25: its balance is prepaid (UNVERIFIED), and the proxy has day caps but no lifetime cap.
- **Trade-off:** $5/day fits about one full G4 per day on `deepseek-flash` plus small tasks, so A3-real, M-3 and a G4 cannot share a day. $10/day doubles the worst-case daily exposure, but expected Phase 0 spend is only ≈ $2–7 (testing.md §3.10).

**E2. Which model runs G4, and the gate cap that follows.**
- **Recommended:** run G4 (and G3 in `model` mode) on `deepseek-flash` with `gate_usd = "3.00"`; use `deepseek-v4-pro` for M-3 and the A-real runs.
- **Alternative:** G4 on `deepseek-v4-pro`, with `gate_usd` raised to ≈ $13.
- **Trade-off:** a v4-pro G4 costs about 4× more (≈ $3.3–12.7 against $1.3–2.0). Flash is the weaker model, so the gate measures a body that is not driven by the strongest available model (roadmap R3). Comparisons are only ever within one model, so the choice does not bias decisions; it only changes what they mean.

**E3. Free disk at M0.**
- **Recommended:** free disk to ≥ 35 GB before M0 (20 GB floor plus ≈ 15 GB typical build peak; roadmap §3.8.2).
- **Alternative:** start with 30 GB. Use the 3-lane plan, and run the measurements and the disk-full test only when no scenario homes exist.
- **Trade-off:** the alternative adds about 2 calendar days and leaves less margin if M-1 measures larger target dirs than estimated. Today the machine has 8.0 GiB free (21:54 +07).

**E4. CI hardware for the sandbox suite.**
- **Recommended:**
  - Run `doctor --sandbox-selftest` on hosted `macos-15` and `macos-26` (plus `macos-14` if private repos still get it) only on release tags and manual dispatch: ≈ $7/month at four runs.
  - Run S-L8 (Landlock ABI ≥ 9) once at M7 on a short-lived Linux ≥ 7.1 VM, or defer it until hosted runner kernels reach 7.1.
  - Add a self-hosted runner on the M2 only once disk allows.
- **Trade-off:** until S-L8 runs, the Linux pathname-socket restriction is unverified. Linux is the second platform, and bwrap's mount namespace is the primary layer there (kernel/sandbox.md §10).

## Requests to the main session (CONVENTIONS.md)

1. **§2 repository layout:** add `kernel/crates/init-testkit` (`publish = false`; bins `init-mockllm`, `init-fakebody`, `init-measure`), `ci/local.sh`, `.github/workflows/ci.yml` and `measurements/`. kernel-supervise request 17 asks for the same crate.
2. **§4 registry, evolution.md row:** "Tiers flow, task identity, commit grammar (typed subject; trailers `Why`, `Predicts`, `Compensates-for`, `Prefix-change`, `Repro`, `Retires`, `Thins`, `Upstream`, `Reverts`), validator codes, predictions and `refs/notes/predict` content, retirement, thinning, upstream (`lineage/seed.git` sync policy), body resync procedure".
3. **§4 registry, testing.md row:** "Acceptance tests A1–A6, CI, test harness (`init-testkit`, mock harness and responders; the mock script format is body/provider.md §3.12), the `INIT_FAULT` mechanism (points are placed by their owning docs), Phase 0 measurement procedures".
4. **§3 runtime layout** (supervise.md §3.1 already lists them): `lineage/seed.git`, `state/kernel/usage.json`, `state/kernel/thinning.json`, `vault/usage.jsonl`, `bin/ext.d/<set>/` with the `bin/ext*` symlinks, `kernel/bin/init-mockllm` and `init-fakebody` (with `--with-testkit`).
5. **§3 runtime layout:** `state/blobs/` (body/log-context.md X6, forwarded; not an evolution item).
6. **§2/§3:** `body/GENESIS.md` as T0 data (awareness.md X7). evolution's T0 paths and gate.md's `t0` scope rely on it.
7. **§5 global decisions:** pin `wasmtime`/`wasmtime-wasi` `=48.0.6` once (body-core request 6 asks the same).
8. **§5 Disk:** keep "≥ 20 GB free during the build"; once the owner decides E3, add the M0 starting requirement.

## R2

Applied 2026-10-19 against CONVENTIONS v0.2 and all six R1 ledgers (body-core, body-log-awareness, ext, kernel-proxy-gate, kernel-supervise, this one).

### Follow-ups applied (id → my doc §)

Every follow-up addressed to evolution.md, testing.md, roadmap.md, adr/005 or adr/006 in the other ledgers was checked. Most were already applied in R1. The table lists each one with where it now lives.

| Ledger id | Follow-up | Where |
|---|---|---|
| body-core SV-2b; kernel-supervise EV-6 | the resync script runs in the body's boot | evolution §3.6 (done in R1) |
| body-core GA-3; body-log-awareness BT-1; body-core TS-1 | one `probe` turn pair per probe-mode boot; scripts follow the E1 decision (settled: probe-mode only) | testing §3.4.4: the stateless probe responder serves exactly one pair per probe-mode boot; the live script has no `probe` turns; "settled" wording |
| body-core TS-6 | E2E-1 checks a probe-mode boot | testing §3.10 E2E-1 (cold boot makes no model call; round trip via `gate --quick`) |
| body-core PX-4; kernel-proxy-gate PV-2; kernel-supervise PV-1 | define how the body learns the task id | evolution §3.1: the `input` frame's `id` (supervise.md §3.10, §4.5) |
| body-core EV-1; kernel-proxy-gate EV-1b | drop the `X-Init-Gate` header | evolution §9 (done in R1) |
| body-core TS-5 | bash head 4 KB + tail 26 KB | testing §3.2 (done in R1) |
| body-core SR-2; ext E15 | wasmtime `=48.0.6` | testing §3.11 M-1, roadmap M0, adr/005 (done in R1; adr/005 now also cites CONVENTIONS §5) |
| ext E14 | A2-repl, not A2-pty | testing §3.9 A2 (done in R1) |
| ext E16 | cite authoring §3.10 C3 for the lane decision | testing §3.11 M-3 (done in R1) |
| kernel-proxy-gate EV-5a | `change_type` in `Validated` | evolution §3.9 (present) |
| kernel-proxy-gate EV-11 | kind `prompt/<file>/<slug>` | evolution §3.1, §3.8 (done in R1) |
| kernel-proxy-gate EV-46; kernel-supervise CF-1 | drop duplicated keys from `[evolution]` | evolution §4.6 (done in R1) |
| kernel-proxy-gate TS-2, TS-8b, TS-K | `proxy.test_mode`; `gate.g3.mode = "scripted"`; `supervise.ready_timeout_s` | testing §3.4.6, §3.6 (done in R1) |
| kernel-proxy-gate RM-1 | key via `init-kernel keys set` | roadmap §3.9.2; testing §3.10 (done in R1) |
| kernel-supervise EV-1 | online pass rate reads the `accept` records | evolution §3.10.3 (done in R1) |
| kernel-supervise EV-5 | `refs/init/candidates/<job>` keyed by job id | evolution §3.1, §3.9, §4.3 (done in R1) |
| kernel-supervise RM-1 | refer to `cache/deps/` | roadmap §3.8, testing §3.3, §3.6 (done in R1) |
| kernel-supervise TS-2, TS-4, TS-5 | implemented fault names; S9 socket outside `work/`; S14 `EPERM`/`ESRCH` | testing §3.8.1, §3.7 T-S2, T-S3 (done in R1) |

**CONVENTIONS v0.2 alignment.**
- **Layout §2.** Extensions are a **separate** workspace `seed/ext/` with its own `ext/Cargo.lock`. My R1 text followed ext/authoring's R1 "members of the body workspace", which the ext ledger E8 later rejected. Fixed in evolution §3.4, testing §3.9 A2/A2b, roadmap M4 and WP14, and adr/005.
- **wasmtime `=48.0.6`.** Done everywhere; "49.0.3" appears only as "newest release".
- **ICJ and money.** The `refs/notes/predict` note and `boot.json.evolution` objects use integers only. The `cost_usd` clause carries `unit:"musd"` values (evolution §3.10.1). Config money stays as decimal strings (evolution §3.14).
- **Env vars and exit codes.** testing §3.8.2 adds F-crash-boot (d): exit 78 on two consecutive boots → `Halted{config}`, not a crash. The fake body gains `exit_before_ready:<n>`.
- **Error codes.** `E_*` for kernel refusals and dotted codes only for the validator (done in R1). The vendor row now cites the 20/day `E_QUOTA`.

**Owner decisions.**
- **Settled:** probe-mode-only boots, the G4 limit of 90 minutes, and ≥ 35 GB free at M0. My E3 is closed; testing, roadmap and evolution say "settled".
- **Pending, marked "PENDING OWNER" with provisional defaults:**
  - money caps: CONVENTIONS §6 E1 proxy/gate + ET-E1 (evolution §3.14, testing §3.10, roadmap §3.9.1, adr/006);
  - G4 model: ET-E2 (evolution §3.14, testing §3.10, roadmap §3.9.1, adr/006);
  - CI hardware: ET-E4 (testing §3.12, roadmap §3.9.1);
  - vendor approval: ESC-1 (evolution §4.1, roadmap §3.9.1);
  - `/opt/homebrew`: ESC-2 (testing §3.7, roadmap §3.9.1);
  - constitution and genesis: OW-1/OW-2 (roadmap M3, §3.9.1).
- roadmap §3.9.1 lists all of them as M0 inputs.

### Could not apply, and why

- **CONVENTIONS §6 ids for my decisions.** My E1, E2 and E4 are renamed **ET-E1, ET-E2, ET-E4**, because §6's "E1/E2" ids belong to kernel-proxy-gate. ET-E2 (G4 model) and ET-E4 (CI hardware) are not yet rows of CONVENTIONS §6, so my docs cite them as "evolution-testing ET-E2/ET-E4". **Request to the main session:** add them to §6, and add ET-E1's Phase 0 parts (test-day `day_usd = "10.00"`, `day_starts_at_utc_offset = "+07:00"`, a $25 prepaid top-up as the hard total) to the E1 row.
- **A2b's expected scope** cannot be fixed yet. gate.md §3.1 still classifies any `ext/Cargo.lock` change as `dep`. The scenario accepts either `t1` or `dep` and asserts which one ran, until gate.md adopts ext E10.

### Remaining open questions (all cross-doc; no owner decision)

1. **kernel/supervise.md:**
   - §3.11.4 install imports the seed from `refs/heads/main`; it should import a `seed-v*` tag from `lineage/seed.git`.
   - Add `init-kernel upstream sync` (§4.8).
   - Support MiB-granular disk thresholds in test mode (also kernel/config.md).
2. **kernel/gate.md:**
   - Classify a source-less-only `Cargo.lock` / `ext/Cargo.lock` diff as `t1` (ext E10).
   - Optional: a test-mode switch that excludes `wall_ms` from `cost_x_wall`.
3. **kernel/initctl.md:**
   - Document `E_QUOTA.data.quota` and `E_BUSY.data.reason = "thinning_baseline"`.
   - Drop the "`state.provisional` stays evolution's code" remark.
4. **body/loop.md:** set `Request.task` from the `input` frame's `id`.
5. **body/boot.md:** adopt evolution §3.6's current resync script. Its §3.6 table still describes the HEAD-based R1 draft.
6. **body/provider.md:** list the built-in probe, G3 and per-task mock responders as accepted harness behaviour.
