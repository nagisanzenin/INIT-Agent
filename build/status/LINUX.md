# LINUX: INIT on Linux as a supported platform (lane LINUX, 2026-10-24)

Status: **done, with the gaps listed** (see "Verified matrix"). All work used the local Docker VM at $0: no Modal, no cloud, no real model
calls. The owner's live `~/.init` and `~/.init-root` were never touched.

## Environment
- Build and test container: `debian:bookworm-slim` (Debian 12), Docker Desktop's linuxkit kernel 6.10.14, aarch64.
  `--privileged --init`; every test ran as uid 1000. Installed: bubblewrap 0.13.0 built from the upstream release
  (sha256 checked), rustup 1.97.1 with wasm32-wasip2, gcc 12, python3, git 2.39.5.
  The cargo target lived on a 5 GiB tmpfs (RAM). The repo was mounted read-only and rsynced into a named volume.
- e2e container: a fresh `ubuntu:24.04`, no Rust, a normal user. It got python3, git and libcap2 from apt, plus the
  bwrap 0.13.0 binary built above.
- Lesson: without `--init`, the container's pid 1 (`sleep`) never reaps. Zombie bwraps then pile up and keep their
  user namespaces, which made `unshare` fail with ENOSPC and the process-group tests fail. Every container needs
  `--init` (a real host has systemd).

## Verified matrix
| What | Where | Result |
|---|---|---|
| Native zone qualification: Build profile, seccomp, no network, real `cc` → `cc1` → `ld`; sealed profile; pidfd STOP/KILL; memwatch kill behind bwrap | Debian 12, Linux 6.10, aarch64 | PASS (CI step "linux zone qualification") |
| cgroup v2 `memory.max`: a hog OOM-killed in its leaf through bwrap's namespaces; a sibling untouched | Debian 12, delegated subtree | PASS |
| Full `ci/local.sh` (Linux subset) | Debian 12 | see "CI" below |
| Genesis body built by the install-mode kernel inside the Linux Build zone, then G1 (self.json) | Debian 12 (`scripts/bundle.sh`) | PASS, about 5 min |
| No-Rust bundle e2e: `get.sh --from` → install → TUI first run (`test-` key, mock) → one line round trip → `/usage` → uninstall | fresh Ubuntu 24.04, unprivileged container (`seccomp=unconfined`, `systempaths=unconfined`, `--init`) | PASS, **19.5 s total** (get.sh plus install 13.5 s; Root running at 14.5 s; TUI done at 18.5 s) |
| systemd user service: enabled and active, Root in `init-root.service/init-root` with `+memory`, survives a restart; uninstall disables it and moves it to the Trash | Ubuntu 24.04, systemd 255 as PID 1 | PASS |
| Product-path cgroup leaf: the real body runs in `w-<id>` with `memory.max = 4 GiB` and `swap.max = 0`; the kernel has no leaf | Ubuntu 24.04, a `Delegate=memory` user scope | PASS |
| x86_64 | `cargo check --all-targets --target x86_64-unknown-linux-gnu`, all four workspaces | compiles; **not run** |
| Not verified | real hardware or a VM, Ubuntu with `apparmor_restrict_unprivileged_userns=1`, SELinux, kernels before 6.1, WSL2, a live provider on Linux | — |

**Bundle:** `target/dist/v0.2.0-preview-linux-arm64/init-v0.2.0-preview-linux-arm64.tar.gz`. It is 62 MB packed and
192 MB unpacked. sha256 is in the `.sha256` file next to it, and `latest.json` sits beside it. It was built from
commit 9418f1c on Debian 12, so it needs glibc ≥ 2.36. Its version label is still `0.2.0-preview`, because Cargo's
version was not bumped. Main decides the release version before anything is uploaded.

## CI (`ci/local.sh` on Linux, final run at 9418f1c, Debian 12 container, 31 min with 3 jobs)
**1,793 tests pass, 3 fail, 31 are ignored.** The 31 ignored tests are the opt-in qualification tests, and the CI
runs them in their own steps. Every step passes except these four:
- **kernel test:** `kernel-serve` `gate_results_and_dotted_errors` hit its 30 s watchdog under full-suite load. It
  passes alone, three times out of three. This is a load flake of the same kind as macOS's `kernel-selfmap` one.
  (`kernel-selfmap` `selfmap` also failed once under load in an earlier run and passes alone.)
- **tui test:** `tui14_confined_session_passes_unchanged` runs `/usr/bin/sandbox-exec`; it needs a macOS gate from
  the TUI lane.
- **tui e2e:** `wire9_workspace_tool_write_and_undo` needs workspace mode, which is capped off on Linux; it needs a
  macOS gate from the TUI lane. The other TUI e2e tests pass on Linux, including `uxk_first_run…` and
  `tui_r_real_root_session`.
- **sealed pack s1 templates:** `t-perms-tar` fails on GNU tar. The S1 lane owns it.

## Done
1. **The cc1 fix (c4ab500) is qualified natively.** These pass in Docker:
   - `linux_zone_real_isolation`: Build profile, seccomp, no network, a real `cc` → `cc1` → `ld` → ELF run in the zone;
   - `linux_sealed_real_isolation`;
   - `linux_pidfd_tracked_stop_kill_isolates_owned_target`;
   - the init-sandbox-profiles `linux::tests`.

   The same zone builds the genesis body during `scripts/bundle.sh`'s scratch install.
