# S-REAL: v0.1 flows on the real model

Status: **done** (2026-10-20, 12:20–17:10 +07). Every check ran on `deepseek-flash` through the full stack, on homes built by the real install path: owner → init-root → init-kernel → init-body → kernel proxy → Root egress → api.deepseek.com. Results, costs and meters are in `measurements/real-v0.1.md`, and the redacted artifacts in `build/real-artifacts/real/<check>/`.

## Results
| Check | Result |
|---|---|
| V-DS-1, 3, 4, 5, 6 | measured live |
| V-DS-2 | contradicts provider.md: DeepSeek accepts its own tool turns without `reasoning_content` and refuses only foreign ones |
| A1-real | **pass** |
| E2E-2 (thinking on) | **pass** 8/8, on 4 homes |
| A2-real | **pass**, relaxed: valid SemVer order; the tie order differs from the fixture |
| **E2E-3** | **pass**: a real promote through G0–G3 (model-mode G3), the handoff, then 5,888 of 6,696 prompt tokens cached on the first request after the boot (≥ 0.9 × head) |
| **A3-real** | **pass**: the agent's own T2 self-edit through G0–G3; the handoff kept the same session, and the new body names its commit. G4 made 13 real paired runs and ended `inconclusive: underpowered`, as designed for the 2/1/1 stand-in pool. |
| E2E-1 | boot half **pass**. The plain `initctl gate --quick` passed G0 live, then hit G1 `self_json_missing` (N 12). G2 round trips are shown by E2E-3's and A3's gates. |
| M-3 | not run: budget (≈ $2.5 Root-metered for 10 trials) |

**Spend.**
- Root-metered: **$0.529** of the $1.00 hard cap, on the input-price meter the cap is defined on. Six homes ran in sequence, and each charter cap was $1.00 minus the spend of the earlier homes.
- Real money: **$0.21**. The balance went from $10.62 to $10.41 (13:34–17:06, 2-decimal resolution, and billing lags).
- Proxy (live scope): $0.084.
- The orchestrator lifted its own $0.50 soft line at 14:45. The hard cap was never touched.

**Key hygiene.**
- The key was read only from `~/.config/init-agent/deepseek.key`, with `tr` into the test Root's key store (0600). It was never echoed, logged or put on a command line, and it was removed on every exit; `keys/providers/` was empty after every run.
- Final grep: **0** occurrences of the key in 38,211 files (artifacts, harness, measurements, testdata, the homes and staging before deletion).
- **0** `sk-` strings in committed paths and in their git history.

## Done
- **Harness:** `tests/real/real.sh` + `tests/real/checks.py`.
  - `install`: the real install path, `REAL_CHARTER_USD`, `REAL_CACHE_READ`, init-mockllm in the release.
  - `run <check>`:
    - per-check kernel budgets, prepare outside the zone, key in and out;
    - a stop line, and a per-check Root-metered cap (interrupt + `/stop`);
    - the trace view saved, the balance read before and after;
    - collect and redact.
  - `S1_DRY=1` runs on the real home with no key ($0).
  - Checks: `vds`, `vds2`, `vds3`, `a1`, `e2e1`, `e2e2`, `e2e3`, `a2`, `a3`, `balance`.
