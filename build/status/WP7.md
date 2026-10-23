# WP7: `kernel-serve`, the initctl server (lane T)

Status: **done** (2026-10-20). The server is complete against an in-test fake backend. It waits on WP4 to implement `Backend` and wire it into `init-kernel`.

## Done
`kernel/crates/kernel-serve/` implements initctl.md v0.3. It uses std threads only: one acceptor per lane socket, one thread per connection. Every frame goes through the `init-proto` types (WP1).
- **`server`**:
  - JSON lines over `run/kernel.sock` and the member and probe lane sockets. One request at a time per connection, and progress frames before the final response.
  - Line limits: a request over 64 KiB gets `E_PROTO` with a null id, then the connection closes. `v≠1` gets `E_VERSION` with the id echoed. Then `E_UNKNOWN_CMD` and `E_ARGS`.
  - A response over 1 MiB has its arrays truncated and gets `"truncated":true`.
  - Error messages are cut to one line of at most 500 bytes. Dotted codes (`trailer.*`, `gate.*`) and `details[]` pass through unchanged.
  - A backend panic is caught and answered with `E_INTERNAL`, and logged.
  - Sockets: a stale socket file is unlinked by exact path, but a file that is not a socket is never deleted. The socket path length is checked (103 bytes on macOS, 107 on Linux), and the socket's directory is set to 0700. `add_lane` and `remove_lane` work at runtime, and `shutdown()` closes everything.
- **`peer`**:
  - Credentials from `getpeereid` + `LOCAL_PEERPID` on macOS, `SO_PEERCRED` on Linux.
  - A peer with a different uid is closed silently.
  - `SandboxPolicy` applies the three-condition rule on macOS: the peer is sandboxed, can read the instance mark, and is denied the Root canary. It uses `sandbox_check` resolved through `dlsym` and fails closed. On Linux, attribution is by socket path.
  - On failure the client gets `E_PEER` with a null id, and `{pid, lane, reason}` is logged to `state/kernel/logs/kernel.jsonl`. `SameUidPolicy` is for tests only.
- **`limits`**: per lane instance, 8 connections (the 9th gets `E_RATE` and is closed), a token bucket of 10 req/s with burst 20 (`E_RATE` echoes the id and keeps the connection), and a 30 s idle close.
- **`gating`** (pure functions):
  - The §3.4 lane matrix. `promote --sha` is live-lane only and needs L2. `reload --wait` on the live lane gets `E_WOULD_DEADLOCK`.
  - Every §3.5 row in table order, from link/stale/outbox (`E_ROOT`) through halted, stopping, safe mode, provisional, depth, disk, busy and queue_full, kernel_candidate and quota, to `E_LEVEL` and `E_CEILING`. Nothing below the level reaches the backend or the Root.
- **`jobs`**:
  - Ids `j-NNNNNN`, monotonic across restarts (`jobs/seq` plus the records). Records in `state/kernel/jobs/<id>.json` are written atomically on state or stage change. The newest 200 are kept and older ones are deleted by exact path. At cold start, non-terminal jobs become `failed` with `reason:"kernel_restart"`.
  - Progress frames: every stage or state change is sent; otherwise at most 1 per second, and only when the progress changed. `pct` never decreases.
  - `wait` returns at a terminal state, `awaiting_boundary` or `awaiting_swap`. It detects when the client has gone (`MSG_PEEK`), and the job continues.
  - `job.status`, `job.wait`/`wait` (`--last`, `--cmd`, `timeout_s` → `timed_out:true`), `job.list` and `job.cancel` are all scoped to their own lane; another lane's job gets `E_LANE`.
  - Ordering guarantee: `JobCtl::awaiting_boundary` and `awaiting_swap` block until every response for that job has been written (bounded at 5 s), so `pause` follows the final response.
