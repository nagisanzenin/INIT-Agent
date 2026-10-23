# Phase 0 measurements (WP16, 2026-10-20, 07:19–07:28 +07)

All runs cost $0 and use the mock only. Procedures follow testing.md §3.11, with the deviations listed below. The tool is `init-measure`, in `kernel/crates/init-testkit/src/bin/init-measure.rs` and `init-measure/`. The raw results, with sorted keys, are in `measurements/m1.json`, `m2.json`, `m4.json`, `m5.json` and `m5-100ms.json`.

## Environment

| Item | Value |
|---|---|
| Machine | Apple M2, 8 cores, 16 GiB, macOS 14.6 (23G80), on AC power |
| Toolchain | rustc 1.97.1 (8bab26f4f 2026-07-14), cargo 1.97.1 |
| Load | load averages 3.2–7.7. Every other build agent had finished. The orchestrator held `ci/local.sh` until these runs were done. |
| Builds | The existing target dirs `seed/target`, `root/target` and `kernel/target/k1`, with `CARGO_INCREMENTAL=0`. No cargo run waited on a lock. |
| Binaries | Frozen copies of `seed/target/debug/init-body` (06:02, sha256 `444e5a61f92e…`) and of k1's `init-mockllm` (06:45, sha256 `8a6c069ceb4d…`), plus k1's `init-kernel` at HEAD `8ce50ad`, which includes FIX6 and FIX7. |
| Disk | 7.4–7.5 GiB free throughout. The floor is 5.3 GiB. |

## Results

| Metric | Threshold (source) | Measured | Result |
|---|---|---|---|
| **M-1** clean build (reused from WP0, n = 1) | cold ≤ 900 s build timeout; risk trigger at > 600 s (supervise.md §3.6.2, testing.md M-1) | seed 116.6 s, kernel 54.6 s, root 8.5 s (`measurements/wp0-build-times.md`); a cold sandboxed seed build inside install: 209.9 s (FIX4) | **pass** |
| **M-1** incremental build (fresh: `touch` one file, n = 5, median / max) | warm ≤ 300 s build timeout | seed `init-body` (`src/loop/mod.rs`): **6.69 / 7.08 s**; root `init-root` (`src/main.rs`): **3.25 / 3.68 s**; kernel `init-kernel` (`src/main.rs`): **1.31 / 2.13 s** | **pass** |
| M-1 no-op rebuild (n = 3) | — | seed 0.14 s, root 0.09 s, kernel 0.15 s | info |
| M-1 target dir | ≤ 4 GB (testing.md M-1) | `seed/target` 1.7 GB (`kernel/target/k1` is 5.5 GB and `root/target` 2.4 GB; the rule is for the body) | **pass** |
| **M-2** warm rebuild of a T1 component (`touch src/lib.rs`, `wasm32-wasip2`, n = 5, median / max) | ≤ 5.0 s, criterion C1 (ext/authoring.md §3.10) | count-lines **0.34 / 0.81 s** (WP14: 0.38 s, **confirmed**); semver-sort (the 196-line spec subject) **0.39 / 0.39 s** | **pass**, so C1 does not trigger the fallback lane |
| M-2 extras | 150–300 KB `.wasm`; compile 30–300 ms | `.wasm` 247,598 B (count-lines) and 248,057 B (semver-sort); host `compile_ms` 43–55; `init-body ext describe` process wall 74–86 ms; semver-sort's first build in a fresh workspace dir was 31.6 s (its wasm dependencies were recompiled) | info |
| **M-3** agent writes a component (10 real-model trials) | ≥ 6/10 (ext/authoring.md §3.10) | **not run**: it needs the real model, and tonight's budget is reserved | **pending** |
| **M-4** prefix hash identical across a T2 restart | 5/5 identical, and the negative control differs (testing.md M-4) | 5 handoffs on fresh processes: 6/6 `turn` requests have the same `prefix_sha256`, 6/6 boot events and ready files have the same `prefix_hash`, and the simulated `cache_read` after each boot (4,032–4,992 tokens) covers the head (about 3,627 tokens). The negative control (an edit to `SELF.md`) changed both hashes. Kernel path (A3, 3 runs): 4/4 scripted `turn` requests have one prefix across the promote and the rollback. | **pass** |
| **M-4** T2 handoff timings | timeouts only: `pause_timeout_s` 1,800 s, `ready_timeout_s` 90 s (supervise.md §3.6.4) | **Body level** (real `init-body`, n = 5, median): pause → paused 0.05 ms; candidate spawn → ready **34 ms**; exit → A gone 1.5 ms; **window from pause to swap 36 ms**; B's first reply 6.9 ms. **Kernel level** (A3, real `init-kernel` under `init-fakeroot`, n = 3, median): G0 0.87 s, G1 0.93 s, G2 1.63 s, G3 5.98 s, **handoff stage 1.52 s**, **promote total 12.4 s** (12.1–12.7 s); asynchronous G4 6.9 s | **pass** |
| **M-5** body memory over a synthetic 1,000-step session (socket mode) | v0.1 rule (provisional owner decision 2026-10-20, testing.md M-5): footprint (`phys_footprint`, macOS); the baseline is the last sample before the first epoch (step 155); slope ≤ 2 KiB/step, and max ≤ 1.25 × baseline. The old rule: ps RSS, steps ≥ 100, base = RSS at step 100. | **v0.1 (M5 lane, 14:09–14:11, `measurements/m5-v0.1.json`), 3 trials:** slope −6.43 / −5.02 / +0.53 KiB/step; max/base 1.141 / 1.117 / 1.198. Footprint 4.2–4.4 MiB at step 0, 17.9–20.4 MiB at the baseline, peak 20.3–23.2 MiB. The old rule on the same runs fails: 4.5–7.1 KiB/step, 1.55–1.77. **WP16 (old rule):** RSS 9.9 MB at step 0, 18.5 MB at 100, 30.1 MB at 500, 32.9 MB at 1000; slope 15.2 KiB/step, max/base 1.78. | **PASS 3/3** (v0.1 rule); WP16: FAIL |
| **Slice-1** time to first reply (recorded runs; no cold install re-run) | none set in the docs | Owner line → idle: fakebody **331 ms**, mock **351 ms** (2 model turns plus the `write` tool), real deepseek-flash **2.39 s**. Root start → ready 1.5 s (fakebody) and 1.7 s (mock). Install 17.3 s (fakebody) and 209.9 s (mock, cold wasmtime build). | info (functional pass) |

