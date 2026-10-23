# Reconciliation round 1: ext (wit.md, host.md, authoring.md, adr/004)

Status: R1 · 2026-10-19 · Area: ext

Sources searched: the "Open questions" and "Spec deltas" sections of every doc, plus the other R1 ledgers present at the time of writing (`body-core.md`, `body-log-awareness.md`, `kernel-proxy-gate.md`, `kernel-supervise.md`). Several kernel and body docs had already adopted the ext first drafts during R1. Where a request had moved on since the first draft, this ledger reconciles against the doc's **current** text.

Counts: **23 accepted** (1 of them moot), **2 rejected**, **0 escalated**.

| id | from doc § | question | decision | change made (your doc §) | follow-up that ANOTHER doc must make (name the doc) |
|---|---|---|---|---|---|
| E1 | body/loop.md §9 Q-L3; body/tools.md §9 Q-T1(b), §3.6 item 4 | Pass the loop's abort flag in `CallCtx` so an owner interrupt stops a running extension at once, not at its 30 s deadline | ACCEPT | host.md §3.6 (`CallState.cancel`), §3.10 (`CallCtx.abort`, the dispatcher waits in 10 ms slices, `ABORT_GRACE` 200 ms, `Aborted` outcome), §3.11 (an `epoch_deadline_callback` on every tick checks cancel and the wall deadline; replaces `epoch_deadline_trap`; `UpdateDeadline::{Interrupt, Continue}` verified in v48.0.6), §3.12 (classification, `aborted` row), §3.13 (`run`/`read` honor cancel), §4.1, §5, §8 H23, §10 row 10.9; wit.md §4.3; authoring.md skill failure table; adr/004 Decision 3 | **body/loop.md:** pass `abort: abort.flag()` in `CallCtx`; update §3.5's abort table ("a running `ext` call stops within one 10 ms tick"). **body/tools.md:** add `abort` to its copy of `CallCtx`; map `OutcomeKind::Aborted` → `aborted` |
| E2 | body/tools.md §9 Q-T1(a) (first draft) | Extension names can never start with `_` (for the reserved `_list`/`_describe`) | ACCEPT (moot) | wit.md §3.7 name row: no name is reserved, and the pattern `^[a-z]…` excludes `_` anyway | — (tools.md R1 dropped `_list`/`_describe`) |
| E3 | body/tools.md §3.5.7, §9 Q-T1(a); reconciliation/body-core.md HO-3 | Describe the shared process helper as a waiter thread + `recv_timeout`, not `try_wait` polling | ACCEPT | host.md §3.13 (`tools::run_group`; waiter thread; cancel; TERM→KILL) | **body/tools.md:** `run_group` must (a) accept a plain `&AtomicBool` cancel signal (the host's per-call flag, not the loop's `Abort`), and (b) offer a `CaptureSpec` that keeps stdout and stderr **separately**, last 1 MiB each, because the frozen WIT `run-output` has two fields |
| E4 | body/tools.md §3.6, §4.1 (R1); body-core.md HO-5, WI-2 | The `ext` description points at `state/ext/catalog.json`; no reserved names; an unknown name returns the list | ACCEPT | wit.md §3.7 "Discovery"; host.md §3.12 unknown-name text; authoring.md §3.8, §4 Discovery row, skill steps 8 and failure table | — |
| E5 | body/tools.md §3.6 step 2 (answer to wit.md) | `/work` = the body's working directory (`ToolCx.cwd`); G4 runs get their task dir as cwd | ACCEPT | wit.md §3.4 Task-workspace row; §9.2 item removed | — (body-core WI-3 already asks supervise.md/gate.md to state `LaunchSpec.cwd`) |
| E6 | kernel/supervise.md §9 item 8 | Are T1 component binaries committed, or built (so that the probe root has them before G2)? | ACCEPT: **built by the gate, never committed** (same as gate.md §3.3) | host.md §3.8 `KEPT` bullet, §8 H24; authoring.md §3.8 keep step and skill ("never commit the .wasm") | — (supervise.md §3.6.3 already points the probe's `bin/ext` at the candidate's set) |
| E7 | evolution.md §9 Q3; reconciliation/kernel-supervise.md EV-2, EH-5 | Where kept T1 artifacts live; `KEPT` is a symlink to an immutable set; the kernel's `reload{kind:"kept", components}` | ACCEPT | host.md §3.8 (`KEPT = bin/ext` → `bin/ext.d/<set>/`; `Kept(names)` row cites the frame), §3.9 rule 3, §9.2, §10 row 10.6 | **body/loop.md:** the input thread maps `reload{kind:"kept", components}` → `ext.begin_reload(Kept(components))`, and `kind:"request"` → `Request` |
| E8 | kernel/gate.md §9 Q-EX1 (first draft); kernel/selfmap.md §3.6, Q-X1 (first draft) | Make ext crates members of the **body** workspace, tagged `[package.metadata.init] kind = "ext"` | REJECT (superseded) | authoring.md §3.3 status note; host.md §3.16 note | — Reason: gate.md itself adopted the separate `ext/` workspace in R1 (its ledger AU-1), as did body/boot.md §3.16, kernel/selfmap.md §3.6 and kernel/initctl.md `vendor --sync`. A merge would change four docs for no gain, and workspace-wide profiles would cost extensions their small-output `opt-level = "s"` profile |
| E9 | kernel/gate.md §3.3 step 3 (R1: extension crates exempt from `forbid(unsafe_code)`) | The `forbid` requirement on extension target roots | ACCEPT the exemption, with compensating checks | authoring.md §3.3 "Rules for extension crates", §5, §7, §8 AU-9, §10; adr/004 Decision 1 | **kernel/gate.md:** add two G0 checks: every ext-workspace member has `crate-type = ["cdylib"]` only, and no body-workspace member depends on an ext crate. Without them the exemption could carry `unsafe` into `init-body`. Also soften "wit-bindgen's generated exports are not reported by the lint anyway": `export!` is a crate-local macro emitting `#[unsafe(export_name)]`, which the lint likely does report (UNVERIFIED; authoring.md AU-9). The Wasm boundary alone justifies the exemption |
| E10 | raised here; kernel/gate.md §3.1 scope table | Any `ext/Cargo.lock` change is scope `dep` (synchronous G4 + handoff), and every new extension adds a member entry to `ext/Cargo.lock` | ACCEPT (as a follow-up) | authoring.md §3.3 "Lockfile entries" | **kernel/gate.md:** a `Cargo.lock` or `ext/Cargo.lock` diff that only adds, removes or changes **source-less** (workspace-member) `[[package]]` entries is not a dependency change. Classify the candidate by its other paths (`t1` for `ext/**`). Otherwise A2b-style keeps always pay a synchronous G4 |
| E11 | reconciliation/kernel-proxy-gate.md AU-1 | Say that the gate builds into supervise's build-slot target dir, not `cache/target` | ACCEPT | authoring.md §3.3 status note, §9.2 | — |
| E12 | kernel/selfmap.md §9 Q-X1 | Does WASI p2 with an empty context plus a `process` stub satisfy every import of a std-built `tool` component? | ACCEPT (confirmed) | host.md §3.15: all of `wasmtime_wasi::p2::add_to_linker_sync` (incl. `terminal-*`, sockets) with an empty `WasiCtx`, plus `init:ext/process@1.0.0` returning `Denied`; nothing else. This is the same import set as the body's `describe` | **kernel/selfmap.md:** `bindgen!` path `kernel/wit/init-ext/v1` (one directory per major), not `wit/init-ext` |
| E13 | kernel/selfmap.md §3.6 | The kernel's `describe` engine uses fuel, `OptLevel::None` and 64 MiB, not host.md's config | ACCEPT | host.md §3.15 (those choices are fine for a one-shot describe; same Wasmtime version is what matters) | — |
| E14 | testing.md §9 Q6 | Does `init:ext@1.0.0` grant process or PTY capability (for A2-pty)? | ACCEPT (answer): `process.run` and pipe sessions yes; PTY no (field `pty` reserved, seed answers `denied`) | wit.md §9.2 | **testing.md:** add variant **A2-repl** (pipes; `python3 -i -u`, or `cat` in CI), not A2-pty |
| E15 | testing.md §3.11 M-1/M-2; roadmap.md M0 and §11; adr/005 versions paragraph | Measurements and the skeleton use wasmtime 49.0.3, while host.md pins 48.0.6 | ACCEPT: keep **48.0.6** (LTS to ≈ 2028-08; host.md §3.2 owns the version; boot.md §3.16 and selfmap.md already use it) | host.md §9.2 | **testing.md**, **roadmap.md**, **adr/005:** change 49.0.3 → 48.0.6 for `wasmtime`/`wasmtime-wasi` |
| E16 | testing.md §3.11 M-3 decisions | M-3's thresholds vs authoring.md's lane criterion C3 | ACCEPT: C3 aligned to M-3 (rerun < 6/10 with ≥ 3 failures in M-3's toolchain classes) | authoring.md §3.10 C3, §10 | **testing.md:** cite authoring.md §3.10 C3 for the lane decision |
| E17 | roadmap.md §9 Q2 | Each Test plan names the testing.md ids its work package must pass | ACCEPT | host.md §8 intro, H16 → M-5, H18 → A2b; wit.md §8 note; authoring.md §8 note | — |
| E18 | body/boot.md §9 Q-B7; reconciliation/kernel-proxy-gate.md BT-3 | The WIT majors the host supports, in `--manifest` | ACCEPT | host.md §4.1 `SUPPORTED_MAJORS: &[u32] = &[1]` (boot.md prints it as `ext_api_majors`) | — |
| E19 | body/loop.md §3.1 (input-thread row) | The input thread calls `begin_reload` as soon as the `reload` frame arrives; commit at the boundary | ACCEPT | host.md §3.9 sequence and rule 3 | — |
| E20 | kernel/initctl.md, kernel/supervise.md, adr/001 | "Step boundary" in kernel docs vs "turn boundary" here | ACCEPT (alias, same point) | host.md §3.9 | — |
| E21 | adr/002 §9; reconciliation/kernel-proxy-gate.md WI-3 | General allowlisted egress / `wasi:http` for extensions | ACCEPT (consistent: none in 1.x) | wit.md Q-W2 cites ADR-002 | — |
| E22 | reconciliation/kernel-supervise.md EA-1, EA-2, EH-1…EH-4 | `vendor --sync`; toolchain read; `state/ext/` in the layout; no `RLIMIT_AS`; `describe` profile; descendant sweep | ACCEPT (done elsewhere) | host.md §9.2 (requests removed) | — |
| E23 | adr/003 Consequences (R1); CONVENTIONS §5; reconciliation/body-core.md HO-9 | `wasmtime-wasi` starts a tokio runtime, against "no async runtime"; `parallel-compilation` brings rayon | ACCEPT (body-core already reconciled the rule's reading). Corrected a factual error here | host.md §3.2: every async-shimmed WASI call runs through `in_tokio` → `RUNTIME.block_on` (verified v48.0.6); `allow_blocking_current_thread(true)` only removes the `spawn_blocking` hop, not the runtime; rayon noted; §10.3 | **adr/003:** replace "it reduces this with `allow_blocking_current_thread(true)`" (inaccurate) with "it removes the extra `spawn_blocking` hop, not the runtime"; mention rayon (compile-time thread pool inside wasmtime). **main session:** CONVENTIONS §5 wording (below) |
| E24 | kernel/supervise.md §3.6.8, last sentence | "The live body already runs the new data: it reloaded it from its working tree" | REJECT the sentence for T1 | — | **kernel/supervise.md:** amend it. For T1 the live body runs the agent-built trial until the kernel's `reload{kind:"kept"}` swaps in the gate-built set (§3.6.8's own bullet). Only T0 data is already live |
| E25 | body/boot.md §3.16 (seed ships `skills/ext-authoring/`) | A `Cargo.toml` inside the template directory would be an unlisted crate under gate.md §3.3 step 3 | ACCEPT (template renamed) | authoring.md §3.1, §3.3 tree, §3.4 (`Cargo.toml.template`), skill step 2, §4, §5, §10 | **body/boot.md:** ship `skills/ext-authoring/assets/template/Cargo.toml.template` and `src/lib.rs` (not `Cargo.toml`) |

Counting note: E2 is counted as accepted (moot). E8 and E24 are the two rejections. The other 22 rows are accepted.

## Escalated to the owner

None. Every item either had a clear technical answer inside the ext area, or had already been reconciled by its owner doc. One item touches a global decision: E23 (tokio inside `wasmtime-wasi` vs "no async runtime"). body-core already settled its reading, and it goes to the main session as a wording change, not to the owner.

## Requests to the main session (CONVENTIONS.md)

1. **§2 repository layout:**
   - `wit/init-ext/` becomes `wit/init-ext/v1/init-ext.wit`, one directory per major (ext/wit.md §4.1).
   - The seed gains:
     - `seed/wit/init-ext/v1/init-ext.wit` (byte-identical copy, checked by G1 `gate.g1.wit_copy_drift`);
     - `seed/ext/Cargo.toml` and `seed/ext/Cargo.lock` (the separate extension workspace, `members = ["*"]`, excluded from the body workspace);
     - `seed/skills/ext-authoring/{SKILL.md, assets/template/Cargo.toml.template, assets/template/src/lib.rs}`.
2. **§3 runtime layout:**
   - `bin/ext` is a symlink to `bin/ext.d/<set>/` (kernel; agent read + exec; immutable published sets; supervise.md §3.1, §3.6.5).
   - `state/ext/` holds `catalog.json`, `<name>/data/` and `<name>/log` (agent read-write; written by the ext host).
   - This is the same request as reconciliation/kernel-supervise.md EH-1.
3. **§4 contract registry**, row "WIT world `init:ext@1.0.0`": fill "Fixed now" with: package `init:ext@1.0.0`, world `tool` (world-level exports `describe`, `call`), interface `process`. Authoritative file `kernel/wit/init-ext/v1/init-ext.wit`. Minors are additive only; the body supports majors N and N−1.
4. **§5 global decisions:**
   - (a) **Body dependencies.** Pin `wasmtime` and `wasmtime-wasi` at **`=48.0.6`** (current LTS, supported to ≈ 2028-08; ext/host.md §3.2), so testing.md, roadmap.md and adr/005 stop citing 49.0.3. Add `wit-bindgen =0.63.0` (plus `serde`/`serde_json`), used only by the extension workspace and never linked into `init-body`.
   - (b) **"No async runtime: std threads only"** reads "no async code in the body". The tokio runtime that `wasmtime-wasi` builds internally for WASI I/O, and the rayon pool Wasmtime uses while compiling, are accepted as internals of allowed dependencies. This is the same wording request as reconciliation/body-core.md.
   - (c) **`#![forbid(unsafe_code)]`** applies to every body-workspace crate. Extension-workspace crates (Wasm guests) are exempt. They must be cdylib-only, and nothing in the body workspace may depend on them (gate.md §3.3; this ledger E9).

## R2

Applied 2026-10-19, against CONVENTIONS v0.2 and all six R1 ledgers.

**Follow-ups addressed to the ext docs, applied:**

| From ledger (id) | Follow-up | Applied in |
|---|---|---|
| body-log-awareness HO-1 (log-context X9) | start the T1 reload text with `[init:reload] ext generation N` | host.md §4.3 (text and example) |
| body-log-awareness WI-1 | promotion to a native tool takes effect at the handoff (one cache miss); promote right after a compaction to combine misses | wit.md §3.7 "Promotion to a first-class tool" |
| body-core HO-3 | describe the shared helper as waiter thread + `recv_timeout` | host.md §3.13 (already applied in R1) |
| kernel-proxy-gate AU-1, kernel-supervise EA-3 | the gate builds the ext workspace in supervise's build slot, not `cache/target` | authoring.md §3.3 status note, §9.2 (already applied in R1); no `cache/target` left in any ext doc |
| kernel-supervise EV-2 | `KEPT = $INIT_HOME/bin/ext` is a symlink to an immutable set; no `bin/ext.prev/` directories | host.md §3.8, §10 row 10.6 (already applied in R1) |

**Alignment with CONVENTIONS v0.2:**
- `wit/init-ext/v1` paths: already used everywhere.
- Layout §3 (`bin/ext` + `bin/ext.d/<set>/`, `state/ext/`, `work/` as the body's cwd, no shared `cache/target`): already used; authoring.md §3.10 now publishes the `js` engine "in the kept set".
- `wasmtime =48.0.6`: already used.
- tokio wording: host.md §3.2, §10.3 and adr/004 now cite CONVENTIONS §5 instead of an open request.
- ICJ:
  - wit.md §3.7: the canonical schema is ICJ v1, and every number in `input-schema` must be an integer (rule 4), because a promoted schema enters the head.
  - wit.md §4.2: the `call` input follows ICJ rules 1–3 and 5, with model-sent numbers passed through unchanged.
  - host.md §4.2: `catalog.json` follows ICJ rules 2–5 but is pretty-printed, because it is a file for reading (`read` cuts lines over 2,000 bytes), never hashed or embedded by the harness.
- Money units: no money values in ext docs.
- Error codes: the ext docs emit `OutcomeKind`s only; body/tools.md maps them to tool error codes.

**Pending owner decisions, marked with the recommended default:**
- **ESC-1 (vendor approval, default "no"):** host.md §3.2 upgrade policy and §5; authoring.md §3.5 error table, §5, and the skill's "Don't" list.
- **ESC-2 (`/opt/homebrew` readable, default "yes"):** authoring.md skill, process section (`node -i`).
- None of OW-1, OW-2, E1/E2 (proxy/gate) or E1 (body-core) appear in ext docs.

**Could not apply:** nothing addressed to the ext docs was left unapplied.

**R1 follow-ups to other docs, still not applied by their owners** (re-listed in each ext doc's §9.2):
- **E1, E7 → body/loop.md:** pass `abort: abort.flag()` in `CallCtx`; map `reload{kind:"kept"}` → `begin_reload(Kept(..))`; update the §3.5 abort row; close Q-L3.
- **E1, E3 → body/tools.md:** `CallCtx.abort`; map `Aborted` → `aborted`; `run_group` takes a `&AtomicBool` cancel and keeps stdout and stderr separately.
- **E9, E10 → kernel/gate.md:** cdylib-only and no-dependents checks for the `forbid` exemption; source-less lockfile entries are `t1`, not `dep` (evolution-testing AU-3 asks the same).
- **E12 → kernel/selfmap.md:** `bindgen!` path `kernel/wit/init-ext/v1`.
- **E24 → kernel/supervise.md:** the §3.6.8 sentence holds for T0 only.
- **E23 → adr/003:** "reduces this with `allow_blocking_current_thread`" is inaccurate.

**Open questions removed as resolved:**
- **wit.md:** Q-W2 (HTTP; decided: none in 1.x), and the testing.md, evolution.md and gate.md items.
- **host.md:** the testing/roadmap/adr-005 version item and the CONVENTIONS requests.
- **authoring.md:** the boot.md, testing.md and evolution.md items.

**Remaining open questions:**
- **Local, Phase 0:**
  - wit.md Q-W1 (PTY implementation, T2 growth), Q-W3 (WASI 0.3 → `init:ext@2`), Q-W4 (host-side input validation);
  - host.md Q-H1 (`concurrency_support(false)` with p2 sync), Q-H2 (component structure, pool sizing), Q-H3 (compile and instantiate times);
  - authoring.md Q-A1 (build times), Q-A2 (wit-bindgen import parameter ownership), Q-A3 (disk cost).
- **Cross-doc:** the six items above.
