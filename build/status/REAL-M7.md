# REAL-M7: runbook, harness, $0 dry run and the lean M7 proof lines

Status: **done, verdict NOT ACCEPTED** (2026-10-21 15:48 +07). Program **$2.67 by balance** ($9.56 → $6.89) of the owner's $3.00; $3.06 conservative priced.
- **Lines:**
  - P and C pass (FA 48‰, FR 86‰, m 114‰);
  - D: proposal p-000001 / v1 `c88be5f` (est 120‰ → go), G0–G3 accept. Its single G4 v2 at standard: **`sealed_g4` inferior (n 29 × 3, d −50‰, m 120‰)**, gate reject `score_inferior`, rollback to LKG;
  - M: skip path, pass;
  - G1L: not run (D not accepted).
- **Deviations, in `measurements/m7-results.md`:**
  - DV-1…DV-3;
  - DV-4: disk stop. Root cause: swap from one runaway sandboxed python3.9;
  - DV-5, DV-5b, DV-5c: the same sha re-gated (rollback, proposal reopened twice, G3 stage timeouts 1800 s);
  - DV-6: watchdog memory cap 4096 MB;
  - DV-7: standalone balance-floor guard, and the test-home sealed ceiling raised.
- **Findings, also in RELEASE_PLAN:**
  - MEM-RUNAWAY: Seatbelt has no memory limit;
  - STRANDED-PROPOSAL;
  - the probe-root leak;
  - the kernel's G3 stage timeouts (300 s) vs a cold build slot.
- **New harness:**
  - `tests/real/m7watch.py`: a disk and memory watchdog, 5 s polls;
  - `tests/real/m7guard.py`: a balance-floor money guard;
  - `checks_m7.py`: `dv5-reopen`, the `g4retry` phase, the balance floor, `[gate.g3]` timeouts, and a promote-gate reject now ends the G4 wait. Selftest 92/92.
- **Test home:** `target/m7-homes/r` (about 1 GB) is kept for the orchestrator. Its Root is stopped and its key removed.

## Integration bugs found by the dry run (each fixed at the smallest scope, with a test)

