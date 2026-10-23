# M7 results: the lean proof lines on real deepseek-flash (REAL-M7)

```
Status: final · 2026-10-21 03:37–15:48 +07 (off-peak until 13:00, then peak by the owner's decision) · Area: evolution (lane REAL-M7)
Protocol: measurements/m7-runbook.md (D-0 sha256 687e77f0cfe9be21…, frozen before mining) · Probe: measurements/m7-probe.md
Artifacts: build/real-artifacts/m7/real/{P,C,D_try1,D_try2,D_screen1,D_screen2,D_promote1,D_g4retry,D_g4retry2,D_g4retry3,M} (redacted, 0 leaks)
```

## Verdict

**NOT ACCEPTED: the engine's pre-registered T2 change was rejected by D-0's single G4. It scored worse on sealed S, so nothing sticks.**
- The mined friction f-000003 (`cap.hit:turns`, s_c 600‰) produced proposal p-000001.
- Its chosen variant v1 `c88be5f` (`src/tools/bash.rs`: a cwd hint when a leading `cd` fails) passed the pre-registered go rule: est **120‰** ≥ 80‰, aim `lineage_gain`. It also passed **G0–G3** and the handoff.
- **G4 v2 at standard, both S arms fresh:**
  - `sealed_g4` status **inferior**: n 29 × R 3, **d = −50‰** (the candidate is 5 points worse), m = 120‰, cost per pass 1.05×;
  - gate **reject `score_inferior`**, then a body rollback to the LKG.
- The same pipeline produced an honest negative. The screen's est 120‰ came from 1 fixed of 2 targeted pool tasks and 1 lost of 2 controls; S did not confirm it.
- **M:** the skip path (current is the confirmed tuple), $0.0002.
- **G1L:** not run (D was not accepted).
- Calibration **C passed** (FA 48‰, FR 86‰, m(standard) 114‰), so the instrument that rejected the change is the calibrated one.

## Lines

| Line | Result | Key numbers | Real $ (priced) | Root $ | Balance |
|---|---|---|---|---|---|
| G1D (mock, $0) | **pass** | G4 v2 slice: lean, n 24, R 2, status `tie`, **d = 0‰ exactly**, m 20‰, cpp 1000‰; verdict reject `no_gain`, no accept class | 0 | – | – |
| P0 (first probe, pre-74a14a1 kernel) | pass | 10 runs, 7 passes, $0.00356/run | 0.0356 | 0.0711 | 9.56 → 9.53 |
| P (74a14a1 kernel) | **pass** | per run: C 0.0033, E.py 0.0041, E.rust 0.0035, T 0.0037, pool 0.0027; mean **$0.00348/run** → plan **trim**; cost prior $0.0041; Root/real 1.999 | 0.0348 | 0.0696 | 9.53 → 9.49 |
| C calibration | **pass** | 32 × 6; **FA 48‰** (upper 59‰), **FR 86‰**, δ r2/r3/r5 68/57/44‰, **m(standard) 114‰**, MDE lean/standard/strong 134/95/74‰; 10 of 32 in band, so S is unselected (n_kept 32); pool 40 runs on 20 tasks, 6 failing (incumbent 85%) | 0.857 (cap 0.94) | 1.714 | 9.49 → 8.63 |
| D try 1 | harness stop (**DV-2**) | the worker was refused by the kernel task budget after $0.0068 | 0.0068 | 0.0135 | – |
| D try 2 | stopped (disk) | the worker filed **p-000001** (3 T2 variants); screen killed | 0.174 | 0.234 | 8.63 → 8.57 (lagging) |
| D screen 1 | guard freeze | a turn resumed from try 2 was interrupted; the Root billed that cut stream an estimated $0.181 (an upper bound; counted conservatively) | 0.182 | 0.183 | – |
| D screen 2 | **go** | **est 120‰** (E3: v1 fixed 1/2 targeted, lost 1/2 controls; v2 and v3 `screen.mini_inferior`); chosen v1 `c88be5f`, aim `lineage_gain` | 0.0875 | 0.175 | 8.57 → 8.48 |
| D promote | **G0–G3 accept**; G4 interrupted (disk) | gate `accept g0_g3` ($0.0019); handoff; G4 `j-000007`: screen + part of the slice, ~16 min, then the freeze at free disk < 5.3 GB (low 0.24 GB) | 0.218 | 0.436 | 8.48 → 8.26 |
| D G4 retry 1 (DV-5) | refused before G4 | `initctl rollback` of provisional `c88be5f` ok (`j-000008`; current = LKG `aaf07b72`). The re-promote was refused by the kernel validator: `proposal.state` (p-000001 is `promoted`, not `screened`). No promote job, no G4, no S run | 0.0144 | 0.0287 | 8.26 → 8.25 |
| D G4 retry 2 (DV-5b) | G0–G3 **reject `timeout`** | proposal reopened; promote `j-000010` of the same `c88be5f`: G0, G1, G2 pass; **G3 timed out after 589.6 s**. The kernel's G3 stage defaults are agent/inner 300 s, and the cold build slot needs ~280 s to build. The harness then waited for a G4 that could not come (fixed). Frozen and stopped at 13:46 | 0.108 | 0.129 | 8.25 → 8.22 |
| **D G4 retry 3 (DV-5c, DV-7)** | **G0–G3 accept; G4 reject `score_inferior`** | G3 with 1800 s stage timeouts; handoff 14:01. G4 v2 `j-000013`, standard, both arms fresh (cache not reused): **`sealed_g4` inferior, n 29 × 3, d −50‰, m 120‰, cpp 1050‰**. Body rolled back to LKG. No canary, tamper or leak trip; watchdog quiet (free disk ≥ 11.6 GB, peak footprint 1.3 GB). Peak prices | 1.328 | 1.328 | 8.22 → 6.89 |
| M | **pass (skip path)** | current = the confirmed tuple (D not accepted): no milestone runs | 0.0002 | 0.0002 | 6.89 → 6.89 |
| G1L | not run | D not accepted (D-0) | – | – | – |
| **Program** | | | **$3.06** (conservative priced, incl. DV-3's $0.18 estimate) | – | **$9.56 → $6.89 = $2.67 by balance** (the owner's $3.00 cap is measured by balance) |

