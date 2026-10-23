# WP21b: the Root install path (seed bundle, install-mode kernel, genesis tuple)

Status: **done.** The Root install path is built, and Slice-1 runs green through it with no workarounds (`s1-install.sh mock`, 05:43). 2026-10-20, about 05:55 +07. Lane X. Owns `root/crates/init-root/` (install, tuple, sup). Spec: root/root.md §4.3 steps 5–10, §4.4; kernel/supervise.md §3.11.4 and §4.8 (`install --from-bundle`).

## What it removes from Slice-1
`scripts/slice1/s1.sh` has two workarounds:
- it hand-writes `state/kernel/lkg.json`, because the genesis tuple had `INIT_SHA=""` (body exit 78);
- it fills `body/` with `git archive HEAD seed`, because `body/GENESIS.md` was missing (body exit 70).

With WP21b, `init-root install --seed-source <repo|bundle>` makes all of this itself, through the real kernel:
- the genesis tuple, with a real seed sha and the built body;
- `lkg.json`, written by the kernel;
- the `body/` clone;
- `lineage/seed.git` and `lineage/body.git`.

## Done
**`install` (install.rs, cli.rs).**
- New flags: `--seed-source <path>`, `--seed-tag <tag>`, `--no-service`.
- **Step 7, `seed_bundle`:** writes `INIT_HOME/kernel/seed/<tag>.bundle`.
  - A directory is the owner's repository (its `.git`, or itself when bare). It is bundled with the hardened `$RG` prefix: `bundle create <tmp> refs/heads/seed --tags=seed-v*`.
  - A file must already be a bundle, and is copied.
  - Either way, `git bundle list-heads` must show `refs/heads/seed`. The tag is `--seed-tag` or the highest-semver `seed-v<x.y.z>` (numeric, so 0.10 > 0.9).
  - It works through a temp file plus rename. On a refusal, the temp file is removed by exact path.
- **Step 5, end: the kernel config template.** When the release ships no `config.toml`, the Root writes `kernel/config.toml` with `schema_version = 2` and `[sandbox] toolchain`/`developer_dir` from install.json. The broker's class rules compare these exactly, and the install-mode kernel needs them for its build.
- **Step 8, `install_seed` (sup.rs):** runs under `run/root.lock`, on a cold-started Root (no daemon, no egress).
  - It stores the kernel half of the tuple first (`tuple::kernel_part`: the `kernel/bin` blobs, `built_from` = `lineage/kernel.git` `main`).
  - It spawns `init-kernel install --from-bundle $INIT_HOME/kernel/seed/<tag>.bundle --seed-tag <tag>` through `zone::spawn_kernel` (profile `kernel`, exec-clean, fds 3/4, logs `logs/kernel-<seq>.stderr`).
  - It serves the control link with the normal `read_loop`/`handle`:
    - `hello` gets a boot block with reason `install`, the known `tuple_kernel` and an empty `tuple_body`;
    - after `ready`, the slot is `running`, so the kernel's `spawn{kind:"build"}` passes the broker (V1–V5, class rules, build profile) and its exit is relayed as `exited`;
    - then `store.put` and `lineage.append promotion{lkg:true}`.
  - **`tuple::on_promotion`** with no current tuple forms the **genesis tuple**: the installed maintainer kernel, the seed body (`sha`, `binary`) and its extension set (`components`, `set`). It appends `tuple`, `lkg` and `audited`. The promotion is signed `kernel:<12 hex of the installed init-kernel>`.
  - **Bounds while it runs:**
    - `kernel_hello_timeout_s`;
    - `hb_stale_s` after `hello`;
    - `INSTALL_TIMEOUT_S` = 1,800 s (the kernel bounds a cold build at 900 s);
    - the disk floor `quotas.disk_min_free_gib`.
  - Any bound trips `zone.kill_all()`, which kills only instances the Root holds (the kernel and its build).
  - It succeeds only on exit 0 with a current tuple that verifies. Afterwards nothing is held, the slot is `stopped`, and `state/root.json` and the view carry the tuples.
  - On failure the error names the exit code, the kernel log tail and `state/kernel/logs/install-build.log`. install.json is removed again (exact path) so `install` can be re-run; every earlier step is idempotent.
