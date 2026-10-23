# FIX4: Slice-1 harness on the real install path; faster tuple hashing

Status: **done for fakebody and mock; watchdog and realdry are pending** (the orchestrator held them back because of disk; see "Left"). 2026-10-20, 06:10 +07. Lane X.

Owns:
- `scripts/slice1/**`
- `tests/slice1/**`
- the `[profile.*]` sections of `root/Cargo.toml`

## Done
**1. The install path is merged into the main harness (`scripts/slice1/s1.sh`).** Every mode now builds its home the way WP21b's `s1-install.sh` did:
- seed packaging, then a git bundle (branch `seed`, tag `seed-v0.1.0`);
- `init-root install --seed-source <bundle> --no-service`, run with the owner's real HOME;
- the install-mode kernel then does the rest: it imports the seed, clones `body/`, runs a sandboxed cold build, does the `store.put`, and the Root forms the genesis tuple;
- the harness removes the build slot (exact path) and reads back an install check: tuple body sha = seed sha = `lkg.json` sha (reason `seed`), and tuple body binary = sha256 of `bin/init-body` = `lkg.json.binary_sha256`. `body/GENESIS.md`, `body/.git`, `lineage/body.git` and `lineage/seed.git` all exist, and the last three lineage records are `tuple, lkg, audited`.

Both workarounds are gone: the hand-written `state/kernel/lkg.json` and the `git archive` into `body/`. The harness also no longer copies a prebuilt body into `bin/init-body`/`.lkg`.

What is unchanged:
- the modes `fakebody`, `mock`, `realdry` and `real`;
- every guard: real mode needs `S1_I_MEAN_IT=1`; the `S1_SPEND_MAX_MUSD` stop; the key is copied with `tr`, never echoed, deleted before collection and again in the EXIT trap; `collect.py` redacts and rescans;
- signals go only to PIDs this script spawned;
- homes are removed by exact path.

`scripts/slice1/s1-install.sh` is removed, since it was merged.

- **New `scripts/slice1/seedpack.py`:** the packaging step, taken out of `s1-install.sh`. It has two kinds of seed.
  - `--kind repo` packages `seed/` at `S1_SEED_REV`.
  - `--kind fakebody` is a new seed whose `init-body` is `init_testkit::fakebody`. It contains `kernel/crates/init-proto` and `init-testkit` (manifests and `src/` only), a root package `init-body`, and `GENESIS.md`. Its `Cargo.lock` is `kernel/Cargo.lock` pruned by `cargo metadata --offline`, so every crate keeps the kernel's locked version.
  - Both kinds get the same vendor cut (APFS clones, platform-only crates stubbed). Fakebody mode needs this seed: on the install path the kernel builds the body itself, so the fake body has to come from a seed.
- **Disk guards in `s1.sh`:**
  - A case needs `S1_START_FREE_MB` free to start (6500, or 5800 for the fake-body seed).
  - A sampler (a child of the harness) logs free space every 2 s, and the verdict fails (`disk_ok:false`) if free space fell below `S1_MIN_FREE_MB` (5300).
  - The charter floor `disk_min_free_gib = 5` makes the Root kill the install (only instances it holds) inside the install.
- **The verdict now records:** `install_ms`, `owner_sock_ms`, `start_to_ready_ms`, `disk{free_start_mb, free_min_mb, peak_use_mb, build_slot_mb, …}`, `install_check`, `seed_kind`/`seed_rev`/`seed_sha` and `boot_sha`, plus all of the old fields.
- **`tests/slice1/run.sh`:** the same four $0 cases, now on the install path, run one at a time.
  - It needs 6500 MB free to start.
  - It prints install time, start time and peak disk for each case.
  - It removes stale verdicts first.
  - It stops the suite at the first case whose sampler fell below the floor.
  - Its exit code is nonzero if any case failed or did not run.

**2. Faster hashing in `root/Cargo.toml`.** `[profile.dev.package.<crate>] opt-level = 3` is set for `sha2` and for the crates its hashing runs through: `digest`, `block-buffer`, `crypto-common`, `hybrid-array` and `cpufeatures`. The Root itself stays a plain debug build. Quality guard: no semantics change, and every hash is still compared with its expected value.

## Tests
Run with `root/target/debug/init-root`, rebuilt with the new profile (`CARGO_INCREMENTAL=0 cargo build --offline -p init-root`, production, 5.2 s), and the frozen k1 `init-kernel` (sha256 `9e6712c90989…`) and `init-mockllm`. Seed: committed HEAD (`S1_SEED_REV` default).

