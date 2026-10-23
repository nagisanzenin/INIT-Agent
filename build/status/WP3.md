# WP3: `init-testkit` fakes, scenario runner, `INIT_FAULT`, live/disposable guard (lane T)

Status: **done** (2026-10-20, about 02:34–03:15 +07).

## Done
All the logic is in the library (`kernel/crates/init-testkit/src/`). The binaries are thin wrappers. Every wire message goes through the `init-proto` types (WP1).
- **`link`**: both ends of the Root↔kernel control link (root.md §4.1.1).
  - fd 3 carries ICJ lines, with `LinkFrame`, `Request`, `Response` and `Event`. Requests are pipelined and responses are routed by id.
  - fd 4 carries `SCM_RIGHTS` datagrams tagged `FdTag`, sent before the line they belong to. Unmatched datagrams are held for 60 s, at most 32.
- **`sys`**: the only `unsafe`, as libc wrappers. It covers sendmsg/recvmsg fd passing, CLOEXEC, inheriting fds 3/4 through `pre_exec`/`dup2`, kill/killpg on held pids only, `waitid(WNOWAIT)` + `wait4` (exit status plus rusage), getuid, and signal flags.
- **`init-fakebody`** (testing.md §3.5):
  - It reads the supervise.md §4.1 env and writes `ready` and `heartbeat` (`ReadyFile`, `HeartbeatFile`) and a `boot` event, plus session events.
  - It speaks stdio-jsonl/1 (`ToBody`, `FromBody`): input with dedup by `input_id`, ack, state, display, pause/paused, resume (with `update_failed`), `exit{75}`, stop (also while paused, D11), reload/reload_done, and notice/notice_ack with dedup.
  - It implements every §3.5 mode, including `crash_first:n` with `state/fakebody.count`.
  - Test hooks in the input text: `/fake crash|exit n|busy ms|hang|display`.
- **`init-fakekernel`** (§3.5a):
  - Startup: `hello` → typed `BootBlock`, `instances`, `ledger.get`, `ceilings.get`, `ready`, and an `hb` every `hb_interval_s`.
  - Body: a broker `spawn` with fds received over fd 4, readiness polling, and owner relay (`owner.in` → durable `state/kernel/inbox.jsonl` dedup by qid → `input`; display → `owner.out`; echo and state). A crashed body is restarted with backoff; 5 crashes in 60 s gives body safe mode, a `body_safe` record and an alert.
  - Root events and requests: `level` events are relayed as notices. A scripted LKG report sends `store.put` of `bin/init-body` with its fd, then `lineage.append promotion{lkg:true}` with `idem`. `stop` exits 0, `prepare_swap` pauses, stops and exits 75, and link loss stops the body and exits 69.
  - Hostile modes: `ignore_term`, `bypass_soft[:n]`, `extra_bodies:n`, `egress:<host>`, `ignore_level`.
- **`init-fakeroot`** (§3.5b):
  - Kernel spawn: it spawns the kernel with the control link on fds 3/4 (`supervise --under-root`) and answers every `KernelRequest`.
  - Broker: real processes in their own process groups, quotas and level gating (`root.level` / `root.quota` plus `ceiling_hit` records), `refuse:<kind>`, safe-mode refusals, and V4 (`INIT_ROOT_*` env refused). When an instance ends it is reaped and its held group is killed, and `exited` events are sent.
  - Store: `store.put` hashes the fd's bytes; `store.get` returns an fd.
  - Lineage: hash-chained R4-format records with `idem` dedup, acked `{seq, hash, dup}`.
  - Other requests: `ledger.get`, `ceilings.get`, `tuple.get`; `system.swap` refused below L3 with `root.level` (L3+ runs a swap); `sealed.*`, `proposal.file` and `alert`.
  - Owner socket `ROOT_HOME/run/owner.sock`, with WP20's frames: attach/replay_end, line, interrupt, panic, the owner queue with redelivery after `ready`, and requests status/stop/start/resume/ledger/lineage/shutdown plus `test.clock|level|event|request|prepare_swap`.
  - Panic from the socket, the `run/panic` latch file and `SIGUSR1`: it kills held groups, closes the egress, writes the `panic` record and a one-level demotion, and the latch survives a restart.
  - Modes: `ok`, `level:n`, `safe`, `refuse:<kind>`, `swap:<job>[@ms]`.
  - **Mock egress** `run/egress.sock` (proxy.md Q-TS1):
    - `GET /_root/ceilings` and `/_root/health`;
    - `POST /<provider>/<path>` forwarded byte-for-byte to `init-mockllm` in upstream mode, with the allowlisted headers, a fake key, and the response streamed back plus the `x-egress-*` headers;
    - spend metered by an SSE usage tap at the charter's peak prices, with reservations;
    - typed refusals with `x-init-egress-error` and the egress.md §3.6 body (`class.model`, `class.level`, `ceiling.day` with `retry-after`, `ceiling.total`, `class.unknown`, `class.disabled`, `class.destination` for CONNECT, and `root.panic` on 503);
    - a usage ledger with the guard-header/bypass flag.