- **Steps 9–10 reordered.** `bin/init-root` and `install.json` are written before step 8, because exec-clean and the broker need them. The service file is skipped with `--no-service`, and the install report names the seed tag and the genesis tuple.

**Faster Root start (asked for by S1):**
- `owner.sock` now opens at once. Before, the Root verified the tuple's blobs on the main thread before serving `owner.sock`. That hashes the 154 MB debug kernel twice, about 25 s in a debug Root.
- The first `start` now runs on the supervisor thread.
- A panic in that window stays safe. `panic` sets the slot before its `kill_all`, and `start` re-checks the slot after the kernel spawn: a kernel spawned into `Panicked`/`Halted` is killed and never set running.
- The hashing itself is still slow in a debug Root (see Needs).

**Disk-saving copies** (quality guard: still a new inode, R-F3; the hash is verified after the copy):
- `util::copy_atomic`: `fs::copy` (an APFS clone on macOS) + mode + fsync + rename. Used for release binaries and for `tuple::install_kernel`.
- `tuple::put_file`: a store put by clone, hashed after the copy. Used for the kernel blobs.

A 154 MB debug kernel no longer costs two extra real copies per install.

**Slice-1 variant: `scripts/slice1/s1-install.sh <mock|realdry|real>`.** It is a new file, because s1.sh and drive.py belong to the S1 agent; it calls drive.py, zonecheck.py and collect.py read-only.
- It packages the seed (the maintainer step):
  - the **working-tree** `seed/` (tracked plus untracked, not ignored files), because WP13's boot code is not committed yet and HEAD's seed is still the WP0 skeleton; `S1_SEED_REV=<rev>` takes `git archive <rev> seed` instead;
  - plus `vendor/` cut to the crates in `seed/Cargo.lock` and `seed/ext/Cargo.lock`, as APFS clones;
  - plus `.cargo/config.toml` pointing at `vendor`.
- Crates that only build on Windows or Linux (`windows*`, `winapi*`, `linux-raw-sys`: 24 of 207) become stubs (manifest, empty lib, empty checksum list), as cargo-vendor-filterer does. The seed tree drops from about 290 MB to **131 MB**.
- It commits on branch `seed` with a fixed identity and date, tags `seed-v0.1.0`, bundles, then removes the temp repo.
- Then:
  - `install --seed-source <bundle> --no-service`, with the owner's real HOME (class rules) and the charter's `disk_min_free_gib = 5`;
  - it removes the build slot (a cache);
  - it checks that the genesis tuple, `lkg.json`, `bin/init-body` and `body/` agree;
  - the rest is s1.sh's: start, ready, the zone check, the owner task, evidence and redacted artifacts in `build/slice1-artifacts/install-<mode>/`.
- It writes **no** `lkg.json`, `bin/init-body` or `body/` by hand.
- Run dirs are `target/slice1-homes/i{m,d,r}`: 63-byte homes, kept apart from s1.sh's.

## Tests
All runs from `root/` with `CARGO_INCREMENTAL=0` and the default target (`root/target`).

