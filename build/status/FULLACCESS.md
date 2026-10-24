# FULLACCESS: full access by default (ADR-012, v0.3.1)

Lanes append their own section; edit only yours.

## Docs lane (FULLACCESS-docs, 2026-10-24)

### Done
- **Seed:** `access_line("full")` is the ADR-012 line (awareness.rs, its tests, awareness.md table). The git sync
  sets `GIT_CONFIG_GLOBAL=/dev/null` on every git call (gitsync.rs), with a test that an owner global config which
  breaks every fetch is ignored (`owner_global_git_config_is_ignored`; it fails without the fix).
- **TUI:** `--workspace` (excludes `--full`) sends `workspace.set{cwd, mode:"workspace"}`, or `full.set{on:false}`
  when the cwd cannot be read. The status bar shows `FULL` (red bold) or `WORKSPACE` (yellow), never dropped, reverse
  video under `NO_COLOR`. New `/full` help and usage text. The fake Root in tests/common gains `charter_full`; new tests
  `workspace_flag_confines_and_full_needs_no_confirmation_by_default` (unit) and `fullaccess_default_and_workspace_flag`
  (fake Root); the launcher test also passes `--workspace`; the e2e test against the real Root uses `--workspace`.
  Golden `wire9_access_status.txt` re-blessed (WORKSPACE added, one NO_COLOR case).
- **Scripts:** `scripts/init` documents `--workspace` and prints a `launchctl bootstrap` hint for a removed job;
  `install.sh` prints the access default and the three ways to confine it.
- **Docs:** docs/adr/012-full-access-by-default.md (new); README (one line under the install notes, the sandbox row,
  the Linux limit); docs/guide.md (daily use, safety model 1 and 7, Linux requirement); docs/tui.md (flags,
  `workspace.set` mode, the badge); CHANGELOG v0.3.1; build/status/RED-TEAM.md §6 (rows plus RT-FA-1…4).
- **Version 0.3.1** (as 3bd6178): kernel/root/seed/tui Cargo.toml and their Cargo.lock member entries (seed/ext's lock
  pins no workspace version), get.sh usage comment, `SEED_TAG=seed-v0.3.1` in tests/real/{evo_test,m7,real}.sh.
  install.sh reads the version from root/Cargo.toml or MANIFEST.json, so it needed no change.

### Tests
- seed: `cargo test --offline` 350 pass, 5 ignored (unit 322, boot 19, self_md 9); `cargo clippy --offline
  --all-targets -D warnings` clean (before the version bump).
- tui: `cargo test --offline` all pass (unit 79, proto 15, tty 4 + 1 ignored, e2e_root 3 against fa-code's
  in-progress `root/target/debug/init-root`); clippy `-D warnings` clean.
- `cargo metadata --offline --locked` passes in kernel, root, seed, tui and seed/ext after the bump.

### Deviations
- `--workspace` without a readable cwd sends `full.set{on:false}` (there is no `workspace.set` to carry the mode).
- docs/tui.md and docs/awareness.md were edited too (they quote the flags and the access line).
- Red-team §6 rows cite fa-code's self-test IDs from its kernel/sandbox.md draft (S-A11…S-A28); their results are
  fa-code's to record here.

### Needs from others
- **CI lane:** regenerate seed/self.json (G1 bless) for the seed edits and the 0.3.1 body version.
- **fa-code:** the TUI relies on `workspace.set{mode:"workspace"}` and on `full.set{on:true}` answering without
  `root.confirm` under a full-default charter (checked by e2e_root against the current build).

### Left
- None in this lane. Not run: a live session in full mode, the Linux full layout natively (fa-code/CI).

## Code lane (fa-code: root/sandbox-profiles, root/crates/init-root, docs/root/root.md, docs/kernel/sandbox.md)