- **Scenario runner** (`scenario`, §3.6):
  - Three ways to start: `start_kernel` (the fake Root plus the fake kernel, or `init-kernel` from `INIT_KERNEL_BIN`), `start_body` (the runner plays the kernel), and `start_root` (the real `init-root`, disposable only).
  - Steps: every §3.6 op, including the Root steps `root_cmd` (approval-type commands refused), `panic` on socket/cli/latch/signal, `clock`, and `kill_root`.
  - Expectations: every §3.6 kind (`events_*`, `files`, `git`, `g4`, `mock`, `processes`, `r4` with chain verification, `level`), plus `link_*`, `body_frames_in_order` and `exit`.
  - Scratch homes are deleted by exact path on success and kept with diagnostics on failure. `scenario_test!` turns a scenario into a test.
- **`fault`**: the `INIT_FAULT` grammar of §3.8.1 (`point[@n]:abort|exit<c>|hang|enospc|sleep<ms>`). It is std-only and self-contained, so the kernel crates and `init-root` can reuse it. The fakes place the points `fakebody.*`, `fakekernel.*` and `fakeroot.*`.
- **`guard`** (D1, §3.1a): the fixed sentinel (`/var/db/init-test-disposable` on macOS, `/etc/…` on Linux) must be root-owned, a regular file, not group- or world-writable, and contain `getuid()`. `INIT_TEST_ENV=disposable` must also be set. No API accepts another path. The runner checks it before creating anything, and `escape_setsid` and `ignore_term` exit 64 without it.
- Fixtures and scenarios:
  - `fixtures/charters/phase0.toml` and `tiny-ceilings.toml`, with the D13 values in test mode;
  - `scripts/wp3-bypass.mock.jsonl`;
  - `scenarios/wp3-fakebody.json`, `wp3-kernel-under-fakeroot.json`, `wp3-fault.json` and `wp3-root-fakekernel.json`.