- **`quota`**: `vendor_day` 20, `proposals_day` 5, `task_submits_day` 10 and `fleet_spawns_day` 50. A unit is taken before the backend runs and refunded if the synchronous part fails. Counts persist in `state/kernel/serve-quotas.json`.
- **`paths`**: path arguments must have no `..`, no symlink in any component below the home, and must lie under the lane's allowed roots. A violation is `E_ARGS`; a symlink or missing path is `E_NOT_FOUND`.
- **`propose`**: `kernel propose` in full except the Root call. It reads the patch with `O_NOFOLLOW` (at most 1 MiB, UTF-8), checks that the text is at most 64 KiB, and writes `state/kernel/proposals/kp-NNNN/{proposal.json,patch.diff}`.
- **`vendor`**: steps 1–2 as pure helpers. `check_sources` gives `E_VENDOR_SOURCE` with `details[]` for git, registry, `[patch]`, `[replace]` or an outside `path` (IC-14). `add_dependency` adds `=x.y.z`.
- **Commands served by serve itself**: `ping`, `level` (cache under 60 s, else `refresh_ledger`; link down gives `E_ROOT`), `job.*`, idempotent `reload` and `promote --sha`, and the `status` overrides (`lane`, `jobs`, and `constitution_sha256` hashed at start).

## The `Backend` trait (for WP4; `src/backend.rs` has the full docs)
```rust
pub trait Backend: Send + Sync + 'static {
    fn view(&self, lane: &LaneCtx) -> KernelView;                       // §3.5 snapshot, from memory
    fn marks(&self, lane: &LaneCtx) -> Vec<PathBuf> { vec![] }          // run/mark/<instance> (macOS peer check)
    fn status(&self, lane: &LaneCtx) -> Result<StatusResult, ErrorBody>; // serve overwrites lane, jobs, constitution_sha256
    fn ledger(&self) -> Option<LedgerSnapshot>;                          // {ledger, ceilings, age_s} cache
    fn refresh_ledger(&self) -> Result<LedgerSnapshot, ErrorBody>;       // ledger.get + ceilings.get
    // Job commands: synchronous part only; keep `job` and drive it from your own threads.
    fn reload(&self, l: &LaneCtx, a: &ReloadArgs, job: JobCtl) -> Result<Begun, ErrorBody>;
    fn gate(&self, l: &LaneCtx, a: &GateArgs, repo: &Path, job: JobCtl) -> Result<Begun, ErrorBody>;
    fn promote(&self, l: &LaneCtx, a: &PromoteArgs, job: JobCtl) -> Result<Begun, ErrorBody>;
    fn rollback(&self, l: &LaneCtx, a: &RollbackArgs, job: JobCtl) -> Result<Begun, ErrorBody>;
    fn vendor(&self, l: &LaneCtx, a: &VendorArgs, job: JobCtl) -> Result<Begun, ErrorBody>;
    fn task_submit(&self, l: &LaneCtx, a: &TaskSubmitArgs, dir: &Path, job: JobCtl) -> Result<Begun, ErrorBody>;
    fn fleet_spawn(&self, l: &LaneCtx, a: &FleetSpawnArgs, job: JobCtl) -> Result<Begun, ErrorBody>;
    fn kernel_promote(&self, l: &LaneCtx, a: &KernelPromoteArgs, job: JobCtl) -> Result<Begun, ErrorBody>;
    fn cancel(&self, l: &LaneCtx, job: &str) {}                          // record already `cancelled`; kill instances
    // Immediate commands.
    fn task_template(&self, l: &LaneCtx) -> Result<Value, ErrorBody>;
    fn task_list(&self, l: &LaneCtx) -> Result<TaskListResult, ErrorBody>;
    fn task_show(&self, l: &LaneCtx, a: &TaskShowArgs) -> Result<Value, ErrorBody>;
    fn budget_set(&self, l: &LaneCtx, a: &BudgetSetArgs) -> Result<BudgetSetResult, ErrorBody>;
    fn model_set(&self, l: &LaneCtx, a: &ModelSetArgs) -> Result<ModelSetResult, ErrorBody>;
    fn fleet_list(&self, l: &LaneCtx) -> Result<FleetListResult, ErrorBody>;
    fn fleet_send(&self, l: &LaneCtx, a: &FleetSendArgs) -> Result<FleetSendResult, ErrorBody>;
    fn fleet_stop(&self, l: &LaneCtx, a: &FleetStopArgs) -> Result<FleetStopResult, ErrorBody>;
    fn kernel_status(&self, l: &LaneCtx) -> Result<KernelStatusResult, ErrorBody>;
    fn file_proposal(&self, l: &LaneCtx, title: &str, text: &str) -> Result<String, ErrorBody>; // proposal.file → root id
}
```
- Every method after `refresh_ledger` has a default that returns `E_INTERNAL` ("not implemented in this kernel build"), so a backend can grow command by command.
- `Begun` is either `Started(fields)` or `Existing(job_id)`. `Started` takes fields such as `sha` for promote, gate and kernel.promote, `from`/`to` for rollback, and `bid` for fleet.spawn. Build it with `Begun::started().with("sha", sha)`.
- `KernelView` holds `link_up`, `ledger_age_s`, `level`, `outbox_backlog`, `halted`, `stopping`, `safe_mode`, `primary_provisional`, `provisional_depth`/`max_provisional_depth`, `disk_low`, `busy: Option<BusyInfo{job, reason}>`, `kernel_candidate` and `bodies: Option<BodyCount{limit, used}>`. Active initctl `promote`, `rollback` and `kernel.promote` jobs are counted by serve; `busy` covers only the rest (an owner `/reject`, a G4 rollback at idle, a thinning baseline).
- `JobCtl` methods:
  - identity and state: `id()`, `is_cancelled()`, `is_finished()`, `record()`;
  - reporting: `progress(state, stage, pct, eta_s, msg)`, `stage_done(StageResult)`, `set_sha`, `set_root_job`;
  - milestones: `awaiting_boundary()` (blocks until the responses are flushed; send `pause` after it), `set_awaiting_boundary()` (non-blocking, for the synchronous part of reload and rollback), `wait_flushed()`, `awaiting_root(Some("s-…"))`, `awaiting_swap()`;
  - exactly one terminal call: `succeed(&T)`, `succeed_value(v)`, `fail(ErrorBody)` or `fail_with_reason(..)`.

  Every call after the job is terminal or cancelled is ignored.