### Done (commits 54c5124, 17aa758)
- **Sandbox crate:** `Grant.full` (live body only; `grant_for` never gives it to members or probes); `FULL_READ_DENY`
  (`.config/init-agent`) + charter `read_deny`; `FULL_WRITE_DENY` (`Library/LaunchAgents`, `.config/systemd/user`,
  `.config/systemd/user.control`, `.local/share/systemd/user`) + `<bin_dir>/init`, `<bin_dir>/init-tui`.
  ROOT_ALLOWS gains the `Z_FULL` group (contiguous, emitted only for a full grant, no LaunchServices/AppleEvents);
  new `profiles/zone-tail-full.sb` (Z1 incl. Unix sockets in ROOT_HOME, Z2, Z3a, Z4, Z5, Z7). `render` picks the tail
  by `grant.full`; full params: `Z_FULL=/`, no `Z_WRITE`/`Z_EGRESS_SOCK`/`Z_HOME`. A full grant with another grant is V5.
- **Linux plan** (`linux::compose`, full Body only): unshare without `--net`; `--bind / /`, fresh `/proc` `/dev`, host
  `/tmp`; deny-entry ancestors pinned (below), `--tmpfs INIT_HOME` + the normal body binds; ROOT_HOME tmpfs+ro; read-deny
  dir → ro tmpfs, file → `/dev/null`; write-deny → ro self-bind; `/run/user/<uid>/systemd` ro tmpfs, `bus` → `/dev/null`;
  no `--remount-ro /`. `Layout` gains `write_deny`, `home`. Root `sys::linux_seccomp(full)` drops the socket-domain rule.
- **Root:** charter `sandbox.mode` ("full" default, skipped in the digest at its default, rule **C18**); template
  documents it. access.rs: session override `Option<bool>`; `workspace.set{cwd, mode?:"workspace"}`; `full.set` on with
  no confirmation under a full charter, typed confirmation under a workspace charter; off = `Some(false)`; `view()` adds
  `mode`; `deny_full()`/`write_deny_full()`; Linux keeps the workspace only while full. zone.rs: lists by `grant.full`;
  full body env `HOME`=owner home, no `CARGO_NET_OFFLINE`, `PATH`+`:<home>/.local/bin:<home>/.cargo/bin`; on Linux the
  Root creates the three user unit dirs (empty) before a full spawn. `init-root status` prints the access line.
- **Docs:** root.md §3.3.1 (ROOT_ALLOWS block), new §3.3.2a (full tail block + params + residuals), §3.4.1 (Linux full
  plan), §3.12 (`sandbox.mode`, C18, digest, apply), §3.17.2–3, §4.2 frames, RR-39/RR-39b; sandbox.md §3.1.1–§3.1.2,
  §7 invariants, §8 rows S-A10…S-A28.

### Tests
- `cargo test --offline -p init-sandbox-profiles`: lib 21, params 4, doc_drift 2 (5 root.md blocks), selftest
  `sandbox_selftest_on_machine_half`: **348 pass, 0 fail, 11 skipped (disposable-env / S-M38), 359 checks**. Full rows
  all PASS: S-A10–S-A10c, S-A11–S-A11c, S-A12–S-A12f, S-A13–S-A13f, S-A14–S-A14c, S-A15–S-A15e, S-A16–S-A16c,
  S-A17, S-A17b, S-A18–S-A18e, S-A19–S-A19d, S-A21–S-A26b, S-A27 (temp dir under `/var/folders`, removed by exact path),
  S-A28. Workspace rows unchanged.
- `cargo test --offline -p init-root --features test-fixtures`: **308 pass, 0 fail, 3 ignored** (incl. tests/access.rs 5:
  new `default_full_access_and_session_choices`, `full_body_writes_the_home_not_the_root`; updated RR-39 test runs
  under `mode = "workspace"`, e2e test sends `mode:"workspace"`). Plain `-p init-root`: **221 pass, 0 fail, 12 ignored**.
