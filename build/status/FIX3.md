# FIX3: real extension host in the body, t_aw_5, `cap`, A2 core path (lane B2)

Status: **done** (2026-10-20). Owned paths: `seed/src/boot/**`, the `ext` wiring in `seed/src/main.rs` (unchanged: it already routes `init-body ext …` to `boot::ext::cli`), and the boot e2e tests in `seed/tests/boot.rs`.

## Done
1. **Real extension host wired** (`seed/src/boot/ext.rs`):
   - `start()` calls `crate::ext_host::from_env()` in normal and probe mode (facts `ext: {"status":"loading"}`; the Boot load of `bin/ext/*.wasm` is queued, not awaited). Safe mode: none (`{"status":"disabled"}`).
   - `cli()` calls `crate::ext_host::cli::main`, so `init-body ext describe|call|inspect` run the real host (exit 0 / 1 load error / 2 usage).
   - `StubHost` is gone from the production path; it stays in `tools/` for WP11's tests (`tools/tests.rs`).
   - If Wasmtime cannot start (engine/linker/ticker error), boot logs a warning, facts `{"status":"unavailable","error":…}`, and an `Unavailable` host answers every `ext` call with `ext_host: … the extension host did not start at boot (<cause>)` (not the misleading safe-mode text of a `None` host).
   - `BootQuiet` wrapper (see Deviations 1): a boot-queued load (`Boot`, `SessionStart`) that changed nothing logs no `reload` event.
   - Test: `manifest::EXT_API_MAJORS == ext_host::SUPPORTED_MAJORS`.
2. **t_aw_5 fixed, root cause found.** macOS checks a freshly written executable on its first exec (provenance/syspolicyd): measured here at **~150–260 ms on the first exec vs 3 ms on the second**, and the time is spent after `spawn` returns, so it counts against the probe timeout. With many tests writing and exec'ing scripts in parallel it can pass the 2 s budget, so the fake `rustc` probed null. Fixes:
   - Probe (`awareness.rs::run_probe`): the deadline now covers spawn; a transient `ETXTBSY` at spawn is retried until the deadline; a successful exit gets a 500 ms grace (was 50 ms) for the reader thread to hand over stdout.
   - Tests: fixture scripts are executed once, untimed, before the timed probe (`warm`), so the tests measure the probe, not macOS. The timeout test was not testing a timeout: `exec sleep 30` ran with a PATH that has no `sleep`, so git exited 127 at once. It now runs `/bin/sleep 30`, leaves a marker first, and the test asserts the marker exists and the probe took ≥ the 2 s timeout (and < 6 s), so "git → null" is the timeout path.
3. **`cap L?` filled or omitted** (`rootview.rs`, `awareness.rs::autonomy_line`): the kernel sends no `cap`. Sources in order: `boot.json.env.root.cap`; the Root's published view `INIT_HOME/root/ledger.json` (`cap`, written by `init-root` `Level::json`); min(`max_level`, `supported_max_level`) from `ledger.json` or, new, `root/charter.json` (`autonomy.max_level`, `supported_max_level`, root.md §3.12.5). If none is readable, ` (cap L?)` is **left out** (`- autonomy: L0 · at your cap: charter max L4`), and unknown parts of the at-cap text are dropped instead of printed as `L?`. `next: null` (the ledger's at-cap marker) now means "at your cap" even without a cap; no root status at all reads `- autonomy: L? · ?`.
4. **A2 core path end to end** (`seed/tests/boot.rs`):
   - `ext_tool_calls_kept_count_lines`: a temp `INIT_HOME` with the real `count-lines` built by the real toolchain (`CARGO_TARGET_DIR=seed/target cargo build --offline --target wasm32-wasip2` in `seed/ext`) published as a kept set (`bin/ext → ext.d/0001/count_lines.wasm`) plus its source dir `body/ext/count-lines/`. The real `init-body` boots against `init-mockllm`; a scripted turn calls `ext{name:"count-lines", input:{path:"a2/notes.txt", needle:"fn "}}`. Asserts: boot facts `loading`; a T1 `reload` event with `count-lines`, `ok`, `prefix_changed:false`, equal head hashes; the `ext` result is the component's real output `{"count":3,"lines":[1,3,4]}`, not an error; both mock expectations held (tools array unchanged, `prefix: "same"` on the second request); the second request carries the result; `state/ext/count-lines/log` has 1 record; `state/ext/catalog.json` lists it.
   - `ext_cli_on_count_lines`: `init-body ext describe` (name, schema), `call` (the real output, data dir from `INIT_HOME`), `inspect` (exports `call`, `describe`; no `init:ext/process` import), and a missing file → exit 1.
   - `ready_heartbeat_pause_stop` now expects `{"status":"loading"}` and checks that a body without extensions logs no `reload` event over its ~10 s life.
   - Harness race fixed: `wait_exit` could return before the stderr reader thread had read the body's last line (seen once: `config_errors_exit_78` "no boot_error line" with empty stderr). `Body::stderr_complete` waits for the reader's EOF; used by the three tests that read stderr after exit.