- Wiring:
  - at kernel start, `let srv = kernel_serve::start(ServeConfig::new(&home, VERSION), backend, Arc::new(SandboxPolicy{canary}))?; srv.add_lane(LaneSpec::live(&home))?`;
  - per probe, `srv.add_lane(LaneSpec::probe(&home, id, purpose))`, and `remove_lane("probe:<id>")` when it ends;
  - per member, `LaneSpec::member(&home, bid, &member_root)`;
  - on `Stopping`, `srv.shutdown()`;
  - for asynchronous G4 jobs, `srv.jobs().create(&LaneCtx::live(), "g4", args)`;
  - `srv.recovered()` lists the jobs marked `kernel_restart`, whose candidate refs must be deleted (§3.7).
- WP4's `promote::Job` trait maps onto `JobCtl` 1:1:
  - `id()` → `ctl.id().into()`
  - `cancelled()` → `is_cancelled()`
  - `progress(s, st, p, m)` → `progress(s, st, p, None, m)`
  - `awaiting_boundary()` → `.is_ok()`
  - `succeed(v)` → `succeed_value(v)`
  - `Jobs::create` → `srv.jobs().create(..)`

## Tests
`cd kernel && CARGO_TARGET_DIR=target/t cargo test --offline -p kernel-serve`: **47 passed, 0 failed** (18 unit, 29 integration). The suite was green 5× in a row, again with 3 concurrent runs of the integration binary under load, and single-threaded. `cargo clippy --offline -p kernel-serve --all-targets -- -D warnings` is clean.
- The integration tests (`tests/ic.rs`) drive the **real `initctl` binary** from WP1. It is built once by the harness with `$CARGO build -p initctl` into the same target dir, or taken from `INITCTL_BIN`. They run against the server and the fake backend in `tests/common/mod.rs`.
  - Raw socket clients are used only for frame-level cases.
  - Homes live under `kernel/target/test-homes/wp7/<tag>-<pid>` and are removed by exact path.
  - The only processes spawned are `initctl`, killed by its own PID if it hangs, and that one cargo build.
- Cases covered:
  - **IC-2, IC-3**; **IC-4**: the full 25 commands × 4 lanes × L0–L3 matrix, where E_LEVEL leaves no backend call and no spawn or system.swap; **IC-5**: E_PEER with the real `SandboxPolicy` and the log line (macOS);
  - **IC-7**; **IC-8**; **IC-9**: initctl is killed mid-G0, the job continues, and `wait` reattaches with progress; **IC-10**; **IC-13**; **IC-15**: 9th connection, burst and refill, and per-lane isolation; **IC-16**: kernel gone gives exit 69, restart works, and the job is `kernel_restart`;
  - **IC-18**: a symlink gives E_NOT_FOUND, the files are written, the alias works, and 5/day then E_QUOTA; **IC-19**; **IC-20**; **IC-21**: integers only, the constitution sha matches, `root` is byte-equal, and link, fleet and soft budgets are present;
  - **IC-22**: at `pause`, the final response is already in the client's socket buffer; **IC-23**; **IC-24**; **IC-25**; **IC-26**; **IC-27**; **IC-28**; **IC-29**;
  - also: job cancel, list, timeout (exit 2) and lanes; every §3.5 row end to end with exit codes; gate pass/fail exits; dotted gate and trailer codes with `details[]`; reload idempotency and E_NO_BODY; rollback with E_ALREADY_LKG; idle close; stale-socket handling.
