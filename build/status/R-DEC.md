# R-DEC: provisional owner decisions in Root code, Root fix-code rows, review F1/F4/F6 and two follow-ups

Lane R-DEC, 2026-10-20. Status: **done**. Two rows were R-SAFE's because they sit in panic.rs and zone.rs: R-SAFE did §5 L5 (7a460a6) and tagged WP24 D7 v0.1.x (see "Left").
Inputs: RELEASE_PLAN.md (provisional decisions), docs/reconciliation/build-deviations.md §1.3–§1.5, §3.9, §3.13,
§3.15 and §5, build/review/root-security-2026-10-20.md (F1, F4, F6), and the orchestrator's follow-ups (F3 on sealed
lanes; `supervise` killpg ordering under R-SAFE's `sys::signal`).
Commits: cdb49e8, 518e68e, 47c618f, d524407 (docs), 3b08245 (sealed-lane F3, `supervise`), plus the commit that adds this file.
Every commit holds only this lane's hunks. Shared files (egress.rs, classes.rs, zone.rs) were committed as HEAD plus
replayed exact edits, and each tree was `cargo clippy -D warnings`-checked in an export under three feature sets
(test-fixtures, none, fault-injection) before it was committed.

## Done

1. **§3.13 Root money meter (provisional owner decision).**
   - The tap reports `Usage {input, cache_read, output}`:
     - DeepSeek `prompt_cache_hit_tokens`;
     - OpenAI `prompt_tokens_details.cached_tokens` and Responses `input_tokens_details.cached_tokens`;
     - Anthropic `cache_read_input_tokens`.
   - Cost = (input − cache_read) × input price + cache_read × cache-read price + output × output price, at the **peak** prices, rounded up once.
   - Model key `cache_read_musd_per_mtok` is optional, and C11 checks it. Absent, hits bill at the input price, which is the old safe default.
   - The built-in `deepseek-flash` and the charter template set it to 6,000 µUSD/MTok (kernel/config.md peak `cache_read` $0.006).
   - The `end` record's `usage` now carries `cache_read`.
   - **S1's real usage** (unit `tap::tests::s1_usage_at_the_cache_read_price`):

     | | Old Root | New Root | Proxy |
     |---|---|---|---|
     | Turn 1 (0 hit / 4,474 miss / 81 out) | 1,440 | 1,440 | |
     | Turn 2 (4,480 hit / 190 miss / 2 out) | 1,404 | 87 | |
     | Total, µUSD | 2,844 | **1,527** | 762 |
     | Root/proxy | 3.73× | **2.00×** | |

     The remaining 2.00× is exactly the peak/off-peak factor, which the Root keeps on purpose, so it stays an upper bound.
2. **Review F1 (HIGH) and F4 (LOW).**
   - `Money::admit` sums are saturating. The sealed lane's budget check and hold happen in one `fetch_update` with `checked_add`.
   - `money::reservation` clamps `max_out` at 2^20 tokens.
   - The held reservation counts input at one token per request byte. The provider cannot bill more input than that, so the actual cost stays within the reservation and the ceiling is a hard wall.
   - D6 rule (c) 3 still uses the kernel guard's `ceil(bytes/3)` figure, which the kernel proxy mirrors, so an honest guard is never judged a bypass.
   - Tests:
     - unit `overflowing_reservations_are_refused_and_max_out_is_clamped`;
     - wp22 `money_ceilings…`: a `max_tokens = u64::MAX` request with spend booked is 429 `ceiling.day` and not forwarded.
3. **§3.15 small S (provisional owner decision).**
   - `sealed import` refuses a resulting S below 12 tasks plus a canary before any check runs. Tasks admitted by an import that still ends below the floor are removed by exact path.
   - A gog that finds fewer than 12 tasks, or no canary, after its leak scan runs nothing. It appends `mutation{pass:null, underpowered:true, sample_n, source, spent_musd:0}` and alerts `gog_underpowered`.
   - R7 never treats it as a trigger and does not count it as a mutation run.
   - wp24 `small_s_is_refused_at_import_and_underpowered_at_the_gog` checks:
     - an 11-task import is refused, and nothing is imported;
     - SE-02 on a 13-task S (a byte clone of `sx-upper` under a new id) leaves 12, and the full 8 + 4 gog still runs, `pass:true`;
     - a second leak leaves 11: `underpowered` with the alert, `spent 0`, and no `ladder` record, though the candidate is agent-built;
     - control: an agent-built `pass:false` in the same Root demotes.
   - MS-G1…G3 still fail. The first three variants passed in this lane's run; the full rerun is in "Tests".
4. **WP24 N.** R7 demotes on `mutation{pass:false}` only when `source == "agent"`. A maintainer-built failure alerts only. The existing wp25 RT-L7 record now says `source:"agent"`.
5. **§3.9 repeated demotion (provisional owner decision).**
   - The third `ladder down` from L0 within 24 h, counted from the lineage, puts the restart on the crash-loop path: audited-good tuple, system safe mode. The alert is `demotion_loop`, sent once.
   - From safe mode, the existing escalation applies: a crash loop leads to `Halted{crash_loop_safe}`, and 24 h healthy clears it.
   - Tests: wp25 `rt_l10_three_demotions_at_l0_in_24h_enter_safe_mode` (boots 1 and 2 normal, 3 and 4 safe), and unit `repeated_demotion_at_l0`.
