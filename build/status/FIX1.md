# FIX1: `run/kernel.sock` under the `kernel` profile (S1 gap 3)

Status: **done** (2026-10-20, lane B1). Crates: `root/sandbox-profiles/` (WP6a) and, with the orchestrator's clearance, a fix in `kernel/crates/kernel-serve/` (WP7).

## Root cause
The Seatbelt `kernel` profile never denied the bind. Under the composed profile (PRELUDE + `kernel` + ROOT_ALLOWS + zone tail), measured on macOS 14.6 with processes and homes this lane created:

| Operation (kernel profile) | Result |
|---|---|
| bind `run/kernel.sock`, `run/proxy.sock`, `run/fleet/b1/proxy.sock`, `probe/p1/run/kernel.sock` | allowed |
| `chmod run/fleet/b1`, `chmod probe/p1/run` | allowed |
| `chmod run/` itself, **even to its current mode 0700** | **`EPERM`** |

kernel-serve's `add_lane` ran `fs::set_permissions(dir, 0o700)` on every lane socket's parent directory. For the live lane that directory is `run/` itself. Tail rule Z4 (root/root.md §3.3.2) has `(deny file-write* (literal (param "Z_RUN")))`, and that deny covers `file-write-mode`. So `add_lane` failed before it ever reached `bind`, and serve.rs reported the error as `run/kernel.sock: Operation not permitted`. The proxy never chmods `run/`, which is why `proxy.sock` worked. The lane sockets under `run/fleet/<bid>/` and `probe/<id>/run/` were never affected, because Z4 protects only `run/` itself.

