# REL: one-command install, release build, uninstall, `init`, README, CI

Status: **done** (2026-10-20, lane REL, 12:20–16:20 +07). The release is built with the quality guard green, and the end-to-end run passes: install, the `init` TUI, one line against the mock, stop, and uninstall to the Trash. The release dir is `target/dist/v0.1.0/` (built from 3610c17 plus other lanes' uncommitted edits, so `dirty: true`; a clean re-cut at the tag is docs/release.md §2).

Owns: `scripts/release.sh`, `scripts/install.sh`, `scripts/uninstall.sh`, `scripts/init`, `scripts/release-e2e.sh` (new, see Deviations), `README.md`, `CHANGELOG.md`, `docs/release.md`, `.github/workflows/**`, and the `[profile.release]` sections of `kernel/Cargo.toml`, `root/Cargo.toml` and `seed/Cargo.toml`.

## Done

- **`scripts/release.sh`** builds `target/dist/v<version>/`:
  - binaries: `init-root`, `init-kernel`, `initctl`, `init-body`, and `init-tui` when `tui/` exists (docs/tui.md §9 item 3);
  - `src/<version>.bundle`: the kernel source, `kernel/` and `wit/` at HEAD, one ref `refs/heads/release` (root.md §3.8.1);
  - `seed/seed-v<version>.bundle`: branch `seed`, tag `seed-v<version>`, its own vendor cut via `scripts/slice1/seedpack.py` (read-only reuse, FIX4);
  - `docs/` (`docs/kernel/*.md`), `wit/init-ext/`, `ext/count_lines.wasm` (the template extension, built for wasm32-wasip2 in the dev profile, as ext/authoring.md §3.4 prescribes);
  - `assets/`: the charter template and the constitution, snapshotted before the build so the wizard writes a charter in exactly this binary's dialect;
  - `MANIFEST.json`: the sha256 of every file, the commit, the dirty flag, rustc, and build and test times.

  Options: the quality guard `cargo test --release --offline --locked` for root and seed, on by default (`--skip-tests`); `--allow-dirty`; `--serial` (one cargo at a time); `--jobs`. A disk check is warm-aware (6.0 GB warm, 8.0 GB cold).
- **`scripts/install.sh`** is the one command:
  1. Preflight: macOS ≥ 14, sandbox-exec, git ≥ 2.39.4, rustc 1.97.x, wasm32-wasip2, home lengths ≤ 64, not already installed.
  2. The release: runs `ci/vendor.sh --locked` if `vendor/` is missing, then reuses a fresh `target/dist` (HEAD, clean, hashes match) or runs `release.sh --skip-tests` (`--with-tests`, `--rebuild`, `--reuse`).
  3. The wizard: purpose, daily $ (5), total $ (25), max_level (4), provider (deepseek), model (deepseek-flash | deepseek-v4-pro) and disk floor (20 GiB). Every prompt has a default, and `--yes` takes them all. The charter is written from the release's template and validated with the release's own `init-root charter check`; `--charter FILE` skips the wizard.
  4. A disk check: floor + 2 GB.
  5. `init-root install --release <dist> --charter … --toolchain … --seed-source <dist>/seed/seed-v<version>.bundle [--no-service]`, with a progress ticker. HOME is the passwd home, which the class rules need.
  6. Frees the build slot.
  7. `kernel/config.toml`: the model, its prices (config.md §4.10), soft budgets under the ceilings (request_max = min($1, day/5)), and the Root's `[sandbox]` kept verbatim.
  8. `init` and `init-tui` go to `INIT_BIN_DIR`, with ROOT_HOME baked in; it warns if that directory is not on PATH.
  9. `launchctl bootstrap` of `dev.init.root`, unless `--no-service`.
  10. Prints the next steps: `init-root keys set deepseek` (the owner runs it), then `init`.

  The script never reads or writes a key. `--mock-upstream 127.0.0.1:PORT` is a test hook: it writes `root.test_mode` and a loopback provider, and requires `--no-service`.
- **`scripts/uninstall.sh`** (committed 371a84b): `init-root stop` through the running Root, `launchctl bootout` only for the real install, a refusal while any Root holds `run/root.lock`, a typed `uninstall`, then `mv` of ROOT_HOME, INIT_HOME, `init` and `init-tui` into `~/.Trash` with a timestamp. It refuses to cross volumes and never runs `rm`.
- **`scripts/init`** (committed 263de610): the docs/tui.md launcher. It looks up `init-tui` from `$INIT_TUI`, then next to the launcher, then in `ROOT_HOME/bin`. An `$INIT_TUI` inside INIT_HOME is refused with exit 77 (T-4), and any other candidate there is skipped. Otherwise it falls back to `init-root attach`. It supports `--which`.
- **`scripts/release-e2e.sh`**: the $0 end-to-end check, described in Tests.
- **`[profile.release]`** in the three workspaces: the defaults, stated explicitly, with the measurement in a comment (see Tests).
- **README.md** (rewritten for the owner as a user), **CHANGELOG.md** (v0.1.0), **docs/release.md** (how to cut a release).
- **`.github/workflows/fast-linux.yml`**: the gate job runs init-proto, initctl, kernel-gate and kernel-selfmap. A second, informational job (`continue-on-error`) runs the seed and the ext template. The workflow pins Rust 1.97.1 with clippy and wasm32-wasip2, runs `ci/vendor.sh --locked`, caches `vendor/` and `kernel/target`, and has a 15-minute timeout.

## Tests

**Release build** (`ci/slot.sh scripts/release.sh --allow-dirty --serial --lean --min-free 5900`, 15:23–15:52, 2 jobs, load 10–30): exit 0.
- Build times: root 33 s (warm), seed 336 s, ext 1 s, kernel 692 s (cold, after the orchestrator removed the partial dir), tui 7 s; 1,709 s in total, including the tests.
- **Quality guard, `cargo test --release --offline --locked`:**
  - root: **105 passed, 0 failed** (init-root 84, rsafe 3, sandbox-profiles 11 + doc_drift 2 + others; the fixture suites are cfg-gated off, as in ci/local.sh);
  - seed: **300 passed, 0 failed, 5 ignored** (lib 275, boot 16, self_md 9).
- Binaries: init-kernel 38.0 MB, init-body 22.7 MB, init-root 5.4 MB, init-tui 1.3 MB, init-mockllm 1.1 MB, initctl 1.0 MB.
- Kernel source bundle `f3397b70`; seed bundle `seed-v0.1.0` = `a5bc3e54` (129 seed files, 183 crates, 24 stubs).
- Lowest free space 6,014 MB. `--lean` removed kernel/, root/ and seed/target/release, each by exact path, once its outputs were staged.

**End to end** (`ci/slot.sh scripts/release-e2e.sh --reuse --keep`, 16:08–16:14): **pass**. The verdict is `build/status/rel-e2e.json`.

| Step | Result |
|---|---|
| `install.sh --yes --no-service --disk-floor 5 --mock-upstream …` (wizard defaults, `--reuse`) | exit 0 in 315 s, of which `init-root install` (the cold body build) took 311 s; lowest free 5,452 MB; build slot freed; genesis tuple formed |
| launcher | `init --which` = `init-tui` (shipped). With it moved aside → `init-root attach`. `INIT_TUI` inside INIT_HOME → exit 77 |
| `init-root start` (our child) | kernel running and body ready in 1.6 s |
| `init --raw --no-replay` (the TUI, line mode) + one line | the model called `write`; `work/hello.txt` = `INIT\n`; reply `done`; idle again after 728 ms; 2 egress requests, 2 mock requests; 3,781 µUSD metered by the Root (mock prices); no ceiling hit; `audit verify --full` exit 0 |
| `uninstall.sh` while our Root ran (no service) | `init-root stop` → 0 instances, then refused (exit 1); homes untouched |
| SIGTERM to our own Root pid | exit 0 |
| `uninstall.sh`, answer `no` | exit 1, nothing moved |
| `uninstall.sh`, typed `uninstall` | exit 0: `r`, `i`, `init` and `init-tui` in the test Trash (`home/.Trash/<name> 2026-10-20 16.13.40`); homes gone |

Lineage: genesis, charter, promotion, tuple, lkg, audited, kernel_start, zone_start, zone_exit, stop.

Three earlier E2E runs failed, each for a harness reason, fixed:
1. `rustup` under the test HOME found no toolchain. install.sh now pins `RUSTUP_HOME` and `CARGO_HOME` to the passwd home.
2. `init --which` assumed no TUI. The release now ships `init-tui`, and the E2E checks both paths.
3. The mock's `last_text_contains: hello.txt` failed. The body now adds an `[init:reload] ext generation 1 · not built: count-lines` note after the owner's line, so the expectation is now `last_role: user` only. The body retried the 500s 20 times over 240 s; the Root metered nothing.

**Release profile measurement.** The `init-root` release build ran back to back under the same load, with the variants set through `CARGO_PROFILE_RELEASE_*` env, so no manifest was edited:

| Variant | Build | init-root bytes | Load (1/5/15) |
|---|---|---|---|
| default (cold, 12:23) | 365 s | 5,191,104 | ~70–100 |
| lto = "thin" + codegen-units = 1 (cold) | 501 s | 4,568,832 (−12.0 %) | 27/52/69 (lighter) |
| lto = "thin" | 138 s | 5,350,368 (+1.1 %) | 41/46/63 |
| default, the run right after | 111 s | 5,293,104 | 58/50/63 |

The first `thin` attempt failed (`ring` C build: `internal.h` not found) while another lane re-vendored `vendor/`; it was re-run. The default sizes differ between 12:23 and 12:57 because the init-root source changed in between (other lanes).

Default-profile release builds of all three workspaces, run in parallel at load 70–110 (12:23–12:55):

| Workspace | Time | Binary |
|---|---|---|
| kernel | 1,903 s | init-kernel 37.8 MB, initctl 1.0 MB |
| seed | 1,651 s | init-body 22.7 MB |
| root | 365 s | init-root 5.2 MB |

Target dirs: kernel 1,129 MB, seed 902 MB, root 318 MB.

**Launcher**, against a fake layout: 9/9 checks green.
- Fallback `--which`.
- Root missing → exit 69; Root not running → exit 69.
- `INIT_TUI` inside INIT_HOME refused (77), also through a symlinked path.
- `INIT_TUI` outside INIT_HOME: `--which` and exec with arguments.
- `init-tui` next to the launcher; `ROOT_HOME/bin/init-tui`.

**Uninstall**, against a fake home with a flock held by the test's own child:
- held lock → exit 1, nothing moved;
- answer `nope` → exit 1;
- `uninstall` → r, i and init moved to the test Trash; an unrelated file in the bin dir stayed.

**Charter wizard**, against the committed template with the release-built `init-root charter check`:
- flash, $2.50/$25, L3, floor 20 → valid;
- v4-pro with `--mock-upstream`, floor 7 → valid (destinations and host loopback, `root.test_mode`);
- a purpose with quotes and a backslash round-trips.

`bash -n` (bash 3.2) is clean for every script. The workflow YAML parses (ruby yaml). shellcheck is not installed.

## Deviations
1. **HOME in the tests.** The brief says HOME should point at `target/test-homes/rel/…`. The scripts' HOME does: `~/.local/bin` and `~/.Trash` land in the test dir. Every `init-root` call, though, runs with the passwd HOME, because the broker's class rules compare the kernel's `HOME_REAL` (its passwd home) with the Root's HOME (WP21b, FIX4, S1 deviation 1). With `--no-service`, nothing is written there. `~/.init`, `~/.init-root`, `~/Library` and `~/.Trash` are never touched.
2. **The release profile states the defaults**, rather than enabling LTO or `codegen-units = 1`: neither paid off (table above). Stating `codegen-units` and `strip` changes cargo's artifact hashes, so the first build after the change was a full one.
3. **The seed source** is the release's `seed-v<version>.bundle`, made by `release.sh` from HEAD's `seed/` plus a vendor cut, not "this repo" directly. The repository has no `seed` branch or `seed-v*` tags, and `init-root install --seed-source <dir>` needs both. This is exactly what WP21b and FIX4 do, and it answers WP21b's open "who produces the seed repository" question (docs/release.md §2).
4. **New file `scripts/release-e2e.sh`**, outside the listed paths, so the end-to-end check can be re-run and docs/release.md can point at it.
5. **TUI.** `release.sh` builds `tui/` when it exists, and `install.sh` installs `init-tui` next to the launcher (the orchestrator's Q3 decision).
6. **The fast-linux gate crates** now have `init-testkit` as a dev-dependency (lane K-FLAKE's watchdog), so the gate job also builds init-testkit. It has `cfg` branches for non-macOS, but no Linux build has been run: no Linux target is installed, and installing one is not allowed.
7. **`--serial`, `--lean` and `--min-free` in release.sh**, for a shared machine with little disk (the orchestrator's slots and load rules). With `--serial`, each workspace runs build → tests → stage → remove before the next starts.
8. **`init-mockllm` ships in the release** (orchestrator decision; G0 runs the seed's provider tests against it, and G0-ROOT eec8d5e makes a production install refuse a release without it).

## Needs from others
- **Orchestrator:** after the push, the first GitHub run of `fast-linux`. Its gate status stays unverified until then.
- **Orchestrator, for the tag:** a clean re-cut. Run `scripts/release.sh` on a clean tree at the tagged commit, then `scripts/release-e2e.sh`. Today's dist has `dirty: true`, because other lanes had uncommitted kernel/root/seed edits.
- **Seed / kernel (FYI):** a fresh install's body reports `[init:reload] ext generation 1 · not built: count-lines`. The install-mode build does not build the seed's reference extension. Harmless at L0; A2 builds extensions itself.
- **S-REAL / slice1 (FYI):** `scripts/slice1/s1.sh mock` uses the same `last_text_contains` expectation that broke here (see failed run 3), so it will hit the same retry loop against today's body.

## Left
- A real (non-test) install on the owner's account: `scripts/install.sh`, then `init-root keys set deepseek` and `init`. The owner runs this; it is the only step that touches `~/.init*` and `~/Library/LaunchAgents`.
- `install --upgrade` has no end-to-end check (docs/release.md §6).
- The fast-linux gate is unverified until its first run.
- `target/dist/v0.1.0/` (87 MB) is kept as the product, for `install.sh --reuse` and the tag. The test homes and `target/*/release` are removed.