- `cargo clippy -D warnings`: init-sandbox-profiles all targets clean; linux.rs clean via a macOS type-check shim
  (`root/target/linuxcheck`, not committed). init-root: no finding in changed lines; pre-existing findings on main
  (egress/money.rs dead code, sealed/*, sys.rs:529 casts, zone.rs `mut argv` on macOS, tests/wp22.rs) left alone.

### Deviations
- Full tail Z1 also denies `network-bind` of Unix sockets in ROOT_HOME (stricter than the brief).
- A session's confinement (`full: false`) is persisted in `state/access.json`, so a Root restart never widens access;
  a confirmed `/full on` under a workspace charter is not persisted (old rule: full never survives a restart).
- `/full on` under the full default clears the override (None) rather than `Some(true)`, so a later charter switch to
  `"workspace"` applies.
- Linux: missing deny entries get no mount (a mount point would create them in the owner's tree); entries are
  canonicalized. Per fa-linux's measured rename bypass, every ancestor of each deny entry below the home (below `/` for
  entries outside it; never ancestors of INIT_HOME/ROOT_HOME) is self-bound rw before the INIT_HOME tmpfs (EBUSY on
  rename), and systemd's other unit dirs were added to `FULL_WRITE_DENY` and pre-created by the Root on Linux.
- Linux member `fleet.workspace_write` grants are disabled (they would be a workspace-mode write the Linux plan refuses).
- Two pre-existing Linux-only clippy lints in linux.rs fixed (redundant closure; `sealed` moved above the test module).
- macOS residual: renaming an ancestor of a protected path (e.g. `~/.local/bin` itself, S-A26) is allowed; Seatbelt is
  path-based, so the launchers' paths stay denied (DoS only).

### Linux items needing Docker qualification (exact test names)
- init-sandbox-profiles `linux::tests::`: `linux_full_plan_binds_the_host_root_and_overlays_the_root_protected_set`,
  `linux_full_pins_deny_ancestors_below_the_home`, `linux_full_overlays_hide_the_user_service_manager`,
  `linux_broad_grants_and_protected_reads_fail_closed`, plus the existing `linux_plan_requires_network_seccomp_and_exact_write_mounts`,
  `linux_body_plan_omits_an_unbound_lane_socket_only`, `frozen_helpers_cannot_write_artifacts_and_sealed_mounts_are_narrow`,
  `offline_kernel_plan_omits_absent_gateway_and_live_plan_pins_only_socket`, `linux_kernel_vault_is_writable_without_exposing_init_home_parent`,
  `linux_without_a_toolchain_the_kernel_placeholder_adds_no_mount` (Layout/foundation changed).
- init-root `sys::linux_zone_tests::linux_seccomp_memfd_is_sealed_and_mount_sources_are_pinned` (now also checks the full
  program), `linux_seccomp_x32_guard_keeps_architecture_validation_first`, ignored `linux_zone_real_isolation`,
  `linux_sealed_real_isolation`; init-root `tests/access.rs` `charter_disables_full_mode_and_home_reads` and
  `default_full_access_and_session_choices` (Linux branches: `"charter"` reason, `none`/`platform` when confined).
- Not written: a real-isolation full-mode test (fa-linux offered to add it to `sys.rs` linux_zone_tests, incl. the rename case).

### User-facing strings (for docs)
- `init-root status`: `access: FULL (default) · workspace <ws|none> · to confine it: init --workspace, /full off, or
  sandbox.mode = "workspace" in <ROOT_HOME>/charter.toml`; `access: FULL (this session) · …`; `access: WORKSPACE
  (this session) · … · /full on, or init without --workspace, gives full access`; `access: WORKSPACE (default) · …`.
- Notices: `[root] workspace: <path> · access FULL: <FULL_TEXT>`; `[root] no workspace (nothing has /undo) · access
  FULL: <FULL_TEXT>`; `[root] full access on: the agent may <FULL_TEXT> (from its next restart).`; `[root] full access
  off: the agent works in its workspace (workspace mode) from its next restart at an idle point`; confirmation (workspace
  charter only): `Full access lets the agent <FULL_TEXT>.<snap> Type \`confirm\` to turn it on.`; refusal: `full access
  is disabled by the charter (sandbox.allow_full = false)`. FULL_TEXT = "read and write anywhere you can and use the
  network, except INIT's Root files (~/.init-root, its service, INIT's launchers); writes outside the start folder are
  not undoable". `/full` line: `[root] access <a>[ · FULL] · workspace <ws> · default <mode> · read_home … · allow_full …`.
- AccessView gains `"mode": "full"|"workspace"` (the charter default).

## Linux lane (FULLACCESS-linux, 2026-10-24)

### Environment
Container `fa-build`, volume `fa-vol` (kept for the release lane's Linux bundle): `debian:bookworm-slim`, `--privileged
--init`, uid 1000 (`dev`), Docker Desktop linuxkit kernel 6.10.14 aarch64, bubblewrap 0.13.0 built from the
sha256-checked upstream tarball, rustup 1.97.1 + wasm32-wasip2 + clippy. The repo is mounted read-only at `/src`;
`git archive HEAD` is synced into `/work/repo` (plus the untracked `vendor/`), and the target dirs are on the volume.

### Finding (fixed by fa-code in 54c5124)
A bwrap bind follows its dentry. In the first full plan, `mv ~/.local/bin x && mkdir ~/.local/bin && echo X >
~/.local/bin/init` replaced the host's launcher, and the same trick worked for `~/.config/systemd/user/init-root.service`
(measured with bwrap 0.13.0). The fix: every ancestor of a deny entry is pinned as a mount point (`--bind A A`, outer to
inner, before the INIT_HOME tmpfs), so a rename returns EBUSY while the contents stay writable. Also flagged and now
covered: the user unit dirs `~/.config/systemd/user.control` and `~/.local/share/systemd/user`. macOS Seatbelt
literals are path-based: under sandbox-exec, creating the literal again after an ancestor rename is still denied
(moving `~/Library` away is only a DoS).

### Tests on Linux (at 17aa758, then with 8bbd51b)
- `root/sandbox-profiles` `cargo test --offline`: 24 pass, 0 fail (lib 20, incl. the linux full-plan/pins/overlay tests).
- `init-root` `cargo test --offline`: 227 pass, 0 fail, 10 ignored. With `--features test-fixtures`: 298 pass, 0 fail
  (bin 229, integration tests 69, incl. tests/access.rs).
- Real isolation (`--ignored --test-threads=1 linux_`, the CI step "linux zone qualification"): 5/5 pass, among them
  `linux_zone_real_isolation`, `linux_sealed_real_isolation`, pidfd, memwatch and the new full-mode test; the new test
  passed 3 more runs in a row.
- New `sys::linux_zone_tests::linux_full_body_real_isolation` (8bbd51b) on the real compose() plan + linux_seccomp(true):
  | Evidence | Result |
  |---|---|
  | write a fresh `/var/tmp/init-full-<pid>/file`, the home, `~/.local/bin/other-tool`, `~/.local/share/x` | allowed, seen on the host |
  | TCP to a 127.0.0.1 listener of the test (host network) | connected, round trip ok |
  | ROOT_HOME: list / read secret / write / mkdir / connect `run/owner.sock` / rename | empty; all refused; host unchanged |
  | systemd unit file + dir, `user.control`, `.local/share/systemd/user`: write, unlink, new unit, drop-in dir | refused; host unchanged |
  | `<bin_dir>/init`, `init-tui`: write, unlink, rename | refused; host unchanged |
  | ancestor rename (`.local/bin`, `.local`, `.local/share{,/systemd}`, `.config{,/systemd}`, the unit dirs) | EBUSY; this check failed before the pins (mutation check) |
  | `~/.config/init-agent` | empty, and the key is unreadable |
  | `kill` of the test process (the Root's stand-in) and of a `sleep` we spawned outside the zone | ESRCH; the sleep survived |
  | `--net` in the plan | absent (asserted on the argv) |
  | INIT_HOME's kernel dir | read-only (the body's plan is unchanged) |
- Not covered in the container: `/run/user/<uid>/{systemd,bus}` do not exist there, so only the argv unit test covers them.
- `cargo clippy --all-targets -D warnings` on Linux is **not clean**. The warnings were there before ADR-012: dead code in
  `init-sandbox-profiles` sys.rs (`dtablesize`, `exec_clean_in_child`, `set_exec_clean`, which only the macOS selftest
  uses) and about 30 warnings in init-root. None comes from the new test. ci/local.sh does not run clippy for root.

### Commits
8bbd51b (root/crates/init-root/src/sys.rs only). Not pushed.

## Release (lane FULLACCESS-release, v0.3.1-preview)

### Progress
- 18:21 both bundles started from e4215ed (fa-ci's self.json bless); macOS on the host, Linux in `fa-build`.
- 18:47 macOS bundle done (1516 s), 18:48 Linux bundle done (1639 s); both G1 ok; latest.json merged.
- 19:09 live DeepSeek smoke PASS (reply `ok` in 2.2 s, 1 request, 543 µUSD); access FULL (default).
- 19:19 macOS e2e: every functional check passes, the 390 s time bound fails (Rust download at about 250 KB/s).
- 19:26 Linux e2e (fresh Ubuntu 24.04): every functional check passes, the time bound fails (same network).
- 19:58 macOS no-op T2 (no knobs): job succeeded, G0–G3 + handoff pass (promote 1089 s); only the time bound fails.
- 19:59 / 20:00 network back to about 4 MB/s: macOS e2e and Linux e2e rerun, both **PASS** (all checks).
- 20:00 T2 rerun started for a verdict with every check green.
- 20:19 T2 rerun **PASS** (all checks); docs and verdicts committed.

### Bundles (binaries from e4215ed)
Both built by `scripts/bundle.sh` from e4215ed (fa-ci's G1 bless; MANIFEST.json and latest.json `commit`). HEAD moved
on to 1a8e016 (fa-ci's fix, `kernel/crates/init-testkit/tests/acceptance_root.rs` only: an integration test that no
binary, the seed or the body contains) and 9895477 (status); `git diff e4215ed 1a8e016 -- kernel root seed wit tui`
is that one test file, so nothing was re-cut or rebuilt. The kernel source bundle in each tarball is e4215ed's.

| Target | sha256 | Bytes | Body sha256 | Build |
|---|---|---|---|---|
| macos-arm64 | `45d99bb1139579c5b91e24406d653bf2bc2bbe45fd6c484956d157ff96bbdaf9` | 54,669,061 (131 MB unpacked) | `af051f10…7f94fa` (seed commit 939d9785) | on the M2 next to fa-ci's full CI: release 741 s, prebuilt body 750 s, total 1516 s |
| linux-arm64 | `54ef2217a2c3c54b5c6331489d2a89bfdd6df8c3643e94ac231122ef35509451` | 66,557,953 (194 MB unpacked) | `e5835983…b47f7d` (seed commit 76e1c80c) | in `fa-build` (debian:bookworm-slim, `--privileged --init`, uid 1000, bwrap 0.13.0, rustup 1.97.1 + wasm32-wasip2); a `git clone` of the checkout at e4215ed into the volume `fa-vol` plus the untracked `vendor/` rsynced in: release 1073 s, body 550 s, total 1639 s. Needs glibc ≥ 2.36 |

Both: G1 reproduced the committed `seed/self.json`; body sha = lkg.json = genesis tuple; body commit = the
`seed-v0.3.1` tag. **latest.json**: the macOS bundle's `version/target/file/sha256/bytes/commit/body_sha256` plus
`sha256_<target>`, `bytes_<target>`, `body_sha256_<target>` for macos-arm64 and linux-arm64 (merged from the two
per-target files `bundle.sh` wrote; same version and commit asserted).

### Verification (on the published assets)
| Check | Result | Times |
|---|---|---|
| macOS `scripts/bundle-e2e.sh` (fresh temp home, PATH `/usr/bin:/bin:/usr/sbin:/sbin`, get.sh installs Rust 1.97.1 + wasm32-wasip2 + clippy into the temp home, mock), first run | every functional check passed; **FAIL** on `install_with_rust_plus_first_reply_le_390s` only: the network ran at about 50–250 KB/s, so rustup's download took 31 min | get.sh incl. Rust 1908 s, start→ready 2.1 s |
| the same, rerun at about 4 MB/s | **PASS** (all checks): genesis body = prebuilt, toolchain under the temp home's `.rustup`, `status --json` and the published view say T2, reply from the mock. **Access:** `init-root status` prints `access: FULL (default) · workspace none · to confine it: init --workspace, /full off, or sandbox.mode = "workspace" in …/charter.toml`; `status --json` `access` = `{"access":"full","full":true,"mode":"full","allow_full":true,…}`; the body's boot block in its first model request says `- access: FULL: read and write anywhere the owner can, network allowed · …`. The published view (`INIT_HOME/root/status.json`) has no access field (it never had one) | get.sh incl. Rust 42.0 s, start→ready 1.9 s, get.sh→first reply 44.7 s |
| macOS `scripts/bundle-e2e.sh --t2`, no knobs, first run (slow network) | job `succeeded`, G0 ✓ G1 ✓ G2 ✓ G3 ✓ handoff ✓; only the time bound failed (get.sh 1177 s) | promote 1089 s: G0 657 s, G1 1.4 s, G2 21 s, G3 404 s, handoff 4.2 s |
| macOS `scripts/bundle-e2e.sh --t2`, no knobs, rerun | **PASS** (all checks): job `succeeded`, G0–G3 + handoff pass; toolchain in the temp home; owner state unchanged | get.sh incl. Rust 39.5 s; promote 1037 s (17.3 min): G0 580 s (cold build, tests, clippy), G1 1.2 s, G2 20 s, G3 430 s, handoff 4.6 s |
| Linux e2e: fresh `ubuntu:24.04` (no Rust: `~/.cargo`/`~/.rustup` absent, nothing on PATH; apt python3 git libcap2 curl ca-certificates; bwrap 0.13.0 binary copied from `fa-build`), `--init --security-opt seccomp=unconfined --security-opt systempaths=unconfined`, not privileged, the same `scripts/bundle-e2e.sh` with the Linux tarball | first run: functional checks pass, time bound fails (get.sh 1701 s, same network). Rerun in a new `fa-e2e`: **PASS** (all checks); `access: FULL (default)`, `status --json` access `full`, the boot block's FULL line; toolchain `…/ube/home/.rustup/toolchains/1.97.1-aarch64-unknown-linux-gnu` | rerun: get.sh incl. Rust 54.7 s, start→ready 0.5 s, get.sh→first reply 55.7 s |
| Live DeepSeek smoke on the macOS bundle | **PASS** | get.sh 14 s, first-run setup 10 s, reply `ok` in 2.2 s |

- Observation (Linux, by design per the code lane): before a full spawn the Root creates the empty unit dirs
  `~/.config/systemd/user`, `~/.config/systemd/user.control` and `~/.local/share/systemd/user` in the **real** home
  (`pwd` home), also for a test install with its own ROOT_HOME; in the e2e container they appeared in `/home/u`. The
  macOS host is unaffected (the bundle-e2e snapshot does not list `~/.config`).
- The bundles' own README.md is e4215ed's (T2 timing "about 16 min … 26 min", "1,870 CI tests"); the repo README has
  the measured 10–16 / 17–26 min and 1,897.

### DeepSeek smoke (as REL-030; one run, $0.000543 of the $0.10 cap)
- Temp home `target/test-homes/rels/` (ROOT_HOME 62 bytes), `get.sh --from <macOS bundle> --no-rust -- --no-service
  --disk-floor 5`, no mock (charter provider `api.deepseek.com`), `init-root start --foreground` (our child).
- The installed `init` in a pty (140×45): provider `1` → the key typed from `~/.config/init-agent/deepseek.key` into the
  hidden field (masked, `•`) → `✓ key stored for deepseek` → model `1` → `✓ model deepseek/deepseek-flash: selected`
  (steps at 4.2 / 4.4 / 6.0 / 6.2 / 6.4 s) → Ctrl-D. Key file 0600, 35 bytes.
- One line through `init --raw`: "Reply with the word ok." Watchdog on settled spend (`money.total_spent_musd` ≥ 30,000)
  or ≥ 6 model requests, polled every 0.5 s: did not fire. Reply **`ok`** in 2.2 s, **1 request**, 5,612 in / 209 out,
  **543 µUSD**, 0 reserved left.
- **Money:** 543 µUSD (≈ $0.0005) in total. DeepSeek balance (`/user/balance`, key in-process only): $11.98 before and
  after (the API reports cents).
- **Key hygiene** (`grep -rlF -f <keyfile>`, counts only): the temp home before deletion (22,481 files, Trash included)
  = 1 file, the 0600 `keys/providers/deepseek` (in the temp home's Trash after the installed `uninstall.sh`). Every
  file this lane created (scratch logs, pty screen bytes, attach frames, egress stats, both bundles, latest.json,
  build/status, the release notes, the public mirror) = 0.

### Owner state
Snapshot (names, sizes, mtimes, modes; names only for the paths the running owner install rewrites on timers) of
`~/.init`, `~/.init-root` (sealed/ unlisted), `~/.local/bin`, `~/.local/share/init`, `~/Library/LaunchAgents` and the
shell rc files: unchanged across every e2e run and the smoke run (each harness diffs before/after), and between the
lane's start (18:19) and its end: see "Cleanup". Nothing of the owner's live install was written or signalled.

## CI lane (FULLACCESS-ci, 2026-10-24)

### G1 bless
- e4215ed: `seed/self.json` regenerated with `INIT_SEED_G1_BLESS=1` (body_version 0.3.1, the new access line;
  `body_tokens` 414,262 → 414,676). Write mode, then gate-mode G1 on the result: pass, 0 warnings, so the two modes
  agree. Build stage 50 s warm, write 16 s, 75 s in total.

### ci/local.sh on macOS (from e4215ed, 18:20–19:30, 70 min; load up to 38 next to the macOS bundle build)
| Step | Time | Passed / failed / ignored |
|---|---|---|
| wit + constitution copy drift | 0 s | identical (3 checks) |
| kernel, root, seed build | 23 s, 10 s, 8 min 24 s | ok |
| kernel test | 22 min 48 s | 881 / **1** / 12 (A6, below) |
| root test | 2 min 32 s | 249 / 0 / 12 |
| root test (test-fixtures) | 16 min 35 s | 308 / 0 / 3 |
| seed test | 9 min 23 s | 350 / 0 / 5 |
| ext build (wasm32-wasip2), ext test (host) | 0 s, 4 s | ok, 3 / 0 / 0 |
| seed G0 + G1 | 3 min 3 s | 1 / 0 / 0 (G0 kernel profile pass in 119 s, G1 pass, 0 warnings) |
| promote under Seatbelt (A3) | 1 min 21 s | 3 / 0 / 0 |
| tui build, tui test, tui e2e (real Root) | 1 s, 22 s, 14 s | 98 / 0 / 1, 3 / 0 / 0 |
| sealed pack s1 lint, tests, templates | 0 s, 82 s, 3 min 18 s | ok, 52 unittest OK, 56 templates 0 failed |
- **Cargo total: 1,896 passed, 1 failed, 33 ignored.** After the fix the kernel step reran in full (13 min, 19:30–19:43):
  882 / 0 / 12, so the totals are **1,897 passed, 0 failed, 33 ignored**, and every non-cargo step passed.
- Flakes: none. kernel-selfmap s08/s09/s12 and the rsafe dry-run passed in the full run; no isolated rerun was needed.

### Fix
- 1a8e016: `kernel/crates/init-testkit/tests/acceptance_root.rs` A6 failed at step 4 ("TCP to the mock's loopback port was
  not refused": `nc -z` succeeded). Not a flake: A6 installs the real Root with a charter that had no `[sandbox]`, so since
  ADR-012 the body ran with full access, where loopback TCP is allowed by design. A6 checks the workspace-mode
  invariants, so its charter now pins `sandbox.mode = "workspace"`; full mode's rows stay in sandbox.md S-A10…S-A28. The
  isolated rerun passed (26 s), then the whole kernel step. No `seed/` change, so no re-bless.
- Not changed: `rustfmt --check` reports pre-existing drift in that file (watchdog call chains), the same before the fix.

### Pushed
- `git push origin main`: 89953ba..1a8e016 (13 commits), no force.