| Command | Result |
|---|---|
| `cargo test --offline -p init-root --features test-fixtures` (before the clone change) | 63 unit + 9 wp20 + 6 wp21 + 2 **wp21b** + 8 wp22 + 4 wp23 + 5 wp24 + 3 wp25, all pass |
| after the clone change: `--bin init-root --test wp21 --test wp21b`, then `--test wp20 --test wp22 --test wp23 --test wp24 --test wp25` | 64 unit (one new) + 6 + 2, then 9 + 8 + 4 + 5 + 3, all pass |
| after the startup change (final tree): `--bin init-root` and every `--test` suite | 64 unit + 9 wp20 + 6 wp21 (3 runs) + 2 wp21b + 8 wp22 + 4 wp23 + 5 wp24 + 3 wp25, all pass. The first try failed 2 wp21 tests: the post-spawn guard also caught the legitimate retry from `Halted`, so it now fires only on a slot that changed to `Panicked`/`Halted` during the start. |
| `cargo test --offline -p init-root --bin init-root` (no features) | 61 pass |
| `cargo clippy --offline -p init-root --all-targets --features test-fixtures -- -D warnings`, and without features | clean |
| `bash -n scripts/slice1/s1-install.sh` | clean. Its seed packaging was dry-run once: 126 seed files, 183 crates, 24 stubs, 131 MB, 4.4 s. |
| the stubbed seed tree: `cargo metadata --frozen` and `cargo tree --frozen -e normal,build --target aarch64-apple-darwin` (vendored config, empty `CARGO_HOME`) | resolves; **0** stubbed crates in the host build graph (quality guard for the stubs) |

**The Slice-1 run through the install path** (`CARGO_INCREMENTAL=0 scripts/slice1/s1-install.sh mock`, 05:39–05:43, **pass**). Artifacts: `build/slice1-artifacts/install-mock/`, 48 files, 0 leaks after redaction.