## Line D in detail (evolution-v2 §3.10 with D-L5)
- **D-0:** frozen before mining (sha256 `687e77f0cfe9be21…`, with the seqs of `sealed_draw` and `sealed_calibrate`).
- **Census and selection:** 7 clusters, 4 eligible. By the rule: **f-000003 `cap.hit:turns`**, s_c **600‰**, 10 pool tasks, 3 labels.
- **The agent's proposal p-000001 (inv, t2):**
  - Hypothesis: runs hit the turn cap because a failing leading `cd` in a bash call returns only "No such file or directory" and skips the `&&` chain, so the model spends turns rediscovering the workspace.
  - Change: bash appends a hint naming the shell's real cwd, and that the shell keeps no state, when a `cd` fails.
  - Predicts `pass_rate@sealed >= parent+15%; cost_usd <= parent+30%`.
- **The 3 variants:**
  - **v1** (chosen): `src/tools/bash.rs` + tests + self.json (+103/−37);
  - v2: `bash.rs`, a different approach (+140/−46);
  - v3: `src/loop/mod.rs` (+244/−46).
- **Screen (E0 + E3, pool incumbent from C's cache):**
  - v1: E0 pass; E3 fixed 1 of 2 targeted tasks, lost 1 of 2 controls, candidate cost_ref 5,377 vs incumbent 13,714 µUSD; **est 120‰** = 0.5 × 0.6 × (1+1)/(2+2) × 0.8, exactly D-0's formula.
  - v2 and v3 were rejected (`screen.mini_inferior`).
- **Go/no-go (D-0 item 5, standard):** 80 ≤ 120 < 150, so **go, aim `lineage_gain`**. The tier stays standard (pinned to D-0 item 6's evaluation after C).
- **T2 check:** v1 is s-measurable and touches `src/tools/bash.rs`: T2.
- **Promote:** the ordinary pipeline. **G0, G1, G2, G3 (model mode) accept**, then the handoff.
- **G4 v2, the single attempt (`j-000007`):** it started at 06:34 +07, at standard. The kernel's visible-pool screen ran from 06:34 to 06:39. The Root's sealed slice ran from 06:39:48 at sealed.parallel 4, and 47 sealed runs had started by the stop. At 06:48:24 one sandboxed `python3.9` inside the slice began to grow. Its memory filled the disk through swap (DV-4), and spend was frozen and the line stopped at 06:51. **No `sealed_g4` record exists**, so the slice produced no statistic (see *Statistical hygiene*).
- **The single G4, completed (`j-000013`, retry 3, 14:01–15:33 +07):**
  - Settings: sealed.parallel 2, gate.g4.parallel 1, at standard, both arms fresh: 5 canaries, then 96 incumbent and 96 candidate runs. All canaries were clean.
  - **Result:** `sealed_g4` **inferior**: d = −50‰ against m = 120‰, n = 29 (3 of the 32 tasks dropped by the Root), R = 3; cost per pass 1.05×.
  - Gate `reject score_inferior`, then the kernel's `body_rollback` (`gate.g4.score_inferior`) to LKG `aaf07b72`.
  - By D-0 item 10, nothing counts toward "sticks". **Verdict: NOT ACCEPTED.**

## Deviations
- **DV-1 (line R's cap):** R spent **$0.181 against its $0.15 cap** (D try 2's worker session $0.174 plus try 1's $0.007). The guard held only D's whole cap during R, a cost-guard bug fixed in `5167369`. D continued on the orchestrator's decision (no S look was used; the G4 stays gated by est ≥ 80‰ and a single attempt).
- **DV-2 (D try 1):** the kernel's per-task budget was $0.05, below one request reservation (about $0.04) plus spend, so the worker was refused after $0.0068, before any proposal. Harness config bug, fixed in `0de9077` (task budget = R's cap for D).
- **DV-3 (resumed turn):** a stop left an unfinished owner turn that resumed at the next boot. Interrupting it made the Root bill an estimated $0.181 for the cut stream, and the guard froze the first screen phase. Fixed in `a3dae6f`: each line starts a fresh agent session. The estimate is counted conservatively; the balance settles it (by balance the program is $1.30, not $1.61).
- **DV-4 (G4 interrupted by disk; root cause from the logs, 2026-10-21):**
  - **Primary cause: swap.** The disk did not fill with run-root or build output. One sandboxed `python3.9` (pid 28390, started 06:48:24 +07 inside the sealed slice) grew to **43.9 GB of compressed memory**.
  - macOS grew its swap files on the same APFS volume: `swapfile5` and `swapfile6` were created at 06:48. Swap growth then failed with ENOSPC (`vnode_setsize for swap files failed: 28`, 06:50:13–37), and jetsam killed the process (`low swap: killing largest compressed process … (Python) and size 43883 MB`, 06:50:38).
  - Free disk fell by more than 3.7 GB inside one 30 s poll (≥ 5.3 GB at 06:48:51, 1.54 GB at 06:49:21).
  - This is the only such event in the kernel log from 00:00 to 07:05. During C's 193 sealed runs at parallel 4, free disk stayed flat.
  - Lowering `parallel` does not prevent it, because one process is enough.
  - **Secondary cause: a cumulative probe-root leak.** Each kernel probe root clones `INIT_HOME/kernel`, including the imported pool, which is read-only. When the probe stops, `remove_dir_all` stops at the first EACCES, and the error is ignored, so finished probes leave their roots behind:
    - about 130 × 2 MB after C;
    - 0.75 GB after D try 2;
    - 1.42 GB `du` at the G4 stop, of which 0.55 GB was unique blocks (the rest were APFS clones).
  - Cleanup without a reinstall: delete `ih/probe/<12 hex>` by exact path while the Root is stopped. `m7.sh` now does this after every line.
  - **Bounded, concurrent footprints:**
    - the sealed run roots (`rh/sealed/run/<id>`, deleted when each run ends; 4 × ~38 MB were in flight at the stop, and the Root deletes them at start with `clean_stale`);
    - the kernel's G4 check dirs (deleted per run; `cache/gate/j-000007/check` is empty);
    - the build slot (1.6 GB, constant, removed between lines).
  - No kernel or Root rebuild is needed.
- **DV-5 (re-running the single G4 on the same sha):**
  - The provisional did **not** roll back: the supervisor shows current `c88be5f` provisional, LKG `aaf07b72`, and a provisional expires only after 24 h.
  - An interrupted G4 job ends as `kernel_restart`, and the kernel has no verb that re-runs G4 on the current provisional.
  - So the retry runs `initctl rollback --reason "DV-5 …"`, then promotes **the same `c88be5f`** again (G0–G3, handoff, then G4 v2 at standard).
  - It is still D-0's single G4, because no `sealed_g4` record exists.
  - Harness: phase `g4retry` (`dv5_decision`, selftested).
  - Run settings: `sealed.parallel 2`, `gate.g4.parallel 1`. A real line starts only with ≥ 10 GB free.
  - A watchdog (`tests/real/m7watch.py`) checks free disk every 5 s. Below 6.0 GB it stops the Root's process tree (SIGSTOP), freezes spend, then kills the tree and the driver by PID.
- **DV-5b (proposal reopened, orchestrator's decision 2026-10-21 11:05):**
  - The first retry's rollback worked (`j-000008`, current = LKG `aaf07b72`). The re-promote was then refused with `proposal.state`: p-000001 had stayed `promoted` after `j-000007` ended as `kernel_restart`.
  - With the Root stopped, the harness set the test-home proposal file `ih/state/kernel/evo/proposals/p-000001.json` from `promoted` back to `screened`. It also appended one history entry (`harness:REAL-M7`, DV-5b). The command was `checks_m7.py dv5-reopen`: same chosen sha only, once, selftested.
  - This is a kernel-state edit by the harness only. The Root lineage, the sealed state, the est (120‰), the chosen variant (v1) and the sha (`c88be5f`) are all unchanged. No S was seen.
  - Then phase `g4retry` again: current is the LKG, so the same `c88be5f` is promoted (G0–G3, handoff, single G4 at standard).
- **DV-5c (third reopen and G3 stage timeouts, owner-approved 2026-10-21 ~13:45):**
  - Retry 2's promote (`j-000010`) was rejected at G3 by the kernel's 300 s stage timeouts on a cold build slot (`74a14a1` raised only the model's bash timeout). No G4 ran, and no S was seen.
  - The harness reopened p-000001 once more (`dv5-reopen`, same sha `c88be5f`), and set `[gate.g3] agent_timeout_s = 1800, inner_timeout_s = 1800` in the test home's kernel config (config only, read at kernel start).
  - The re-run was at peak prices, under the program cap of **$3.00 real by balance** from $9.56: a guard floor of $6.56, with each line's cap bounded by the balance left above it.
  - The Root lineage, the sealed state, the est, the chosen variant and the sha are unchanged.
- **DV-7 (retry 3, money handover and the Root's sealed ceiling, orchestrator-approved 2026-10-21 ~14:58):**
  - D's G4 runs both S arms fresh (the runbook's ~211 runs at standard), not only the candidate's 96. At peak prices, the projected D total was ~$1.45–1.50 real, so the harness's stale D line cap ($1.07) would have frozen the G4 at ~75%.
  - The owner's rule is $3.00 real by balance, a $6.56 floor, with priority G4 > M > G1L. To apply it:
    1. at 14:58 the harness SIGSTOPped its own `m7.sh`, so its cleanup could not stop the Root;
    2. it stopped its driver by PID;
    3. `tests/real/m7guard.py` took over the money guard: the same meters and the same `Guard`, with the cap set by the balance floor ($1.66 real from the line's $8.22 start), freeze plus stopping the Root by PID on a trip. The watchdog (6.0 GB, 4096 MB) stayed on.
  - The Root's sealed ceiling would have bound mid-slice: `sealed.reserve_usd` was $2.98 with $2.43 already spent, and at peak the Root meter equals real.
  - So only that test-home ceiling was raised, through the owner's `charter check --apply`: to $3.53 (total $5.26). That is spent + the $0.94 left above the floor + 2 in-flight reservations. Nothing else was changed.
  - The kernel's slice budget (`gate_usd` $2.54 minus the job's spend) did not bind.
- **DV-6 (per-process memory cap, approved 2026-10-21):** from the G4 retry on, the watchdog SIGKILLs any single workload process below the Root whose physical footprint (resident + compressed) exceeds **4096 MB**. The Root and the kernel are never killed; their footprint is only logged. Why this cap changes outcomes only by protecting the disk:
  - a run over 4 GB on these ≤ 10 s tasks is a runaway, and it fails in either arm (cap kill, jetsam kill or timeout);
  - the parent arm comes from C's cache, which ran with no cap, but C's 193 runs never came near it (free disk stayed flat, and there was no swap event).

## Findings
- **Product safety: a sandboxed workload can exhaust RAM, then swap, then disk on macOS.**
  - Seatbelt (`sandbox-exec`) has no memory limit, and macOS does not enforce `RLIMIT_AS`/`RLIMIT_DATA`.
  - Neither the Root's sealed sandbox nor the kernel's spawn path sets any memory bound (`grep rlimit`: only `RLIMIT_NOFILE`).
  - One `python3.9` in a sealed run reached 43.9 GB compressed and took free disk from about 7 GB to 0.24 GB in about 2 minutes, before jetsam killed it. The owner's machine and live install were at risk.
  - The same path exists in daily use: the agent's bash tool runs model-written code under the same kind of sandbox.
  - The Root's `quotas.disk_min_free_gib` floor does not help, because swap growth bypasses it.
  - A product fix needs a supervisor-side memory watchdog, as `m7watch.py` does for M7: phys_footprint via `proc_pid_rusage`, a per-instance cap, and a kill by PID.
- **An interrupted G4 strands its proposal:**
  - A kernel restart during G4 marks the job `kernel_restart`, and the provisional then waits 24 h to expire.
  - The proposal stays `promoted`. The validator accepts an evo commit only while its proposal is `screened` (`proposal.state`), so the same chosen sha can never be promoted again, and no verb re-runs G4 on a provisional.
  - Fix: on a G4 job ending as `kernel_restart` (or on rollback without a G4 verdict), return the proposal to `screened`; or add a `promote --g4-only` for the current provisional.
- **Kernel probe-root leak:**
  - Probe roots clone `INIT_HOME/kernel`, including the read-only imported pool.
  - `ProbeRoot::delete` then fails at EACCES, and `probe.rs` ignores the error (`let _ = root.delete(..)`), so finished probes keep their roots, cumulatively (up to 1.4 GB `du` per G4 screen here).
  - Fix: make the clone writable before deleting, or skip the pool in the clone, and log delete failures.

## Statistical hygiene (j-000007)
- **Nobody saw a partial S outcome from `j-000007`:** no pass count, no partial d, no per-task result. Everything that was read is listed below.
  - **Root lineage after the stop:** record kinds and gate records. There is no `sealed_g4` record and no G4 gate record.
  - **The kernel job record:** `running`, stage G4, pct 0, no result.
  - **Money meters:**
    - the harness guard's priced usage and request counts;
    - the egress usage ledger: requests per minute by lane, and the number and timing of sealed runs (run ids, HTTP status, tokens). It has no task ids and no scores.
  - **Listings, without reading any content:**
    - `rh/sealed/runs.json` (4 run ids in flight);
    - the file names and mtimes in `rh/sealed/cache`: only the incumbent's 32 files from C (04:40), none written during j-000007.
  - **The macOS kernel log:** one sandboxed `python3.9` in the slice was killed for memory. Its run, arm and task are unknown and were not looked up.
  - I ran no `initctl status`/`evo status` and did not open `sealed/g4`, S instances or run roots. The orchestrator received only these summaries.
- **The candidate's partial runs are discarded.** The Root caches only the incumbent arm (keyed by its sha), so a new G4 starts the candidate's pre-set sample (standard, 32 × 3) from zero. The 4 leftover run roots are deleted unread at Root start. The parent arm comes from C's incumbent cache, as in the first attempt (max age 72 h).
- **The decision to continue was made before any look,** on non-S grounds (disk), by the orchestrator ("approved in principle", 2026-10-21 07:00 +07).

## Money
- **By balance (the owner's measure): $9.56 → $6.89 = $2.67, under the $3.00 cap.**
- Conservative priced: $3.06. This includes DV-3's $0.18 upper-bound estimate for a cut stream, and the meter prices every request at list.
- D's completed G4 ran at peak prices by the owner's decision (`$1.33` real for the line). Off-peak it would have been about $0.66.