- **Fixed in my lane, each with a $0 regression test:**
  1. `669d5a1`, **DeepSeek foreign-turn fallback** (`seed/src/provider/deepseek.rs`).
     - Live V-DS-3: the §3.10 flatten is accepted, but the model then re-runs the flattened tool call. After a provider switch, that repeats side effects.
     - The first fallback now adds `"reasoning_content": ""` and keeps the tool calls. That is accepted, and the model uses the result. The flatten is the second fallback.
     - Tests: `fallback_adds_empty_reasoning_before_flattening`, the updated `flatten_foreign_turns`, the strict-mock `mock_strict_rejects_missing_reasoning_then_empty_reasoning`, `real_streams_decode` (3 live SSE fixtures), and `classify_errors` (the V-DS-6 text).
  2. `f4cde26`, **provider and tools tests on a body checkout under G0.**
     - Socket test homes go under `$CARGO_TARGET_DIR`, with a cwd-relative socket path to fit `sun_path`.
     - A missing `INIT_MOCKLLM_BIN` skips the mock tests with a note.
     - The tools registry doc golden runs only when the repo docs exist.
  3. `a730d10`, `tools::testutil::homes_root()` under `$CARGO_TARGET_DIR` (49 loop and tools tests, under the build text). G0-SEED's request.
  4. `98c9842`, the slice1 mock turn 1 expects a user message last, because the body appends `[init:reload]` notes. This applies to `scripts/slice1/s1.sh` and `tests/slice1/watchdog.mock.jsonl`. It matches REL's verified fix; not re-run here (a cold install would have competed for disk).
  5. Harness fixes, all in `tests/real/`, `scripts/slice1/collect.py` and `scripts/slice1/s1.sh`:
     - busy-period tracking across `/new`;
     - per-check trace windows;
     - a valid E2E-3 commit message;
     - the selfmap env (cargo on PATH, an absolute `--root`, the 1 GB `body/target` removed after);
     - the G4 stand-in task cap raised to 150,000 µUSD (30,000 was below one request's reservation);
     - `collect.py` skips `target/`, `.git` and `vendor/`, and prunes request bodies;
     - the cache-read charter line is written only for a Root that knows the key.

## Tests
- `cd seed && INIT_MOCKLLM_BIN=<kernel debug init-mockllm> cargo test --offline --bin init-body provider:: tools::tests::registry`: 62 passed. `tools:: loop::`: 55 passed, from the dev repo and from a simulated G0 slot (cwd `slot/src`, `CARGO_TARGET_DIR` = `slot/out/target`). `cargo test --offline --no-run` green, rustfmt clean.
- Live, in a real install, G0 passed on the seed at `7c4b1a9` (j-000001, home 2b), and the full promote passed at kernel `2bb7ffa` (E2E-3, A3).
- Real runs: `S1_I_MEAN_IT=1 INIT_SLOT_POOL=real ci/slot.sh tests/real/real.sh real run <check>`.

## Bugs for other lanes (each found live, with a repro; status as of 17:10)
1. **K: build timeouts ignored.** Install mode and G0 ignored `supervise.build_timeout_*`: `BuildSlot::new` used the defaults. Two cold installs died at 900 s at load 150–240. Now `BuildSlot::configured` in install.rs:171 and stages.rs:136. The G3 inner gate's G1 still uses `BuildSlot::new(..).src()`, which only takes the path.
2. **K-FLAKE `f8c212d`: `gate --quick` HEAD fetch.** `gate --quick` on a clean worktree fetched `refs/heads/HEAD` (E_FETCH). Verified fixed live on home 2e.
3. **Seed `fade67a`: `forbid_missing`.** The tests/*.rs crate roots lacked `#![forbid(unsafe_code)]`, so G0 rejected the seed itself.
4. **Selfmap `40600e0`: relative root.** `--root .` passed the relative `./vendor` to the ext builds.
5. **G0-ROOT `d05c1e3`/`eec8d5e` and G0-SEED `3008357`/`f76e749`: G0 `test` in a real install.**
   - (a) The sandbox denied every socket.
   - (b) init-mockllm was not shipped.
   - (c) The ext_host fixtures had no vendor dir.
   - (d) The boot.rs socket path was over 103 bytes.
   - (e) Seed test homes were under the read-only export.
6. **K-GATE `70f30d3`: G1's OUT.** G1's build-profile OUT was `cache/gate/<job>/g1`, so the candidate binary and components were hidden (`gate.g1.manifest_error`).
7. **K-GATE `c712701`: zero-request G4 runs.** G4 scored runs that made 0 requests (every run `cost_capped`) and computed a verdict.
8. **v0.1.x, cost: cold selfmap build.** Every agent T2 attempt pays a cold `selfmap` build: ~1 GB and ~5 min in `work/target`, and again in G3's probe root.
9. **Kernel proxy, minor: refused 400s.** A 400 that DeepSeek refuses, with no usage, is booked at ~1.6k µUSD by the proxy; the Root books 0. The proxy's live stats also exclude the gate lanes.
10. **Log/context lane: no `thinking` key.** `thinking` is hard-coded `enabled` in `seed/src/context/render.rs`, so E2E-2's disabled mode cannot run through the body; it needs a `[model] thinking` key. Sending `reasoning_content: ""` on own assistant messages that have none would also make a provider switch need no fallback.
11. **Docs lane.**
    - provider.md §3.10 and §8 need the live V-DS-2 and V-DS-3 facts.
    - testing.md E2E-2's path is ambiguous with cwd `work/`: the model wrote `work/work/e2e/hello.txt`.
    - The A2 check should accept any valid SemVer order of equal-precedence versions.
    - The G4 task format should state that `limits.max_cost_musd` must be ≥ one request's reservation.
12. **Seed owner, open: no committed `self.json`.** The seed has no committed `self.json`, so `initctl gate --quick` on an unchanged fresh install fails G1 `self_json_missing` (E2E-1, home 2e). Fix: commit a generated `seed/self.json` with a drift check in CI, or have seedpack/install generate and commit it.

## Deviations
1. Each check sends `/new` first, to keep contexts small. E2E-3 runs right after an E2E-2 smoke in the same session, as the doc asks.
2. E2E-1 on home 1 used `gate --quick main` (bug 2). Home 2e used the default form.
3. The E2E-3 candidate adds `#![forbid(unsafe_code)]` to tests/*.rs when the installed seed lacks it (home 1 only).
4. The A2 pass criterion is a valid SemVer order (E2E-5: "relaxed event matching").
5. Thinking disabled was checked at the provider level only (bug 10).
6. Every home kept the input-price meter (`REAL_CACHE_READ=0`), so the cap's meaning stayed the same across homes (the orchestrator agreed).
7. The G4 stand-in pool is A3's 2/1/1 shape with R = 2: the cap forced a smaller design than gate.md's defaults. Its verdict is therefore always `underpowered`.
8. I freed the agent's `work/target` (1.2 GB, a build cache) mid-A3, after its selfmap, so that G3's probe build stayed above the disk floor.

## Left
- M-3, which needs a budget the owner sets.
- E2E-4 (A1-real × 3): 1 run.
- The over-window half of V-DS-6, which needs the owner's approval.
- E2E-1's G2 through `gate --quick` itself, after N 12.
- Re-measure the Root/real cost ratio on a Root with cache-read billing (R-DEC).
- Every S-REAL home and staging dir (`target/real-homes/`, `target/s-real/`) is deleted by exact path; no S-REAL process is left.