## Details

### M-1
- Procedure: one warm-up build, which is not counted. Then 3 no-op builds and 5 builds, each after `touch <file>`, each run under `/usr/bin/time -l`. Every touch build recompiled the crate, and none waited on a lock.
- With `CARGO_INCREMENTAL=0`, a touched crate is rebuilt in full. That means these numbers are the upper bound for an edit to that crate.
- The kernel number covers only the `init-kernel` binary crate, whose `main.rs` was touched. It then links the 154 MB debug binary.
- Not run, and still pending:
  - the semantic edit (a string literal), because it would need edits to other lanes' sources;
  - the `cargo check`, `clippy` and `test --no-run` variants;
  - fresh cold builds, because creating new target dirs was ruled out;
  - the gate-path step 5, which is from M5 on.

### M-2
- count-lines builds in `seed/ext`. semver-sort is `fixtures/ext/semver-sort/`, staged as `body/ext/semver-sort/` of a throwaway body tree, with the seed's extension workspace, its WIT and an absolute `vendor/` path. Both use `CARGO_TARGET_DIR=seed/target`.
- The cold step 4 was not re-run, because it needs an empty target dir. WP0's cold count-lines build took 32.9 s, and semver-sort's first build took 31.6 s once its wasm dependencies were recompiled.

### M-4
- **Body level** (`init-measure m4`, socket mode). The tool stands in for the kernel's side of supervise.md §3.6.4 on one session:
  1. `pause`, then wait for `paused`.
  2. Spawn B with `reason=promote` and a new `INIT_SHA`. B uses the same binary, because the comment-only `m4-nop.patch` changes no code.
  3. Wait for `state/run/ready`.
  4. Send `exit` 75 to A.
  5. Send one owner line to B.
- The mock ran with `--simulate-cache`, and it recorded 0 expectation failures and 0 protocol errors.
- What the body-level run does not cover is the proxy's `last_head_hash`, because socket mode has no proxy. The kernel-level A3 run covers that path.
- **Kernel level** (`init-measure m4-a3`).
  - It runs WP15's A3 acceptance test unchanged, three times; all three passed, in 28.3–28.7 s. It snapshots the kernel's job records (`created_at`, `finished_at`, `stages[].wall_ms`) before the test removes its home.
  - G3 includes the probe's quiet period.
  - The A3 G4 verdict was `reject: no_gain`, carried out as a rollback at idle, as WP15 recorded.

### M-5 v0.1 (M5 lane, 2026-10-20 14:09–14:11 +07): PASS 3/3
- **Rule** (provisional owner decision 2026-10-20, FIX8). The metric is the macOS physical footprint (`proc_pid_rusage` `ri_phys_footprint`). It matched `footprint -p` to the KiB at the start and the end of every run. ps RSS is still sampled, as a diagnostic. The baseline is the last sample before the first epoch event in the session log, which here is a `mask` after step 155. The thresholds are unchanged. Sampling is every 200 ms (143–246 samples per run).
- **Body.** `seed/target/debug/init-body` sha256 `318e83d918a5…`, i.e. HEAD 9055953 plus this lane's change to seed/src/log and seed/src/context. Mock: `init-mockllm` from kernel/target/debug. Load averages were 13–16 (a slot was held; other lanes kept building). Each run took 34–52 s and logged 1,001 turn requests, 2 compactions, 8 masks and 0 errors.

