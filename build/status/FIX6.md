# FIX6: bind `run/kernel.sock` early; the runner can wait for sockets

Status: **done** (2026-10-20, lane B2). Crates: `kernel/crates/init-kernel/`, `kernel/crates/kernel-serve/`, the start-up order in `kernel/crates/kernel-supervise/src/supervisor.rs`, and the scenario runner's wait helpers in `kernel/crates/init-testkit/src/scenario.rs`.

## Root cause (WP15, FIX1 observation)
`serve::start` bound `run/kernel.sock` only after it had built the promote pipeline's `Env`. That build SHA-256es the kernel's own binary for the gate record's `kernel.build_sha256`. The debug binary is 154 MB and the kernel workspace compiles sha2 unoptimized, so the hash alone takes 2.7 s on an idle M2. Under load (parallel builds) it took far longer, which is the "2 s to more than 60 s" WP15 measured. The hash also waited for the cold start to finish (`Services`).

## Done
**1. Start-up order (kernel-supervise).**
- `run_with_proxy` now takes `run/kernel.lock` and writes `run/kernel.pid` (§3.9 step 1, the new `take_lock`) **before** it calls `on_start`.
- So whatever `on_start` binds belongs to the only kernel. `cold_start` takes the lock file as an argument and runs steps 2–12 as before.
- `Handle::home()` is new.
- Exit codes do not change: a held lock still exits 1.

**2. The early lane (kernel-serve).**
- `EarlyLane::bind(spec, limits)` binds a lane socket on the same path as `add_lane` (shared `bind_lane`: directory at 0700 only when needed (FIX1), canonical paths, length check, stale socket unlinked by exact path, refuses a non-socket file).
- Until it is adopted, every request gets `starting_error()` with its own `id`, then the connection closes. `starting_error()` is `E_BUSY`, `data:{reason:"starting", retry_after_ms:1000}`: retry yes, CLI exit 75.
  - A malformed line still gets `E_PROTO`.
  - The uid check of §3.3 and the lane's connection and rate limits apply.
  - No backend runs and no peer policy is consulted, so nothing except "starting" is disclosed.
- `ServerHandle::adopt_lane(early)` takes over the **same listener** (same inode). No client ever meets a missing or refusing socket, and the server serves connections still in the backlog.
- An early lane dropped without being adopted unlinks its socket.
- `add_lane` is now `check_new` + `bind_lane` + `run_lane`, with the same behaviour.

**3. init-kernel (`src/serve.rs`).** `serve::spawn` (the `on_start` hook) runs in this order:
- binds `run/kernel.sock` as an `EarlyLane` at once;
- starts hashing the kernel binary on its own thread;
- when the supervisor's loop answers `Services`, starts the server and adopts the socket. If adoption fails it binds the socket again.
- `ping`, `status`, `level`, `reload` and `job.*` are served from then on. `promote`, `gate`, `rollback` and `kernel status` wait for the gate `Env` (bounded by `ENV_WAIT` = 300 s, then `E_BUSY{starting}`), which is built as soon as the hash is done. `status` reports `pipeline:false` and `g4_job:null` until then, because no pipeline can run without an `Env`.
- A server that fails to start drops the early socket, logs and sends the `serve_failed` alert, as before.
- `shutdown` also removes an early socket that was never adopted.
- The start-up log lines carry `ms` since the kernel's start, plus `ts`.

**4. The runner (`scenario.rs`).**
- A new step: `{"op":"wait_socket","path":…,"timeout_s":60,"serving":false}`, with the public helpers `wait_socket(path, timeout)` and `wait_serving(path, timeout)`.
  - `wait_socket` waits until the socket accepts a connection. A missing path, a plain file and a stale socket keep it waiting.
  - With `serving:true` it also waits until an initctl `ping` gets an answer from the server itself, not `E_BUSY{starting}`. `E_PEER` counts as served.
  - `is_starting(answer)` is exported.
- The `owner.sock` wait in `start_kernel` and `start_root` is now `OWNER_SOCK_WAIT_S` = 60 s (was 10 s and 20 s). A step can override it with `owner_sock_timeout_s`. `start_kernel` now waits for the socket to accept a connection, not just for the path to exist.

## Measured (e2e, real kernel under init-fakeroot, idle M2, debug build)
Times are from the kernel's own log, in ms after the kernel was spawned:

| Event | Before | After |
|---|---|---|
| `run/kernel.sock` bound and accepting | ≈ cold start + 2.7 s hash (WP15: 2 s to >60 s after body ready) | **+10 ms** (`ms:0` log line, before hello) |
| body spawned / `ready` sent to the Root | +76 / +94 | unchanged |
| server serving (`ping` ok) | same as bound | **+110 ms** |
| body ready (owner `ready` frame) | — | ≈ +390 ms |
| gate `Env` ready (exe hash) | — | +2679 ms (hash 2678 ms, in the background) |

From the test's own client, measured from the Root's spawn, over 2 runs:
- **accepting − body ready = −232 and −372 ms**: it accepts before the body is ready.
- **served − body ready = −125 and −265 ms**.
- The client saw 1 typed `starting` answer per run.

