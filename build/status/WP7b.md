# WP7b: kernel integration, initctl inside `init-kernel` (lane K1)

Status: **done** (2026-10-20). `init-kernel supervise --under-root` now runs WP7's initctl server over the real supervisor. promote, gate and rollback go through WP8b's pipeline. Data-only promotions (t0/t1, supervise.md §3.6.8) replace WP8b's `E_INTERNAL`. One end-to-end test drives the real `initctl` against the real `init-kernel` under `init-fakeroot` and `init-mockllm`, at $0.

## Done
**`init-kernel`** (the binary is where the lane crates meet, as `proxy.rs` already was):
- `src/serve.rs` holds `KernelBackend`, which implements `kernel_serve::Backend`:
  - **`view`**: read from memory through `Handle::latest()`. The supervisor publishes this snapshot every loop turn (at most 100 ms old), so a request never waits on the supervisor. It covers every §3.5 row serve can see:
    - link, ledger age (supervisor copy or `refresh_ledger` copy, whichever is younger), level and outbox backlog;
    - `Halted{reason}`, `Stopping`, safe mode (body or system), provisional and depth/max;
    - `disk_low` (statvfs every 5 s against `min_free_build`);
    - `busy` while a non-initctl handoff or rollback runs;
    - `parallel_bodies` from the ceilings.
  - **`marks`**: `run/mark/<instance>` for the primary and, during a handoff, the other side. Probe lanes use their own instance.
  - **`status`**: every field from the snapshot plus `lkg.json`, `sandbox_status()` and the uptime. Serve fills `lane`, `jobs` and `constitution_sha256`.
  - **`ledger`** and **`refresh_ledger`**: `ledger.get` and `ceilings.get` on the control link.
  - **`reload`**:
    - `E_NO_BODY` when no primary is running;
    - otherwise `set_awaiting_boundary`, then `Cmd::Reload` sends `reload{job, kind:"request"}`;
    - the body's `reload_done` ends the job with the `ReloadOutcome`, or `E_TIMEOUT` after 1800 s.
  - **`promote`, `gate`, `rollback`**: WP8b's 7-line `ServeJob` adapter (`tests/serve_seam.rs`, verbatim) around `promote::{promote, gate_quick, rollback}`. The sync result becomes `Begun::started().with("sha" | "from"/"to")`.
  - **The other commands**:
    - `task template`: a two-instance skeleton family.
    - `task list` (L1): empty. `task show` (L1): `E_NOT_FOUND`.
    - `fleet list`: no members. `fleet stop all`: `[]`. `fleet stop` and `fleet send` (L2) on a bid: `E_NOT_FOUND`.
    - `fleet spawn` (L2): `fleet::check_spawn` refusals, then "not built".
    - `kernel promote` (L3): `kupgrade::check_request` refusals, then "not built".
    - `kernel status` (L3): the running kernel.
    - `kernel propose`: `proposal.file` to the Root.
    - At L0, serve refuses all level-gated commands before they reach the backend.
  - **Kernel-internal jobs** (asynchronous G4, F12 retries, the escalated re-run) go into serve's `JobStore` on the live lane (`ServeJobs`), so `initctl job status` and `job wait` see them.
  - **`promote::Env`** is built from `Handle::services()`:
    - `GateCfg` comes from `kernel/config.toml`;
    - `KernelInfo` holds the exe SHA, the config SHA and the boot block's charter SHA;
    - `sup` is the `Handle` and `lineage` is the `Outbox`.
  - **Startup**: `serve::spawn` runs from the new `main_under_root_started` hook. Once the loop answers `Services`, it:
    - opens the job store and deletes `refs/init/candidates/<job>` for each `kernel_restart` job;
    - binds `run/kernel.sock`;
    - installs the probe-lane hook;
    - starts the watch thread.

    A failed start is logged and alerted (`serve_failed`) and the kernel runs on without initctl. `serve::shutdown` runs when the supervisor returns.
  - **Peer policy**: `SandboxPolicy`, with the canary at `<dir of paths.egress_sock>/canary`, which is `ROOT_HOME/run/canary`.
  - **Provisional watch**: `promote::provisional_watch` runs every 60 s with the provisional age taken from `Prov.since_ms` (G-46 resubmission, F16 expiry).
- `src/proxy.rs` adds `KernelGate`, the `promote::GateProxy` over the kernel's `ProxyHandle`: gates, probe/G3/run scopes, summaries and the trace reader. Labels carry the configured model and the current level.
- `src/main.rs`:
  - `supervise` wires the server;
  - **`init-kernel selfmap …`** dispatches to `kernel_selfmap::cli::run`. WP9 asked for this, and G1 runs `$INIT_HOME/kernel/bin/init-kernel selfmap`.

