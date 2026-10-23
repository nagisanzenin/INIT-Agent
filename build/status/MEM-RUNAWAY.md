# MEM-RUNAWAY: per-workload memory watchdog and workload disk floor

Status: done (macOS), 2026-10-22 · lane MEM-RUNAWAY · RELEASE_PLAN.md finding HIGH MEM-RUNAWAY (M7 D, measurements/m7-results.md DV-4, DV-6)

## Done
- **Root, `memwatch.rs` (new).** Every held zone instance (kernel, body, probe, build, check, git read) and every sealed run is a *leader*. Each direct child's process tree is a *unit*. Members are remembered by pid and start time after their parent exits, and orphans left in the unit root's process group join it. Each unit's `phys_footprint` (`proc_pid_rusage(RUSAGE_INFO_V4)`) is summed every 1 s.
  - A unit over `sandbox.mem_cap_mib` is SIGSTOPped top-down (once more for children born meanwhile), recorded, then SIGKILLed bottom-up, by pid.
  - A leader whose own footprint passes the cap is killed whole (`sys::signal`, held). The kernel never is.
  - Below `sandbox.disk_floor_gib` every unit is killed, plus every leader that is a workload itself (build, check, git read). The kernel and bodies stay up.
  - The zone loop starts in the daemon (`cli.rs` start). Sealed `supervise` runs the same poll for its run and ends it as `mem_cap` / `disk_floor`. At the end of a sealed run it kills the leftover units, which are process groups of their own and out of reach of the group kill.
- **Root, `sys.rs`.** Adds `Proc`, `proc_info` (PROC_PIDTBSDINFO), `proc_table`, `footprint`, and `signal_tracked`, the ADR-008 amendment. `signal_tracked` sends only SIGSTOP or SIGKILL. Right before the syscall it requires the same start time and uid, our uid, and sandboxed, with pid > 1 and not the Root. Its one caller is `memwatch::kill_members`. Linux stubs carry a TODO for cgroup v2 `memory.max`.
- **Charter.** Adds `sandbox.mem_cap_mib` (default 4096, C16 bounds 512–65536) and `sandbox.disk_floor_gib` (default 5, 1–1024); test mode lowers the minimums to 64 and 0. Both are left out of the effective JSON at their defaults, so existing digests are unchanged. The template shows both commented.
- **Refusals.** `zone::spawn` refuses every kind except body below the floor, with `root.quota{quota:"disk_floor", free_mib, floor_mib}`. Sealed `supervise` refuses before spawning. `run_task` and `check_v` turn a `disk_floor` stop into an error (the slice stops as `internal`, so no sample is counted). `mem_cap` is a scored outcome (DV-6).
- **Reporting.** Each kill is appended to the events file before the signal. For the zone that is `INIT_HOME/root/mem-events.jsonl` (readable by every body through `PUB`, rotated at 256 KiB). For a sealed run it is `<run>/mem-events.jsonl`, created before the spawn and written only through the Root's own fd. Bodies get `INIT_MEM_CAP_MIB` and `INIT_MEM_EVENTS` from the Root at spawn. The owner gets the alerts `mem_cap` and `disk_floor` (notice, `alerts.jsonl`, `notify.cmd`) and the notice `disk_floor` (info) on recovery.
- **Body, `tools/memcap.rs` (new; reads only, no enforcement).** When a bash call dies by a signal the body did not send, and a Root record matches its pgid since its spawn, the trailer reads `[process exceeded memory cap N MiB, killed (its process tree reached F MiB) wall_time=…]` or the disk-floor sentence; it is an error with summary `memory cap` / `disk floor`. Bash refuses (`error: disk_floor: …`) while the newest disk record is a `disk_low` at most 15 s old. `init:ext/process` `run` appends the sentence to stderr.
- **Docs.** root/root.md §3.14.1, body/tools.md §3.5.8, the ADR-008 amendment, the RELEASE_PLAN entry, CHANGELOG Unreleased.

## Tests
All cost $0 and use homes under `root/target/test-homes/{memwatch,mw}/` and `seed/target/test-homes/`.
- `cd root && ci/slot.sh cargo test --offline -p init-root --features test-fixtures --bin init-root -- memwatch charter sealed::run sys::`: 25 of 25 pass.
  - The memwatch planner, on fake tables: units are child trees, summed; only the over-cap unit goes; a leader over the cap goes whole; the kernel never goes; orphans stay in their unit by identity; a reused pid is not ours; strangers, leaders and the Root are never members.
  - The charter bounds: 511, 65537 and 0 are rejected; 512, 65536, 1 and 1024 are accepted; explicit defaults keep the digest; test mode lowers only the minimums.
  - Live, `a_workload_over_the_cap_is_killed_alone_and_recorded`: a sealed run whose `sh` leader runs a 400 MiB perl hog, a sleeper and a `setsid` sleeper, all three sandboxed. With a 256 MiB cap the hog alone is killed, about 1 s in. The leader and both siblings survive it, the record (`mem_cap`, its pgid, more than 256 MiB) precedes the kill, and the alert is written. At the end of the run the setsid sibling is killed too.
  - `a_sealed_run_is_refused_below_the_disk_floor`: a fake floor of 2^50 bytes; nothing spawns.