## Tests
`cd kernel && CARGO_TARGET_DIR=target/t cargo test --offline -p init-testkit`: **105 passed, 0 failed**. That is 76 lib tests (53 of them WP2's), 9 in `tests/fakes.rs`, 6 in `tests/scenarios.rs`, and WP2's 8 integration tests and 3 stub tests. The full suite ran 5× in a row with no flakes, in about 10 s per run. `cargo clippy --offline -p init-testkit --all-targets -- -D warnings` is clean, and my files are rustfmt-clean.
- `fakes.rs` covers these round trips:
  - full stack: owner line → Root → kernel inbox → body → display → owner client; session log; store.put; chained lineage; hb; level notice; stop → exit 0;
  - golden: the boot block, spawn response, ledger, ceilings and `exited` frames have the shape of the init-proto `doc-examples.tsv` rows, and every link line either fake wrote is canonical ICJ and parses as a `LinkFrame`. The fake body replays supervise.md §4.5's example exchange byte for byte (input → state/ack/display, pause → paused, `exit{75}` → 75), and the redelivered input is deduplicated;
  - egress → strict mock: peak metering, and every typed refusal;
  - a tiny ceiling gives a 429 `ceiling.day`;
  - panic over the socket and the latch, the egress closed, a demotion, resume;
  - `prepare_swap` → 75 → restart;
  - hostile `extra_bodies`, `ignore_level` and `egress:`;
  - `crash_first:2` restarts with backoff.
- `scenarios.rs` covers:
  - the fake-body scenario;
  - a kernel scenario under the fake Root (owner I/O, broker, egress to the mock, level, R4 verify, stop);
  - an `INIT_FAULT` scenario (`fakekernel.after_hello:exit78` → `zone_exit{class:config}`);
  - **SW-6**: the fake-kernel Root scenario is skipped in this account even with `INIT_TEST_ENV=disposable`, and creates nothing;
  - a live file with `start_root` or an approval command is rejected;
  - a failing expectation keeps the scratch home and prints diagnostics.
- Every process was spawned by a test or by the fake broker and stopped by its held pid or pgid; there was no pattern kill. `kernel/target/test-homes/wp3/` is empty after the runs.

## Deviations
1. **New dependencies `libc` and `toml`** (both already vendored and pinned in the workspace). testing.md §3.3 says "std only otherwise", but fd passing needs `sendmsg`/`SCM_RIGHTS`, and charter fixtures are TOML. The `Cargo.lock` commit carries only the `init-testkit` dependency hunk.
2. **Mode variables pass through env filters.** Both Roots `env_clear` the kernel, and the kernel clears the body's environment. The fakes therefore also read `INIT_TEST_FAKEKERNEL_MODE` and `INIT_TEST_FAKEBODY_MODE` (`INIT_TEST_*` passes in test mode), and the fake body reads a sidecar `<exe>.mode` file, so a gate candidate's mode can differ from the live body's.
3. **The fake Root's lineage is unsigned** (`sig: "fake:unsigned"`). The runner's `r4.verify` checks the hash chain; for the real Root it also runs `init-root audit verify`.
4. **The fake kernel's `boot.json` is minimal**, not the full `BootFile`. That is enough for the fake body; the real body under the fake kernel is not supported.
5. **Fake-kernel modes added**: `no_body`, `no_hb`, `no_hello`, `exit_after_hello:<c>` and `crash_after_ready:<ms>`, for the Root's crash, hello and heartbeat rules (WP21).
6. **Runner additions**: `start_body`, `send_frame`/`wait_frame`/`wait_exit`/`stop_body`, `wait_root_log`, `root_level`/`root_event`/`root_request`, `sleep`, and the expectations `link_*`, `body_frames_in_order` and `exit`. `wait_*` ops use a cursor (file order).
7. **Hang modes still exit on stdin EOF** and kill their held `sleep` child, so a fake body never outlives its kernel on the live machine.
8. Not run here, by design: `start_root` and every disposable path. They are implemented to root.md §4.3 and testing.md §3.6 (install with `INIT_ROOT_HOME`, `start`, `status --json`, `audit verify`) but were exercised only up to the guard. `g4.verdict_consistent` and `wait_job` follow testing.md's text on `state/kernel/jobs/*.json` and are untested until WP7/WP8 exist.
9. I deleted the stray 232 MB build cache `kernel/crates/init-testkit/src/target/` by exact path (a relative `CARGO_TARGET_DIR` from an earlier run, untouched since 02:31; gitignored). Free disk is at about 6.7 GiB.

## Needs from others
- **WP4/WP5/WP7 (kernel):** to run the real kernel under the fake Root, use `fixture.kernel: "real"` with `INIT_KERNEL_BIN`, or `init-fakeroot --kernel <bin>`. Fault points can use `init_testkit::fault` behind `fault-injection`.
- **WP21/WP22 (Root):**
  - `init-fakekernel` is ready as the kernel under `init-root`; copy it to `INIT_HOME/kernel/bin/init-kernel` as the runner does.
  - The real-Root runner path assumes `INIT_ROOT_HOME`, `install --home --root-home --charter`, `start`, `status --json` and `audit verify`. Tell me about any drift.
- **WP15:** extend `fixtures/charters/` (WP3 seeded `phase0.toml` and `tiny-ceilings.toml`) and write A1–A6 with `scenario_test!`.
- **testing.md owner:** adopt deviations 2, 5 and 6, and the `INIT_TEST_FAKE*_MODE` names.
- **Owner/integrator:** create the disposable account and its sentinel to run `wp3-root-fakekernel.json` and the sweep tests.

## Left
- Run `wp3-root-fakekernel.json` in a disposable environment once WP21's broker exists.
- `init-measure` is still the WP0 stub (WP16).