| Step | Result |
|---|---|
| seed packaging | working-tree seed: 126 files, 183 crates + 24 stubs; bundle 18 MB; 14.5 s |
| `install --seed-source` | exit 0 in **172 s**. That is the real kernel in install mode: import, clone, and a **cold sandboxed build of init-body (wasmtime)**. Build slot 964 MB at the end. Free disk was 7,143 MB before and **5,452 MB at the lowest** (above 5.3 GB and above the Root's 5 GiB floor), and 6,353 MB once the slot was removed. |
| install check | genesis tuple `t3b940a04dee3`. `body.sha` = seed `17c029728e04…` = `lkg.json.sha` (reason `seed`); `body.binary` = sha256 of `bin/init-body` = `lkg.json.binary_sha256`; `body/GENESIS.md` and `body/.git`, `lineage/body.git` and `lineage/seed.git` all present. Records: `genesis, charter, promotion, tuple, lkg, audited`. |
| `start` | kernel running and body ready in 9.8 s. Both sandboxed (`sandbox_check` = 1), each with its own pgid. The body booted with `INIT_SHA` = the seed sha: no exit 78, no exit 70. |
| owner task | the model calls `write`: `hello.txt` = `INIT\n`, reply `done`, 2 model turns, 2 egress requests, 7,281 µUSD at the Root, 0 ceiling hits, `audit verify --full` ok. Lineage: `genesis, charter, promotion, tuple, lkg, audited, kernel_start, zone_start`. |

The first attempt (05:38) died in the harness, before install: a detached `git gc --auto` raced the `du` and `rm` of the temp seed repo. The fix is `-c gc.auto=0 -c maintenance.auto=false`.

`tests/wp21b.rs`:
- **`install_runs_the_kernel_in_install_mode_and_forms_the_genesis_tuple`** (about 15 s). It uses the **real** `kernel/target/k1/debug/init-kernel` (an APFS clone; skipped if absent), a tiny seed crate on branch `seed` with tags seed-v0.9.0 and seed-v0.10.0, and a decoy branch checked out as HEAD. It checks:
  - the bundle is named seed-v0.10.0;
  - `lineage/body.git` `main` and `lkg`, `lineage/seed.git` `seed` and `body/` HEAD all equal the tag's sha, and `body/GENESIS.md` is present;
  - `bin/init-body`, built under the build profile through the Root's broker, prints the tag's text, not the decoy's;
  - record order is genesis < charter < promotion < tuple, ending `tuple, lkg, audited`, with the promotion signed by the installed kernel;
  - the tuple's body sha is the seed sha (not `""`) and its binary is the built body; the kernel is the installed one, `maintainer`; the ext set is empty with `set: null`; the blobs and the tuple are stored;
  - `lkg.json` agrees (sha, `binary_sha256`, reason `seed`, `lineage_seq` = the promotion's seq);
  - `config.toml` `[sandbox]` equals install.json;
  - `state/root.json` is stopped, with 0 instances and all pointers on genesis; the view's `audited_good.body_sha` is the seed; every mark is gone;
  - a second install is refused (`already installed`).
- **`a_bad_seed_source_is_refused_and_install_can_be_rerun`:** a repo with no `seed` branch, then a junk file, each exit 1 before any kernel runs; install.json is removed and no bundle or temp file is left.
- **Unit:** `seed_tags_are_semver_and_the_template_matches_install_json` (semver ordering and rejects; the template parses as TOML with escaped paths).

Safety: the only processes are the tests' own `init-root` and what that Root spawned and holds. After the runs `ps` showed no test-home process. The test homes and `root/target/test-homes/wp2*` were removed by exact path.

## Deviations
1. **root.md §4.3 step 7's command is wrong.** `--branches=seed` means `refs/heads/seed/*`: git appends `/*` to a pattern with no glob character (git-rev-list(1)), so the branch is left out. The Root names `refs/heads/seed` instead. A test found this.
2. **Order.** `bin/init-root` and `install.json` are written before step 8 (exec-clean and the broker's V1–V5 and class rules need them). On a failed seed step, install.json is removed so `install` can be re-run.
3. **New `--no-service`.** A harness that runs `install` with the owner's real HOME, needed because `HOME_REAL` must equal the kernel's passwd home, writes nothing under `~/Library`.
4. **The kernel `config.toml` template** written by the Root is only `schema_version` plus `[sandbox]`. config.md has no fuller template yet; the kernel's defaults fill the rest.
5. **The genesis tuple's `kernel.built_from`** is now `lineage/kernel.git` `main` (it was `""`), for seedless homes too.
6. **The install-mode kernel** gets no `zone_start`/`zone_exit` records (the spec lists only `tuple`, `lkg`, `audited`). It is bounded by 1,800 s and by the disk floor.
7. **Seedless homes** (no `--seed-source`; the WP20–25 tests) keep WP21's kernel-only genesis at first start (WP21 deviation 3). Running the install-mode kernel at first start, when no tuple exists but a bundle does, is **not built**; a failed seed install is re-run with `install` instead.
8. **The Slice-1 seed is the working-tree `seed/`** (orchestrator, 05:0x). Its tuple sha is the packaging commit's, not a repo commit. The vendor is cut and stubbed as described above.

## Needs from others
- **S1 agent:** switch `s1.sh` to the install path. Take `s1-install.sh`'s seed packaging and its `install --seed-source … --no-service` (with the owner's real HOME). Then drop both harness workarounds: the hand-written `state/kernel/lkg.json`, and `git archive HEAD seed` into `body/` (S1.md gaps 1–2). Also drop copying a prebuilt body into `bin/init-body`/`.lkg`: the kernel builds them.
- **Doc owner (root.md §4.3):**
  - deviation 1 (the step 7 command);
  - deviations 2–3 (order, `--no-service`);
  - the config template (4);
  - the install-mode bounds (6).
- **Doc owner (CONVENTIONS §2 / the release process):**
  - who produces the seed repository (branch `seed`, `seed-v*` tags, its own `vendor/`) from the monorepo;
  - whether platform-only crates may be stubbed in it.
- **WP13:** commit the boot code, so the seed can come from a commit (`S1_SEED_REV=HEAD`).
- **root/Cargo.toml owner (the orchestrator):** consider `[profile.dev.package.sha2] opt-level = 3`. sha2 at opt-level 0 hashes about 13 MB/s, which makes every debug-Root start and rollback hash the 154 MB debug kernel slowly. root/Cargo.toml is outside this WP's paths.

## Left
- Kernel install at first start (deviation 7), and `install --upgrade` with a new seed bundle.
- The R1 disk pause during normal running (only the install run has the floor).
- The cost of a cold seed build per fresh home is about 3 min and about 1 GB of transient disk. A reusable template home (kernel `install --template`, supervise.md §4.8) would make repeated harness runs cheap.