**`kernel-supervise`**:
- **`supervisor.rs`**:
  - `Snapshot` gains link, backlog, ledger/ceilings and age, free bytes, instances, start time, boot reason, system safe, handoff and charter.
  - `Handle::latest()` and `Handle::alive()`.
  - `Services` gains `link` and `boot`.
  - New commands `Cmd::Reload` (pending replies keyed by job, answered on `reload_done`) and `Cmd::DataOnly`.
  - New entry point `main_under_root_started`.
  - `sandbox_status()` is now `pub`.
- **Data-only promotions** (§3.6.8, SV-26), in `Supervisor::data_only`:
  - check that the running version is still the gated parent and that no handoff is in progress;
  - `update-ref main Y X` (CAS);
  - the kept set: swap `bin/ext`;
  - set current = Y, provisional at depth+1, relabel the live proxy scope, notice `promoted{handoff:false}`;
  - then **either** the §3.6.6 LKG commit with `lkg.json.reason = "data_only"`. If the Root refuses it, the version stays provisional with `lkg_store_failed`, retried every minute with the same reason;
  - **or** `promotion{lkg:false, scope}` plus notice `provisional`;
  - finally `reload{kind:"kept", components}` when the set changed. `binaries.current.built_from` stays X.
- **`promote/pipeline.rs`**, `run_data_only` after G0–G1 (t0), G0–G1 plus the synchronous G4 (t0_escalated), or G0–G3 (t1):
  - publish the gate-built kept set;
  - lineage before LKG: the gate record is acknowledged first. If it is not, G-46 applies: provisional plus a pending record;
  - `Supervise::data_only`, a new trait method with a default, so the existing seams compile;
  - the terminal result `{…, handoff:false, lkg}`;
  - the asynchronous G4 for t1.
  - No pause, no `awaiting_boundary`, no handoff stage.
- **`extset.rs`** (new): `bin/ext.d/<set>` publish (set id = 12 hex over the sorted `name\tsha256` lines, new inodes, tmp dir + rename), atomic `bin/ext*` links, `swap` (keeps `.prev`), `follow`, `report`, GC by exact path. The LKG commit now `store.put`s each component (`kind:"ext"`), reports `ext_set`/`components` and points `bin/ext.lkg`. A rollback commit makes `bin/ext` follow `bin/ext.lkg`.
- **`probe.rs`**: a process-wide `LaneHook` (`open`/`started`/`close`). `init-kernel` uses it to bind `probe/<id>/run/kernel.sock` before the probe spawns and to remove it before the root is deleted. It is a no-op when unset (tests, `sv-kernel`).
- **Bug fix in WP8b's seam**: `SupState.busy` mapped the body's own `state{busy}`. That would refuse every `initctl promote`/`rollback` issued from inside a turn with `E_BUSY`. It now means "a handoff or rollback is in progress", as the error text says.
- **Fixture `sv-agentbody`**:
  - answers `reload` with `reload_done`;
  - merges `ext/agent-plan.json` over `agent-plan.json`, so a t1 candidate (changes only under `ext/`) can carry behaviour.

## Tests (all $0, mock model only)
- `cd kernel && CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/k1 cargo test --offline -p init-kernel --test e2e`: **1 passed** (about 27 s). `initctl_status_and_t0_promote_under_init_fakeroot`:
  - the real `init-fakeroot` (phase0 charter, `--upstream unix:` to the real `init-mockllm`) starts the real `init-kernel`;
  - the body is the fixture (`sv-agentbody`) behind the kernel's real proxy;
  - `initctl ping`, `status` (lane, slot, mode, current/lkg = X, link, `jobs:[]`) and `level` (ledger L0);
  - `task list` gives `E_LEVEL`, and `fleet list` gives no members;
  - an owner line gets a tool round trip and "hello owner" back from the mock;
  - `reload` reaches `awaiting_boundary` at once and the job ends `succeeded` with `ok:true`; `reload --wait` gives `E_WOULD_DEADLOCK`;
  - **T0 promote**: `initctl promote evo/t0a --wait` (a SELF.md line plus the regenerated self.json) passes fetch, validator, the real build, G0 and G1 (the kernel's own `init-kernel selfmap`). Stages are G0 and G1 only. The job result is `scope:"t0"`, `handoff:false`, `lkg:true`; `main` and `lkg` = Y; `lkg.json.reason = "data_only"` with `lineage_seq`; the body's `boot_seq` is unchanged; the Root chain holds `promotion{lkg:true, scope:"t0"}` after the gate record; `status` shows Y/Y, normal and running;
  - `rollback` gives `E_ALREADY_LKG`, and `job list --all` shows the job;
  - the fake Root shuts down cleanly, and the socket is gone with the kernel.
  - Green twice in a row after the reload step was added.
- `cd kernel && CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/k1 cargo test --offline -p kernel-supervise`: **104 passed, 0 failed**.

  | Suite | Count | Note |
  |---|---|---|
  | lib | 66 | was 65; `extset::publish_swap_follow_report_gc` is new |
  | `slice1` | 10 | WP4's Slice-1 suite, unchanged and green; `supervise --under-root` without initctl is byte-for-byte the old path through `main_under_root_with` |
  | `promote` | 6 | the new **`sv26_t1_data_only_promotion_without_a_handoff`** (below) plus WP8b's 5 |
  | `handoff` | 8 | |
  | `recovery` | 5 | |
  | `process` | 3 | |
  | `probe` | 3 | |
  | `install` | 1 | |
  | `fakeroot_xcheck` | 1 | |
  | `serve_seam` | 1 | |

  `sv26_t1_data_only_promotion_without_a_handoff` covers:
  - G0–G3 pass, with no pause and no handoff stage;
  - `main` = Y and `lkg` still X;
  - Provisional with **the same primary instance**;
  - `supervisor.json` has `current.sha` Y and `binaries.current.built_from` X;
  - Root `promotion{lkg:false, scope:"t1"}`;
  - the asynchronous G4 accepts, then LKG = Y with `reason:"g4_pass"`, still with no restart.
- `--features fault-injection --test faults`: **2 passed**.
- `cargo clippy --offline -p kernel-supervise -p init-kernel --all-targets -- -D warnings`: clean. With `--features fault-injection` on kernel-supervise: clean.
- Homes: `kernel/target/test-homes/w7b/e2e` (and `wp7b/extset-<pid>`), removed by exact path. Every process is the test's own child (mock, fake Root; the fake Root's broker stops the kernel and bodies), stopped by its handle.

