# UX-BUNDLE: a prebuilt bundle and a one-command install for v0.2.0-preview (macOS arm64)

Status: **done** (2026-10-24 00:44, lane UX-BUNDLE). A user without Rust installs INIT with one command: 24 s for
get.sh (verify, unpack, install), 30 s to the first mock reply, on the shared M2. Nothing pushed or published.

## Done
- **Version 0.2.0** in the four workspaces (`kernel`, `root`, `tui` `[workspace.package]`, `seed` `[package]`) and
  their Cargo.lock files (only the workspace crates' version lines changed). Seed tag `seed-v0.2.0` (also in
  tests/real/*.sh). The pre-release label `preview` names the bundle (`v0.2.0-preview`), never the seed tag: the Root
  and the kernel select `seed-v<a>.<b>.<c>`. `seed/tests/boot.rs` reads the version from `CARGO_PKG_VERSION`.
  `seed/self.json` regenerated for the new body version (see Tests).
- `init-root install`'s "already installed" error no longer names the nonexistent `install --upgrade`.
- **Prebuilt body install** (root `install.rs`, kernel `kernel-supervise/src/install.rs`): `init-root install
  --prebuilt-body FILE --prebuilt-sha256 HEX [--prebuilt-commit SHA]`. The Root verifies the file, stages it in
  `INIT_HOME/kernel/prebuilt/` (Root-written, zone read-only); the install-mode kernel installs it instead of the
  seed build (commit = the seed tag's, copy's sha256 = the recorded one; a new inode, O_NOFOLLOW); after the install
  the Root checks the genesis tuple's body blob equals HEX (and the commit) and removes the staging copy.
- **No toolchain**: `--toolchain none`; the kernel config's `[sandbox] toolchain` is then `INIT_HOME/kernel`, the
  broker's own fallback (before: `toolchain = ""`, which the kernel refuses; an omitted key failed every body spawn
  with V2 "TOOLCHAIN: not absolute", found by the e2e). A build there finds no cargo and fails cleanly.
  `init-root toolchain [--set [DIR]]` (Root stopped) records a 1.97.x toolchain with wasm32-wasip2 in install.json
  and the kernel config's `[sandbox]` together.
- **Telling the user and the agent**: `init-root status` prints `self-evolution T0 · …` with the exact rustup
  command and the `init-root toolchain --set` step; `status --json` and the published `INIT_HOME/root/status.json`
  carry `evolution {tiers, toolchain, hint}`. install.sh prints the same at the end. INIT never runs rustup.
- `scripts/release.sh --from-head`: builds HEAD's export (`target/release-src/`, removed on exit) into the default
  target dirs, so other lanes' uncommitted edits in the shared checkout never reach a release; untracked files no
  longer count as dirty.
- `scripts/bundle.sh`: release (from HEAD) → prebuilt body from a scratch install-mode build (the kernel's own G0
  build command) → sha/commit/genesis checks → G1 (`init-kernel selfmap` gate mode reproduces the committed
  self.json) → `target/dist/v0.2.0-preview/init-v0.2.0-preview-macos-arm64.tar.gz` + `.sha256` + `latest.json`.
- `scripts/install.sh` bundle mode (detected by `SHA256SUMS` + `body/BODY.json`): verifies SHA256SUMS, needs no Rust,
  passes the prebuilt body; a rustup on PATH with 1.97.x + wasm32-wasip2 is recorded, anything else is T0.
- `scripts/get.sh` (POSIX sh, dash-checked): macOS 14+ arm64, not root, CLT present → HTTPS download or `--from`
  a local tarball → sha256 (flag, sidecar or latest.json; refuses without one) → `~/.local/share/init/<version>/`
  (read-only) → SHA256SUMS → `install.sh --yes [args]` → prints the `init-root keys set deepseek` step.
  `INIT_DIST_URL` is a placeholder (`https://releases.invalid/init`); publishing is the owner's decision (OW-UX1).
- `scripts/bundle-e2e.sh`: the no-Rust e2e (below). docs/release.md §1, §2, §2b, §5b, §6.

## Tests
- `cargo test --release --offline -p init-root --bin init-root -- install::` 9 passed (3 new: prebuilt staging,
  evolution tiers, no-toolchain config + `toolchain --set` editing); `-p kernel-supervise --lib -- install::` 3 new passed.
- `scripts/bundle.sh` at 051d2e5 (release `--from-head --skip-tests` 266 s warm, prebuilt body via the scratch
  install-mode build 629 s on a loaded machine, total 924 s): body sha256 = lkg.json = genesis tuple; commit = seed
  tag; **G1 reproduces the committed self.json**. The body sha256 `e46792de…0a30` came out identical in two separate
  scratch builds. Bundle `target/dist/v0.2.0-preview/init-v0.2.0-preview-macos-arm64.tar.gz`: 51 MB packed (54,298,129
  bytes), 130 MB unpacked, sha256 `7d682f6392fdfbbaa0ca7c8d935f7a7c86079c9d50f1449f9898cad0115816c9`.
- `scripts/bundle-e2e.sh` **PASS** (`build/status/bundle-e2e.json`): PATH `/usr/bin:/bin:/usr/sbin:/sbin` (no cargo,
  rustup or rustc), HOME/ROOT_HOME/INIT_HOME under target/test-homes/ube (61-byte ROOT_HOME), `--no-service`, mock
  `--mock-upstream`, a `test-` key. get.sh 24.3 s, start→ready 3.0 s, one line → "hello from the mock" 2.1 s,
  **get.sh → first reply 29.8 s** (target ≤ 90 s). Genesis body = the bundle's; install.json `toolchain: null`;
  `init-root status` prints `self-evolution T0 · …sh.rustup.rs…`; `status --json` and `INIT_HOME/root/status.json`
  carry `evolution.tiers = ["T0"]`. Snapshot diff of the real ~/.init, ~/.init-root (sealed/ not listed),
  ~/.local/bin, ~/.local/share/init, ~/Library/LaunchAgents and the shell rc files: **unchanged**.
- `seed/self.json` regenerated with the release `init-kernel selfmap --write --no-build` on the seedpack cut; diff =
  the two version fields, the Cargo.toml/Cargo.lock/tests/boot.rs entries and the digest.
- get.sh: `sh -n` and `dash -n` clean; run under /bin/sh by the e2e.
- Not run: the full ci/local.sh and the release-mode test guard (disk and time; another lane builds in parallel);
  `init-root toolchain --set` end to end (only its config editing is unit-tested); a real curl download.
- Cleanup: release target dirs (kernel/root/seed/tui `target/release`, `seed/target/wasm32-wasip2`),
  `target/dist/v0.2.0{,.logs}`, scratch homes and test homes removed by exact path; only the bundle dir stays. Free
  disk 9.9 GB at the end.

## Deviations
- ux-v1 §3.2.2's `bin/` split, minisign, `init-fetch`, the launcher binary `init-cli`, the background warm-up and
  the repro check are not built. The bundle is the flat release dir plus `body/`, `scripts/`, `SHA256SUMS`.
- The prebuilt body is made by a scratch install-mode build on the maintainer's Mac (so it is exactly the kernel's
  build), not by a separate build script.
- The genesis tuple records the prebuilt body's sha like a built one (no `built:"prebuilt"` field); the install log
  and install.sh say "prebuilt body, verified bit for bit".
- Only targeted release-mode tests ran (disk/time; another lane builds debug in parallel), not the full ci/local.sh.

## Needs from others
- UX-KEYMODEL: its TUI first-run setup (0e8be3c) is in the bundle; get.sh now points to it. Showing
  `status.json` `evolution` (T0 vs T1/T2) in the TUI status bar would help (ux-v1 §3.5.4 "evolve off").
- REL-DOCS: README quickstart could add the one-command path (`sh get.sh --from <bundle>`) once the owner decides
  the release host.

## Left
- Owner: release host and signing (OW-UX1/2); then set `INIT_DIST_URL` and publish the tarball, `.sha256`,
  `latest.json` and `get.sh`.
