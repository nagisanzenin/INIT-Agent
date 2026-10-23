# G0-SEED: the seed passes its own G0

Status: **done for the production G0 path** (2026-10-20, lane G0-SEED; seed + kernel-gate test + ci/local.sh; `source ci/env.sh`, `ci/slot.sh`).

## Done

### 1. Seed test fixes (G0 runs `cargo test` on a body checkout with OUT the only place for sockets)
- **ext_host fixtures (cause 3, 18 tests).** `cargo_wasm_build` passes the gate's two `--config` flags with the vendor dir absolute: `<seed>/vendor` in a body checkout, `<repo>/vendor` in the dev repo. A fixture workspace under the target dir sees no `.cargo/config.toml`. Commit 3008357.
- **tests/boot.rs (cause 4, 13 tests).** Homes live in the test binary's target dir (`seed/target` in the dev repo, `slot/out/target` under G0), not in `CARGO_MANIFEST_DIR/target` (the read-only export). The proxy and kernel sockets get short names, `<target>/sk/<tag><n>.{p,k}`, and are removed by exact path. The longest such path is 101 bytes on S-REAL's install path (`.../target/real-homes/r/ih`) and 93 in the check. Commit 3008357.
- **The same class of bug, found by the check** (commit f76e749): awareness `TempHome` and `FakeProxy` (socket `<target>/sk/a<pid>-<n>`), gitsync and housekeep test homes, and log/context `testutil::homes_root` now use `$CARGO_TARGET_DIR` when it is set and absolute (G0 sets it), else `<crate>/target`. This is S-REAL's pattern in provider.
- **Provider mock race** (mock_tests.rs, handed to this lane by the orchestrator): init-mockllm appends a request's log line only *after* it has sent the response. A client that has just seen `Done` could read `mock.log.jsonl` one line short. This happened once under G0 load: `mock_full_turn_anthropic_and_openai`, log 1 vs 2. `MockLlm::log_n(n)` waits up to 5 s for n lines at all 5 call sites, and `log()` skips a line that is still being written. `Home` in mock_tests.rs already puts sockets under `$CARGO_TARGET_DIR` (S-REAL f4cde26), with a cwd-relative path when the absolute one is over 103 bytes. It passes under G0, so it is unchanged.

### 2. The check: `kernel/crates/kernel-gate/tests/seed_g0.rs` + ci/local.sh step "seed G0" (commit 3610c17)
1. `seedpack.py --kind repo` cuts the seed (default `--rev worktree`; `INIT_SEED_G0_REV=HEAD`) into `ih/cache/build/slot/src` of a stand-in INIT_HOME (`seed/target/g0/ih`): `seed/` plus its own `vendor/` plus `.cargo/config.toml` naming `vendor`. `ih/kernel/bin/init-mockllm` is copied from `kernel/target/debug`, as G0-ROOT's eec8d5e install ships it.
2. Build stage: supervise's `BUILD_ARGS` with the build slot's environment, in the `build` profile.
3. `kernel_gate::g0::run`, the call promote makes, with a `Spawner` that runs every command under `/usr/bin/sandbox-exec`. The profile texts come from `root/sandbox-profiles/profiles/` (PRELUDE + SECTION + ROOT_ALLOWS + ZONE_TAIL, all parameters). Default profile: `kernel`, which is where G0 runs as built (DirectSpawner children of the kernel; G0-ROOT d05c1e3 section 6a SLOT_OUT). `INIT_SEED_G0_PROFILES=kernel,build` adds gate.md's `build` text.
4. The target dir `slot/out/target` is **kept warm** (G0 lets `EnvSpec.target_dir` be set): 1.3 GB, about 6.5 min per run under load. The previous run's `vendor/` is reused when the crate dirs and package checksums are equal. A fresh cut gives directories new mtimes, and cranelift-isle's directory `rerun-if-changed` then rebuilt cranelift and wasmtime (+3 min). `INIT_SEED_G0_COLD=1` starts cold. The CPU caps from `ci/env.sh` (`CARGO_BUILD_JOBS`, `RUST_TEST_THREADS`) are added to G0's environment when set. They change only parallelism.
5. On a failing `test` step it prints the failing tests and their panics. Logs: `ih/vault/gate/jobs/seed-g0-<profile>/g0/`, `ih/state/kernel/gate/build.log`. Watchdog 70 min.