| Trial | Baseline (step, KiB) | Peak KiB | Slope KiB/step | Max/base | Old rule (RSS, step 100): slope / ratio |
|---|---|---|---|---|---|
| 1 | 154, 20,353 | 23,217 | −6.43 | 1.141 | 4.50 / 1.550 |
| 2 | 152, 18,177 | 20,305 | −5.02 | 1.117 | 7.11 / 1.633 |
| 3 | 155, 17,921 | 21,473 | +0.53 | 1.198 | 6.74 / 1.737 |

- **Margin.** The slope passes easily. The ratio's margin is thin (worst case 1.198 against 1.25).
  - An earlier batch of the same body, re-analysed with this rule, gave 1.069, 1.198 and 1.131.
  - The ratio is set by allocator timing and by the copies of the context that every request holds. Two of the three peaks fall at a compaction (steps 464 and 895), when the window, the fork's projection and render, and the provider's copy coexist.
  - My first implementation took the baseline at the first request *after* the epoch. With it, 2 of 6 runs across two builds of this body failed (ratios 1.278 and 1.271), because the sample there swung 14.4–19.0 MiB as the allocator returned the old view's pages at its own pace.
- **The rule still discriminates.**
  - The pre-FIX8 body under footprint from step 155 failed 2 of 3 runs (FIX8.md: ratio 1.23–1.45).
  - The stored RSS-only runs fail the old rule and fail RSS from step 155: m5.json 14.0 KiB/step and 1.60; m5-fix8-before 12.7 and 1.60; m5-fix8-after 6.8 and 1.31.
  - `init-measure`'s tests check that a 3 KiB/step leak after the epoch fails.
  - The FIX8 body (HEAD) passed 1 run at 0.17 KiB/step and 1.148, under load 70–150.
- **What changed in the body (FIX8 follow-up; requests byte-identical).** Window heap per masked step went from 3,800 to 2,611 B (structural estimate, `bounded_view_heap_per_masked_step`), and `Loaded` from 568 to 248 B:
  - `EventBody::Message` is boxed (`EventBody` 504 → 184 B), and so are `MessageEv.usage` and `request`;
  - the view also drops a message's `usage`, `stop_reason`, `provider`, `api` and `model`, the `tool_call` fields and non-owner timestamps, plus the `mask` entries' duplicated id and part;
  - the estimates use a projection that copies no content they do not read (2 of 3 projections per request);
  - a compaction frees the byte cache before its fork renders.

### M-5 WP16 (old rule: fails; kept for the record)
- **Run.** `gen-steps` produced 1,000 tool calls: `bash printf '%2048s\n' x`, plus a `read` of a 5,000-line fixture every 100th step. The mock ran `mock-frontier-1`, which has a 131,072-token window, with `--record-bodies none`. The run took 23.6 s, with 0 errors and 0 protocol errors.
- **Session.** The mock received 1,001 `turn` requests and 2 compactions, and the body logged 8 `mask` events. The session log grew to 4.4 MB.
- **RSS curve.** RSS rose steadily up to the first compaction, at about step 500. It stayed flat at about 31.5 MB from step 515 to 780, then rose again.
  - Over the second half of the session the slope was still 6.4 KiB/step, or 8.4 KiB/step at 100 ms sampling.
  - The size of that slope is close to how fast the session log grew, about 4.4 KB per step. This suggests, but does not prove, that the body keeps the whole event log in memory.
- **Size.** In absolute terms the peak is small: 33 MB.
- **Status.** This is a real threshold failure. The fix belongs to the body: loop.md item 21 and log-context.md retention. The decision belongs to the testing.md owner, because skipping 100 steps does not get past the context-fill phase on a 131k window with 2 KB steps.
- **Pending:** the full-stack variant and the variant that reloads a T1 component every 100 steps.

### Slice-1
- The numbers come from `build/slice1-artifacts/{fakebody,mock,real}/run/verdict.json`: the FIX4 runs on the install path at 06:02 and 06:04, and S1's real run at 05:26.
- Owner line → idle is `drive.wall_ms`.
- No cold install was re-run.

## Deviations from testing.md §3.11
1. **M-1 and M-2.** Edits are mtime-only `touch`es, not content edits, so no other lane's source was edited. The existing target dirs were reused and no cold runs were made (orchestrator rule).
2. **M-4.** The procedure measures prefix identity, as testing.md defines it. At the orchestrator's request it also records handoff timings. The 5 handoffs were run at body level in socket mode, plus the kernel path through A3. The `m4-nop.patch` and `m4-prefix.patch` scenarios do not exist; the same binary with a new sha, and a `SELF.md` edit, stand in for them.
3. **M-5.**
   - `gen-steps` is an `init-measure` subcommand, because `init-mockllm` has no such command and WP16 does not own it.
   - A run reports the full-stack and T1-reload variants as not run.
   - The 1 s sampling rule gives only 22 samples in the window, because the run lasts 24 s. A second run at 100 ms gives 199 samples and the same verdict.
4. **M-3** is pending: it needs the real model, which tonight's $10 plan reserves.
