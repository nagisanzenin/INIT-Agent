# R-SAFE: owner-machine mode for v0.1 (ADR-008)

Lane R-SAFE, 2026-10-20. Status: **done**. The full-suite result is under "Tests".

## Done

1. **Signals only to held pids.**
   - `sys::signal`, the only `kill`/`killpg` in a default build, now checks `sys::held(pid)` itself. `held` is `waitid(P_PID, pid, WEXITED|WNOHANG|WNOWAIT)`: it is true only for an unreaped child (running, stopped or a zombie) and never reaps.
   - The broker kills (`kill`, `kill_all`), the reaper, panic, stop, the kernel-crash paths, the resume rollback and sealed `cancel_all`/`supervise` all go through it.
   - `sys::signal_member` is the only way to reach a non-child (a D1 escapee). It is compiled only with `live-sweep` and called only from `zone::scan`.
   - `zone::scan` (live-sweep) also needs the disposable sentinel (`sweep::disposable`, the testing.md §3.1a rule: a root-owned `/var/db/init-test-disposable` naming our uid).
   - `live-sweep` is in no default or implied feature. A build without `debug_assertions` that names it hits a `compile_error!` (sweep.rs), so release builds cannot carry it. Checked: `cargo check --features live-sweep` compiles in debug; with `profile.dev.package.init-root.debug-assertions=false` it fails with the ADR-008 message. Neither build was run.
   - Cold start never signals a recorded pid (unchanged behaviour). It is now audited and live-tested.
2. **Sweep dry-run.**
   - `init-root doctor --sweep-dry-run` (src/doctor.rs) enumerates the real process table (`sys::LiveTable`: `proc_listpids`, `sandbox_check`, `proc_pidpath`; now built in every macOS build and read-only).
   - It applies D1 through `sweep::member`/`sweep::classify` to every instance mark plus the zone mark, and returns the would-kill set: pid, marks, exe, and whether `state/root.json` records it. Rejects are counted by the first condition they fail. It reports `signals_sent: 0`.
   - `LiveTable` keeps `sandbox_check` three-valued: an error is "cannot tell", never "denied".
3. **Review F5: escapee detection, never a kill.**
   - When the zone empties (panic, stop, crash) and at cold start, zone.rs `escapees()` runs D1 on the zone mark, excluding held pids.
   - A match on two passes 300 ms apart gets an `escapee{after, mark, pids, signalled:false}` record and an owner alert with the pids.
   - The panic result has `sweep:"held_only"`, and its message says that escapees are reported, not killed.
   - There is a new lineage kind `escapee` (lineage/kinds.rs).
4. **Tests.**
   - tests/rsafe.rs has 3 source audits, which run in every build:
     - no `live-sweep` in default or implied features, scripts or workflows, and the release guard is present;
     - the only `kill`/`killpg` are `sys::signal` (behind `held`) and `signal_member` (under live-sweep);
     - the 5 reviewed `sys::signal` callers;
     - the cold start has no signal;
     - `doctor.rs` and the `LiveTable` impl hold no signalling call.
   - tests/rsafe.rs has 3 live tests (test-fixtures). They are the owner-machine SW-1o…SW-5o and SW-7o rows in testing.md §3.13.0.
   - Unit tests cover `held`/`signal` against our own child, refuse -1, 0, 1, self, our parent and a reaped pid, and cover `classify`, the sentinel rule and `evaluate`.
5. **Docs.**
   - docs/adr/008-owner-machine-safety-v0.1.md is new.
   - root/root.md: §3.5.4 (an owner-machine paragraph), §3.9.2 step 4 (an owner-machine paragraph) and §3.10.5 (the `escapee` row).
   - roadmap.md: the M2 row and the M2 check-command row.
   - testing.md §3.13.0: the owner-machine variants table.

## Live dry-run numbers (the owner's live table, macOS 14.6)

Three runs, each with this test's own instances alive, evaluated 448–475 processes of our uid. The would-kill set was exactly the test's instances: {kernel, body}, the stand-in, or {kernel, leader, escapee}. Every other process was rejected:

| Rejected for | Processes per run |
|---|---|
| (a), not sandboxed | 236–267 |
| (b), cannot read the mark | 194–199 |
| (c), reads the decoy or the canary | 13–14 |
| cannot tell | 0 |

The 13–14 rejected for (c) are the 2026-10-20 incident class: foreign sandboxes that can read the mark. Raw lines: `root/target/test-homes/rsafe/dry-run-summary.jsonl`. They are removed at clean-up; the numbers are copied into ADR-008.

## Tests (command, result)

