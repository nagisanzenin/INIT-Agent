# R-SEC: Root egress fixes from the security review (F2, F3, F7)

Lane R-SEC, 2026-10-20. Status: **done**. Input: build/review/root-security-2026-10-20.md. Paths:
`root/crates/init-root/src/egress.rs`, `root/crates/init-root/src/egress/classes.rs`, `root/crates/init-root/tests/rsec.rs`,
the matching rows of `docs/root/egress.md`, this file. `egress/money.rs`, `egress/lane.rs` and `egress/tap.rs` were not touched (R-DEC).

## Done

1. **F2, default-deny metering (classes.rs `ROUTES`, `billed`, `route`).**
   - Every model-class request must match an explicit `(provider, method, path)` row. Each row is either billed or free.
     - A billed row is inspected, gated by model and level, reserved and settled, on the wire family `Family::of` its path.
     - A free row costs $0. The free rows are `GET models`, `GET user/balance` (S1's balance read), `GET v1/models` and Anthropic `POST v1/messages/count_tokens`.
   - The rows mirror kernel/proxy.md §3.5's preset tables (deepseek, anthropic, openai). A provider without a preset gets generic rows: the completion routes of each family, and `models`.
   - `route` refuses a class-listed route with no row with the new typed code `class.unmetered` (403, `x-init-egress-error`, a `refuse` ledger line). It is refused before any key is attached and is never forwarded. The method is part of the rule, so `GET chat/completions` and `POST user/balance` are refused too.
   - `check` meters on `Route.metered`. The old `c.model && family != None && POST` heuristic is gone.
   - C11 (`validate_actions`) warns once per listed model path that has no row for any of the class's providers. It is a warning, not an error, so existing charters still load.
2. **F3, bounded connections (egress.rs).**
   - Cap: `MAX_CONNS` = 96 open kernel connections. The 97th is answered 503 `root.busy` by the accept thread, which spawns nothing for it.
   - The count is released by an `Open` guard. The guard drops on thread exit, on a panic unwind, or when a thread spawn fails (`thread::Builder`, so a failed spawn no longer kills the accept loop).
   - Accept errors (EMFILE) back off 50 ms instead of spinning.
   - Timeouts (`Timed` reader):
     - 120 s idle for the first byte of a request; the connection is then closed without a reply;
     - 30 s for the rest of its head and body; then 400 `request.invalid`;
     - 120 s per stalled response write.
   - Each connection now holds one descriptor (`Arc<UnixStream>` shared by the reader, the writer and `panic`) instead of three.
3. **F7.** `GET /_root/health` and `GET /_root/ceilings` now run inside the same `MAX_INFLIGHT` (64) guard as forwarded requests, and are refused `root.busy` above it.
4. **Docs.** docs/root/egress.md got small edits: the §3.1 wire and connection-bound bullets, the §3.5 `paths` row, the §3.6 D6 "not a hit" row, a §5 failure row, and §8 EG-10 and EG-11.

## Tests

- Unit tests (`cargo test --offline -p init-root --bin init-root egress`): 26 pass, including these new ones:
  - `classes::unmeterable_routes_are_refused`;
  - `egress::every_forwarded_model_route_is_metered_or_free`;
  - `egress::local_answers_are_behind_the_inflight_bound`;
  - `egress::connection_reads_are_bounded_in_time`.
- `tests/rsec.rs` (test-fixtures, a test-mode daemon against a loopback upstream with fake keys): 2 pass.
  - `unmeterable_paths_are_refused_and_free_ones_cost_nothing`:
    - `embeddings`, `completions`, `GET chat/completions` and `POST user/balance` are refused 403 `class.unmetered`;
    - the upstream saw none of them, there was no `ceiling_hit`, and there are 4 `refuse` lines;
    - `GET user/balance` was forwarded with the key, with `reserved_musd` 0 and cost 0;
    - a completion was reserved and charged.
  - `a_connection_flood_stays_bounded_and_owner_and_panic_still_work`:
    - of 136 held partial-head connections, 96 were served and 40 got `root.busy`;
    - the Root's thread count stayed within base + 96;
    - `/_root/health` got `root.busy`;
    - `owner.sock` `status` answered, and `panic` closed all 96 held connections;
    - afterwards `root.panicked` was answered.
- Full runs (in a build slot, `source ci/env.sh`, at load ≈ 10–60):
  - `cargo test --offline` in root/: all green. init-root has 79 unit tests and sandbox-profiles all pass.
  - `cargo test --offline -p init-root --features test-fixtures --no-fail-fast` (`INIT_MOCKLLM_BIN=kernel/target/debug/init-mockllm`): every suite is green except one, 82 unit tests plus fix5, owner_frames, rsafe, rsec, wp20, wp21, wp21b, wp22 (8), wp23 and wp25.
  - The exception is wp24 `import_audit_aa_and_no_leak` (5 of 6 pass). It asserts that every usage line is `t:"sealed"`, but R-DEC's uncommitted egress/lane.rs now writes a synced `start` line before each sealed upstream call. Their uncommitted wp24.rs already changes that assertion; the test binary was built mid-edit. This is not an R-SEC path.
  - This run used the shared `root/target`, before the orchestrator's `root/target/fx` rule. A re-run in `fx` was skipped: a new target dir would have cost about 2 GB with 6.2 GB free against the 5 GB floor.
  - A `sys::tests::live_table_reads_our_own_entry` failure seen once in an earlier run (R-SAFE's uncommitted sys.rs) passed in this run.
- `cargo clippy --offline -p init-root --all-targets --features test-fixtures -- -D warnings` and `cargo clippy --offline -p init-root -- -D warnings`: clean.

## Deviations

- `class.unmetered` is **not a D6 hit** (no demotion, no `ceiling_hit`). The owner listed the path; the gap is the Root's, as with `root.unsupported`. A path the class does not list is still `class.path`, which demotes under rule a.
- `MAX_CONNS` is 96, not 2 × 64. This keeps the descriptor budget (96 connections + ≤ 64 upstream + a pool of 16 ≈ 176) under launchd's default soft limit of 256 (`launchctl limit maxfiles` = 256), because the Root never raises `RLIMIT_NOFILE`.
- Local GETs are bounded by the shared in-flight guard; they are not cached.
- Size: by rootloc, egress without money.rs is ≈ 1,457 at this commit, against the 1,460 sub-budget. R-DEC's cdb49e8 already took it from 1,385 to 1,411; this lane adds about 46.

## Needs from others

- **sys.rs / install.rs owner:** raise the Root's soft `RLIMIT_NOFILE` (for example to 4,096, or `SoftResourceLimits.NumberOfFiles` in the LaunchAgent plist). F6's fd stash (≈ 512 descriptors) plus the egress can pass 256 under launchd. The egress now backs off on EMFILE, but `owner.sock` and the ledger would also fail at the limit.
- **R-DEC (egress/lane.rs):** the sealed lane's accept loop has the same unbounded thread-per-connection shape and no read timeouts. Only the sealed body can reach it, so the risk is lower. `Timed` and `Open` in egress.rs can be reused.
- link.rs `serve` (owner socket) has the same shape, but it is owner-only and not a zone vector (the review agrees).

## Left

- Nothing for F2/F3/F7.
