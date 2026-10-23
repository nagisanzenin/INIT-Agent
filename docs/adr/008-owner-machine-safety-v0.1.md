# ADR-008: Owner-machine safety for v0.1 (held pids only, live-sweep compiled out)

Status: provisional, taken 2026-10-20 under the release plan (the owner said not to wait; the owner may override) · Area: root
Depends on: v03-decisions D1, CONVENTIONS §1 (live-system safety), root/root.md §3.5.4 and §3.9, build-deviations WP21 D1/D2, testing.md §3.13.0   Defines: owner-machine mode, `init-root doctor --sweep-dry-run`, the `escapee` record

## Context

On 2026-10-20 a sandbox sweep test killed 33 of the owner's processes, Docker Desktop among them. The v0.2 two-condition rule, (a) sandboxed and (b) can read the instance mark, matched foreign application sandboxes. D1 replaced it with three conditions: (a), (b) and (c), where (c) means the process is denied the sibling decoy and the Root canary. D1 also requires that sweep tests run only in a disposable environment: a VM or a separate macOS user. roadmap.md M2 then said that no real `init-root` runs on the owner's live account until SW-1…SW-6 pass on that build in such an environment.

v0.1 must run on the owner's live account, and there is no disposable account yet. The release plan's item 4 therefore asks for a mode that is safe on the live account as it is.

## Decision

v0.1 ships in **owner-machine mode**: INIT never signals a process it did not spawn. Four locks hold this.

1. **One signalling primitive, and it checks the hold itself.** `sys::signal` is the only `kill`/`killpg` in a default build. It refuses every pid that is not an unreaped child of the Root: `waitid(P_PID, pid, WEXITED | WNOHANG | WNOWAIT)` must succeed, and that call only looks, it never reaps. Every spawn is its own process group (`process_group(0)`), and an unreaped leader (alive or a zombie) keeps both its pid and its group id the Root's. So `killpg(leader)` reaches only the instance's own group. Panic, stop, a kernel crash, the resume rollback and the reaper all go through it (zone.rs `Broker::kill`, `kill_all` and the reaper; the held sealed runs). A pid that is already reaped, a previous Root's pid, or any pid named by a file is refused before any syscall that could deliver a signal.
2. **`live-sweep` is compiled out.** The D1 scan over the live process table (`zone::scan` → `sys::signal_member`) exists only with the cargo feature `live-sweep`. No other feature implies it, and it is not in `default`. A build without `debug_assertions` that names it stops at a `compile_error!` (sweep.rs), so a release build can never carry it.
3. **Even a `live-sweep` build refuses the owner's account.** Its scan runs only when the disposable sentinel of testing.md §3.1a exists. That file is `/var/db/init-test-disposable`: a regular file owned by root, not writable by group or others, whose content is our uid. Without sudo it cannot be created, so a test cannot fake it. With no sentinel, the build signals held pids only and logs why.
4. **A source audit.** tests/rsafe.rs runs in every build and fails the build when any of these drifts:
   - `live-sweep` appears in default or implied features, or in any script or workflow;
   - a `kill`/`killpg` appears outside `sys::signal` or the live-sweep-only `signal_member`;
   - a `sys::signal` caller appears that is not one of the five reviewed held-pid sites;
   - `signal` loses its `held` check;
   - the cold start gains a signal;
   - `src/doctor.rs` gains any call that could signal.

**Cold start** never signals a pid that a previous Root recorded, because the pid may have been reused. It deletes their marks by exact path, logs them as "never signalled by pid", and reports any survivor (below).

**Sweep dry-run.** `init-root doctor --sweep-dry-run` lists our uid's pids (`proc_listpids`) and applies D1 through the sweep's own `member()` against every instance mark and the zone mark. It returns the would-kill set: each member's pid, marks, executable, and whether `state/root.json` records it. Every other process is counted by the first condition it fails. It sends no signal, and its result says `"signals_sent": 0`.

