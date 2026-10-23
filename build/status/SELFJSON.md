# SELFJSON: the seed ships its committed self.json

Status: **done** (2026-10-20, lane SELFJSON; `source ci/env.sh`, `ci/slot.sh`). Commit b31a263, not pushed.

## Finding (S-REAL, home 2e)
The seed had no committed `self.json`. G1 (`promote::stages::g1`) reads the committed file from the export root
(`src.join("self.json")`), so `initctl gate --quick` on an unchanged, freshly installed body always failed
`gate.g1.self_json_missing`. Promotions passed only because the candidate commit ran selfmap.

## Done
- **`seed/self.json`** (b31a263): the seed root, i.e. `self.json` at the root of the seed repo and `body/self.json`
  after install. It was generated the way the body's T2 procedure does it: `init-kernel selfmap --root <checkout> --write`
  on a `seedpack.py --kind repo` cut that was made a git checkout (`git init`, as the installed `body/` is), with G0's
  environment and target. The cut matters because seedpack rewrites `.cargo/config.toml` (`../vendor` → `vendor`; it is
  in the inventory) and adds `vendor/` (not inventoried). Result: generator 2, self_v 2, no `live` (D12), 129 files,
  `body_tokens` 359,557, tools bash/edit/ext/read/write, extension count-lines, 0 warnings.
- **The check** (`kernel/crates/kernel-gate/tests/seed_g0.rs`, the same ignored test, so the same ci step): after G0 it
  runs G1 on the same export the way `stages::g1` does. The shipped `ih/kernel/bin/init-kernel` (an APFS clone of
  `kernel/target/debug/init-kernel`, `INIT_KERNEL_BIN`) runs `selfmap --root <export> --binary … --components … --out …`
  under the real `build` profile text with `SRC` = the export and `OUT` = `ih/cache/gate/seed-g1`. Its components are
  the `ext/<name>.wasm` copies that `stages` makes, built in G0's env, which is a no-op after G0. Then it runs
  `g1::compare_self_json` against the export's `self.json` and `g1::check_wit_copy` against the repo's `wit/`. G1 runs
  even when G0 failed. On failure it prints the code (`gate.g1.self_json_missing`, or `self_json_drift` with the
  changed files named) and the regenerate command:
  `source ci/env.sh && (cd kernel && INIT_SEED_G1_BLESS=1 ../ci/slot.sh cargo test --offline -p kernel-gate --test seed_g0 -- --ignored --nocapture)`, then commit `seed/self.json`.
- **Bless mode** `INIT_SEED_G1_BLESS=1`: build stage, then write mode on the cut, then a copy to `seed/self.json`, then
  gate-mode G1 on the result. So every bless also proves that write mode and gate mode agree. It skips G0 unless
  `INIT_SEED_G0_PROFILES` is set. It takes about 2 min warm.
- **ci/local.sh**: the step is now "seed G0 + G1 (the seed passes its own G0 and G1)" and passes `INIT_KERNEL_BIN`.
- **docs/kernel/selfmap.md §3.10**: an as-built note (where the file lives, how it is generated, the check, and that
  any `seed/` change needs a regenerated `seed/self.json` in the same commit).
- **seedpack.py / install**: no change needed. seedpack exports every tracked or untracked file under `seed/` (the cut
  now has 130 files, `self.json` included), `release.sh` runs `git add -A` on the cut, and the install imports that
  commit. Nothing strips `self.json`, and it is not gitignored.

## Tests
- Bless, cold: build stage 408 s, write mode 119 s, then G1 pass with 0 warnings (536 s in total).
- Repro on the pre-fix HEAD 404096a (`INIT_SEED_G0_REV=HEAD`, G0 skipped): **red**, `gate.g1.self_json_missing`, with
  the regenerate command printed. This is S-REAL's failure.
- Drift: one `SELF.md` sha256 tampered in the worktree `seed/self.json`: **red**, `gate.g1.self_json_drift`,
  "changed: SELF.md". The file was restored byte for byte.
- Full check, worktree = b31a263's seed, `kernel` profile: **G0 pass** in 216 s (check 71 s, clippy 10 s, ext 4 s,
  test 131 s), then **G1 pass** with 0 warnings on the same export. So G0's tests leave nothing in the export that
  G1's walk would pick up. 231 s in total.
- Committed tree (`INIT_SEED_G0_REV=HEAD` at b31a263, G0 skipped): **G1 pass** with 0 warnings (62 s).
- `cargo clippy --offline -p kernel-gate --tests -- -D warnings`: clean. rustfmt (edition 2024): clean.

## Deviations
- The check uses gate mode (`--out`, the exact G1 path, walk file set, under the build profile), not
  `selfmap --check`. `--check` is write mode (git file set, its own build), so it would test a different path from
  the one that failed in the field. Bless uses write mode, so the two modes check each other.
- `selfmap --check --root seed` in the dev repo reports drift on `.cargo/config.toml` (`../vendor` there), and that
  is expected. Regenerate only through the bless command, which works on the cut.
- The check reuses the warm target `seed/target/g0` (now 1.8 GB with the wasm build). I removed it by exact path
  when I finished, as asked, so the next `ci/local.sh` run starts cold (about 7 min for the build stage at 2 jobs).

## Needs from others
- **kernel-supervise (K-GATE or the promote owner):** `promote/pipeline.rs` `lkg_body_tokens` reads
  `budget.body_tokens`. That key does not exist. The schema has top-level `body_tokens` (= `budget.tokens`,
  selfmap.md §3.2). So the LKG's size is always 0, and the accept rule's `body_tokens_c.cmp(&body_tokens_p)` always
  sees the candidate as larger. `stages::g1` survives only through its `budget.tokens` fallback. Fix: read
  `r["body_tokens"]` (fall back to `budget.tokens`) in both places. Before this lane it read 0 anyway, because the
  genesis LKG had no `self.json`.
- FYI: the seed's `body_tokens` is 359,557 against the spec's 25k body budget (spec §4). Nothing enforces that budget
  today. Whoever owns the budget should know this before it is enforced.

## Left
- Nothing for v0.1.0. Every later commit that touches `seed/` must regenerate `seed/self.json` (the ci step enforces it).
