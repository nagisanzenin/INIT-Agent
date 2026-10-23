# WP1: `init-proto` and `initctl`

Status: **done** (2026-10-20, lane K1).

## Done
- **`kernel/crates/init-proto`**, which depends on serde and serde_json only. It re-exports `serde_json`, so `initctl` stays "std + init-proto". It has these modules:
  - `icj`: ICJ v1 encoding through `Value`, so keys are sorted. It refuses floats, reporting the JSON pointer, and caps depth at 32. It also has `canonicalize` and `is_canonical`, a `V1` marker (it serializes as `1`, and any other `v` fails with "unsupported version"), and `Versioned<T>`, which adds and checks `v` for frame enums.
  - `envelope`: `Request` (`parse_line` checks in the server's order: E_PROTO, then E_VERSION, then id, cmd and args), `Response` (`id` may be null, as in E_PEER), `ProgressFrame`, the client-side `Frame` parser, the control-link `Event` and `LinkFrame`, the line limits, and the per-lane limits.
  - `errors`: all 39 `E_*` codes plus the dotted codes, each with its exit code and its retry rule. It also has the `exit::*` constants and typed `error.data` (`LevelData`, `CeilingData`, `QuotaData`, `BusyData`, `RootData`, `GateFailedData`, `KupgradeFailedData`, `BudgetData`). There are constructors `ErrorBody::level`, `ceiling`, `root` and `busy`.
  - `initctl`:
    - all 25 commands as `CommandName`, with the §3.4 level and lane columns and the §3.5 state-gating predicates;
    - a typed `Command` with each command's args, parsing (E_UNKNOWN_CMD or E_ARGS), validation and wait defaults;
    - every result struct;
    - the job model: `JobState` with the `wait_returns` and `cancellable` milestones, `Stage`, `Progress`, `RecordProgress`, `StageResult` and `JobRecord`.
  - `files`: `LkgFile`, `BootFile` (including `SandboxStatus`), `UpdateFailedFile`, `SupervisorFile`, `ReadyFile`, `HeartbeatFile` and `LineageLocalRecord` (`vault/lineage.jsonl`), plus `encode`/`decode` and the path constants.
  - `stdio`: `ToBody` and `FromBody` for stdio-jsonl/1. An unknown `t` parses as `Unknown`, and the codec adds and checks `v`.
  - `root_link`:
    - every kernel→Root request: `hello`, `ready`, `spawn`, `kill`, `instances`, `store.put`, `store.get`, `tuple.get`, `lineage.append`, `ledger.get`, `ceilings.get`, `system.swap`, `proposal.file`, `sealed.*` and `alert`, each with its typed result;
    - the boot block;
    - `Ledger`, and `Ceilings` (the money block plus broker counts; unknown fields are kept for verbatim relay);
    - the Root→kernel requests `stop`, `prepare_swap` and `owner.in`, and events `exited`, `level`, `mode`, `ceilings` and `swap`, where an unknown event is tolerated;
    - the kernel→Root events `hb` and `owner.out`;
    - `FdTag`, `RootErrorCode` (13 `root.*` codes) and the kernel exit codes.
- **`kernel/crates/initctl`**: a std-only CLI (`cli.rs` for args, `client.rs` for transport and exit codes, `gitsnap.rs` for `gate --quick` resolution).
  - It covers the whole §4.5 command list.
  - Socket selection: `--sock`, then `INIT_KERNEL_SOCK`, then `INIT_HOME/run/kernel.sock`. Connecting times out after 2 s.
  - It makes one connection, reads lines of at most 1 MiB, and prints progress on stderr as `[job stage pct% eta Ns] msg`.
  - stdout gets one pretty JSON document with sorted keys. `--json` prints the raw frames instead, and `--quiet` drops progress.
  - Exit codes follow the sysexits table exactly.
  - `gate --quick` resolves its target in the doc's four steps. A dirty tree is snapshotted through a temporary `GIT_INDEX_FILE`, and `status` runs with `--no-optional-locks`.

## Tests
`CARGO_TARGET_DIR=target/k1 cargo test --offline -p init-proto -p initctl` (in `kernel/`): **60 passed, 0 failed**. The integration suite ran 3× in a row with no flakes. `cargo clippy --offline -p init-proto -p initctl --all-targets -- -D warnings` is clean.
- init-proto unit tests (35): ICJ golden vectors (control characters, `/`, U+007F, non-ASCII, nested order), float and depth refusal, the `v` marker, the error and exit table, the §3.4 level and lane matrix (IC-20 included), the §3.5 gating rows, argument validation, IC-2 (64 KiB + 1 gives E_PROTO), IC-3 (`v:2` gives E_VERSION with the id echoed), and E_PEER with a null id.
- init-proto `tests/golden.rs` (6):
  - **IC-1**: all 46 doc examples in `tests/fixtures/doc-examples.tsv` (initctl.md §4.1/§4.4, supervise.md §4.3–§4.7, sandbox.md §4.2, root.md §4.1/§4.5, egress.md §4.2) round-trip through the typed structs **byte-equal**.
  - A hand-written canonical example for every command's request and result, every stdio frame, and every control-link request, event and result.
  - A `v:2` error for every kernel file.
- initctl unit tests (6): argument parsing to the exact JSON line for every CLI form (35 cases), usage errors, progress format, exit mapping and socket selection.
- initctl `tests/cli.rs` (13), run against an in-test fake socket:
  - **IC-17**: every row of the exit-code table and every E_* code in each row;
  - IC-7, IC-8 (shape), IC-10, IC-13 and IC-20 (client side);
  - **IC-11**: a dirty-worktree snapshot is sent, while the index bytes, refs and `git status` stay unchanged and the temporary index is removed;
  - exit 69 when there is no socket, or when the connection drops mid-stream;
  - exit 76 for garbage, `v:2`, a foreign id, or a line over 1 MiB;
  - a client `--timeout` gives exit 2;
  - `--json` and `--quiet`;
  - every CLI form produces a request the server-side parser accepts.

  Test homes live under `kernel/target/test-homes/wp1/`, are removed by exact path, and the directory is empty afterwards.

## Deviations
1. **Wire names of the two-word commands** are dotted, like `job.status`: `task.submit`, `fleet.spawn`, `kernel.promote` and so on. initctl.md doesn't fix the string. The parser also accepts the space form (`"task submit"`) and the aliases `wait` and `propose-kernel`.
2. **`ping.lane`** is one of `live`, `member` or `probe`. The doc's `owner` lane is gone in v0.3, and `member` is new.
3. **`level` result.** `ledger` and `ceilings` are carried as raw JSON, passed through unchanged as the doc says. initctl.md §4.1's example (`next.checks[{key,ok}]`, `supported_max`) contradicts root.md §4.5, which owns `ledger.json` (`next.criteria[{id,met}]`, `supported_max_level`, `max_level`). The typed `root_link::Ledger` follows root.md.
4. **Control-link names follow root.md §4.1**, its owner, where supervise.md §3.13 differs:
   - `lineage.append` uses `idem`, not `dedup_key` (the local `vault/lineage.jsonl` keeps `dedup_key`);
   - `system.swap` takes `{version, source_repo, source_sha, init_kernel, initctl, report}`, not `built_from`/`k_report`;
   - the `level` event is `{from, to, cap, cause, trigger, r4_seq, evolution_share_max_permille}`, not `{level, cap, reason}`.
5. **Job record changes:**
   - Its `progress` has no `job` field, as in the §4.4 example, so it gets its own `RecordProgress` type.
   - Added an optional `reason`, because §3.7 and §5 say a job is "marked failed with reason:kernel_restart".
   - Added an optional `timed_out` for `job.wait`.
   - `lane` is the `Lane` enum.
6. **`rollback.reason`** is required on the wire. The CLI sends `"no reason given"` when `--reason` is absent.
7. **CLI additions:**
   - `vendor --workspace body|ext` (the wire has it; the usage line didn't);
   - `--wait`/`--no-wait` on `reload` and `gate`, to reach IC-7;
   - `help`, `--help` and `--version`.

   Relative paths for `task submit <dir>` and `kernel propose <patch>` are sent absolute (joined to the cwd, symlinks not resolved), so the kernel doesn't depend on the agent's cwd. `--prompt-file` and `--rationale-file` are read by the CLI, capped at 64 KiB and 8 KiB.
8. **CLI exit details the doc leaves open:**
   - `wait` returning a `failed` or `cancelled` record exits with that job error's code (default 1);
   - an expired client `--timeout` exits 2;
   - `--timeout` on `wait` maps to `timeout_s`, and the client deadline is `timeout_s` + 30 s;
   - local failures (usage, connect, git) print to stderr only, with stdout left empty;
   - "no socket configured" exits 64.
9. **`budget set` values** are sent as integers when they parse as i64, and as strings otherwise. Config money values are decimal strings, and ICJ forbids floats.
10. **Open enums.** Value sets that a newer peer may extend are open enums (`Other(String)`), so an older client still parses: job states, stages, update-failed stages, instance kinds, zone boot reasons and `root.*` codes. Kernel-owned closed sets (`Lane`, `Slot`, `Mode`, …) stay strict.
11. **Smaller choices:**
    - `promote.ref` must be `evo/<non-empty>`; no stricter grammar exists, and the kernel still runs `git check-ref-format`.
    - `error.details` is always written and `error.data` is omitted when absent.
    - `status.sandbox` is optional; sandbox.md §4.2 says `status` carries it, but initctl.md's table doesn't list it.
    - init-proto has a crate-wide `#![allow(clippy::result_large_err)]`.

## Needs from others
- **initctl.md owner:** align the `level` example with root.md §4.5. Fix the `ping` lane list. Adopt the dotted wire names. Add `reason` and `timed_out` to §4.4's job record.
- **supervise.md owner:** align §3.13's control-link field names with root.md (deviation 4).
- **Root lane (WP20+):** `init-proto` can be linked by path (`../kernel/crates/init-proto`; serde and serde_json at the same pins) for the control-link types. The alternative is to mirror them, using `tests/fixtures/doc-examples.tsv` as the shared golden data.
- **WP7 (serve):** owns the peer check (`Peer`, `peer_of` over libc; init-proto has no libc), rate and connection limits, the job store, and the server-side IC tests (IC-4/5/9/15/16/19/21–29). It should use `Command::from_request`, `CommandName::{min_level, allowed_on, refused_*}`, `ErrorBody::{level, ceiling, root, busy}` and `RequestReject::response`.
- **Housekeeping:** an early run of the IC-11 test, before the test set `GIT_CEILING_DIRECTORIES`, let git discover the repo checkout. That left 3 unreachable "initctl: quick-gate snapshot" commits (and their blobs) in this repo's object store. No ref, index or worktree file changed. `git gc` prunes them in time, and no action is needed.

## Left
- The server side belongs to WP7, and the fake kernel and fake Root to WP3; both should build on these types.
- IC-9 (reattach after kill) and IC-16 (exit 69 after a panic) need the real server and the fake Root.
