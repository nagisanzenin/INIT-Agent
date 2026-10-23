# G0-ROOT: sockets under `OUT` for G0, and `init-mockllm` in every install

Status: **done** (2026-10-20, lane G0-ROOT). Release blocker found by S-REAL (`vault/gate/jobs/j-000004/g0/test.log` of the real home): G0 `test` failed the seed (unit 238/33, boot 3/13). Two of the causes were the Root's:
1. `Operation not permitted` on every `UnixListener::bind` of the seed's provider and awareness tests (stub servers, `FakeProxy`).
2. `INIT_MOCKLLM_BIN=$INIT_HOME/kernel/bin/init-mockllm does not exist`: `install` copied the mock only with `--with-testkit`.

## Root cause (finding)
The EPERM did **not** come from the `build` profile. As built, G0's commands run through kernel-gate's `DirectSpawner` as children of the kernel (init-kernel `serve.rs`, WP8b deviation 1), so they inherit the **`kernel`** profile. That profile binds only under `RUN`/`PROBE` and connects only to `egress.sock`. The seed's tests create their sockets under the export (`slot/src/target/test-homes/…`, cwd-relative or `CARGO_MANIFEST_DIR`), which the kernel profile may write but not bind. Under the `build` profile, the doc's design, those tests would fail even earlier: `SRC` is read-only. So three things are needed for a green G0 on a real install:
- the `build` profile allows sockets under `OUT` (the decision; done here);
- while G0 still runs as kernel children, the `kernel` profile allows the same, under the slot's `OUT` only (done here, section 6a, see Deviations);
- the seed's tests put their sockets under `OUT` (`TMPDIR` = `slot/out/tmp`, `HOME`, `CARGO_TARGET_DIR` = `slot/out/target`), not under the source tree (G0-SEED, see Needs).

## Done
- **`build` profile, section 6** (`root/sandbox-profiles/profiles/build.sb`, byte-identical to kernel/sandbox.md §3.4): `(allow system-socket (socket-domain AF_UNIX))`, `network-bind` on `(local unix-socket (subpath OUT))`, `network-outbound` on `(remote unix-socket (subpath OUT))`. Nothing else: no IP or IPv6 rule, no socket outside `OUT`. `check` shares the text. Measured: Seatbelt checks no `network-inbound` for a Unix listener's `accept`, so the decision's two operations suffice.
- **`kernel` profile, section 6a** (`profiles/kernel.sb`, root/root.md §3.3.3): the same two rules over a new Root-set parameter `SLOT_OUT` = `INIT_HOME/cache/build/slot/out` (`ZoneLayout::slot_out`, `kernel_profile_params`, `kernel_params(Kernel)`). The zone tail Z1–Z7 is unchanged.
- **Parameter validation**: unchanged rules; `OUT` already must lie strictly under `INIT_HOME/cache/`, which holds no lane, Root or agent socket. New tests pin it: `OUT` = `run/`, `run/fleet/b1`, `probe/p1/run`, `work/`, `body/`, `vault/` are refused (`class`) for both `build` and `check`. A unit test pins the texts: the build SECTION's only network lines are the three above, no IP rule in `build` or `kernel`, and `SLOT_OUT` appears only in two `unix-socket (subpath …)` rules.
- **Self-test** (`selftest.rs`, `probe.rs`; there is no `init-kernel doctor --sandbox-selftest` and no `init-root doctor --sandbox-selftest` yet; the suite runs as `init-sbx-selftest run` and inside `cargo test -p init-sandbox-profiles`, as FIX1 ran it). New probe op `tcp6-connect PORT`; the runner holds an IPv6 `[::1]` listener and two more stand-in listeners (`cache/build/slot/outl`, `outside/l.sock`). Rows (kernel/sandbox.md §8):

  | Id | Profile | Check | Expect |
  |---|---|---|---|
  | S-B4a | build | one instance serves `OUT/tmp/a.sock` (bind, accept); a second connects and its line arrives | allow |
  | S-B4a2 | build | bind `OUT/home/b.sock` | allow |
  | S-B4b | build | connect the live `run/kernel.sock` | deny |
  | S-B4c | build | TCP connect to the runner's `127.0.0.1` listener | deny |
  | S-B4d | build | bind in `SRC` | deny |
  | S-B4e | build | bind `slot/outb` (`OUT`'s path is a string prefix of it) | deny |
  | S-B4f | build | connect a live listener at `slot/outl` | deny |
  | S-B4g | build | connect the Root's `egress.sock` (the tail leaves it open) | deny |
  | S-B4h | build | connect `owner.sock` | deny |
  | S-B4i | build | connect a live listener outside `INIT_HOME` (stands for `docker.sock`, another `~/.init*`) | deny |
  | S-B4j | build | connect `run/kernel.sock` through a symlink created in `OUT` | deny |
  | S-B4k/l/m | build | UDP send to `127.0.0.1`; TCP bind on loopback; TCP connect to the runner's `[::1]` listener | deny |
  | S-B4ac / S-B4bc | check | bind in its `OUT` / connect `run/kernel.sock` | allow / deny |
  | S-Z8p | kernel | serve `cache/build/slot/out/tmp/p.sock`; a second kernel instance connects | allow |
  | S-Z8q | kernel | bind in `slot/src` (writable to the kernel) | deny |
  | S-Z8r / S-Z8s | kernel | connect `slot/outl` / a listener outside `INIT_HOME` | deny |
  | S-Z8t | kernel | connect `owner.sock` through a symlink in `slot/out` | deny |

  Socket paths in the new rows are relative to the instance's cwd where an absolute path would exceed `sun_path` (as the seed's tests do); Seatbelt checks the resolved path (S-B4a, S-B4d `../src/b.sock`, S-Z8p).