- `cargo test --offline -p init-root --features test-fixtures --test rsafe -- --test-threads 1`: **6/6 ok** (34 s).
- `cargo test --offline -p init-root --test rsafe` (default features): 3/3 ok (the audits).
- `cargo test --offline -p init-root --bins -- sys:: sweep:: doctor::`: 9/9 ok.
- `cargo clippy --offline -p init-root --features test-fixtures --all-targets`: no warnings in R-SAFE files.
- `cargo test --offline -p init-testkit --test scenarios sw6` (kernel workspace): SW-6 ok. `cargo test --offline -p kernel-proxy --test proxy rt_c1`: RT-C1 ok.
- `INIT_MOCKLLM_BIN=… cargo test --offline -p init-root --features test-fixtures --no-fail-fast --test rsafe --test wp20 --test wp21 --test wp21b --test wp22 --test wp23 --test fix5`: **all ok** (rsafe 6, wp20 9, wp21 6, wp21b 2, wp22 8, wp23 4, fix5 1).
- wp24 and wp25 failed in that run for environmental reasons: another lane's `cargo build` replaced `target/debug/init-root` mid-run with a build without test-fixtures ("init-root test …: only in a test build"), and test homes collided ("Directory not empty"). Re-run in a build slot: wp25 4/4 ok; wp24 5/6, then `gate_of_gates_refuses_broken_gates` passed 1/1 when run alone. Its first failure was a home collision ("sealed/M/base: No such file"). So every Root suite is green with the R-SAFE changes. From now on, run `--features test-fixtures` with `CARGO_TARGET_DIR=root/target/fx` (orchestrator rule, after this binary swap).
- Bin unit tests: 77/78 under load 270. `lineage::tests::rr16…` (not R-SAFE code) failed at its view-copy check in 2 of 3 runs and passed in the third. It is most likely a concurrent run by another lane using the same `test-homes` path.

## Task 3: live-safe Root tests run, and what stays disposable-only

Run live in owner-machine mode (all spawn only their own test Root, decoys and fake kernel):
- RT-P1, P3, P4, P6 (non-launchd), P7, P9 and P11 (tests/wp21.rs); RT-P2 as built, which is the latch-only fallback (wp20); RT-P10, the mirror ref (wp23 rr19).
- RT-C2, C4, C7, C8, C9, C10, RT-E1, E2 and E5 (wp22); RT-C3 (wp21 `config_halt_retry_and_parallel_body_refusal`); RT-C1 (kernel-proxy `rt_c1_soft_cap_typed_429`).
- SW-6 (init-testkit); SW-1o, SW-2o…SW-4o, SW-5o, SW-7o and SW-8o (rsafe).

Disposable-only, and why:
- **SW-1…SW-5 as written.** They prove the live D1 *kill*, which signals processes picked by a predicate rather than held pids. That needs a `live-sweep` build and the sentinel, so it is forbidden on the owner's account (CONVENTIONS §1). The owner-machine variants prove the same predicate without signals.
- **RT-P5's escapee half.** "Sweeps the escapee" needs the live D1 kill. Live, the escapee survives and is reported (SW-1o).
- **RT-P2 as specified.** The CLI fallback's generic sweep and the SIGKILL of a possibly stale `root.pid` both signal unheld pids. The fallback as built is latch-only and runs live.
- **RT-P6's real variant.** A launchd restart needs a real LaunchAgent under `~/Library`. The tests use `--no-service`.
- Not built (not a safety question): RT-C5's L1 bypass demotion is covered only by RT-L7/L9 (wp25); RT-C6's CPU, disk and INIT_HOME quotas are gap §18; RT-E3…E7 are M7; RT-E8 has no test; RT-P8 has no dedicated mid-stream panic test.

## Deviations

- **D1.** I edited `sys.rs` (the `held`/`signal` guard, `signal_member`, `LiveTable` built without live-sweep and three-valued, and `proc_pidpath`). It is not in my path list, but it holds the only FFI for kill and for the process table that my tasks need, and the edits are small.
- **D2.** One line each in `cli.rs` (the `doctor --sweep-dry-run` dispatch and its usage line) and `main.rs` (`mod doctor`). init-root had no `doctor` subcommand, so I added the closest one.
- **D3.** `lineage/kinds.rs`: the `escapee` Root kind (review F5).
- **D4.** The dry-run labels pids `recorded` (from `state/root.json`), not `held`. With no Root running, the file is a previous Root's record.
- **D5.** `sandbox_check(pid, NULL, 0)` answers 1 ("sandboxed") for a pid that is gone, on macOS 14.6. The path form never answers "allowed" for it, so (b) fails and it is never a member (unit-tested). `sys::sandboxed` (the owner-socket peer check) keeps its fail-closed reading.
- **D6.** ci/local.sh is unchanged: its `root test` and `root test (test-fixtures)` steps already run tests/rsafe.rs.

## Needs from others

- **R-DEC (sealed/run.rs).** `supervise` calls `sys::signal(pid, SIGKILL, true)` after `child.try_wait()`/`wait()` has reaped the leader, at line ~182 and in the try_wait branch, and `cancel_all` can race the reap. `sys::signal` now refuses these calls (the pid is no longer held), so leftover group members of a finished sealed run are not killed. Fix: `sys::wait_exit(pid)` (WNOWAIT), then killpg, then `child.wait()`. Sent to the orchestrator.
- **R-DEC (sup.rs `Kctl::take_fds`), review F6.** Cap the total stashed fds (for example 64), shorten retention to 10 s, and close any datagram whose fd count differs from its tag. Sent to the orchestrator.
- **Owner/integrator.** The disposable account (ADR-008, "Plan to enable live-sweep later") before live-sweep is ever enabled.

