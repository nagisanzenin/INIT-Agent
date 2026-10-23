# WRAP-LINUX: Linux port (native) and Windows through WSL2 (docs/platforms.md)

Status: **paused before any code change** (lane WRAP-LINUX, 2026-10-22). The orchestrator paused the lane
(the owner runs the evolution test first). This lane changed no source file, so HEAD's macOS build is not
affected by it. It created no Docker container or volume and pulled no image. Mac disk was 13 GiB free.

## Done
- Read the binding design and its inputs: BUILD_AGENT_BRIEF.md, docs/CONVENTIONS.md, V1_PLAN.md,
  docs/platforms.md (all), kernel/sandbox.md §3.6, §3.11–§3.13 and §8 (S-L rows), root/root.md §3.4.
- Surveyed the macOS-only code paths and what already exists for Linux (list below).

## Findings: where Linux stands today
| Area | File | State on Linux |
|---|---|---|
| zone launcher | `root/crates/init-root/src/zone.rs` `launcher()` | `cfg(not(macos))` stub returns "the Linux zone … is not built yet (WP6b)", so every spawn fails |
| escapee report and sweep | `zone.rs` `escapees()`, `scan()` | return 0 off macOS. That is correct for Linux, where the pid-namespace hold is complete (`held_pidns`) |
| FFI | `root/crates/init-root/src/sys.rs` | `peer` (SO_PEERCRED) and `sandboxed` (pidns inode) exist for Linux but are UNVERIFIED. `clonefile` and `fs_type` are stubs. `proc_info`, `proc_table`, `footprint` and `signal_tracked` are stubs that see nothing, so memwatch is inert on Linux |
| memwatch | `root/crates/init-root/src/memwatch.rs` | built on `proc_pid_rusage` (macOS). sys.rs carries a TODO for cgroup v2 `memory.max` plus `memory.events` |
| snapshots and undo | `snapshot.rs`, `access.rs` (`sys::clonefile`, APFS skip) | no FICLONE and no copy path |
| profiles | `root/sandbox-profiles/src/lib.rs` | `seatbelt` and `selftest` are `cfg(macos)`; `params`, `access` and `probe` are portable; there is no bwrap renderer |
| inner stage | `kernel/crates/kernel-sandbox/src/lib.rs` | a WP0 skeleton. `landlock` and `seccompiler` are declared as Linux-only dependencies and vendored (`vendor/landlock`, `vendor/seccompiler`) |
| Root seccomp dependency | `root/Cargo.toml` | `seccompiler = 0.5.0`, Linux-only, already declared |
| service | `install.rs` `service_file`/`service_path` | to check: platforms.md §3.4.7 gives the exact systemd unit |

## Plan (next steps, priority order; resume when the orchestrator lifts the pause)
0. **Docker lane** ($0): `rust:1.97.1-bookworm` (bwrap 0.8.0 from apt meets the 0.8.0 floor; git ≥ 2.39.4).
   Mount the repo read-only. Put a single shared `CARGO_TARGET_DIR` in a named volume `init-wrap-linux-target`, so
   kernel and seed share the wasmtime build (disk). Use `source ci/env.sh` and 2 jobs. Run the test container with
   `--security-opt seccomp=unconfined` (or `--privileged`) so bwrap can create user namespaces. Inside the
   container only: `apt-get install bubblewrap gcc-x86-64-linux-gnu libc6-dev-amd64-cross` and
   `rustup target add x86_64-unknown-linux-gnu`, for the x86_64 `cargo check` (ring needs a C cross compiler).
   Watch `df -h /` (Docker.raw grows on the Mac disk, 4.7 GiB used now) and stop below 5.5 GiB.
1. **PLAT-L0**: `cargo build`/`check` for all four workspaces on aarch64 (native) and x86_64 (check); fix
   cfg gaps in new `cfg(target_os = "linux")` modules only.
2. **PLAT-L1/L2 sandbox**:
   - New `root/sandbox-profiles/src/bwrap.rs`: a pure renderer of zone args and per-profile mounts (sandbox.md
     §3.6.1, §3.6.4; root.md §3.4 for `kernel`), with goldens that also run on the M2.
   - New `seccomp_zone.rs` (Linux): zone rules plus the namespace denies of platforms.md §3.4.2.
   - In init-root: a Linux `launcher()` (`--args 6`, `--add-seccomp-fd 5`, `--info-fd 7`, exec-clean keeps them),
     plus the pidns emptiness check after a kill.
   - In kernel-sandbox: `init-kernel sandbox-inner` (Landlock best effort, seccomp, exec), with one small hook
     in init-kernel's main (coordinate with K8-KSIDE).
   - A Linux selftest with the S-L rows that mirror the macOS rows' allow/deny semantics.
   - Fd-pinned binds: `openat2` RESOLVE_NO_SYMLINKS|BENEATH and `/proc/self/fd/N`. Note that bwrap
     `realpath()`s bind sources, so a race window may remain. Test it as PL-L6 at reduced scale in Docker; the
     fallback is in platforms.md §3.4.2.
3. **Service**: render the systemd user unit of platforms.md §3.4.7 (golden), add a `manual` fallback, and
   add a Linux branch in `scripts/install.sh`.
4. **memwatch on Linux**: a `sys_linux.rs` holding `proc_table` and `proc_info` from `/proc/<pid>/stat`, and
   `footprint` from `smaps_rollup` (Rss + Swap). Use cgroup v2 `memory.max` when it is delegated. Test it by
   killing a hog in the container. **Snapshots**: per-file `FICLONE`, falling back to a copy on
   EXDEV/EOPNOTSUPP/EINVAL. Test it with an undo round trip.
5. **WSL2**: documentation only (platforms.md §3.5.2), plus doctor detection through `WSL_DISTRO_NAME` or
   `/proc/sys/fs/binfmt_misc/WSLInterop`.
6. **CI**: an optional `INIT_CI_LINUX_ARM=1` tier in `ci/local.sh` that runs the Docker build and tests.

## Tests
None were run: the lane paused before the build container existed.

## Deviations
None yet.

## Needs from others
- K8-KSIDE: one dispatch line for `sandbox-inner` in `kernel/crates/init-kernel/src/main.rs`, once step 2 resumes.
- Shared files this lane will touch, with small additive edits only: `zone.rs` (the Linux `launcher`),
  `sys.rs` (Linux stubs re-exported from a new module), `sandbox-profiles/src/lib.rs` (new `mod` lines),
  `ci/local.sh`.

## Cleanup
This lane created no Docker resources. All containers and volumes present belong to the owner
(redswarm-*, rsva-loop, brain_rs_local_pgdata) and were left untouched.