The e2e asserts that the socket accepts by the time the body is ready (+250 ms slack), that every `starting` answer has its typed shape, that the server answers within 30 s of the body's ready, and that the gate `Env` comes up before the promote. The e2e went from about 27 s to 12.9 s. The acceptance runs got faster too: A2 43.8 s (was 57–103), A3 24.2 s (was 42–49), A4 19.8 s (was 24–33).

## Tests
- `cd kernel && CARGO_TARGET_DIR=target/k1 CARGO_INCREMENTAL=0 cargo test --offline -p init-testkit --test acceptance -- a1_ a2_ a3_ a4_ --nocapture`: **4 passed** (A1 11.5 s, A2 43.8 s, A3 24.2 s, A4 19.8 s).
- `… --test acceptance_root`: **A6 passed** (15 s, real init-root, Seatbelt `kernel` and `body`; the early bind runs inside the kernel profile).
- `… -p init-kernel --test e2e -- --nocapture`: **1 passed**, twice.
- `… -p kernel-supervise --test slice1`: **10 passed**. `--test recovery` 5, `serve_seam` 1, `handoff` 8, `probe` 3, `process` 3, `install` 1, `fakeroot_xcheck` 1 and lib 66 passed.
- `… -p kernel-serve`: **green**: 19 unit, 30 `ic` (one new: `early_lane_answers_starting_then_the_server_adopts_it`, which checks a client looping connect+ping across the takeover: every connect succeeds, `starting`* then only real answers, same inode), `sandboxed_lane` 1.
- `… -p init-testkit --lib scenario`: **5 passed** (new: `wait_socket_steps_parse`, `wait_socket_and_wait_serving`).
- `cargo clippy --offline -p kernel-serve -p init-kernel -p kernel-supervise -p init-testkit --all-targets -- -D warnings`: clean. With `--features fault-injection` on kernel-supervise: clean. `scenario.rs` is `rustfmt --edition 2024` clean (the other files were not rustfmt-formatted at HEAD and are left in their style).
- Not green, and not caused by this change: `kernel-supervise --test promote`: `g31_a3_full_t2_path_to_lkg`, `g46_lineage_down_at_accept_then_lkg` and `sv26_t1_data_only_promotion_without_a_handoff` fail with G4 `inconclusive, reason:"underpowered:holdout"`. That is FIX2's D14 holdout rule (194ba30) in kernel-gate meeting these tests' 1-task holdouts. See "Needs from others".
- Safety: every process was a test's own child. Homes were removed by exact path:
  - `target/t15/{bin,fxt,gen,tm-*}` after the acceptance run;
  - the failed promote tests' kept homes `kernel/target/test-homes/w8b/{a,b,t1}`;
  - a stray `target/k1` at the repo root (210 MB), created by one mistyped `CARGO_TARGET_DIR`.
- Lowest free disk was 6.4 GiB.

## Deviations
1. **The typed "starting" error is `E_BUSY` with `data.reason:"starting"`**. The registry has no starting code, and init-proto's `BusyReason` is a closed enum that I do not own, so `data` is written as JSON. `E_BUSY` already means "retry: yes, exit 75", which is the behaviour wanted.
2. **The early lane answers every command with "starting", `ping` included.** So a `ping` that succeeds means the server is up. WP15's `initctl ping` loops keep working unchanged.
3. **No peer policy during "starting".** The policy needs the boot block (the canary path comes from `paths.egress_sock`) and `config.toml`, and both arrive after the lock. The early lane runs no backend, closes after one answer, and discloses only "starting". The uid check still applies, and the socket path is reachable only from the zone's profiles.
4. **The exe hash stays.** `KernelInfo.build_sha256` (promote/mod.rs, not mine) is still computed by the kernel, only off the start-up path. The commands that need it wait for it.

## Needs from others
- **init-proto / initctl.md §4.2 (docs owner):** add `starting` to `E_BUSY`'s `data.reason` (`BusyReason::Starting`). initctl.md §3.3 "Socket files": the kernel binds `run/kernel.sock` right after `run/kernel.lock` and answers `E_BUSY{reason:"starting"}` until its server is ready. supervise.md §3.9 step 1: same note.
- **kernel-gate / WP8b owner (FIX2 follow-up):** `kernel-supervise/tests/promote.rs` g31, g46 and sv26 expect a G4 accept on a 1-task holdout, which D14 now makes `inconclusive: underpowered:holdout`. Their fixtures need a holdout of at least 4 tasks (or an expectation of `inconclusive`).
- **WP15 (optional):** the scenarios can use `wait_socket` instead of the agent's ping loop, and drop the `INIT_KERNEL_BIN` wrapper now that the `owner.sock` wait is 60 s.
- **kernel workspace `Cargo.toml` owner (optional):** `[profile.dev.package.sha2] opt-level = 3`, as FIX4 did for root, would make the 2.7 s hash about 0.3 s. It is no longer on the start-up path, so it is not needed.

## Left
- Nothing for FIX6.