## Left

- A per-instance escapee report while the zone keeps running (a body escapee whose kernel stays up is reported only when the zone empties).
- The ADR-008 enabling steps (disposable SW-1…SW-6), which belong to the owner.

## Follow-up (orchestrator, 2026-10-20 afternoon)

1. **build-deviations §5 L5: done (7a460a6).** `panic::resume` is now refused (`root.mode`, CLI exit 77) while the mirror's `refs/heads/panic` exists.
   - It reads the state from the mirror thread's last `ls-remote` (`lineage::mirror::panic_ref_seen`), because a fresh git call could take up to 120 s and the CLI waits 10 s.
   - A refused resume moves the next poll to now, so the owner can delete the branch and resume about a second later.
   - With `mirror.url = ""` (until M0), a URL the mode refuses, or `panic.mirror_ref = false`, nothing is polled and nothing is checked; there is no branch to watch.
   - Right after a Root restart the cache is empty until the first tick (≤ 1 s). A resume in that window is allowed, and the first poll then re-panics on the branch, which fails safe.
   - Test: wp23 `rr19…`. Resume is refused while the branch exists (exit 77, the message names the ref, the slot stays `panicked`). After `update-ref -d` it succeeds, and the panic fires only once. root.md §3.9.5 has an as-built note.
2. **R-SEC RLIMIT_NOFILE: done (7a460a6).** `sys::raise_nofile(4096)` raises the soft limit to min(hard, 4096) and never lowers it. `cli::start` calls it before boot and logs `RLIMIT_NOFILE soft <before> -> <after> (hard <h>)`. The LaunchAgent is unchanged, because a process may raise its own soft limit up to the hard one.
   - Tests: the unit test `sys::tests::nofile_soft_limit_is_raised_and_read_back` (getrlimit read-back, never lowered). The rsafe cold-start test parses the daemon's log line and checks that the soft limit is ≥ min(hard, 4096).
   - Runs: rsafe 6/6, wp21 6/6, wp23 4/4, sys 4/4; clippy clean.
3. **WP24 D7 (sealed spawns in the broker table): design note, tagged v0.1.x.** It is not done in v0.1, for the reasons below.
   - **What already holds in v0.1.** The total never exceeds `quotas.max_instances`.
     - The zone's quota is `max_instances − sealed.parallel` (zone.rs `quotas`).
     - Sealed operations are serialized (`sealed::run::begin`), and each runs its tasks one at a time (`run_task` loops in audit.rs and gog.rs). So at most one sealed leader is alive, which is ≤ `sealed.parallel` (1–4, and ≤ max − 2 by C12).
     - Panic cancels sealed runs through `sealed::cancel_all`, which signals only through the same `sys::signal` held-pid guard.
     - So D7's remaining gap is bookkeeping (one instance table, one cancel path), not a quota or safety hole.
   - **Why it is too invasive for v0.1.** A plain `Broker::add(kind:"sealed")` would break five things:
     1. `Broker::list()` and `counts()` feed the kernel's `instances` reply and `ceilings.json`, both zone-visible. Sealed runs would show the agent when an audit runs, a side channel into sealed evaluation.
     2. `kill_all()` also runs on a kernel crash or link loss (sup.rs, 4 call sites), where sealed runs must survive.
     3. `save()` would write sealed runs into `state/root.json`, and cold start and `escapees()` would treat them as zone orphans.
     4. Sealed reaps through `std::process::Child::wait` on its own thread. The broker's `reaped` flag must be set under the table lock before that reap, or `Broker::kill` could race it. The zone reaper gets this right; sealed does not yet.
     5. The quota would double-count against the static reservation.

     The change touches zone.rs (R-SAFE), sealed/run.rs and sup.rs (R-DEC), and link.rs.
   - **v0.1.x shape.**
     - `Broker::add_sealed(run_id, pid)` with kind `sealed`, and `Broker::reaped_sealed(run_id)`. The second sets `reaped` under the table lock after `sys::wait_exit` and the group `killpg`, and before `child.wait()`, matching R-DEC's new `supervise` order.
     - `sealed` is excluded from `list()`, from the zone-visible `counts()`, from `save()`, from cold-start orphan handling and from the crash-path `kill_all`.
     - It is included in the owner's `status.instances`, in `count(None)` for `max_instances` (with the zone keeping `max − sealed.parallel`, and sealed limited to `sealed.parallel`), and in panic's `kill_all`. Panic's kill then replaces `cancel_all`'s pid loop, and `cancel_all` keeps only the run-root clean-up.
     - Tests:
       - a sealed run counts toward `max_instances`;
       - a panic kills a running sealed run through the broker;
       - a kernel crash spares it;
       - the kernel's `instances` and `ceilings.json` never show it;
       - `sys::signal` still refuses its pid after the reap.
