# WP4: `kernel-supervise` (lane K1)

Status: **done for Slice-1 and for most of supervise.md v0.3; partial overall** (2026-10-20). The items under "Left" mostly wait on WP7 (serve) or WP8 (pipeline glue).

## Slice-1 readiness (read this first)

**Ready.** `init-kernel supervise --under-root` runs under the Root and carries an owner message to the body and the body's reply back. It was verified against two fake Roots: the in-test one, and lane T's independent `init-fakeroot` (`tests/fakeroot_xcheck.rs`).
- **Control link** (root.md §4.1): it takes fds 3 and 4, sends `hello`, then sends `ready` and `hb` every `hb_interval_s` from its own thread. EOF on fd 3 stops the bodies and exits 69.
- **Spawn:** it asks the Root's broker for `spawn{kind:"body", profile:"body", lane:"b0"}`.
  - The parameter set is exactly the one WP6a validates. The environment is the §4.1 set, with no keys and no `INIT_ROOT_*`.
  - It takes `[stdin_w, stdout_r, stderr_r]` from the fd-4 datagram tagged with the request id.
- **Owner relay** (§3.10):
  - `owner.in` goes into the durable inbox, deduplicated by `qid`. The kernel answers `ok`, sends `owner.out{echo}`, then writes the stdin `input` frame.
  - The body's `ack` drains the inbox. The body's `display` and `state` frames go back as `owner.out`.
  - Owner commands: `/stop`, `/resume`, `/status`, `/kernel` and `/later`. `/accept`, `/bless` and `/reject` are recorded and never gate anything (D4).
- **Liveness:**
  - Readiness comes from `state/run/ready` with a matching `boot_id`.
  - A heartbeat older than 30 s gets the body killed and counted as a crash.
  - Exit codes: 0 means stopped; 75 is only valid while retiring; 78 at boot twice in a row gives `Halted{config}`; anything else is a crash.
  - Backoff runs 1→60 s and resets after 300 s ready.
- **Body safe mode:** the 5th crash on LKG within 600 s puts the body in safe mode, reports `body_safe{on:true}` and raises the alert `safe_entered`. Three crashes in safe mode give `Halted{crash_loop_safe}`, and the owner's `/resume` re-enters safe mode with fresh counters.
- **Lineage:** every record is written to `vault/lineage.jsonl` and `state/kernel/root-outbox.jsonl`, then sent as `lineage.append{kind,data,idem}` with `idem = <kboot>:<seq>`. The seq continues across restarts, and a resend reuses the original key.
- **Root requests:** `stop` makes the kernel exit 0. `prepare_swap` pauses the body, stops it while paused (D11) and exits 75; a body that never pauses gets `root.busy`, and the kernel keeps running.
- **Proxy:** wired through a `ProxyCtl` seam that `init-kernel` implements with `kernel-proxy` (`init-kernel/src/proxy.rs`). It covers `Proxy::start` with `paths.egress_sock`, `set_level`, `set_ceilings` (from `ceilings.get` and from events), `set_label` and `set_body_id("live", <instance>)`, and `shutdown` on stop. `cargo check -p init-kernel` and clippy are clean.
- **Install:** `init-kernel install --from-bundle <bundle> [--seed-tag]` works.

**What Slice-1 integration must provide:**
- an `INIT_HOME` laid out by `init-root install`, which then runs `init-kernel install` to make `bin/init-body` (= LKG), `lineage/*` and `body/`. Or prepare `bin/init-body` and `bin/init-body.lkg` by hand.
- `kernel/config.toml` with `[sandbox] toolchain` and `developer_dir`, sent as `TOOLCHAIN`/`DEVDIR`; the Root validates them.
- the Root's boot block.

Without `lineage/body.git`, ref moves are logged instead of made.

## Done (`kernel/crates/kernel-supervise/`, plus `init-kernel/src/{main.rs,proxy.rs}`)
- `machine.rs`: a pure state machine, `on(ev, now) -> Vec<Act>`. It covers the body slot of §3.3 and §3.7: cold start, starting, running, backoff, stopped, disk-full, halted and stopping. It also covers the overlapped handoff and rollback of §3.6.4 and §3.6.7 (pausing → fencing → await ready → settling 3 s → commit → retiring, with every abort path), probation and provisional crash rollback, rollback at the next idle point (forced after `rollback_max_defer_s`), and `stop`/`prepare_swap`.
- `supervisor.rs`: the driver.
  - cold start, steps 1–12 of §3.9, including the interrupted-handoff table and step 6's restore from the tuple;
  - the relay, readiness and heartbeat polling;
  - `boot.json`, `update-failed.json`, `supervisor.json` and `alerts.jsonl`, plus `alert` to the Root (at most 30 per hour);
  - the handoff commit (bin renames, `update-ref main`, `promotion{lkg:false}`);
  - the **LKG commit** of §3.6.6: `store.put` with the fd, wait for `promotion{lkg:true}`, install `.lkg`, write `lkg.json` with `lineage_seq`, set `refs/heads/lkg`, export `lineage/trees/<sha12>`, and retry every minute while the Root refuses (`lkg_store_failed`);
  - G4 fail → rollback at idle.
  - The command handle (`Cmd::{Handoff, Rollback, G4Pass, G4Fail, Snapshot}`) is the seam WP7 and WP8 call.