| Case | Result | Install | owner.sock | Start → ready | Free: start → lowest (peak use) | Build slot |
|---|---|---|---|---|---|---|
| `s1.sh fakebody` (06:02) | **pass**: `fake reply: hello from slice-1`; proxy probe → egress → mock 200 (8 µUSD); both instances sandboxed; install check ok; tuple `ta10452910e7a` | **17.3 s** | 174 ms | **1.5 s** | 6717 → 6504 MB (**213 MB**) | 209 MB |
| `s1.sh mock` (06:04–06:08) | **pass**: model calls `write`; `hello.txt` = `INIT\n`; reply `done`; 2 turns (`tool_use`, `end_turn`); request 2 carries `Wrote 5 bytes (1 lines) to hello.txt (new file).`; Root 7,281 µUSD, proxy 1,890 µUSD; no ceiling hits; `audit verify --full` exit 0 | **209.9 s** (cold wasmtime build) | 349 ms | **1.7 s** | 8405 → 6832 MB (**1,573 MB**) | 963 MB |

More detail on the mock run:
- **Boot:** the body booted with `INIT_SHA` = seed `e04599ff9333…`, with no exit 78 or 70.
- **Lineage:** `genesis, charter, promotion, tuple, lkg, audited, kernel_start, zone_start`.
- **Root start before and after the hashing change:** start to ready took 9.8 s in WP21b's install-mock run and 25 s in S1 (debug sha2 at about 13 MB/s over the 154 MB kernel). It now takes **1.7 s**.
- **Peak disk use:** the 1,573 MB includes the seed packaging (188 MB, transient) and whatever other lanes did at the same time. Plan on about 1.6–1.7 GB per cold repo-seed install, not 1.1 GB: the final link of `init-body` adds about 0.5 GB at the very end.
- **Artifacts:** redacted, in `build/slice1-artifacts/{fakebody,mock}/`; `collect.py` reported 0 leaks after redaction.
- **Checks:** no `slice1-homes` process was left (`ps`), and `bash -n` is clean for both scripts.
- **Not run:** the real DeepSeek mode, as instructed.

## Deviations
1. **The default seed is now committed HEAD** (`S1_SEED_REV=HEAD`). `s1-install.sh` used the working tree because WP13's boot code was not committed then; it is now (`1ca38e4`, `6949816`). HEAD is reproducible and cannot pick up other lanes' half-finished edits. `S1_SEED_REV=worktree` gives the old behaviour.
2. **Fakebody mode installs a fake-body seed** (seedpack `--kind fakebody`) instead of copying `init-fakebody` into `bin/`. The overrides `S1_BODY_BIN` and `S1_FAKEBODY_BIN` are gone, because the kernel builds the body.
3. **install runs with the owner's real HOME and `--no-service`** (taken from `s1-install.sh`). S1 used HOME = the run dir, with a LaunchAgent file written there.
4. **The suite was not run as one `run.sh` pass.** At 06:02 only 6.7 GB was free, and three cold installs at about 1.67 GB each would have gone below the 5.3 GB line. The orchestrator then cleared disk to 8.2 GB and asked for **one** cold case, `mock`. `fakebody` comes from the smoke run just before.

## Needs from others
- **The owner or the orchestrator:** about 7.1 GB free before each cold repo-seed case, so the run keeps 5.3 GB after the roughly 1.7 GB peak. Then run `tests/slice1/run.sh`.
- **Doc owner:** root.md §4.3 / supervise.md §3.6.2. The disk figure for a cold seed install is about 1.6–1.7 GB at the lowest point, not about 1 GB.

## Left
- **Pending: the `watchdog` and `realdry` cases on the install path** (each is a cold install). Their artifacts in `build/slice1-artifacts/{watchdog,realdry}/` are still from S1's runs on the old workaround path. Command: `tests/slice1/run.sh` (all four), or `S1_TAG=watchdog S1_MOCK_SCRIPT=tests/slice1/watchdog.mock.jsonl S1_SPEND_MAX_MUSD=1000 scripts/slice1/s1.sh mock` and `scripts/slice1/s1.sh realdry`.
- **A reusable template home** (kernel `install --template`, supervise.md §4.8) would make repeated cold cases cheap. Until then each case costs about 3.5 min and about 1.7 GB of transient disk.
- `build/slice1-artifacts/install-mock/` is WP21b's last `s1-install.sh` run and is kept as history.