- Unit tests: gating rows in order, job store (ids and restart, retention, wait with flush ordering, cancel, timeout and hangup), limits, quota, paths, vendor checks (IC-14), truncation, clock and peer.

## Deviations
1. The immediate answer of a `wait:false` job command reports `state:"queued"` even if the backend's thread has already started, as §4.1 writes. A milestone or terminal state already reached is reported as is: reload and rollback set `awaiting_boundary` in their synchronous part.
2. The ordering guarantee is stricter than §4.1: **every** job command response counts as a pending final, not only `wait:true` ones. A `pause` therefore never overtakes even a non-waiting `promote` answer.
3. Idempotency:
   - `promote --sha S` while a job for S is active returns that job;
   - `promote <ref>` during any pipeline gets `E_BUSY{job}`, because the SHA is known only after the fetch. The backend may still return `Begun::Existing`.
   - A pending reload of the same lane is returned before the backend is called.
4. Serve counts the quotas `vendor_day`, `proposals_day`, `task_submits_day` and `fleet_spawns_day` per **UTC day** (`resets_at` = next 00:00Z). The evolution count keys stay with the backend. The quota row is checked in §3.5 order, before `E_LEVEL`.
5. Job ownership is per lane instance (`live:b0`, `member:<bid>`, `probe:<id>`) and held in memory. After a restart, only live-lane records keep an owner; older member and probe records answer `E_LANE`. Recovered records carry `error:null` and `reason:"kernel_restart"`, and `wait` on them exits 1.
6. `level` with the link down returns `E_ROOT` even when the cache is fresh (fail closed; IC-23). `fleet spawn` with `parallel_bodies` unknown returns `E_ROOT{stale}`.
7. Path arguments resolve against the lane's home: `INIT_HOME`, or the probe root on a probe lane. Member roots are given by the backend in `LaneSpec::member`. Error codes:
   - outside the roots, `..`, non-UTF-8 or a directory/file mismatch: `E_ARGS`;
   - a symlink or a missing path: `E_NOT_FOUND`;
   - a patch over 1 MiB, or rationale + patch over 64 KiB: `E_TOO_LARGE`.

   Proposal files are written after the Root accepts. If the local write then fails, the answer is `E_INTERNAL`, naming the Root id.
8. `E_PEER` and internal errors are logged to `state/kernel/logs/kernel.jsonl` as `{"c":"serve","ev",…}`, per initctl.md §3.3. Supervise's `klog` writes to stderr, so there are two sinks.
9. `reload --wait` is refused only on the live lane, as specified. On member and probe lanes it returns at `awaiting_boundary`.
10. `E_RATE` is checked after the line is parsed, so the id can be echoed. A connection over the cap gets `E_RATE` with a null id after the peer check.

## Needs from others
- **WP4 (kernel-supervise):**
  - implement `Backend` (shape above) and wire `start`, `add_lane`, `remove_lane` and `shutdown` into `init-kernel supervise`;
  - fill `marks()` from the Root's `spawn` results;
  - use `JobCtl` for its `promote::Job`/`Jobs`;
  - delete the candidate refs of `recovered()` jobs;
  - place the `serve.promote.after_fetch` fault point in its promote synchronous part (§3.7).
- **Vendor job owner (WP4/WP8):** steps 3–8 of `vendor` (local registry over the egress `crates` class, offline resolve, `cargo vendor`, delta, lineage record) and IC-14b. Steps 1–2 are in `kernel_serve::vendor`.
- **initctl.md owner:** record deviations 1–4 and 6.

## Left
- An end-to-end test with the real supervise backend and `init-fakeroot`, after WP4.
- IC-12 (Linux pid namespaces) needs Linux. IC-6 belongs to the sandbox (WP6a) and IC-11 to the client (WP1).
- A full `cargo build --offline` of the workspace was not run in `target/t`: it would build wasmtime, and free disk was at 5.6 GB against the 5 GB floor. `init-kernel` uses only `kernel_serve::CRATE`, which is unchanged.