**Escapee detection** (security review F5). When the zone empties (after a panic, a stop or a crash) and at cold start, the Root runs D1 on the zone mark over the live table. Pids it holds are excluded. Anything that still matches on two passes 300 ms apart gets an `escapee{after, mark, pids, signalled:false}` record and an owner alert with the pids, and is never signalled. The panic result carries `sweep:"held_only"`, and its message says that a process which left its group is reported, not killed.

## Residual risk

A zone process that calls `setsid()` (or `setpgid`) leaves the leader's process group. On panic, the Root's `killpg(leader)` and `kill(leader)` miss it, so it **survives the panic** until `live-sweep` is enabled in a build verified on a disposable account. The damage is bounded:
- it keeps the inherited Seatbelt policy (zone tail Z1–Z7): no IP network, no reads of ROOT_HOME or keys, and writes only inside INIT_HOME;
- its only route out is the Root's egress socket, which panic closes;
- it is reported at once as an `escapee` record and an alert naming its pids, so the owner can inspect it (`ps -p <pid>`) and end it.

A second, smaller gap is the sealed runs (R-DEC's sealed/run.rs). They kill their group after reaping the leader, and `sys::signal` now refuses that call, because the group id is no longer held. Leftover members of a sealed run's group that outlive its leader are then not killed. The fix is to kill the group before reaping the leader (`sys::wait_exit`, then `killpg`, then `wait`), and it has been routed to R-DEC. Sealed runs are confined by their own profile and cannot reach the zone or the network beyond their lane socket.

## What is verified live (macOS 14.6, the owner's account, 2026-10-20)

All on the live process table, with only the test's own processes signalled, and only by their handles. Command: `cargo test --offline -p init-root --features test-fixtures --test rsafe`; 6 of 6 pass.
- **Dry-run, exact selection.** A test Root runs its kernel (the real `kernel` profile) and body (`body` profile), next to the SW-2/3/4 decoys (unsandboxed; a permissive `(allow default)` sandbox; a sandbox that denies the canary and the marks). The would-kill set is exactly {kernel, body}: each reads only its own mark (Z5), and both read the zone mark. Across three runs, 448–475 processes were evaluated:

  | Outcome | Processes per run |
  |---|---|
  | selected (the two instances) | 2 |
  | rejected for (a), not sandboxed | 236–267 |
  | rejected for (b), cannot read the mark | 194–199 |
  | rejected for (c), reads the decoy or the canary | 13–14 |
  | cannot tell | 0 |

  The 13–14 rejected for (c) are the incident class: foreign application sandboxes that can read the mark. D1 rejects every one of them.
- **SW-5 owner-machine variant (RT-P1).** Panic kills exactly 2 instances / 2 processes, and every decoy keeps running. The resume rollback to audited-good (RT-P7) and stop (RT-P11) also spare every decoy. No `escapee` is reported.
- **SW-1 owner-machine variant.** A body forks a `setsid` child. The dry-run selects kernel, leader and escapee, with only the escapee unrecorded. Panic kills the two held instances; the escapee survives, is reported (`escapee{after:"panic"}` plus an alert), and ends on its own.
- **Cold start.** The stand-in is a process that meets D1 exactly, recorded as a previous Root's instance. The new Root does not signal it, removes its mark, and reports it (`after:"cold_start"`). After a held-only panic it reports it again, and the process is still running.
- **Unit tests.** `sys::signal` refuses pids -1, 0, 1, our own pid, our parent and a reaped child, and reaches a running or zombie child and its group. `sandbox_check` reports a gone pid as sandboxed, but the gone pid never "may read", so it is never a D1 member. `classify` agrees with `member` on every case. The sentinel rule is covered.
- **Builds.** `cargo check --features live-sweep` compiles in a debug build. With `debug-assertions = false` it stops at the `compile_error!`. Neither build was run.
- Already live before this ADR: tests/wp21.rs (RT-P1, P3, P4, P6, P7, P9, P11; the held-only build), tests/wp22.rs (RT-C2, C4, C7–C10, RT-E1, E2, E5), and init-testkit SW-6 (a refusal).

## Plan to enable live-sweep later

1. Provision a disposable environment: a separate macOS user with no other processes of its own, or a VM. Its root-owned sentinel names that user's uid.
2. In it, with `INIT_TEST_ENV=disposable`, run SW-1…SW-6 (testing.md §3.13.0) against a `live-sweep` debug build. Use real decoys shaped like the owner's applications (SW-3: a permissive application sandbox), and record the build's commit.
3. Run the dry-run on the owner's live account with that same commit, and confirm the would-kill set is only INIT's instances. This is already possible, and is the owner-machine half of the evidence.
4. The owner amends this ADR. Only then is the `compile_error!` relaxed for that verified build, or replaced by a charter opt-in such as `[root] sweep = "d1"`, while the sentinel requirement stays for tests. `status.sweep` becomes `"d1"`. The escapee report stays as a cross-check.
5. Until all four steps are done, no `live-sweep` build runs on the owner's account. The sentinel lock enforces this even if such a build is made by mistake.

## Alternatives considered

| Alternative | Why rejected |
|---|---|
| Ship live-sweep on the owner's account, trusting D1's unit tests | That is the incident's pattern: a predicate proved only on fake tables. D1 has not been proved on a disposable account. |
| Kill escapees found by the dry-run from the CLI after asking the owner | That is still a pattern kill on the live machine, and a stale pid can be reused between the scan and the kill. The owner ends a reported process themselves. |
| Block v0.1 until a disposable account exists | The owner asked for the earliest release, and owner-machine mode needs none. |
| Use `killpg` with a negative or zero pid to catch the whole session | Session and group ids of other processes are not held. This is refused by the audit. |

## Consequences

- v0.1's panic guarantee reads "every process the Root holds, and every escapee reported", not "every zone process". The panic result and the docs say so.
- The `escapee` lineage kind is added (root/root.md §3.10.5).
- A new signalling site needs an edit to tests/rsafe.rs's reviewed list, which forces a review.
- The sealed group-cleanup fix (Residual risk) was owed by R-DEC. As built: it landed in 3b08245, where sealed `supervise` kills the group while the leader is an unreaped zombie and only then reaps it (unit test `the_group_is_killed_before_the_leader_is_reaped`), so the second gap is closed.

## Amendment: the memory watchdog (MEM-RUNAWAY, 2026-10-22; provisional, the owner may override)

**Why.** In M7 D, one sandboxed `python3.9` grew to 43.9 GB of compressed memory. Swap took free disk to 0.24 GB (measurements/m7-results.md DV-4). Seatbelt has no memory limit, so the agent's bash tool carries the same risk on the owner's live install. The process to stop is a grandchild of a held instance (body → tool call → workload), and killing the held body would not reach it, because each tool call is its own process group. The harness stopgap that worked (tests/real/m7watch.py, DV-6) killed such processes by pid and start time.

**Decision.** There is one new signalling primitive, `sys::signal_tracked`, with one caller: `memwatch::kill_members` (root/root.md §3.14.1). It sends only SIGSTOP or SIGKILL, to a process the watchdog observed below a held instance (a ppid chain into a leader `sys::held` confirms, or remembered from such a poll by pid and start time). Right before the syscall it re-reads the pid and requires all of these: the same start time and uid, our uid, `sandbox_check` = sandboxed, pid > 1, and not the Root itself. The predicate is identity, not a pattern: a reused pid has another start time, and no owner application is a descendant of a Root-held instance. Leaders are still killed only through `sys::signal`. The kernel is never killed by the watchdog.

**Audit.** tests/rsafe.rs now expects the kill sites `signal`, `signal_member` and `signal_tracked`. It checks that `signal_tracked` keeps its identity, uid, sandbox, pid and signal checks, and that `kill_members` is its only caller.

**Verified live** ($0, the owner's account, 2026-10-22, `cargo test -p init-root --features test-fixtures`). A 400 MiB `perl` hog under a 256 MiB cap is killed alone, about 0.8 s after it starts, in a body instance of a test Root (tests/memwatch.rs) and in a sealed run (`sealed::run` unit test). The leader and its sibling workloads keep running, the record is written before the kill, and the alert is raised. tests/rsafe.rs: 6 of 6 pass, including the live dry-run and the escapee test.