- `link.rs` (fd 3/4, out-of-order responses, fds stashed for 60 s, `call_fds`, `call_with_fd`, and the `Spawner` impl), `outbox.rs`, `inbox.rs`, `bodyio.rs` (stdout discipline: 1 MiB lines, raw lines as info, flood above 64 MiB/min, stderr capped at 4 MiB with a 4 KiB tail), `binstore.rs` (§3.6.5 renames and repair), `recovery.rs` (`store.get` restores, `bin/ext*` re-pointing), `layout.rs` (K-F5, session ids), `spawnspec.rs`, `config.rs` (`[supervise]`, `[sandbox]`, `[model]`, the `_mib` test-mode rule), `fault.rs` (`INIT_FAULT`, feature `fault-injection`), `sys.rs` (the only `unsafe`: `SCM_RIGHTS`, statvfs, clonefile, passwd home).
- `probe.rs`: `probe()`, `wait_ready`, the probe window (at least 2 heartbeats), `send_input`, and `stop` (stop frame, then kill, `keep_on_stop`, agent outputs only for `Probe`, root deleted). Built on `proberoot.rs`, which uses clone-based roots (D8). Proxy scopes and lane sockets are not wired yet.
- Helper-built modules (same lane, merged and committed here): `git.rs` (the hardened `$G`, CAS refs, candidate checks E_NOT_DESCENDANT, E_TOO_LARGE, E_FOREIGN_COMMIT and E_TREE, neutral export, fetch and archive, `clone_for_root`), `build.rs` (`deps_key`, `DepsCache` with LRU 3, the stub workspace, `BuildSlot` export/request/run/stage, `E_DISK`), `proberoot.rs`.
- `install.rs`: seed mirror, highest-semver tag, `body.git` with `info/attributes`, `main`/`lkg`, the agent clone, a real offline build through `spawn{kind:"build"}`, `store.put`, `promotion{lkg:true}`, `lkg.json` (`seed`), and the exported tree.
- `fleet.rs` and `kupgrade.rs` are **stubs** with real refusal rules: `E_LEVEL`/`E_CEILING` before any spawn (SV-40), and `kernel promote` refusals (§3.15 step 2). The demotion and safe-mode hooks are called.
- Test fixtures: `sv-testbody` (a scripted stdio-jsonl/1 body; its plan is keyed by `INIT_SHA`) and `sv-kernel`.

## Tests
`cd kernel && CARGO_TARGET_DIR=target/k1 cargo test --offline -p kernel-supervise` gives **90 passed**, 3 runs in a row with no flakes. Adding `--features fault-injection --test faults` gives **2 more**.

| Suite | Count | Covers |
|---|---|---|
| lib | 59 | machine SV-1…SV-12, SV-28, stop/swap/F31; config (SV-2, C-10); sys fd passing; outbox/inbox durability; bodyio; binstore SV-22; spawnspec SV-24; layout; fleet SV-40; kupgrade; git (incl. SV-20); build; proberoot |
| `slice1` | 10 | relay round trip, SV-14/15/17/24/30/31/38, F-crash-boot (c) and (d), F-hang (b), SV-34, `prepare_swap` (ok and `root.busy`), outbox resend |
| `process` | 3 | SV-33, exit 0/75 as a process, kill -9 of the kernel (by held pid) → one body, bin intact, conversation intact |
| `handoff` | 8 | SV-5+SV-14, SV-6, SV-8, SV-9, SV-10, `initctl rollback`, SV-37, SV-12 (G4 fail at idle) |
| `recovery` | 5 | SV-22, SV-35, F15, interrupted `committing` and `pausing`, system safe mode and `body_safe{on:false}` |
| `probe` | 3 | G2 copy-log probe with window, G4 run with `keep_on_stop` (SV-32), F-crash-boot (a) |
| `install` | 1 | full install with a real build |
| `fakeroot_xcheck` | 1 | owner line round trip under WP3's `init-fakeroot`, which must be built in `target/k1`; the test is skipped otherwise |
| `faults` (feature) | 2 | SV-36, `supervise.cold.after_sweep:abort` then a clean restart |

Clippy (`-D warnings`, `--all-targets`, with and without the feature) is clean on kernel-supervise and on init-kernel. Test homes live under `kernel/target/test-homes/wp4/` and are empty after the runs. Every process the tests start is the test's own child, stopped by its held handle.

**Disposable-env only, not run:** SV-16 (panic sweep of setsid/double-fork daemons), the kill-by-sweep half of SV-46, F-hang (a), the full F-kill-kernel chaos variant (it relies on the Root's cold-start sweep), and SV-18/SV-19/SV-21/SV-25 (sandbox properties; WP6a's S-* suite covers them).