## Tests
- Dev repo: `cargo test --offline --test boot` 16/16; `--bin init-body ext_host::` 21/21 (154 s); `boot:: log:: context::` 129/129; `provider:: boot::awareness` 99/99; seed `cargo clippy --all-targets -D warnings` clean; `cargo clippy -p kernel-gate --tests -D warnings` clean.
- The check, `kernel` profile (production G0), run 3 (worktree with all fixes): **G0 pass** in 173 s: metadata, build_config, shape, check 27 s, clippy 12 s, ext 1 s, test 133 s. Unit 275/0 (5 ignored), boot 16/16, self_md 9/9, ext 3/3.
- **Green sha: 3610c17** (seed tree last changed in f76e749), the check on seed HEAD (`INIT_SEED_G0_REV=HEAD`, `kernel` profile, root profiles at HEAD with G0-ROOT d05c1e3, shipped mock): build stage 63 s; **G0 pass** in 145 s (check 20 s, clippy 9 s, ext 1 s, test 114 s). Unit 275/0, boot 16/16, self_md 9/9, ext 3/3. Whole check 212 s warm.
- `build` text (opt-in), run 3: everything passes except 49 tests that all go through `tools::testutil::homes_root` (assigned to S-REAL below).

## Failure classification (S-REAL's j-000004 log: unit 238/33, boot 3/13)
| Tests | Cause | Fixed by |
|---|---|---|
| awareness fetch_stats ×3 | socket bind denied (profile) and socket under the export | G0-ROOT d05c1e3 (AF_UNIX under SLOT_OUT) + G0-SEED f76e749 (socket in target) |
| ext_host ×18 | fixture cargo without vendor config | G0-SEED 3008357 |
| provider mock ×7 | init-mockllm not shipped | G0-ROOT eec8d5e |
| provider stub ×4 | socket bind denied | G0-ROOT d05c1e3 (S-REAL f4cde26 already put the homes in the target dir) |
| tools registry ×1 | read ../docs | S-REAL f4cde26 |
| boot.rs ×13 | socket path > 103 B; homes in the export | G0-SEED 3008357 |
| provider mock_full_turn (new, run 2) | mock log line written after the response | G0-SEED f76e749 |

## Deviations
- Default profile `kernel`, not `build`. G0 runs as the kernel's children (serve.rs, WP8b deviation 1, confirmed by G0-ROOT and the orchestrator). The `build` text is opt-in.
- Socket files live in `<target>/sk/`, outside `test-homes/<wp>/`. A path under test-homes/ cannot fit 103 bytes on long install paths. They are removed by exact path.
- The check composes the profile text itself (the same concatenation as `seatbelt::render`, on the same files), because kernel-gate cannot link init-sandbox-profiles (separate workspaces). Any parameter it does not know fails the check loudly.
- The warm target and the reused vendor/ are check-only (production resets the slot). Guards: cargo fingerprints; vendor reuse only on equal per-crate package checksums; `INIT_SEED_G0_COLD=1`.

## Needs from others
- **S-REAL** (seed/src/tools/**): `tools::testutil::homes_root()` (and `tools/tests.rs` `bash_env`, which uses it) writes into `CARGO_MANIFEST_DIR/target`. Under the `build` text, where the export is read-only, that fails 49 tests (loop ×25, tools ×24). It does not matter under the `kernel` profile G0 uses today. Fix: the `$CARGO_TARGET_DIR`-when-absolute pattern from provider. After that, `INIT_SEED_G0_PROFILES=kernel,build` should be green.
- **Gate/supervise owners (v0.1.x, cost):** the build stage's environment lacks G0's `RUSTC_WRAPPER=` / `CARGO_BUILD_RUSTC_WRAPPER=` (empty) and `RUSTC`. wasmtime's cc build script likely reruns on that difference (cc reads `RUSTC_WRAPPER`), so every T2 gate recompiles wasmtime, wasmtime-wasi and init-body twice, about 2 min. Giving both the same environment would save that.

## Left
- Make `kernel,build` green after S-REAL's tools fix (optional for v0.1; it is not the production path).