## Tests
- Full suite, run twice back to back: `cd seed && INIT_MOCKLLM_BIN=<repo>/kernel/target/k1/debug/init-mockllm CARGO_INCREMENTAL=0 cargo test --offline` → **unit 269 passed, 0 failed, 5 ignored; tests/boot.rs 16 passed; tests/self_md.rs 8 passed** (both runs; load average about 20 from other lanes). Before the fix, a baseline run at HEAD had passed t_aw_5 too: the failure WP14 saw depends on machine load (root cause above).
- `cargo clippy --offline --all-targets -- -D warnings`: clean. `rustfmt --edition 2024` applied to `boot/ext.rs`, `boot/awareness.rs` (+ `awareness/tests.rs`), `boot/rootview.rs`.
- New/changed unit tests: `boot::ext::tests` (3), `t_aw_5_*` (warmed fixtures, real timeout), `t_aw_12_autonomy_and_ceilings` (omitted cap, charter.json and ledger.json through `render_boot`), `rootview` (charter fallback).

## Deviations
1. **No `reload` event for a boot-queued load that changed nothing.** With the real host, every boot of a body without extensions logged `[init:reload] ext generation 1\n(no changes)` at its first idle boundary, which (a) adds a context item per restart and (b) is rendered after the probe instruction in the next probe body's first request (that broke `probe_round_trip`'s `last_text_contains`). `BootQuiet` (in boot, around WP14's host) drops a report only if it is `ok`, has no components and no lines but `(no changes)`, and no `Request`/`Kept` reload was ever asked through the trait. The registry swap still happens; every report of `initctl reload` or a kept publish is passed on unchanged, so `reload_done` and A2's order are untouched. host.md §3.9/§4.3 do not say whether a no-change Boot report is logged; WP14/WP11 may prefer to move this rule into the host or the loop.
2. **New session at first boot:** boot calls `reset_trials()` (WP11's request), which queues a `SessionStart` load that lists names from `body/ext/*/` source dirs (host.md §3.8). A kept `bin/ext/x.wasm` without `body/ext/x/` is therefore unloaded at a new session, by spec. The A2 test ships the source dir, as the seed does.
3. Probe deadline now includes the spawn call (it used to start after it).

## Needs from others
- **WP14 / WP11 (optional):** decide where the "no event for a no-change boot load" rule lives (Deviation 1); `BootQuiet` can then go.
- **kernel-supervise:** still no `cap`/`supported_max_level` in `boot.json.env.root`; harmless now (Root view or omission), but sending them avoids a file read.
- **Sandbox (X2):** the body must be able to read `INIT_HOME/root/` for the Root-view fallback (kernel/sandbox.md `PUB` already allows it).
- **Docs:** host.md §4.5 example could state that `inspect` lists the world's exports as `call`, `describe`.

## Left
- Nothing for FIX3. A2 proper (the agent writes, builds and reloads `semver-sort`) is the scenario's job.
