# MERGE-T2CACHE: warm T2 promote builds through `cache/deps`

Status: **done** (2026-10-24). All runs used the mock model at $0. Test homes were under `kernel/target/test-homes/wp4/` and `target/test-homes/m2`, and were removed afterwards. `~/.init` and `~/.init-root` were not touched.

## Done
- **The deps cache is now used** (supervise.md §3.6.2). Before this, `DepsCache::commit` had no caller (REL-G0 root cause 1).
  - On a miss, `promote::stages::prepare_deps` builds the stub workspace in the build slot before any candidate code runs.
  - The stub runs the build command (build profile), then G0's compiling commands (`check`, `clippy`, `ext`, `test --no-run`) through G0's spawner and environment.
  - It prunes the stub's own first-party artifacts and stages `slot/out/target` as `cache/deps/<key>.tmp`, with an in-memory tree digest.
  - The candidate then builds on the warm target.
  - The entry is published (digest re-checked, `manifest.json` written, renamed) only when the candidate's G0 passes. Otherwise it is discarded. Afterwards the LRU keeps `deps_cache_keys` entries.
- **A hit** is used only after the entry's tree digest matches its manifest. A corrupt entry is deleted, and the build falls back to the stub (cold).
- **The extension workspace** gets a stub too: the workspace tables plus a member stub per crate.
- **Two warm-killers fixed:**
  - `vendor/` mtimes are pinned to 2000-01-01 after every export. `git archive` stamps the commit time, which tripped `rerun-if-changed`.
  - Slot builds use a fixed `SOURCE_DATE_EPOCH`. cc-rs (wasmtime's build script) tracks it, so a per-commit value rebuilt wasmtime and everything above it on every promote.
- The G0 record has a `deps` object (`mode`, `key`, `ms`, `build_ms`, `published`, `note`).

## Measured (real seed, real Root-broker Seatbelt `build` profile, G0 under the `kernel` zone profile, load 10–23)
`INIT_T2_WARM_BENCH=1 cargo test --offline -p kernel-supervise --test promote_seatbelt -- --ignored --nocapture t2_seed_build_is_warm_on_the_second_promote` (opt-in, so the CI step "promote under Seatbelt" stays 1-2 min)
- Promote 1, the seed (miss):
  - build stage 924.5 s: stub 907.0 s, body 11.0 s;
  - G0 90.9 s (check 4.5 s, clippy 5.8 s, ext 1.0 s, test 79.5 s);
  - total 1029 s.
- Promote 2, a no-op comment on `src/main.rs` (hit):
  - build stage 32.3 s: verify and clone 14.6 s, body 14.2 s, and only `init-body` compiles;
  - G0 93.8 s;
  - total 126 s.
- Before the `SOURCE_DATE_EPOCH` fix, the hit still recompiled wasmtime: the build took 3 min 34 s.
- One key is 1.0 GB.

## Tests
- `cargo test --offline -p kernel-supervise --lib`: 129/129. New tests:
  - `deps_cache_rejects_tampered_and_corrupt_entries`
  - `prune_first_party_removes_only_the_named_packages`
  - `pin_vendor_mtimes_sets_one_old_time`
  - `make_stub_workspace_stubs_the_ext_members`
  - `promote::tests::deps_cache_stub_then_warm_hit_then_corrupt_falls_back` (real cargo). It covers three cases: stub, then a hit with no dependency recompiled although the export is newer, then a poisoned rlib that is rejected and rebuilt cold. With the mtime pin disabled, it fails.
- `--test promote`: 9/9. `--test extend --test install --test serve_seam --test promote_seatbelt`: all pass. `cargo check -p init-kernel`: ok.
- The new ignored test `t2_seed_build_is_warm_on_the_second_promote` passes (above).
- `cargo clippy -p kernel-supervise --all-targets --no-deps -D warnings`: clean except two pre-existing findings in `kupgrade/kstate.rs` (not touched here). kernel-proxy also has one pre-existing finding.

## Deviations
- Docs said the stub runs after the pipeline. It now runs before the candidate on a miss, so the first promote costs about one cold build, and G3's inner gate and later promotes are warm.
- `SOURCE_DATE_EPOCH` for slot builds is 946684800, not the commit time (supervise.md env table).
- sandbox.md: the stub uses the slot's `SRC`/`OUT`, not `cache/deps/<key>.tmp/{src,out}`.
- Residual risk (documented): G0 children run inside the `kernel` zone profile (WP8b deviation 1) and can write `cache/deps`. The digest catches corruption, but not a deliberate rewrite of an entry together with its manifest.

## Left
- The install-time source build (`install.rs`, no prebuilt) does not fill the cache. With a bundle's prebuilt body, the first T2 promote pays the stub (about one cold build).
- G0's environment has no `CARGO_BUILD_JOBS` (pre-existing; G0 compiles at full parallelism).