6. **WP21 D9.**
   - The `Halted{crash_loop_safe}` cap of 4 retries per day uses the charter's owner day (`egress::owner_day`).
   - Retries are counted from `halt{on:false}` records, so a restart cannot reset the count.
   - A standing halt survives a Root restart with its `retry_at`. "Standing" means its newest `halt{on:true}` has no later `halt{on:false}`, `stop`, `resume` or `zone_start`.
   - Tests:
     - unit `halts_fold_from_the_records`, at +07:00, where the cap differs from the UTC day;
     - wp21 `crash_loop_safe…`: the daemon is killed and restarted, and it stays `halted` in safe mode with no new kernel boot.
7. **WP21 D10 and D12.**
   - `state/root.json` gets `counters`, folded from the records with the level (`crashes_total`, `rollbacks`, `swaps_ok`, `swaps_failed`), and `crash` (`backoff_until`, `consecutive`, `window`).
   - At start, `crash` is seeded from the previous file: window entries inside `crash_window_s` are kept, and a pending backoff is waited out.
   - Kernel stdout and stderr go through a pipe relay (`sup/klog.rs`). Each file is capped at 4 MiB; when the next write would pass the cap, the relay keeps the newest half from a line start, behind a marker. Ten files are kept, and the relay never blocks the kernel.
   - Tests: unit `kernel_logs_are_capped_and_rotated` (10 MiB in, ≤ 4 MiB out, whole lines, 0600, the 11th log removes the 1st) and `root_json_counters_fold_from_the_records`; wp21 checks `counters` and `crash` in root.json.
8. **Review F6.** The fd stash keeps at most 64 descriptors (oldest dropped first) for at most 10 s. A datagram whose fd count differs from its tag's `n` is closed at once. Unit test: `the_fd_stash_is_bounded`.
9. **WP23 D3.** New `fault-injection` feature, which `test-fixtures` turns on. Lineage, mirror and kernel.git fault points are compiled under it. The export check builds it alone too.
10. **WP24 D6.**
    - The sealed lane writes an fsynced `start` record (`id <run>-<n>`, `lane:"sealed"`, `reserved_musd`) before each upstream call. Its `t:"sealed"` line carries the same id.
    - Σ sealed spend plus holds must stay ≤ `sealed.reserve_usd`; otherwise the request gets 429 `ceiling.total`. Σ sealed is seeded per process from the ledger, and a start with no line is charged its reservation.
    - Sealed lines stay out of the zone's rebuild and checkpoint.
    - Tests: unit `the_sealed_reserve_is_cumulative`; wp24 aa asserts the start/line pairs.
11. **WP24 D8.**
    - Each kernel-reported record is leak-checked as it is appended. This costs one directory listing when nothing matches, and the full scan still runs at every audit and gog.
    - Stale run roots are removed at Root start (`sealed::clean_stale`, which signals nothing).
    - Test: wp24 sa12, where a fake-kernel `lineage.append` naming `sx-cubes` drops it at once.
12. **§5 L4.** `kgit::import_release` creates `refs/heads/lkg` together with `main`. Tests: wp23 `install_imports…` and the kgit unit test.
13. **Follow-ups from the orchestrator.**
    - Sealed lanes take R-SEC's F3 pattern: at most 32 connections, 503 `root.busy` beyond that, timed reads and writes through `egress::Timed`, and a spawn guard. Unit test: `lane_connections_are_capped`.
    - `sealed::run::supervise` kills the group while the leader is an unreaped zombie, and only then reaps it. A WNOWAIT waiter thread watches for the exit. Before this, the post-reap `killpg` was a no-op under R-SAFE's `sys::signal`, so a leader's leftover group survived. Unit test: `the_group_is_killed_before_the_leader_is_reaped`.
14. **Docs** (d524407, exact edits, marked "provisional owner decision 2026-10-20" or with the row they close):
    - root.md: §3.6.6 halt row, §3.5.1 kernel log, §3.12.2 template, §3.13.3 trigger table and step 4, §4.3 step 6, §4.5 root.json.
    - egress.md: §3.4 formula and metering bullet, §3.5 model schema, §3.8 sealed lanes, §5 rows.
    - sealed-eval.md: §3.1 refresh and leak check, §3.3 floor, §4.1 record.

## Tests
All runs below are on the committed tree (3b08245; `git diff HEAD -- root/crates` was empty), through `ci/slot.sh`, with `source ci/env.sh` (2 jobs and 2 test threads). The mock is `INIT_MOCKLLM_BIN=kernel/target/debug/init-mockllm`. Cost $0 throughout, and no real model was used.