- **`init-mockllm` in every install** (`root/crates/init-root/src/install.rs`): `KERNEL_BINS = [init-kernel, initctl, init-mockllm]` always, `TESTKIT_BINS = [init-fakebody]` with `--with-testkit`. A release directory with `init-kernel` but no `init-mockllm` is refused (exit 1) outside `root.test_mode`, and only warned about in test mode. Unit test `kernel_bin_set_always_has_the_mock`.
- **Docs**: kernel/sandbox.md §3.4 (text, the decision paragraph), §3.6.4 (Linux note), §3.9 (G0 row), §8 (S-B4 and S-Z8p–t, counts); root/root.md §3.3.3 (section 6a text, `SLOT_OUT` row, the 6a note, "What it can do"), §4.x install row and the release-directory paragraph. `tests/doc_drift.rs` passes: doc and code texts are byte-identical.

## Tuple and `built_from`: no format change
Tuples name only the `init-kernel` and `initctl` blobs (`tuple.rs` `kernel_part`, `install_kernel`, root.md §3.7). `init-mockllm` stays outside the tuple, like `kernel/docs/` and `kernel/wit/`: Root-written at install, read-only to the zone (Z4). So the genesis tuple's form and hash inputs are unchanged, and `built_from` (the `lineage/kernel.git` `main` commit) is unchanged. A rollback leaves the installed mock in place. Caveat for later: nothing refreshes `kernel/bin/init-mockllm` after install (`install --upgrade` is not built), so a future mock change needs the upgrade path to copy it.

## Tests
- Red before green (mutations on the profile texts, then restored byte-for-byte):
  - old texts (no socket rules): S-B4a, S-B4a2, S-B4ac fail with `EPERM`; without section 6a, S-Z8p fails with `EPERM`;
  - sockets widened to all of `INIT_HOME` in both SECTIONs: S-B4b, S-B4f, S-B4j, S-B4bc, S-Z8d, S-Z8n, S-Z8q, S-Z8r fail ("allowed"). S-B4d/e still deny there, because a build also cannot create a file outside `OUT`.
- `cd root && ci/slot.sh cargo test --offline -p init-sandbox-profiles`: **green**: 11 unit, 2 doc-drift, 4 params, 1 self-test.
- **Self-test** (`root/target/debug/init-sbx-selftest run --base root/target/test-homes/g0root`, and inside `cargo test`): **240 pass, 0 fail, 11 skipped (251 checks)**, about 12–18 s. Before: 222/0/11 of 233. The 11 skips are unchanged (disposable-env only, plus S-M38).
- `cd root && cargo test --offline` (whole workspace): **green**: init-root 84 unit (one new) plus integration suites, sandbox-profiles as above.
- Fixtures, in `root/target` (7.5 GB free): `INIT_MOCKLLM_BIN=kernel/target/debug/init-mockllm cargo test --offline -p init-root --features test-fixtures`: **green**: 87 unit + 50 integration (fix5 1, owner_frames 3, rsafe 6, rsec 2, wp20 9, wp21 6, wp21b 2, wp22 8, wp23 4, wp24 6, wp25 4). wp21 composes the real `kernel` profile with `SLOT_OUT` for the zone kernel.
- `cargo clippy --offline --all-targets -- -D warnings` (workspace) and `-p init-root --features test-fixtures --all-targets`: **clean**.
- Size budget (root.md §4.8 method, approximate): self-test runner 1,454 → 1,510 of 1,530; profile code about +10 of the 49 spare; `install.rs` about +12 of the 23 spare.

## Deviations
1. **`kernel` profile section 6a is beyond the stated scope** (the decision named the `build` profile). G0 actually runs in the `kernel` profile, so the `build` change alone could not unblock G0. 6a applies the same decision (Unix sockets under the build's `OUT`, nothing else) where G0 runs today, as a separate, three-line block with its own parameter, documented as temporary in root.md. Remove it when G0 runs in `build` instances through the broker.
2. `system-socket AF_UNIX` is included in the `build` text, like `body` and `kernel`; Seatbelt 14.6 does not refuse socket creation anyway.

## Needs from others
- **G0-SEED (blocking for a green real G0):** the seed's tests must create their test homes and sockets under `TMPDIR`/`HOME`/`CARGO_TARGET_DIR` (inside `slot/out`), not under `CARGO_MANIFEST_DIR`/cwd `target/test-homes` (that is `slot/src`, read-only in `build` and not socket-allowed in `kernel` 6a). Affected: `provider/mock_tests.rs` `Home` (stubs and mock), `boot/awareness/tests.rs` `FakeProxy`, `tests/boot.rs` homes (also the `sun_path` failures). Keep socket paths ≤ 103 bytes: a cwd-relative path (`../out/tmp/…` from `slot/src`) is fine, Seatbelt resolves it.
- **WP8b / kernel lane (v0.1.x):** a sandboxed G0 spawner (G0 in `build` instances through the broker), then drop `kernel` section 6a. Until then, candidate test code runs with the kernel's profile.
- **REL:** stage `init-mockllm` in every release directory (release.sh already builds it); `install` now refuses a production release without it.
- **DOCS:** CONVENTIONS.md (lines 49, 77), testing.md (141, 227), kernel/supervise.md §3.1 (line 101) and the reconciliation notes still say `init-mockllm` is installed only with `--with-testkit`; now only `init-fakebody` is. kernel/sandbox.md §8 still names `init-kernel doctor --sandbox-selftest`, which is not built (the runner is `init-sbx-selftest`).

## Left
- Nothing else for G0-ROOT.