2. **Memory and disk on Linux** (memwatch.rs, cgroup.rs, sys.rs):
   - **Watchdog.** `memwatch::launcher_view` makes the bwrap hops (outer bwrap, then the namespace's init bwrap)
     transparent. Before this change a Linux instance was ONE unit, so a runaway tool call would have killed the whole
     body, or the kernel. Records now carry the tool call's pid-namespace pid (`NSpid`), which is what the body matches.
     Live test: `sealed::run::tests::linux_a_workload_over_the_cap_is_killed_alone_behind_bwrap_and_recorded`.
   - **cgroup v2 `memory.max` backstop.** The systemd unit says `Delegate=memory`. At daemon start the Root moves into
     `<unit cgroup>/init-root` and enables `+memory`. Each workload instance (not the kernel) and each sealed run gets
     `w-<id>` with `memory.max = mem_cap` and `swap.max = 0`, joined in `pre_exec`. The watchdog records cgroup OOM
     kills (`scope: "cgroup"`) and alerts the owner. Without delegation the Root logs this and falls back to the
     polling watchdog. Live test: `cgroup::tests::a_workload_over_memory_max_is_oom_killed_inside_its_leaf`, through
     bwrap's namespaces, opt-in with `INIT_TEST_CGROUP=1`.
   - **Disk floor:** portable `statvfs`, unchanged.
3. **ci/local.sh on Linux.** It builds root and seed before the kernel tests, which a fresh checkout needs on macOS
   too. It skips the Seatbelt promote step on Linux, and adds the step "linux zone qualification (bwrap, seccomp,
   pidfd, memwatch)".
4. **Bundle.**
   - `scripts/bundle.sh` builds `init-v<ver>[-label]-linux-arm64|linux-x86_64.tar.gz`.
   - `scripts/get.sh` maps `uname -m` to the asset (from `latest.json`'s version) and refuses a bundle built for
     another target. Its Linux edits went in with commit d75346b.
   - `install.sh`:
     - Linux preflight: bwrap ≥ 0.13.0, a bwrap userns/pidns probe, `unshare --net`, python3, git, and a systemd user
       session unless `--no-service`;
     - `sha256sum`, and no `bc`;
     - `systemctl --user enable --now`.
   - `uninstall.sh`: `systemctl --user disable --now`, then moves the unit and the homes to the freedesktop Trash, with
     `.trashinfo` records.
5. **e2e:** see the matrix.
6. **Docs:** docs/platforms.md §3.4.0 (the current truth), README, CHANGELOG.

## Fixes the native suite and the bundle e2e found (all committed)
- **Bundle and install on Linux:**
  - `/etc/os-release` clobbered install.sh's `VERSION`;
  - GNU `cp -R dir/` nested the release inside the bundle;
  - `release.sh` staged from per-workspace dirs while it built into `CARGO_TARGET_DIR`;
  - `seedpack.py` used `cp -c` and stubbed `linux-raw-sys`, which rustix needs on Linux;
  - a no-Rust install's toolchain placeholder (`INIT_HOME/kernel`) was refused as a mount overlap.
- **The zone ran as root of the outer user namespace.** The kernel's passwd lookup then gave `/root` as HOME_REAL,
  and the broker refused every Build spawn (V3). The zone now maps the owner's uid and gid (`bwrap --uid/--gid`), and
  `linux_zone_real_isolation` asserts it.
- **A lost kernel was SIGKILLed before the outer bwrap reported its exit code.** Exit 78 (config) and 75 (swap) were
  recorded as crashes. Fixed with `Broker::kill_lost`, which gives the leader up to 2 s and uses `sys::has_exited`.
- **The probe kernel lost fds 3 and 4.** Linux sealed runs go through `exec-clean`, which closed the control link. The
  conformance probe exited 69. Fixed: `--keep 3 --keep 4`.
- **Bodies were refused when a lane socket was not bound yet** (bwrap cannot bind a missing path). An absent `*_SOCK`
  is now left out; this grants nothing.
- **ADR-009 on Linux.** With the default charter (`read_home = true`) every body spawn was refused with "Linux broad
  home reads are not yet supported". On Linux the Root now caps home reads, workspace writes and full mode off
  (`LINUX_HOME_GRANTS`), and says so in `workspace.set` / `full.set`. This fails closed.

## Gated tests and why
| Test | Gate | Why |
|---|---|---|
| kernel `init-testkit/tests/acceptance_root.rs` (A6) | macOS | Seatbelt `sandbox_check`, the CLT developer dir, APFS clones. The Linux zone invariants are covered by the linux_* qualification tests. A Linux A6 port is open |
| root `tests/wp21.rs` `slice1_kernel_in_zone_owner_round_trip_and_stop_start` | macOS | Seatbelt `sandbox_check` on both instances. The Linux round trip is covered by the bundle e2e |
| root `tests/access.rs` workspace/undo (2 tests) | macOS | workspace mode is capped off on Linux |
| `promote under Seatbelt` CI step | macOS | Seatbelt profiles |
| `cgroup::tests::a_workload_over_memory_max…` | `INIT_TEST_CGROUP=1` | needs a delegated cgroup v2 subtree |

## Needs from others
- **TUI-UX lane (tui/\*\*):**
  - gate `tests/proto.rs` `tui14_confined_session_passes_unchanged` to macOS (it runs `/usr/bin/sandbox-exec`);
  - gate `tests/e2e_root.rs` `wire9_workspace_tool_write_and_undo` to macOS, since workspace mode is capped off on
    Linux.

  These are the only TUI failures on Linux.
- **S1 pack lane:** the `t-perms-tar` template fails `s1validate` on Linux (GNU tar). This lane does not read
  root/sealed/packs.

## Left
- Linux ADR-009 split-bind home plan (docs/platforms.md §3.4.4).
- Landlock; a Secret Service keystore.
- x86_64 run, real hardware or VM, Ubuntu with the AppArmor userns restriction, WSL2.
- Shipping or bundling bwrap 0.13 (no LTS distro has it).