| # | Bug | Where | Fix (commit) | Test |
|---|---|---|---|---|
| 1 | **Nothing ran the incumbent over the visible pool before a G4.** Without it the miner (EV-MINE) has no failing pool runs and E3 has no cached incumbent before D; EVO-LITE's status expected line C to provide it. | kernel | `initctl evo calibrate [--repeats R] [--limit N]` = `evo.screen` with `pool_repeats`: the LKG alone on the pool, every run into the pool cache, failing logs retained, then a mine (`087b5e5`) | `screen::tests::pool_calibration_fills_the_cache`, `cli::tests::evo_calibrate_line` |
| 2 | **Root sealed runs never booted the real body.** It exited 78, `INIT_HOME: missing`: the runner passed only PATH, HOME, TMPDIR and INIT_PROXY_SOCK. So no probe, calibration, slice, milestone or audit of a real body had ever made a request; the Root's tests use fixture bodies. | init-root `sealed/run.rs` | `body_home()`: the run root laid out like a kernel probe root. T0 comes from the lkg tuple's kernel-exported tree `INIT_HOME/lineage/trees/<sha12>/`, never the agent-writable `body/`. It also gets `kernel/CONSTITUTION.md` and `config.toml`, `boot.json` with the run's identity, supervise.md §4.1's env, and no kernel socket (`1ec0975`) | `sealed::run::tests::body_home_is_a_bootable_probe_like_root` |
| 3 | The sealed task's input frame had no `id` or `seq`, so the body warned `input: missing id` and idled until the wall timeout | init-root `sealed/run.rs` | `input_frame()` (`1ec0975`); the outcome now records `exit_code` | `sealed::run::tests::input_frame_has_id_and_seq` |
| 4 | init-mockllm never detected a task session of a real body. A cold boot sends `[init:boot]` as the first user message and the `[task <id>]` line in the next, so every G4, screen and sealed task session fell through to the owner script (script exhausted, 500) | init-testkit | `detect_task` skips the body's `[init:…]` notices (`3b3cf7c`) | `mockllm::responders::tests::detect_task_skips_body_notices` |
| 5 | The mock could not script the Root's health canary, because Root instance ids are random, so every mock calibration stopped `harness_health` | init-testkit | `<tasks_dir>/_default.mock.jsonl` serves unscripted task sessions (`195ef9d`). m7.sh's default writes `ok.txt` = `OK`: health passes on both arms, while every real task and the other canaries fail | `mockllm::tests::default_task_script_serves_unscripted_tasks` |
| 6 | The kernel socket refuses peers outside the zone (`E_PEER`), so the harness cannot run `initctl` itself | harness | the live agent starts the fixed command (as S-REAL's promote checks did); the harness waits on the kernel's job record and reads the result from it, never from the model's reply | dry P, dry C |
| 7 | bash caps its `timeout` at 3600 s, and C's pool calibration may exceed an hour | harness | `evo calibrate --no-wait` + wait on the job record | dry C |
| 8 | A freeze to exactly $0.00 is refused by charter rule C5 (`day_usd` > 0) | harness | the zone freezes at spent + $0.01, below the $0.0786 reservation of any request | `freeze-test` (live), selftest |
| 10 | init-mockllm's G3 responder ran selfmap with the bash tool's 120 s default; a cold body build outlives it, so self.json stayed unchanged and G3 rejected the commit (`gate.g3.shape`) | init-testkit | a 1800 s bash timeout on the selfmap step (`b3fe379`) | `mockllm::responders::tests::g3_walks_the_five_steps` |
| 11 | Harness: the milestone's same-tuple skip (nothing promoted) counted as a failure; a line refused by the ledger still started the Root and wrote a verdict; the canned dry-run proposal lacked `subject`, `compensates_for` and `prefix_change`, and used a non-grammar subject type; home removal failed on the read-only pool mirror and pack; a turn resumed from an interrupted line consumed the mock's script | harness | skip recorded, and it passes with a clean run-down; refusal before start (`params`, rc 5); full canned schema; `chmod -R u+w` before exact-path removal; `m7.sh mock settle` (waits for a fresh boot) | dry M, dry G1L, dry D, selftest |
| 12 | Guard: the first in-process balance read of real P failed (back to back with the start read) | harness | retry the start balance 3×, adopt a late first read, and no immediate second read | selftest (runner thread) |
| 9 | The harness's own null-commit message had a `§` in its `Why:` trailer (the validator wants printable ASCII); a refused promote also made the driver wait hours for a boot and a G4 | harness | ASCII message; the driver reads the promote job and stops (`promote_refused`) when it failed | selftest (ASCII check), dry G1D |

Not mine and not fixed: `cargo clippy -p init-root -- -D warnings` reports pre-existing errors in `sealed/pack.rs:302`, `sealed/slice.rs:283` and `sys.rs:423,432`. The code added here is clippy-clean.

## Dry run (`tests/real/m7.sh mock …`, real pack s1, init-mockllm, all at $0): **green**
- **P:** the draw is clean; every probe group made requests; `evo calibrate` ran 2 pool runs; Root/real ratio 1.999.
- **C:** complete 32 × 6, FA 0‰, FR 0‰, m 16‰, n_kept 32, `selected:false` (no task is in band on a mock); pool 40 runs on 20 tasks, failing logs retained.
- **G1D:** the null change went through the validator, G0–G3, the handoff (`boot{reason:promote}`) and G4 v2. Slice: lean, n 24, R 2, status `tie`, **d = 0‰ exactly**, m 20‰, cpp 1000‰. Verdict: reject `no_gain`, no accept class. Minimum free disk 6.2 GB.
- **D:**
  - D-0 frozen (sha256 recorded); the miner found 12 clusters, 3 eligible, all `task.fail:<label>` with attested `run:` refs; the mock forced the first, since none spans 2 families.
  - The canned proposal `p-000001` was accepted; variant k 1, s-measurable; `evo screen` ran E0 (build + G0) and E3 on C's pool cache.
  - est = **26‰**, matching D-0's formula (s_c 392‰, 0/4 fixed, T 0.8). The kernel set `abandoned:below_resolution`, so there is no chosen variant: the honest no-go path. The promote→G4 path is G1D's.
- **M:** the Root skipped the milestone (current = genesis, nothing to check) and spent no reserve; the run-down was clean. The milestone's measurement path needs a second tuple and was not exercised; it shares `run_task` with the probe, calibration and slice, and ROOT-S1a's M7-R17 covers its statistics.
- **G1L:** refused before start (D not accepted).
- **Disk:** the dry run's G0 and G3 cold builds peak at about 2.9 GB. The build slot is removed after every line. Two stops under 5.3 GB (my own runs) were handled as instructed.
- **Observation (not fixed):** `INIT_HOME/probe/` kept about 130 probe roots of ~2 MB each after the dry lines (kernel probe cleanup); worth a look later.
- **Residual risk for real D:** the kernel's G3 instruction does not tell the probe agent to use a long bash timeout for selfmap. S-REAL's real-model G3 passed (761 s), but a model that keeps the 120 s default would fail G3 `shape` on a cold build.

## Real lines
- **P (done, pre-authorized):** $0.0356 real (priced) / $0.0711 Root-metered (ratio 1.999); balance $9.56 → $9.53; 10 runs, 7 passes.
  - Per type, real $ per run: C 0.0017, E.py 0.0049, E.rust 0.0042, T 0.0037, pool 0.0033.
  - Plan **trim** (mean $0.00356, just above the full band's $0.00347); `cost_prior_usd` $0.0049; D tier **standard**.
  - Details: `measurements/m7-probe.md`.
- **Expected next (real $):** C $0.83 (cap $0.93); D $0.78 + R $0.09 (line cap $1.09); M $0.30 (cap $0.36); G1L $0.48 only if D counts. Program: $2.03 if D is not accepted, $2.51 if it is (≤ $3.00).
- **C, D, M, G1L wait for the orchestrator's go**, run one at a time (`M7_I_MEAN_IT=1 M7_GO=<line> INIT_SLOT_POOL=real bash ci/slot.sh bash tests/real/m7.sh real run <line>`). Off-peak windows: until 08:00 +07, then 11:00–13:00 and after 17:00.

## Readiness and cost per real line (real $, off-peak; `checks_m7.py`)

High is the line's cap: the most the guard lets it spend. Caps are rescaled by P's measured $/run before C.

| Line | Runs | Low ($0.002/run) | Expected ($0.003/run) | High (cap) |
|---|---|---|---|---|
| P cost probe | 8 probe + 2 pool (+ 2 agent turns) | $0.02 | $0.03 | $0.06 |
| C calibration | S 32 × 6 + pool 20 × 2 | $0.46 | $0.70 | $0.84 |
| R inside D | worker + E3 | $0.07 | $0.08 | $0.15 |
| D G0–G3 + G4 v2, standard, both arms fresh | 211 | $0.45 (strong if it fits: $0.71) | $0.66 | $0.80 |
| M milestone + run-down | 123 | $0.25 | $0.37 | $0.45 |
| G1L (only if D counted) | 127 | $0.28 | $0.41 | $0.50 |
| **Program, D not accepted** | | **$1.25** | **$1.84** | $2.30 |
| **Program, D accepted** | | **$1.53** | **$2.26** | **$2.80** |

Plan bands (P's mean $/run): full ≤ $0.0035; trim ≤ $0.0037; t5 (lean D, go at ≥ 250‰) ≤ $0.0044; above that, stop and re-probe.

## Decisions and deviations
- **G1 move (approved):** G1D is a $0 exact symmetry check before any real D; G1L is the lean real A/A, only after D counted.
- **Runbook §9:** D-R1…D-R11. The new ones:
  - D-R9: C's pool half is `evo calibrate`.
  - D-R10: C is 32 × 6 only (the lean Root has no replacements or template probes).
  - D-R11: Predicts is always +15%, because the kernel writes the variant's commit from the proposal before est exists.
- **D-0** carries the orchestrator's two rules: item 10 (only `better` or `lineage_gain` count; `tie_break:<x>` is reported only) and item 8 (T2 only, else stop before G4).
- **Charter ceilings per line and pool:** share × cap × Root/real factor + 4 reservations; Root sealed ops and kernel gates get +4 × the $0.10 task cap for per-run reservations; the kernel's `gate_usd` covers the Root's slice estimate ((5 + 2nR) × $0.005).
- `gate.g4.cost_prior_usd` is set from P (the maximum of the per-type mean and the overall mean, real $) before C and D, as the orchestrator asked. `gate.g4.tier` is pinned per line.

## Tests
- `bash tests/real/m7.sh selftest`: 81/81.
- `bash tests/real/m7.sh freeze-test`: pass, live.
- Lane tests:
  - `cargo test -p kernel-gate --lib -- screen::` (7);
  - `-p initctl -p init-proto`;
  - `-p init-root --bin init-root -- sealed::run` (4);
  - `-p init-testkit --lib -- mockllm` (54).
- All are clippy-clean in the changed code.

## What is left
- G1D, D, M and G1L dry runs, then real P, then stop and report P's numbers to the orchestrator.
- C, D and M wait for the orchestrator's go.
