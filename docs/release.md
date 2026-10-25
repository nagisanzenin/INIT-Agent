# Cutting a release

*Owner: the maintainer (lane REL; the prebuilt bundle: lane UX-BUNDLE). Scripts: `scripts/release.sh`, `scripts/bundle.sh`, `scripts/get.sh`, `scripts/install.sh`, `scripts/uninstall.sh`, `scripts/release-e2e.sh`, `scripts/init`. The install path itself is root/root.md §4.3.*

A release is a directory of binaries and bundles built from one commit, plus a git tag. There are two ways to install it:
- **From source** (maintainers, and owners with Rust): `scripts/install.sh` builds the same directory on their Mac (or reuses a fresh one), and the install-mode kernel builds the body.
- **Prebuilt** (anyone on macOS 14+ arm64, no Rust): `scripts/bundle.sh` packs the release with a prebuilt body into one tar.gz (§2b), and `scripts/get.sh` installs it in about a minute (§5b).

## 1. Preconditions

1. `main` is clean: `git status --porcelain -- kernel root seed wit` prints nothing.
2. The version is the same in the four workspaces: `[workspace.package] version` in `kernel/Cargo.toml`, `root/Cargo.toml` and `tui/Cargo.toml`, and `[package] version` in `seed/Cargo.toml` (0.2.0 since 2026-10-23); refresh the lock files (`cargo metadata --offline` in each workspace) and regenerate `seed/self.json`, which records the body version (G1 fails on drift; `scripts/bundle.sh` checks it). `release.sh` refuses a mismatch. The seed tag is `seed-v<version>`, always plain semver: a pre-release label such as `preview` names the bundle (`v0.2.0-preview`), never the seed tag, because the Root and the kernel pick seed tags by `seed-v<a>.<b>.<c>`.
3. CI is green: `ci/local.sh` three times in a row with no flake (RELEASE_PLAN.md item 5), and the GitHub `fast-linux` job.
4. `CHANGELOG.md` has the version's entry.
5. Disk: at least 9 GB free (the release target dirs grow by about 4 GB; the floor is 5 GB).

## 2. Build: `scripts/release.sh`

```sh
scripts/release.sh            # builds, runs the release-mode test guard, assembles target/dist/v<version>/
```

| Flag | Effect |
|---|---|
| `--skip-tests` | skip the test guard (install.sh uses this; a release cut never does) |
| `--allow-dirty` | build a tree with uncommitted changes; `MANIFEST.json` records `"dirty": true`. The binaries are the working tree's; the bundles are always HEAD's |
| `--from-head` | build HEAD, not the working tree: HEAD's `kernel/ root/ seed/ wit/ tui/` are exported to `target/release-src/` (with `vendor/` symlinked) and built into each workspace's default target dir, so other lanes' uncommitted edits in the shared checkout never reach a release. `MANIFEST.json` records `"dirty": false`. The export goes again by exact path. Untracked files never count as dirty (they build nothing unless a tracked file names them, which then is dirty) |
| `--out DIR` | another output directory |
| `--jobs N` | `CARGO_BUILD_JOBS` (ci/env.sh caps it at 2 while many lanes share the machine) |
| `--serial` | one cargo at a time instead of the workspaces in parallel (a shared, loaded machine) |
| `--lean` | remove each release target dir (exact path) once its outputs are staged and its tests ran; with `--serial` the kernel's goes right after its build. Saves about 2.4 GB at the price of a cold next build |
| `--min-free MB` | override the free-space check (8 GB cold, 6 GB warm) when the tree is partly built |

What it does:
1. Preflight: rustc 1.97.x, `wasm32-wasip2`, `vendor/` present, the version check, the dirty check, disk.
2. Snapshots `root/crates/init-root/assets/{charter-template.toml,CONSTITUTION.md}`, the files `init-root` embeds, so install.sh's wizard writes a charter in exactly this binary's dialect.
3. Builds in parallel, `--release --offline --locked`, each workspace in its default target dir: `init-kernel`, `initctl` and `init-mockllm` (kernel/; the $0 mock the gate's G0 runs the seed's provider tests against, inside every T2 gate; it makes no network calls), `init-root` (root/), `init-body` (seed/). Then the template extension `count-lines` (seed/ext, `wasm32-wasip2`, dev profile: ext/authoring.md §3.4 says the dev profile is the only one used for extensions).
4. **Quality guard:** `cargo test --release --offline --locked` in `root/` and in `seed/`, as ci/local.sh runs them (no features). Both must be green.
5. Bundles, from HEAD, with a fixed identity and date so they are reproducible:
   - `src/<version>.bundle`: the kernel source (`kernel/` and `wit/`; kernel-selfmap reads `../../../wit`), one ref `refs/heads/release` (root.md §3.8.1). It holds no `vendor/`.
   - `seed/seed-v<version>.bundle`: the seed repository, branch `seed`, tag `seed-v<version>`. This answers WP21b's open question of who produces the seed repository from the monorepo: `release.sh` does, with `scripts/slice1/seedpack.py --kind repo --rev HEAD`. The tree is `seed/` at HEAD plus its own `vendor/`, cut to the crates in `seed/Cargo.lock` and `seed/ext/Cargo.lock` (APFS clones). Crates that build only on Windows or Linux (`windows*`, `winapi*`, `linux-raw-sys`) are stubbed, as cargo-vendor-filterer does; FIX4 checked that none of them is in the host build graph, and a build that needed one would fail loudly.