- `--test memwatch` (zone end to end, a test Root daemon plus the fake kernel): 2 of 2 pass.
  - A body instance under the real `body` profile gets `INIT_MEM_CAP_MIB=256` and `INIT_MEM_EVENTS`. Its 400 MiB perl hog is killed 0.82 s after it starts, the body and its sibling keep running, and the record and the alert are written.
  - With `disk_floor_gib = 1024`, a `check` spawn is refused with `root.quota`/`disk_floor`, a body still starts, and a `disk_low` record appears.
- `--test rsafe`: 6 of 6 pass. The audit now expects the third kill site, `signal_tracked`, its checks, and its single caller.
- Full `init-root` suite (`--features test-fixtures --no-fail-fast`): unit tests 112 pass (2 ignored); access 3, fix5 1, memwatch 2, owner_frames 3, redteam 5 (1 ignored), rsafe 6, rsec 2, wp20 9, wp21 6, wp21b 2, wp22 8, wp23 4, wp25 4. `m7` (1 of 3) and `wp24` (0 of 6) fail for a reason that predates this change; see "Left / pre-existing". Clippy `-D warnings` is clean on every line this change touched; the remaining findings are older, in `sealed/exact/tests.rs`, `pack.rs:302`, `slice.rs:283` and `raise_nofile`.
- Full seed suite: 286 + 17 + 9 pass; clippy `--all-targets -D warnings` is clean.
- `cd seed && ci/slot.sh cargo test --offline --bin init-body -- tools::memcap`: 4 of 4 pass.
  - A Root-style kill (the record, then SIGKILL to the call's group) yields the memory-cap tool error.
  - The disk-floor kill text; a kill with no record stays `signal=9`.
  - Only this call's fresh record counts.
  - Bash is refused under a fresh `disk_low`, runs after `disk_ok`, and runs on a stale `disk_low`.

## Deviations
- **The kernel crate is unchanged.** "Kernel probe and gate instances" are covered by the Root: the kernel never spawns a zone process itself (kernel-supervise `instance.rs`), and every probe, build, check and gate-judge child descends from a Root-held instance. A kernel build was also not affordable on this disk (cold, about 4 GB).
- **The floor is a new key, not `quotas.disk_min_free_gib`.** That key defaults to 20 GiB, which would refuse every workload on the owner's machine (about 8 GiB free). It stays the install-time floor. root.md §3.14's table still describes spawn checks against it that were never built.
- **A body spawn is allowed below the floor**, so the agent can tell the owner. Its tool processes are killed while the disk stays low.
- **Kill granularity is a unit, a direct child's tree.** A build's leader is `cargo`, so each `rustc` is its own unit. A body's bash call that runs `cargo build -jN` is one unit, and its sum can pass 4 GiB on large crates; the owner can raise `sandbox.mem_cap_mib`. A follow-up should measure a cold seed build through bash.
- **Residual gap.** A process that calls `setsid` and is orphaned within one poll, before it is ever seen, is in no unit. It is still reported as an `escapee` (ADR-008).

## Needs from others
- REL / owner: the live install gets the Root fix only with the new `init-root` binary. `install --upgrade` is not built (release.md §6). The minimal path is for the owner to stop the Root, replace `~/.init-root/bin/init-root` with the release build, and start the Root again. This swap is not tested end to end. The supported path remains uninstall (to the Trash) plus install. The body side reaches the live body only through a seed update or a reinstall. Enforcement does not depend on it; without it bash shows `signal=9`.
- Kernel lane (optional): surface the `mem-events.jsonl` record in gate errors for a killed build or check instance; today the gate sees `signal 9`.

## Left / pre-existing
- `tests/m7.rs` (`m7_draw_and_g4_slice`, `m7_calibrate_probe_milestone`) and `tests/wp24.rs` fail before any sealed spawn with `sealed run: no lkg tuple: a sealed body run has no T0 source`. That error comes from REAL-M7's `body_home`, not from this change. Their test homes have no LKG tuple.
- Linux: cgroup v2 `memory.max`, `memory.swap.max = 0`, `memory.events` (TODO in sys.rs and root.md §3.14.1).