## Deviations
1. **Control-link names follow root.md**, as WP1 does: `lineage.append.idem`. The local `vault/lineage.jsonl` keeps `dedup_key`. The body lane is `"b0"` (supervise.md §3.4); root.md's example shows `"live"`.
2. **`x-egress-body` is the Root instance id** (egress.md §4.1), not the boot id that proxy.md's text names.
3. **The proxy sits behind a seam.** `ProxyCtl` is implemented in the `init-kernel` binary, so kernel-supervise does not depend on kernel-proxy (lane independence, and no Cargo.lock change).
4. **Test mode** (`proxy.test_mode`) only logs the 64-byte `INIT_HOME` rule and does not enforce it, because repo test homes are about 85 bytes. These tests bind no lane sockets.
5. **Config.** Supervise reads its own sections strictly (an unknown `[supervise]` key is an error; so is `notify_cmd`), plus `[sandbox]`, `[model]` and `proxy.test_mode`. The full cross-section schema is config.md's loader.
6. **Timing.**
   - The 90 s ready timeout starts when the Root answers the spawn.
   - A Root spawn refusal backs off without counting a crash (alert `spawn_failed`).
   - A first exit 78 respawns after `backoff(1)`.
   - The body-safe decision is made, and reported, at the 5th crash; the safe spawn follows the normal backoff.
   - A body that ignores `/stop` is killed after 30 s + `exit_grace_s`.
   - `/resume` from any `Halted` enters body safe mode (the §3.3 row).
7. **Spawn deferral.** The next primary spawn waits up to 12 s for a killed instance's `exited` event, so the old body releases the log lock first.
8. **Install** sends `ready` right after `hello` (the seed build can take minutes; `hb` keeps flowing). It reports `promotion{scope:"seed"}` and exits 0 or 1.
9. **`probe()`**: `INIT_BODY_ID=probe`. The probe's `boot.json` is the live one, used as a template.
10. **Inbox compaction** keeps only the qids of unacked lines. The Root dropped the acked ones when it got `ok`.
11. **Helper's choices** (git/build):
    - the stub dependency build runs in the build slot and is then committed to `cache/deps/<key>`, so the absolute paths match;
    - the stub `.cargo/config.toml` is cut down to an allowlist;
    - symlink targets are resolved through the tree's own links;
    - `deps_key` puts a NUL between its parts;
    - `init_bare` adds `--template=` and `--initial-branch=main`.

## Needs from others
- **WP7 (serve):**
  - drive `supervisor::Handle`: `Cmd::Snapshot` for `status`; `Cmd::Rollback` for `initctl rollback`;
  - bind the probe and fleet lane sockets;
  - the `run/spawn.sock` relay for `init-kernel sandboxed-upload-pack` (§3.6.1). It is not written yet, and `git::Git::fetch` takes the upload-pack command as a parameter.
- **WP8 (pipeline glue):** fetch, checks and validator (`git.rs`), then `BuildSlot` (`build.rs`), then G0–G3 using `probe::probe`, then `Cmd::Handoff{Promote}`. G4 sends `Cmd::G4Pass` or `Cmd::G4Fail`. The deps cache is filled from `build::make_stub_workspace` when the slot is idle.
- **WP21 (Root):**
  - spawn `init-kernel install --from-bundle kernel/seed/<tag>.bundle` with reason `install`;
  - accept the body lane `"b0"`;
  - allow `spawn` and `store.put` before `ready` during install, or accept the early `ready` (deviation 8).
- **Docs owners:**
  - supervise.md §3.13: `idem`, the `level` fields and `system.swap` names (WP1 deviation 4);
  - proxy.md: the `x-egress-body` meaning;
  - §3.11.2 vs `kernel-gate/src/evo/repo.rs`: the identity there is `INIT Kernel`/`kernel@init.local`, the doc says `init-kernel`/`kernel@init.invalid` (found by the helper);
  - `core.logAllRefUpdates` keeps no reflog for `refs/init/*`.

## Left
- **Not built yet:**
  - data-only promotions (§3.6.8, SV-26);
  - kept-component sets moving with the binary (`bin/ext*` swaps and publish, SV-29). Recovery already re-points them;
  - selfmap's served view rebuild (§3.16);
  - `doctor`, `upstream sync` and `install --template`;
  - the level cache's 120 s staleness refusal (`E_ROOT`) for level-gated requests. It belongs at WP7's command entry; the ledger is refreshed every 60 s.
- **SV-13 at the handoff and commit fault points** needs a process-level promote trigger (WP7). The `cold` and `root.*` points are tested.
- **L2 fleet mechanics** (SV-40 spawn, SV-41…SV-43) and **L3 K0–K3, `system.swap` and kernel-candidate fetch** (SV-44, SV-45): stubs plus refusal rules only.
- **Disk** (SV-23 on a small volume): the machine logic is unit-tested, but the hdiutil scenario was not run.
- **The disposable-env half** (listed under Tests).