| Command | Result |
|---|---|
| `cargo test --offline -p init-root --features test-fixtures --no-fail-fast` | **136 passed, 0 failed** (exit 0) |
| `cargo test --offline` (root workspace) | **green** (exit 0): init-root unit 83 (the integration files are fixture-gated), rsafe 3, init-sandbox-profiles 10 + doc_drift 2 + params 4 + selftest 1 |
| `cargo clippy --offline -p init-root --all-targets -- -D warnings`, with `--features test-fixtures`, with none, and with `--features fault-injection` | **clean** on every committed tree (exports of HEAD plus this lane's blobs) |

The fixtures run, by target:

| Target | Passed | Time |
|---|---|---|
| unit | 86 | |
| fix5 | 1 | |
| owner_frames | 2 | |
| rsafe | 6 | |
| rsec | 2 | |
| wp20 | 9 | |
| wp21 | 6 | |
| wp21b | 2 | |
| wp22 | 8 | |
| wp23 | 4 | |
| wp24 | 6 | 696 s |
| wp25 | 4 | |

New tests: tap `s1_usage_at_the_cache_read_price`; money `overflowing_reservations_are_refused_and_max_out_is_clamped` and `the_sealed_reserve_is_cumulative`; classes cache-read key checks; ledger `repeated_demotion_at_l0` and `root_json_counters_fold_from_the_records` (and agent-only and underpowered cases in the RT-L7 and L3 tests); sup `halts_fold_from_the_records` and `the_fd_stash_is_bounded`; klog `kernel_logs_are_capped_and_rotated`; lane `lane_connections_are_capped`; run `the_group_is_killed_before_the_leader_is_reaped`; wp24 `small_s_is_refused_at_import_and_underpowered_at_the_gog`, plus D6 and D8 assertions in aa and sa12; wp25 `rt_l10_three_demotions_at_l0_in_24h_enter_safe_mode`; wp21 restart and root.json assertions; wp22 F1 request; wp23 and kgit `lkg` assertions.

Flakes seen while lanes ran the same suites at once, all re-run green:
- wp24 `gogbad` once lost `sealed/M/base` when another lane's run of the same fixed-name test home deleted it.
- wp24 `small_s` once ran against a `target/debug/init-root` that a plain build had just replaced with a binary without fixtures.
- The lineage unit tests `mirror_chunks` and `rr16` failed once under load average 274, and passed alone.

## Deviations
1. An owner `resume` from `Halted{crash_loop_safe}` writes the same `halt{on:false}` record as an automatic retry, so it counts toward the 4-a-day cap. This is the conservative reading; the record format was left unchanged.
2. Only `Halted{crash_loop_safe}` is restored at a Root restart. A `Halted{config}` retries at once, because a restart is usually how a fixed `config.toml` gets applied. Integrity halts are re-detected at start.
3. The underpowered gog record is `pass:null` plus `underpowered:true`; every normal gog record now carries `underpowered:false`. The floor also requires at least one canary, since M4 and M7 need it.
4. The sealed reserve rule follows the fix-code row (Σ sealed ≤ `reserve_usd`). egress.md's table says sealed lanes run "up to `money.total_usd`"; with the zone held to total − reserve, the two agree on the hard total.
5. F4 is fixed in the held reservation (one token per byte) rather than with a re-gate at settle. The kernel guard's figure is unchanged, so this needs no kernel change.
6. `counters` live in the R7 fold (`ledger::Level`), because that is the per-record fold the Root already keeps. `crash.window` is written as RFC 3339 strings.
7. §3.9's safe-mode boot keeps `reason:"demotion"` in the boot block and in the `safe` record. Only the `demotion_loop` alert names the repeat, because adding a `Plan` field would touch panic.rs (R-SAFE).
8. New integration tests use per-process home names (`s<pid>`, `r<pid>`) and remove their homes on success, because lanes running the same suite in this shared checkout clobbered each other's fixed homes.

## Needs from others
- **R-SAFE:** nothing open. §5 L5 is done (7a460a6). WP24 D7 has a design note in build/status/R-SAFE.md, tagged v0.1.x.
- **S-REAL:** `scripts/slice1/s1.sh`'s charter has no `cache_read_musd_per_mtok = 6000`, so a real batch on it still bills hits at the input price, the old 3.7×. That is safe but conservative. Add the key to re-check §3.13's ratio on real traffic (§3.17).
- **DOCS:** mark build-deviations §1.3–§1.5 and §5 L4 fix-code rows as fixed (commits above), and §3.9/§3.13/§3.15 as taken. testing.md could adopt `rt_l10`, the wp24 small-S test and EG-F1 as named cases.

## Left
- WP24 D7 (sealed spawns in the broker's table) is v0.1.x, per R-SAFE's design note. Its gap is bookkeeping, not a quota or safety hole: sealed pids are held and killed only through `sys::signal`.
- WP7 D4 (the owner day for initctl serve quotas) is kernel code, not Root code.
- A real-model check of the 2.00× ratio waits for S-REAL's batch.