6. Assembles `target/dist/v<version>/`:

| Path | What |
|---|---|
| `init-root`, `init-kernel`, `initctl`, `init-mockllm`, `init-body`, `init-tui` | the release binaries (APFS clones of the target dirs' files); `init-tui` when `tui/` exists |
| `src/<version>.bundle` | the kernel source |
| `seed/seed-v<version>.bundle` | the seed |
| `docs/` | `docs/kernel/*.md`, the kernel docs the agent reads, including the selfmap recipes (root.md §3.2.2) |
| `wit/init-ext/` | the extension WIT |
| `ext/count_lines.wasm` | the template extension, built |
| `assets/` | the charter template and the constitution (step 2) |
| `MANIFEST.json` | version, commit, dirty flag, rustc, host, the bundles' shas, build and test times, and the sha256 and size of every file |

`init-root install --release <dir>` copies `init-kernel`, `initctl`, `docs/`, `wit/` and `src/` into `INIT_HOME/kernel/` (root.md §4.3 step 5) and imports `src/<version>.bundle` into `lineage/kernel.git` (step 6). It ignores the release-profile `init-body`: the install-mode kernel builds the body from the seed bundle itself, in the dev profile (kernel/supervise.md §3.6.2), so the agent's body is always built from its own source. The one exception is a bundle's prebuilt body (§2b), which is that same build, made once by the maintainer and checked bit for bit.

## 2b. The prebuilt bundle: `scripts/bundle.sh`

```sh
scripts/bundle.sh                 # release.sh --from-head --skip-tests, the prebuilt body, checks, the tar.gz
scripts/bundle.sh -- --with-tests # the same with release.sh's test guard
```

It writes `target/dist/v<version>-<label>/` (label `preview` by default, `--label` changes it): `init-v<version>-<label>-macos-arm64.tar.gz`, its `.sha256` and `latest.json` (version, file, sha256, bytes, commit, body sha256). Then it removes the release directory and its scratch homes by exact path (`--keep-release` keeps the release directory), so only the bundle stays.

1. **The release directory** of §2, built from HEAD (`--from-head`); `--reuse-release` takes an existing one that names HEAD and is clean.
2. **The prebuilt body.** It installs the release into scratch homes `target/ub/{r,h}` with `install.sh --reuse --yes --no-service --mock-upstream` (nothing outside `target/`; no model call), so the body is the one the install-mode kernel builds with the G0 build command from the seed tag, as on an owner's Mac. It checks that `bin/init-body`'s sha256 equals `lkg.json`'s and the genesis tuple's body blob, and that its commit is the seed tag's.
3. **G1.** `init-kernel selfmap` in gate mode on the seed tag's tree, with that body and the release's components, must reproduce the committed `self.json`; otherwise the agent's first gate would fail `gate.g1.self_json_drift`.
4. **The bundle.** The release directory, plus `body/init-body` and `body/BODY.json` (`sha256`, `commit`, `tag`, `profile: "dev"`, `rustc`), `scripts/{install.sh,init,uninstall.sh,get.sh}`, `VERSION` (`0.2.0-preview`), `README.md`, and `SHA256SUMS` over every other file. It is packed without macOS metadata.

Measured on 2026-10-23 for v0.2.0-preview: see build/status/UX-BUNDLE.md.

`install.sh` run from an unpacked bundle (it detects `SHA256SUMS` and `body/BODY.json` next to `scripts/`) verifies `SHA256SUMS`, needs no Rust, and passes `--prebuilt-body body/init-body --prebuilt-sha256 <BODY.json sha256> --prebuilt-commit <commit>` to `init-root install`. The Root checks the file's sha256, stages it in `INIT_HOME/kernel/prebuilt/` (Root-written, read-only to the zone), and the install-mode kernel installs it instead of building: the commit must be the seed tag's and the staged copy's sha256 the recorded one. After the install the Root checks that the genesis tuple's body blob is the prebuilt one, bit for bit, and removes the staging copy. The agent's later self-changes are built from its own source (`~/.init/body`, cloned from the seed bundle), as with a source install.

**Without Rust** the install records no toolchain (`--toolchain none`; install.json `toolchain: null`; the kernel config's `[sandbox] toolchain` is `INIT_HOME/kernel`, the same harmless readable directory the broker validates against when install.json has none, so the body spawns and a build finds no cargo and fails cleanly). Chat, tools and T0 self-evolution (skills, prompts, extension source) work; T1/T2 (rebuilding the body or the kernel) need Rust 1.97.1 with `wasm32-wasip2`. `init-root status` says so in its `self-evolution` line and its JSON `evolution` object, the Root publishes the same object in `INIT_HOME/root/status.json` for the agent, and install.sh prints the commands. INIT never installs Rust itself. The owner runs:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.97.1 --target wasm32-wasip2
init-root stop; init-root toolchain --set; init-root start
```

`init-root toolchain --set [DIR]` (Root stopped) checks `rustc 1.97.x` and the Wasm target and records the toolchain in `install.json` and the kernel config's `[sandbox]` together, which the broker compares. A rustup on PATH with 1.97.x and the target at install time is recorded directly.

### Checking the bundle: `scripts/bundle-e2e.sh`

```sh
scripts/bundle-e2e.sh [TARBALL]          # $0: get.sh with Rust into a temp home, `init`, one line, the mock's reply
scripts/bundle-e2e.sh --t2 [TARBALL]     # $0, 20-30 min: plus a no-op T2 promote through the shipped gate (G0-G3)
scripts/bundle-e2e.sh --pause [TARBALL]  # $0, about 30 s: Esc/Ctrl-C before the first token pauses, the resume is accepted
UBE_LIVE_KEYFILE=~/.config/init-agent/deepseek.key scripts/bundle-e2e.sh --live [TARBALL]   # COSTS MONEY (about $0.0005)
```

Each mode installs the bundle with its own `get.sh` into its own test dir under `target/test-homes/` (`ube`, `ub2`, `ubp`, `rels`), with no Rust on PATH, starts its own Root (`--no-service`), drives the installed `init --raw`, and checks that the owner's `~/.init`, `~/.init-root`, `~/.local/bin`, `~/.local/share/init`, `~/Library/LaunchAgents` and shell rc files did not change. The test dir is removed unless `--keep`. The default TARBALL is the macOS bundle in `target/dist/v<version>-preview/`. For the Linux bundle, run the same script with the Linux tarball in a fresh container (build/status/REL-033.md has the exact `docker run`). The verdicts go to `build/status/bundle-{e2e,e2e-t2,pause,live}.json`.

- **`--pause`** (loop.md §3.5.1, provider.md §3.8.1) runs on the strict DeepSeek mock, whose first turn stalls before any event. 2 s after `busy` the harness sends SIGINT to `init --raw`. It expects `paused · type to continue, or say what to do instead`, no `aborted`, and idle within 1 s. Then "Please continue." must get the mock's second reply. The resume must be exactly one request: no 400 in the strict mock's log, the `[init:paused]` note in it (and not in the first request), and the paused turn sent as `[no visible output]` with `"reasoning_content": ""`.
- **`--live`** sends one turn, "Reply with the word ok.", to the bundle's charter provider (api.deepseek.com), with no mock. `UBE_LIVE_KEYFILE` names a file holding the key. The harness copies it into the test Root's key file and never prints it. A watchdog stops the turn at `UBE_LIVE_CAP_MUSD` of settled spend (default 10000 µUSD = $0.01) or at 4 provider requests, read from the test install's `INIT_HOME/root/ceilings.json`. The verdict records requests, tokens and µUSD after the Root meters the request (up to 75 s). Before the test dir is removed, the key must be in exactly one of its files. Run it only with the owner's go and a cap.

Both modes are T0-only installs (`--no-rust`): neither needs a toolchain, so they skip the Rust download.

### The release profile

The three workspaces set `[profile.release]` explicitly, to the defaults:

```toml
[profile.release]
opt-level = 3
lto = false
codegen-units = 16
strip = "debuginfo"   # the default, stated: symbols stay, so panic backtraces keep function names
```

Why the defaults (measured 2026-10-20 on `init-root`, back to back under the same load; `build/status/REL.md`): `lto = "thin"` alone made the binary 1% larger and the build 24% slower; `lto = "thin"` with `codegen-units = 1` made it 12% smaller and the build 1.4–2× slower, and the kernel and the body, which compile Cranelift and wasmtime, would pay that most. The binaries are built once per install, are not CPU-bound (the Root's hashing crates already run at opt-level 3), and their size is irrelevant next to the disk floor, so neither pays. `panic` stays `unwind`. `strip = "symbols"` is not used, because crash logs need symbol names. Stating `codegen-units` and `strip` explicitly changes cargo's artifact hashes, so the first build after the change is a full one. The seed's release profile affects only `release.sh`'s check build of `init-body`; the agent's own builds use the dev profile.

## 3. Check: `scripts/release-e2e.sh`

```sh
scripts/release-e2e.sh        # $0; about 5 minutes plus the build if target/dist is not fresh
```

It runs the owner's path end to end inside `target/test-homes/rel/`, with the mock model:
1. a loopback `init-mockllm` (two turns: `write hello.txt`, then `done`);
2. `install.sh --yes --no-service --disk-floor 5 --mock-upstream 127.0.0.1:<port>` with `HOME`, `INIT_ROOT_HOME`, `INIT_HOME` and `INIT_BIN_DIR` in the test dir;
3. a `test-` key written by the harness (install.sh never handles keys), then `init-root start`;
4. the installed `init` command (no TUI, so `init-root attach`), one owner line, frames until the body is idle;
5. checks: the reply says `done`, `work/hello.txt` is `INIT`, 2 egress requests, 2 mock requests, no ceiling hit, `audit verify --full` exit 0;
6. `uninstall.sh` while the Root still runs (it stops the zone and refuses to move a running home), a `no` answer (nothing moved), then a typed `uninstall`: the homes and the `init` command land in the test Trash.

The verdict goes to `build/status/rel-e2e.json`. Signals go only to the processes the script spawned. `init-root` always runs with the passwd `HOME`, because the broker compares the kernel's `HOME_REAL` (the passwd home) with the Root's `HOME`; with `--no-service` nothing is written there.

## 4. Tag

```sh
git tag -a v<version> -m "INIT v<version>"     # after steps 2 and 3 are green on the tagged commit
git push origin v<version>                       # the orchestrator or the owner pushes
```

## 5b. The one-command install: `scripts/get.sh`

```sh
curl -fsSL "$INIT_DIST_URL/get.sh" | sh                                   # once releases are published
sh get.sh --from init-v0.2.0-preview-macos-arm64.tar.gz [-- <install.sh args>]   # a local bundle
```

POSIX `sh` (checked with dash). It refuses root, anything but macOS 14+ on arm64, and a Mac without the Command Line Tools; downloads over HTTPS only (or takes `--from` a local file); checks the archive's sha256 against `--sha256`, the `.sha256` file next to it, or `latest.json`, and refuses without one; unpacks into `~/.local/share/init/<version>/` (`INIT_APP_DIR` overrides the parent), checks `SHA256SUMS`, makes the tree read-only, and runs the bundle's `scripts/install.sh --yes` plus any arguments after `--`. The provider, key and model are for the first run of `init` (lane UX-KEYMODEL); until that lands it prints the `init-root keys set deepseek` step. **`INIT_DIST_URL` is a placeholder** (`https://releases.invalid/init`): the repo is private, and where releases are published is the owner's decision (docs/ux-v1.md OW-UX1). Trust on first install rests on HTTPS plus the sha256; there is no signature yet (OW-UX2).

## 5. What the owner runs

`scripts/install.sh`, then `init-root keys set deepseek`, then `init` (README.md). Notes:
- `install.sh` reuses `target/dist/v<version>/` when its `MANIFEST.json` names HEAD, a clean build, a still-clean tree and files that match their hashes; otherwise it runs `release.sh --skip-tests` (`--with-tests` keeps the guard, `--rebuild` forces a build).
- The wizard writes the charter from the release's `assets/charter-template.toml` with the owner's answers and checks it with the release's own `init-root charter check` before installing.
- After the install it writes `INIT_HOME/kernel/config.toml`: the model, its prices (kernel/config.md §4.10), soft budgets under the ceilings (`request_max_usd = min(1.00, day/5)`, so the D6 clamp `ceiling − request_max` stays positive), and the `[sandbox]` table the Root wrote, verbatim.
- It frees the build slot `INIT_HOME/cache/build/slot` (a cache the next build resets, supervise.md §3.6.2), installs `init` into `INIT_BIN_DIR` with ROOT_HOME baked in, and loads the LaunchAgent with `launchctl bootstrap` unless `--no-service`.
- `--mock-upstream` is a test hook: it writes `root.test_mode = true` and a loopback provider, and requires `--no-service`.

## 6. Upgrades (not supported yet)

`init-root install --upgrade --release <dir>` (root.md §4.3) replaces `init-root`, imports a new kernel source bundle and swaps the kernel, or adds a new seed bundle. A release that changes only docs needs no upgrade. Until `--upgrade` has its own end-to-end check, the supported path is uninstall (to the Trash) and install.
