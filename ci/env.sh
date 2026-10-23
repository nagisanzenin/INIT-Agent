# Source this before any cargo command in this repo: `source ci/env.sh`.
# Disk budget (CONVENTIONS §5): no incremental caches, and line-table-only debug info, which roughly
# halves debug target dirs while keeping file:line in backtraces. Tests are unaffected.
export CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=line-tables-only
export CARGO_PROFILE_TEST_DEBUG=line-tables-only
# CPU/RAM budget while many lanes run at once (load reached 274 and swap 5 GB on 2026-10-20 13:11):
# each lane uses at most 2 compiler jobs and 2 test threads. ci/local.sh's final runs may override.
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export RUST_TEST_THREADS="${RUST_TEST_THREADS:-2}"