## Deviations
1. **The Backend lives in `init-kernel`**, next to `proxy.rs`, not in kernel-supervise. kernel-supervise therefore keeps no normal dependency on kernel-serve: it stays lane-independent, its artifact hashes stay the same, and the disk budget holds. The supervisor side (snapshot, commands, data-only, ext sets, lane hook) is in kernel-supervise.
2. **Same-uid peers only in a test kernel**: `proxy.test_mode = true` **and** `INIT_TEST_SERVE_PEER=same-uid`, a test variable the Root passes through (root/root.md §4.4). Otherwise it is `SandboxPolicy`. The canary path is derived from `paths.egress_sock`, because the boot block has no `ROOT_HOME`.
3. **G0 runs as a direct child of the kernel**, inside the kernel's own sandbox profile (WP8b deviation 1). **The fetch uses plain `git-upload-pack`** for the same reason (WP8b deviation 2; the `run/spawn.sock` relay is not built).
4. **`Env.level` is the level at kernel start**: the pipeline's `Env` is one per kernel. A level change mid-run reaches the proxy labels (`KernelGate` reads the current level), but not the gate record's `level` until the kernel restarts.
5. **A t0 on top of a provisional chain is not committed as LKG.** It stays provisional and gets the asynchronous G4 for the head, so a provisional binary never skips its G4. A t0_escalated whose synchronous G4 judged the whole tree against LKG still commits.
6. **A data-only promotion reports no `handoff` stage** in `stages`. Its progress uses `state:handing_off`, stage `handoff`, with the message "data-only promotion (no handoff)".
7. **The G-46 path for a t0** (gate record not acknowledged) leaves the version provisional with a pending record. `provisional_watch` then commits it through `g4_pass`, so `lkg.json.reason` is `g4_pass` there.
8. **Members and kernel candidates**: no fleet member lanes are bound (fleet is a stub). `fleet spawn` and `kernel promote` return their documented refusals, then `E_INTERNAL` "not built". `task submit`, `budget set`, `model set` and `vendor` keep serve's default `E_INTERNAL`.
9. **The binary links selfmap now** (`init-kernel selfmap`), so the debug `init-kernel` is about 154 MB.

## Needs from others
- **Root (WP21)**: create `run/mark/<instance>` for every body and probe instance (initctl.md §3.3), and keep `run/` writable for the kernel profile (`run/kernel.sock`, `probe/<id>/run/kernel.sock`). Optionally put `ROOT_HOME` (or the canary path) in the boot block (deviation 2).
- **WP6a / Root**: a broker-side G0 runner, and the `run/spawn.sock` relay for `sandboxed-upload-pack` (deviation 3).
- **Docs**: initctl.md: `promote`'s waited result has no `scope`/`lkg`; the job record's `result` carries them. supervise.md §3.6.8: record deviations 5 and 7.

## Left
- Fleet member lanes and L2 mechanics; K0–K3 and `system.swap`; `task submit` admission; `budget set`, `model set` and `vendor` steps 3–8.
- The 120 s staleness refusal is serve's (`E_ROOT{stale}`) and now works from the published ledger age. A kernel-side check for non-initctl paths is not added.
- SV-29: kept sets moving with a **binary** handoff (t2 that also changes `ext/`). Data-only t1 sets, LKG and rollback are done.
- A t1 test with real wasm components (the `ext/` workspace build); the t1 test here carries no component.