## Done
- **kernel-serve fix** (`src/server.rs`). The new `prepare_lane_dir` creates the lane directory and chmods it to 0700 **only when its mode differs**. The Root's install already creates `INIT_HOME/run` at 0700 (init-root `make_dirs`), so the kernel never touches `run/`. Z4 is unchanged.
- **No profile text changed.** `kernel.sb` already allows `network-bind`/`network-inbound` under exactly `RUN` and `PROBE`, which is root/root.md §3.3.3 verbatim. The Root tail Z1–Z7 is untouched, and `tests/doc_drift.rs` still passes.
- **Self-test** (`src/probe.rs`, `src/selftest.rs`):
  - New probe ops:
    - `chmod MODE PATH`;
    - `lane-bind PATH`, the kernel's lane-socket sequence: mkdir, chmod the directory 0700 only if needed, unlink a stale socket by its exact path, bind, chmod the socket 0600;
    - `serve-lane PATH`, which runs that sequence, then accepts one connection.
  - The setup now creates `run/` at 0700, as the Root does.
  - New rows (20):

  | Id | Profile | Check | Expect |
  |---|---|---|---|
  | S-Z8f | kernel | chmod `run/` itself to 0700 (Z4; the S1 EPERM) | deny |
  | S-Z8e | kernel+body | the kernel serves `run/kernel.sock` with the lane sequence over a stale socket file; a body instance connects and its line arrives | allow |
  | S-Z8g–l | kernel | lane sequence on `run/proxy.sock`, `run/spawn.sock`, `run/fleet/b2/{kernel,proxy}.sock` (dir created), `probe/p2/run/{kernel,proxy}.sock` (dir chmod-ed) | allow |
  | S-Z8m–o | kernel | bind in `run/mark/` (Z4), in `work/` (not under RUN/PROBE), outside INIT_HOME (Z6) | deny |
  | S-M39b–h | body | bind each of the 7 kernel-owned paths above while absent (with an existing file, the bind fails with `EADDRINUSE` before the sandbox decides; S-M33 already shows a body cannot unlink the kernel's socket) | deny |
  | S-M49j | body/probe | bind in its own lane dir `<probe>/run/` | deny |
  | S-M57g | body/member | bind in its own lane dir `run/fleet/b1/` | deny |

- **kernel-serve tests:**
  - Unit test `prepare_lane_dir_chmods_only_when_needed`. An existing 0700 directory keeps its ctime, so no chmod happened. A 0755 directory becomes 0700. A missing directory is created at 0700.
  - `tests/sandboxed_lane.rs` (macOS) re-runs the test binary under `sandbox-exec` with `(allow default)` plus exactly Z4's `run/` deny. Inside the sandbox it asserts that `chmod run/` gives `EPERM`, then calls the real `add_lane(LaneSpec::live)` and pings over `run/kernel.sock`.

## Tests
- `cd root && cargo test --offline -p init-sandbox-profiles`: **green**. It runs 10 unit, 2 doc-drift, 4 params and 1 self-test.
- **Self-test: 222 pass, 0 fail, 11 skipped (233 checks), about 8 s.** Before this change it was 202/0/11 of 213; the 20 new rows all pass. The 11 skips are unchanged (disposable-env only, plus S-M38).
- `cargo clippy --offline -p init-sandbox-profiles --all-targets -- -D warnings`: clean.
- `cd kernel && CARGO_TARGET_DIR=target/k1 CARGO_INCREMENTAL=0 cargo test --offline -p kernel-serve`: **green**, with 19 unit tests (one new), 29 `ic` and the new `sandboxed_lane` test (1 pass plus its inner test). clippy `--all-targets -D warnings` is clean.
- **Red before green.** With the old unconditional chmod:
  - `sandboxed_lane` fails with exactly S1's `Os { code: 1, … "Operation not permitted" }` from `add_lane`.
  - A mutated `lane-bind` that chmods unconditionally fails S-Z8e, S-Z8g and S-Z8h with `chmod-dir: EPERM`.
  - So the checks are not vacuous. The mutation was reverted, and its kept home was deleted by exact path.
- **End-to-end, $0.** I rebuilt `init-kernel` in `kernel/target/k1` with `CARGO_INCREMENTAL=0`, then ran `scripts/slice1/s1.sh` twice.
  - `S1_TAG=fix1-mock … mock`: **pass**, with hello.txt `INIT\n` and `done`.
  - `S1_TAG=fix1-initctl S1_MOCK_SCRIPT=<script> … mock`: **pass**. The mock model's turn 1 is a `bash` call that pipes one ping frame through `/usr/bin/nc -U` to `$INIT_KERNEL_SOCK` from inside the body sandbox.
    - The tool result was `{"id":"fix1.1","ok":true,"result":{"kernel_version":"0.1.0","lane":"live","proto":1,…},"v":1}`.
    - The kernel stderr now logs `initctl server on …/ih/run/kernel.sock (live lane b0)`. It used to log `initctl server not started: … Operation not permitted`.
  - The run dirs were removed by the harness, and I deleted the two artifact dirs by exact path.

## Deviations
1. **Scope.** The orchestrator asked for a `kernel` profile change. The measurement showed the profile was already right, and the defect was in kernel-serve, so I fixed it there after the orchestrator cleared it. Z4 stays as it is: root.md says its `run/` literal stops the zone from replacing `run/` before the Root creates a mark in it.
2. **Bind rules not narrowed to exact socket paths.** root/root.md §3.3.3 (binding; drift-tested) specifies `subpath RUN`/`subpath PROBE`. The kernel also binds paths other than the lane names: `run/spawn.sock`, and kernel-gate's judge relay at `probe/<12 hex>/p.sock` (uncommitted WP code). A literal-only rule would break both. S-Z8m–o pin what stays denied: `run/mark/`, `work/`, outside `INIT_HOME`. Narrowing would need a root.md change first.
3. **New row ids** (S-Z8e–o, S-M39b–h, S-M49j, S-M57g) are not yet in kernel/sandbox.md §8. The docs owners should add them.

## Needs from others
- **Root (WP21b): nothing new.** No new parameter is needed. The Root must keep creating `INIT_HOME/run` at mode 0700 (`make_dirs` does). If `run/` were ever another mode, the kernel's chmod would be refused by Z4, and the live lane would fail loudly instead of silently.
- **Docs (sandbox.md §8, initctl.md §3.3):** add the new rows. Note in initctl.md "Socket files" that the kernel never chmods `run/` itself (Z4).
- **S1 harness:** nothing is required. Optionally, stage `initctl` in `$REL` so the zone has `kernel/bin/initctl`. Today it carries only `init-kernel`.
- **WP7b (observation, not blocking):** in the mock runs the live lane came up several seconds after the body was ready.
  - No socket was visible during a 13 s mock run; the bash ping in the second run connected within its 20 s wait.
  - The likely cause is `serve::spawn` waiting on `h.services()`, which has a 10 s timeout per try, during the cold start. Worth a look if early `initctl` calls matter.
- **Body authors (observation):** inside the body, `[ -S "$INIT_KERNEL_SOCK" ]` is always false. The `body` profile grants `file-read-metadata` on `RUN` only, not on the socket file, so `stat` gives EPERM. Connecting works. Probe the socket by connecting, not by `stat`.

## Left
- Nothing for FIX1.
