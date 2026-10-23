# FIX9: root doc_drift covers the §3.3.6 sealed text; the wp21 slice-1 flake was a test race

Status: **done** (2026-10-20, lane R, root workspace, `CARGO_INCREMENTAL=0`, default target `root/target`). Paths: `root/**` only. `docs/root/root.md` was not edited: its sealed block already matched the code byte for byte.

## Done
### 1. `doc_drift`: `root_md_texts_are_verbatim` (CI: "scheme blocks left: 4 right: 3")
- **Doc vs code.** The 4th `scheme` block of root.md (§3.3.6, added by docs sync 986669f) is byte-identical to the code's sealed text (`TAIL` in `init-root/src/sealed/run.rs`, minus the raw string's leading newline). Checked with `diff`. No doc fix was needed.
- **The sealed text moved into `init-sandbox-profiles`**, next to the other profile texts, so the drift test can see it. `init-sandbox-profiles` cannot depend on `init-root`, and root.md §3.3.1's sketch already names `SEALED_TAIL = include_str!("../profiles/sealed-tail.sb")`.
  - `root/sandbox-profiles/profiles/sealed-tail.sb` (new): the verbatim text.
  - `seatbelt.rs`: `pub const SEALED_TAIL` and `pub fn render_sealed()` = PRELUDE + SEALED_TAIL (root.md §3.3.1 `profile(kind sealed)`).
  - `init-root/src/sealed/run.rs`: `const TAIL = init_sandbox_profiles::seatbelt::SEALED_TAIL`, so root.md's "`TAIL` in `sealed/run.rs`" still points at the right name. The profile passed to `sandbox-exec` is `PRELUDE + TAIL`. The only byte difference from before is one blank line fewer (the old raw string began with `\n`), which SBPL ignores. wp24's sealed audit and gate-of-gates runs pass with it.
- **Test.** `root_md_texts_are_verbatim` now expects 4 blocks. It keeps the 3 existing assertions and adds `b[3] == SEALED_TAIL` (§3.3.6) and `render_sealed() == PRELUDE + b[3]` (§3.3.1).

### 2. `wp21`: `slice1_kernel_in_zone_owner_round_trip_and_stop_start` was flaky. It was a test race, not a product race.
- **Where it failed.** In the CI log, `wp21.rs:238:31` is `h.fk("spawn")[0]`, not `h.fk("boot")[0]` (line 227). Nothing has touched `wp21.rs` since d7afcb0 (04:21), before the 07:39 run, and column 31 is the `[0]`.
- **The product is right.** The slot turns `Running` only on the kernel's `ready` (`sup.rs` `on(Ev::Ready)`, after the `zone_start` record). That is exactly root.md §3.6.2 (`Starting` + `ready` -> `Running`). The fake kernel logs `boot` before it sends `instances` and `ready`, so `boot` always exists once the slot is `running`. The body `spawn` is the kernel's own first step **after** `ready`, and it is logged only after the Root's answer. So `running` can come before `spawn` lands in `fk.jsonl`. The Root must not wait for it, because the kernel decides when to spawn. FIX4 runs the first start on the supervisor thread, which lets `running` show up sooner and widens the window.
- **A second race on the next lines.** `sandbox(bpid) == 1` straight after the spawn answer. The Root answers once `init-root exec-clean` is spawned. `sandbox-exec` applies the body profile only after that, before it execs `/bin/cat`. During that short window `sandbox_check(bpid)` is 0. The window is harmless: only `exec-clean` and Apple's `sandbox-exec` run unconfined, and body code never does. This probably explains the reported line-241 failure: line 241 is that `sandbox` assert, not an index.
- **Fix (tests only, bounded waits, no bare sleeps).**
  - `Home::fk_wait(secs, ev, n)` polls `fk.jsonl` until there are `n` lines of `ev` (the `wait` helper, 50 ms steps, panics at the bound).
  - `boot`: `fk_wait(5, "boot", 1)`. Already guaranteed by the order above; it costs nothing.
  - `spawn`: `fk_wait(10, "spawn", 1)`.
  - Before the sandbox assert: `wait(5, "the body runs cat", || alive(bpid, "cat"))`, then the same strict assert. Once the pid runs `cat`, the profile has to be in place, so this cannot hide an unconfined body.
- **Sweep of the other tests (wp20, wp21, wp21b, wp22, wp23, wp24, wp25, fix5)** for `fk(..)[0]` or `.pop()` after `running()`/`restarted()`:
  - wp21 `owner.in` (lines 273, 294): read after attach saw the body's echo, and the fake kernel logs `owner.in` before it writes to cat. Causal, so safe.
  - wp21 panic test `store.put`/`promotion`/`store.get`/`spawn`: read after `wait(.., "tuple.get")`, the last step. Safe.
  - wp21 `fk("boot").pop()` after resume, `boots` after `halted`, `spawn2`: either `boot` comes before `ready`, or the read sits behind a `wait`. Safe.
  - wp25 `fk("boot").pop()` in `check` comes after `restarted()`, which waits for a new `zone_start`, appended after `ready`. Safe. wp25 `fk("start")[0]`: logged first thing at fake-kernel start. Safe. wp25 `event`: behind a `wait`. Safe.
  - fix5 `call`: behind a `wait`. Safe.
  - wp20, wp21b, wp22, wp23, wp24 do not read `fk.jsonl`.
  - No other test needed a change.

## Tests
- `cargo test --offline -p init-sandbox-profiles`: **pass** (lib 10, doc_drift 2, params 4, selftest 1).
- `wp21 slice1_kernel_in_zone_owner_round_trip_and_stop_start` alone, **20 runs in a row: 20/20 pass** (about 1.5 s each, 32 s total).
- `INIT_MOCKLLM_BIN=kernel/target/k1/debug/init-mockllm cargo test --offline -p init-root --features test-fixtures`: **pass**. unit 65, fix5 1, wp20 9, wp21 6, wp21b 2, wp22 8, wp23 4, wp24 5 (218 s), wp25 3. Total 282 s.
- `cargo test --offline` in `root/`: **pass**. init-root unit 62 (integration tests are gated on `test-fixtures`), sandbox-profiles 10 + 2 + 4 + 1.
- `cargo clippy --offline -p init-sandbox-profiles --all-targets -- -D warnings` and `-p init-root --features test-fixtures --all-targets`: clean. `cargo fmt --check -p init-sandbox-profiles`: clean.
- Mock model only. Live-safe features only (no `live-sweep`). The only processes were the tests' own children. Disk: 6.4 GiB free throughout.

## Deviations
- The sealed text now lives in `init-sandbox-profiles` (`SEALED_TAIL`, `profiles/sealed-tail.sb`) instead of a literal in `sealed/run.rs`, which keeps a `TAIL` alias. This is what root.md §3.3.1's sketch already describes.

## Needs from others
- Docs (root.md §3.3.6, last paragraph): "This block is to be drift-tested … that test is not built yet" is now stale. The test exists (`root/sandbox-profiles/tests/doc_drift.rs`, block 4 == `SEALED_TAIL`). Optionally, §3.3.1's sketch comment "as built: `TAIL` in sealed/run.rs" could say "`SEALED_TAIL` in `root/sandbox-profiles` (`TAIL` in sealed/run.rs)". I did not edit the doc, because no byte fix was needed.
- Note for the orchestrator: `ci/local.sh` looks for `kernel/target/debug/init-mockllm`, which does not exist right now. Only `kernel/target/k1/debug/init-mockllm` does, so the CI step would fall back to its "missing" branch.

## Left
- Nothing for FIX9.
