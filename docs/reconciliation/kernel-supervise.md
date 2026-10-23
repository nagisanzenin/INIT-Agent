# Reconciliation round 1: kernel-supervise

```
Status: R1 · 2026-10-19 · Area: kernel
Docs owned: kernel/supervise.md, kernel/initctl.md, kernel/sandbox.md, adr/001-overlapped-handoff.md
Sources searched: the "Open questions" and "Spec deltas" sections of all 26 docs, plus every
"→ kernel/supervise.md | initctl.md | sandbox.md" reference elsewhere in their text.
```

**Totals:** 63 items. 56 ACCEPT (some modified, as noted), 5 REJECT, 2 ESCALATE-OWNER.

One change was found and verified while reconciling. A probe root's `kernel`/`lineage` symlinks could not be traversed under Seatbelt: a link inside a directory the profile does not allow cannot be followed. This was tested on macOS 14.6, 2026-10-19. The fix adds one line to the body profile (`LINK_*`, sandbox.md §3.3.1). That line has been tested on its own, but not yet with the full 45-check suite (S-M49/S-M50 are pending).

## Ledger

| id | from doc §, question | decision | change made (my doc §) | follow-up another doc must make |
|---|---|---|---|---|
| AW-1 | awareness.md §9 X1: `boot.json` fields for the boot block (`branch`, `crash`, `cause`, `env`, `body_tokens_prev`, evolution fields, `stash`); meaning of reason `handoff` | ACCEPT (mapped) | supervise §4.3: added `branch`, `cause`, `crash {exit_code, signal, log_tail}`, `env` (kernel half of the snapshot), `selfmap.body_tokens_prev`; `handoff` defined in §4.1 (`cause`: `clear_safe`, `model_change`, `owner_restart`) | awareness.md: read evolution fields from `boot.json.evolution.*` and token counts from `boot.json.selfmap.*`; `stash` comes from the body's own sync (body/boot.md), not from the kernel |
| AW-2 | awareness.md §9 X3: `initctl status` fields incl. `constitution_sha256` | ACCEPT (mapped) | initctl §4.1 `status`: added `constitution_sha256`; mapping note `running_sha`=`current.sha`, `lkg_sha`=`lkg.sha`, `provisional`=`current.provisional`, `restarts_since_lkg`=`counters.restarts_since_lkg`, `safe_mode`=`mode=="safe"` | awareness.md §4.3: use the nested names |
| AW-3 | awareness.md §9 X6: report write / read-only / hidden roots and network mode in `boot.json.env.sandbox` | ACCEPT | sandbox §4.2 (object defined, with example); supervise §4.3 `env.sandbox` | — |
| BT-1 | body/boot.md §9 Q-B1 (and body/provider.md Q-P2): `boot.json` example uses `deepseek-chat` | ACCEPT | supervise §4.3 example → `deepseek-flash` | — |
| BT-2 | body/boot.md Q-B3, body/loop.md Q-L1c, body/tools.md Q-T3: live body cwd at spawn | ACCEPT | supervise §3.4: `LaunchSpec.cwd = $INIT_HOME/work` (probe: `<root>/work`) | — |
| BT-3 | body/boot.md Q-B4, body/loop.md Q-L4: heartbeat continues while paused; do settle and staleness still work? | ACCEPT (rule changed) | supervise §3.5: settle no longer needs a heartbeat from B; per-instance `last_seen[boot_id]`; staleness not evaluated for A (paused) or B (pre-commit) during a handoff | body/boot.md §3.13 diagram: "settle 3 s (≥ 1 heartbeat from B)" → "settle 3 s (B alive)" |
| BT-4 | body/boot.md §10 item 4: body exit sub-codes (64, 70, 78, 101) | ACCEPT (new handling) | supervise §3.3: two consecutive exit 78 (`EX_CONFIG`) at boot → `Halted{config}`, not counted as crashes; §4.9 `config_error_exits`; §5 F23; test SV-28 | — |
| BT-5 | body/boot.md Q-B6: G3 drives the probe body with `input` frames | ACCEPT | supervise §3.6.3 `ProbeHandle::send_input` | kernel/gate.md: G3/G4 use `send_input` |
| LC-1 | body/log-context.md §9 X1a: are slash commands (`/compact`, `/new`, `/branch`, `/label`) delivered to the body? | ACCEPT | supervise §3.10 item 1: only the exact kernel commands of §4.6 are intercepted; every other line, slash or not, goes to the body | — |
| LC-2 | log-context X1b: pause/resume with `{len, leaf}` | ACCEPT (already) | none (frame fields `log_len`, `leaf`, supervise §4.5) | — |
| LC-3 | log-context X1c: `INIT_SESSION` = content of `state/session`, empty only on a fresh install | ACCEPT | supervise §3.4, §4.1: kernel no longer generates ids; empty string when missing or invalid | — |
| LC-4 | log-context X1d: `boot.json` fields mirrored by the `boot` event | ACCEPT | see AW-1 | — |
| LC-5 | log-context X2: async kernel notices via a `subscribe` command on `run/kernel.sock` | REJECT | supervise §4.5 note: notices go only over the body's stdin, with ack, redelivery and dedup by `id`. One ordered channel; exactly one live body receives it; crash-safe | body/log-context.md §3.13 and X2: consume stdin `notice` frames; drop the subscription proposal |
| LC-6 | log-context X6: add `state/blobs/` to the runtime layout | ACCEPT | supervise §3.1 | CONVENTIONS §3 (request below) |
| LP-1 | body/loop.md Q-L1a: optional `mode` on `input` (`steer` / `follow_up`), owner prefix `/later` | ACCEPT | supervise §4.5 `input.mode`; §3.10 item 1 and §4.6 `/later <text>`; test SV-31 | body/loop.md: honour `mode` |
| LP-2 | body/loop.md Q-L1b: confirm `promote` returns before `pause` | ACCEPT | initctl §4.1 `promote`: ordering guarantee (pause only after every pending response on the job is written); test IC-22 | — |
| PV-1 | body/provider.md Q-P7: how does the body learn the current task id (for `x-init-task`)? | REJECT (not a kernel concern) | supervise §9 item 5: task ids are created by the body (`state/tasks/<id>/`); the kernel assigns none and learns them only from the owner's `/accept` | body/loop.md or body/tools.md (with evolution.md): define where the body sets `Request.task` |
| TL-1 | body/tools.md Q-T2: exec `/bin/kill`, `/usr/bin/kill`, bash; same-sandbox signals; files under `state/spill/` | ACCEPT (already allowed) | sandbox §8: tests S-M51, S-M52 added (pending run) | — |
| TL-2 | body/tools.md Q-T3: `state/spill/<sid>/` in the layout | ACCEPT | supervise §3.1 | CONVENTIONS §3 (request below) |
| EV-1 | evolution.md Q2: owner acceptance recorded by the kernel (`/accept [task_id]`) | ACCEPT | supervise §3.10 item 8, §4.6, §4.7 kind `accept`, `notice{kind:"accepted"}`; test SV-30 | evolution.md: online pass rate reads the `accept` records; kernel/gate.md provides `init-kernel gate task import` |
| EV-2 | evolution.md Q3 (with ext/host.md): kernel-published kept T1 artifacts in `bin/ext/` | ACCEPT (modified: immutable sets + symlinks) | supervise §3.1, §3.6.5 (`bin/ext.d/<set>/`; `bin/ext`, `.prev`, `.staged`, `.lkg` are symlinks swapped with the binary), §3.6.8, §4.3 (`lkg.json.ext_set`, `supervisor.json.binaries.ext`); test SV-29 | ext/host.md: `KEPT = $INIT_HOME/bin/ext` is a symlink to an immutable set (no `bin/ext.prev/` directories) |
| EV-3 | evolution.md Q4: `state/kernel/usage.json`, `state/kernel/thinning.json`, `vault/usage.jsonl` | ACCEPT | supervise §3.1 | CONVENTIONS §3 (request below) |
| EV-4 | evolution.md Q6: `lkg.json` records the model id the LKG passed G4 with | ACCEPT | supervise §4.3 `lkg.json.model {provider, id}` | — |
| EV-5 | evolution.md Q7a: confirm `lineage/body.git`; add `lineage/seed.git` and the `refs/init/*` namespace | ACCEPT (adopted evolution's ref names) | supervise §3.1, §3.6.1 (`refs/init/candidates/<job>`, `refs/init/archive/<sha>`, replacing `refs/init/fetch/*` and `refs/evo/*`), §3.11.1 (seed.git paragraph, `refs/upstream/*` copied from seed tags), §3.11.4, §3.11.5 (`upstream` remote = `lineage/seed.git`) | evolution.md §4.3: keep `refs/init/candidates/<job>` keyed by job id (it says `<gate_id>`, which is the same id) |
| EV-6 | evolution.md Q7b: the kernel fast-forwards `body/` during the pause (`git merge --ff-only`) | REJECT | none (supervise §3.11.3, K-F3). Running git with a cwd in an agent-writable repo executes agent-chosen code (`core.fsmonitor`, hooks), verified locally 2026-10-19. The body syncs itself at boot (§3.11.5) | evolution.md §3.6: move the stash/switch procedure to the body's boot (reason `rollback`); body/boot.md implements it |
| EV-7 | evolution.md Q8, testing.md Q12: `initctl wait [<gate_id>\|--last] [--timeout]`; `error.details[]`; register codes | ACCEPT | initctl §4.1 (`wait` with `last`, `cmd`), §4.5 CLI, §3.2 (`details`, dotted codes); test IC-19 | — |
| EV-8 | evolution.md Q12: agent needs write access to all of `body/.git` | ACCEPT (already) | none (sandbox §3.3.1 writes `BODY`; §10 delta) | — |
| EA-1 | ext/authoring.md §9.2, ext/host.md §9.2: `initctl vendor` must vendor both workspaces (`--sync ext/Cargo.toml`) | ACCEPT | initctl §4.1 `vendor`: `--sync ext/Cargo.toml` when present; new arg `workspace: body\|ext` | — |
| EA-2 | ext/authoring.md, ext/host.md: sandbox must read/exec the toolchain incl. `wasm32-wasip2` and `wasm-component-ld` | ACCEPT (already covered) | sandbox §3.8: doctor checks that the target and the linker exist (`TOOLCHAIN` rule covers them) | — |
| EA-3 | ext/authoring.md §9.2 (to gate): G0 builds the ext workspace with `CARGO_TARGET_DIR=$INIT_HOME/cache/target` | REJECT (path) | supervise §3.6.2: there is no `cache/target`; the ext workspace builds in the slot (`cache/build/slot/out/target`), and `deps_key` now covers `ext/Cargo.toml` and `ext/Cargo.lock` | ext/authoring.md and kernel/gate.md: use the build-slot path |
| EH-1 | ext/host.md §9.2: `state/ext/catalog.json`, `state/ext/<name>/data/`, `state/ext/<name>/log` in the layout | ACCEPT | supervise §3.1 | CONVENTIONS §3 (request below) |
| EH-2 | ext/host.md Q-H5: no `RLIMIT_AS` below 256 GiB (pool needs about 130 GiB of virtual address space) | ACCEPT | sandbox §3.10: no `RLIMIT_AS` anywhere | — |
| EH-3 | ext/host.md: a profile for `init-kernel selfmap --describe` | ACCEPT | sandbox §3.4 table and §3.9: `build` profile, `OUT = cache/gate/<job>/` | — |
| EH-4 | ext/host.md Q-H4: kill every descendant when the body exits or crashes | ACCEPT (already) | none (supervise §3.8.2: mark-based sweep on macOS, pid namespace on Linux; tested on macOS) | — |
| EH-5 | ext/host.md: kernel reload request must carry the published names (`ReloadKind::Kept`) | ACCEPT | supervise §4.5 `reload{kind:"kept", components}`; §3.6.8; initctl §4.1 `reload` (`kind:"request"`) | — |
| CF-1 | kernel/config.md Q-C1: pause/probation/rollback-defer/depth keys duplicated in `[evolution]` and `[supervise]` | ACCEPT | none: they stay in supervise §4.9 | evolution.md §4.6 and kernel/config.md: drop them from `[evolution]` |
| CF-2 | kernel/config.md Q-C3: `model.provider`, `model.id`, `model.effort` copied into `boot.json` | ACCEPT (mapped) | supervise §4.3: `boot.json.model = {provider, model, effort}`, where `model` holds config `model.id` (the name body/provider.md and body/boot.md read); `env.model` uses `id` (awareness) | kernel/config.md: state the mapping |
| GT-1 | kernel/gate.md Q-SB1, kernel/selfmap.md Q-SB2: G1 executes candidate code, so it runs in the `build` profile, not in the kernel | ACCEPT | sandbox §3.9 table, §3.4 table | — |
| GT-2 | kernel/gate.md Q-SV1 a–d (confirmations: probe() for G2/G3/G4, `needs_handoff`/`needs_g4`, stub builds use G0 list, G3 fetched via sandboxed upload-pack) | ACCEPT | supervise §3.6.3 title and text | — |
| GT-3 | kernel/gate.md Q-SV1e: `ProbeOutcome` includes the last `boot` event | ACCEPT | supervise §3.6.3 (`last_boot_event`, read per K-F2 before deletion) | — |
| GT-4 | kernel/gate.md Q-SV1f: `ProbeOpts {session, mode, boot_reason, keep_alive, extra_files}` | ACCEPT (extended) | supervise §3.6.3: `ProbeOpts` adds `purpose` (Probe/Run) and `ext_set`; `probe()` takes the proxy `OpenScope`; `ProbeHandle` | kernel/gate.md: pass `purpose` and `ext_set` |
| GT-5 | kernel/gate.md Q-SV1g: `work/task/` placed before spawn | ACCEPT | supervise §3.6.3 table (`extra_files`) | — |
| GT-6 | kernel/gate.md Q-SV2: layout `cache/gate/<job>/`, `vault/gate/{pool,jobs,noise.json}` | ACCEPT | supervise §3.1 | CONVENTIONS §3 (request below) |
| GT-7 | kernel/gate.md Q-SV3: kernel-originated user messages for probe and run homes | ACCEPT | supervise §3.6.3 `send_input` (same `input` frame) | — |
| GT-8 | kernel/gate.md Q-SV4: owner acceptance calls `init-kernel gate task import state/tasks/<id> --pool` | ACCEPT | supervise §3.10 item 8 | — |
| GT-9 | kernel/gate.md Q-IC1: probe-lane refusals; `gate --quick` refused in a G4 run | ACCEPT | initctl §3.4 (lane purposes `probe` and `run`); test IC-20 | — |
| GT-10 | kernel/gate.md §10: `dep` (and `t0_escalated`) scope runs G4 synchronously before the handoff | ACCEPT | supervise §3.6.6 (synchronous G4 commits straight to LKG); initctl §4.1 `promote` stages; supervise §10 | — |
| GT-11 | kernel/gate.md §10 and §7: an `inconclusive` G4 keeps the version provisional and asks the owner | ACCEPT | supervise §3.6.6 (`/approve` → LKG, `/deny` → rollback), §3.7.1 code `provisional_needs_owner`, §4.6, F24 | — |
| GT-12 | kernel/gate.md §3.4: G2 probes the candidate and the parent in parallel; G4 runs 4 in parallel | ACCEPT | supervise §3.6.3: `max_concurrent_probes` = 6; §4.9 | — |
| PX-1 | kernel/proxy.md Q-S1: connect only to literal kernel socket paths, never a subtree; deny `vault/`, `cache/`, `probe/` to the live profile | ACCEPT (clarified) | sandbox §3.3.4 §6 bullet: kernel sockets are `path-literal`; the `subpath WORK` rules are only for the agent's own sockets, and no kernel socket lives there. `probe/` is denied by default; test S-M50 | — |
| PX-2 | kernel/proxy.md Q-S2: supervise calls `set_label("live", …)` at every body boot | ACCEPT | supervise §3.4 live-body steps | — |
| PX-3 | kernel/proxy.md Q-S2: supervise passes an `AlertSink` at start | ACCEPT (record shape aligned) | supervise §3.7.1: `alerts.jsonl` record = `{"v":1,"ts","source","code","message",…}` (the proxy's shape) | kernel/gate.md: emit through the same sink |
| PX-4 | kernel/proxy.md Q-S2: layout entry `vault/traces/v1/` | ACCEPT | supervise §3.1 | CONVENTIONS §3 (request below) |
| PX-5 | kernel/proxy.md Q-I1: `status.budget` and cache rate as integers | ACCEPT | initctl §4.1 `status` (`*_musd`, `cache_hit_permille`); test IC-21 | — |
| PX-6 | kernel/proxy.md Q-SB3: confirms the proxy reaches only configured providers | ACCEPT (no change) | none | — |
| RM-1 | roadmap.md Q3: `install` creates test homes from a template quickly and APFS-clones a warm target dir | ACCEPT (modified) | supervise §4.8 `install --template <home> --home <new>` (clonefile of the whole home, incl. `cache/deps/`; there is no `cache/target`) | roadmap.md: refer to `cache/deps/` |
| TS-1 | testing.md Q4: scriptable owner channel; owner notification as a file | ACCEPT (already) | supervise §3.10 item 9 (same socket and frames; `alerts.jsonl`) | — |
| TS-2 | testing.md Q5: implement the fault points of §3.8.1 with those exact names | ACCEPT (modified names) | supervise §4.10 mapping table (`supervise.swap.*` → implemented names; `lkg` and `rollback` names kept); initctl §3.7 places `serve.promote.after_fetch` | testing.md §3.8.1: use the implemented names |
| TS-3 | testing.md Q10: `install` places `init-mockllm` and `init-fakebody` in `kernel/bin/` | ACCEPT (opt-in) | supervise §3.1, §4.8 `--with-testkit` | — |
| TS-4 | testing.md §3.7 S9: connecting to "a test socket in `$TMPDIR`" must fail | REJECT | none: `$TMPDIR` is `work/tmp/`, where agent-local Unix sockets are allowed on purpose (sandbox §3.3.1 section 6, §10) | testing.md S9: use a socket the runner creates outside `work/` |
| TS-5 | testing.md S14: is signalling the kernel blocked on macOS? (UNVERIFIED there) | ACCEPT (answered) | none: verified 2026-10-19, `kill -0` on a process outside the sandbox fails with `EPERM` (sandbox §8 S-M40/41) | testing.md S14: assert `EPERM` (macOS) / `ESRCH` (Linux: pid not visible) |
| ESC-1 | own (initctl §4.1 `vendor`), raised by spec §2.1 invariant 5 "the owner approves any action that leaves the sandbox" | ESCALATE-OWNER | none yet | — |
| ESC-2 | own (sandbox §3.3.4 §4 and §9 item 4): `/opt/homebrew` readable by default | ESCALATE-OWNER | none yet | — |

### Escalations

- **ESC-1. Should `initctl vendor` need owner approval?** Recommended: no approval by default (`[vendor] require_approval = false`), bounded by crates.io-only, exact versions, 20/day, refused while provisional, no package code run, checksums recorded. Trade-off: the agent can add dependencies on its own, and a full G4 still guards them; a strict reading of invariant 5 would require `/approve` for every fetch, which slows T2 growth.
- **ESC-2. Should `/opt/homebrew` be readable inside the sandbox by default?** Recommended: yes, minus `var/` and `etc/`, so the agent can use the owner's installed CLIs (jq, rg, python). Trade-off: the agent can read installed package files under `/opt/homebrew` (normally no secrets; databases and configs in `var/`/`etc/` stay denied), against losing those tools unless the owner lists them in `[sandbox] extra_read`.

## Requests to the main session (CONVENTIONS.md)

**§3 runtime layout** (all already in kernel/supervise.md §3.1):

1. `bin/`: add `init-body.lkg` (hard link to the LKG binary) and the kept-component sets: `bin/ext.d/<set>/<name>.wasm` (immutable), with symlinks `bin/ext`, `bin/ext.prev`, `bin/ext.staged`, `bin/ext.lkg` (EV-2, EH-1).
2. `body/`: replace "Only the kernel advances `main`, on promotion. Remote `upstream` points to the seed." with: "Not authoritative: the authoritative `main` is in `lineage/body.git`, advanced only by the kernel. Remotes: `origin` = `lineage/body.git`, `upstream` = `lineage/seed.git` (both read-only)."
3. `state/`: add `spill/<sid>/` (TL-2), `blobs/` (LC-6), `ext/` (`catalog.json`, `<name>/data/`, `<name>/log`; EH-1).
4. `state/kernel/`: add `inbox.jsonl`, `alerts.jsonl`, `jobs/`, `gate/`, `probe/`, `vendor/`, `proposals/`, `logs/`, `usage.json`, `thinning.json` (EV-3).
5. New row `lineage/` (kernel; agent read-only): `body.git` (authoritative lineage and gate notes), `seed.git` (seed mirror), `trees/<sha12>/` (exported LKG and current trees) (EV-5).
6. New row `probe/<id>/` (kernel; agent none; each probe instance sees only its own root).
7. `cache/`: replace `cache/target/` with `cache/deps/<key>/` (dependency build cache), `cache/build/slot/{src,out}/` (the single build slot), `cache/gate/<job>/` (GT-6), `cache/vendor/<job>/`, `cache/git-home/`. Reason: a single shared target dir lets one candidate's `build.rs` poison later builds (supervise.md §3.6.2).
8. `vault/`: add `traces/v1/` (PX-4), `gate/{pool,jobs,noise.json}` (GT-6), `usage.jsonl` (EV-3).
9. `run/`: add `owner.sock` (owner only; never reachable from any sandbox), `kernel.lock`, `kernel.pid`, `panic` (panic latch), `mark/<instance>` (macOS sandbox-instance marks). Agent access stays "connect to `kernel.sock` and `proxy.sock` only".
10. `kernel/bin/`: `init-mockllm`, `init-fakebody` only in installs made with `--with-testkit` (TS-3).

**§4 contract registry:**

11. Env-var row: add `INIT_BOOT_ID`, `INIT_UI`, `INIT_KERNEL_VERSION`; note that `INIT_SESSION` may be empty (body starts a new session) (LC-3).
12. Exit-code row: add "78 at boot twice in a row → `Halted{config}` (not a crash)" (BT-4).
13. New rows owned by kernel/supervise.md: body stdio protocol (`INIT_UI=stdio-jsonl/1`); owner channel (`run/owner.sock`, `init-kernel attach`); `state/run/ready` and `heartbeat` formats; `vault/lineage.jsonl` envelope; `alerts.jsonl` record; lineage refs `refs/init/*` and `refs/upstream/*`.
14. Gate-notes row: add `refs/notes/predict` (evolution.md).
15. initctl row: add progress frames, `error.details[]`, and dotted error codes from evolution.md/gate.md.
16. Owner-config row: add kernel/supervise.md (`[supervise]`) and kernel/sandbox.md (`[sandbox]`) as section owners, with kernel/config.md as the file owner.

**§2 repository layout:**

17. Add `kernel/crates/init-testkit` (`publish = false`), as testing.md Q1 and roadmap.md Q1 request (the testkit binaries are installed by `install --with-testkit`).
18. Clarify that `initctl` is "std + `init-proto`; no async runtime, no network crates" (initctl.md §9 item 2).

## R2

Round 2, 2026-10-19. Inputs: the follow-up columns of all six R1 ledgers, CONVENTIONS v0.2, and the coordinator's extra R2 items (gate Q-SV5, config MiB thresholds, evolution install/sync/codes, ext E24).

### Follow-ups applied

| id (source ledger) | follow-up | applied in |
|---|---|---|
| WI-3 (body-core), E5 (ext) | state the live body's `LaunchSpec.cwd`; G4 runs get the task dir as cwd | supervise §3.4 (`$INIT_HOME/work`; probes `<root>/<ProbeOpts.cwd>`), §3.6.3 `ProbeOpts.cwd` (G4: `work/task`) |
| PX-4 (body-core), PV-2 (kernel-proxy-gate) | how the body learns the task id | supervise §3.10 item 8: task id = the `id` of the owner `input` that started the run (CONVENTIONS §4); no kernel-assigned ids |
| CF-1 (body-core) | copy config `model.id` into `boot.json.model.model`; fix the example | supervise §4.3 (done in R1; example uses `deepseek-flash`) |
| SV-4 (body-log-awareness) | the kernel chooses new session ids `YYYYMMDD-HHMMSS-<6hex>` and records them in `boot.json.session` | supervise §3.4, §4.1: reverted my R1 "empty string" change; `INIT_SESSION` is always set by this kernel (CONVENTIONS §3); an empty value is still tolerated by the body (CONVENTIONS §4 env row) |
| SV-5, SV-6, SV-10 (body-log-awareness) | boot event mirrors `boot.json`; `update_failed` fields; `handoff` cause | no change needed: R1 names (`exit_code`, `signal`, `log_tail`) are what log-context §4.2 now reads; `boot.json.cause` stays as an extra the body may use instead of deriving it |
| SV-18 (evolution-testing), RS-2; coordinator item 2 | MiB-granular disk thresholds in test mode | supervise §3.7 Disk (config.md `[supervise.disk]` `_gib`, plus `_mib` siblings with precedence in `proxy.test_mode`; resume at 2 × the run threshold), §3.3 table and diagram, §4.9, SV-23 |
| RS-1 (evolution-testing); coordinator item 3a/3b | install from a seed **tag**; `init-kernel upstream sync` | supervise §3.11.4 install row (seed.git mirror → `refs/upstream/<tag>` → `main`, `lkg`), §4.8 (`install --seed-source … --seed-tag`, `upstream sync`), §3.7.1 alert `seed_tag_moved` |
| IC-2 (evolution-testing); coordinator item 3c | document `E_QUOTA.data.quota` and `E_BUSY.data.reason = "thinning_baseline"`; drop the `state.provisional` note | initctl §4.2 rows `E_QUOTA` (`promote_attempts_task`, `promotes_day`, `quick_gates_day`, `vendor_day`, `proposals_day`; `limit`, `resets_at`) and `E_BUSY` (`pipeline`, `handoff`, `queue_full`, `thinning_baseline`); §9 note removed |
| E24 (ext); coordinator item 4 | §3.6.8: "the live body already runs the new data" is true for T0 only | supervise §3.6.8: T0 is live; T1 keeps running the agent's own trial build until `reload{kind:"kept"}` |
| Q-SV5 (gate R2); coordinator item 1 | `ProbeHandle::stop()` deletes the root; the G4 check needs the workspace | supervise §3.6.3: `ProbeOpts.keep_on_stop` (copied after the sweep, before deletion, into `cache/gate/<job>/`, symlinks not followed); `ProbeOutcome.kept`; the gate owns and deletes the copies by exact path |
| AW-3 (kernel-proxy-gate) | install copies selfmap.md §4.4's recipes to `kernel/docs/selfmap.md` | supervise §4.8 install row (`kernel/docs/`) |
| BT-2, SV-5, SV-6 (kernel-proxy-gate) | `send_input` to probes; `ProbeOpts`, boot event in `ProbeOutcome`; `set_label("live")`; `AlertSink` | done in R1 (supervise §3.4, §3.6.3, §3.7.1); nothing more |
| IC-3d (kernel-proxy-gate) | register the `gate.*` codes in initctl §4.2 | initctl §4.2 (`gate.*` row by reference to gate.md §4.1; the failing stage's code is `error.code`; `E_GATE_FAILED` only when the gate gave none), §4.1 gate/promote results (`stages[].code`) |
| IC-4 (kernel-proxy-gate), CONVENTIONS §5 ICJ | integers only | initctl §4.1 `status` (`disk.free_mib`), gate result and job record (`wall_ms` instead of float `secs`) |
| HO-4, SB-1 (kernel-proxy-gate) | G1 row in sandbox §3.9 | done in R1 |
| CONVENTIONS v0.2 §3 | layout mirror | supervise §3.1 `vault/` line: `keys/<provider_id>` (0600, `init-kernel keys set`), `traces/v1/{req/,packs/,scopes.jsonl}`, `gate/{pool/,jobs/,noise.json}` |
| CONVENTIONS v0.2 §4 env row | `INIT_TEST_*` honoured only with `INIT_TEST=1` | supervise §4.1: the kernel sets `INIT_TEST=1` and copies `INIT_TEST_*` from its own environment only when `proxy.test_mode = true` |
| CONVENTIONS §6, settled by the main session | probe round trip on probe-mode boots only | supervise §3.5 readiness contract |
| CONVENTIONS §6 pending | mark provisional defaults | initctl §4.1 vendor `require_approval = false` → PENDING OWNER (ESC-1); status example caps → PENDING OWNER (E1 proxy/gate); supervise §4.3 `env.budget` example → PENDING OWNER (E1); sandbox §3.3.4 and §9 `/opt/homebrew` → PENDING OWNER (ESC-2); sandbox §8 CI runners → PENDING OWNER (CI, E4) |
| sandbox profile changed in R1 | re-run the macOS suite | sandbox §3.3.1 and §8: re-run 2026-10-19 on macOS 14.6 with the text extracted from sandbox.md. 58/58 applicable `body`/probe checks pass (incl. new S-M49a–h, S-M50a–b, S-M51, S-M52); `build` 6/6; `git-read` 3/3 (the alternates leak is still blocked). One first-run failure (S-M49g) was a harness artifact: the probe's cwd was outside its root; with the kernel's cwd it passes. `/usr/bin/kill` does not exist on macOS (test corrected). No profile text changed in R2 |

The 90-minute G4 limit (settled) does not appear in my docs: gate.md owns it.

### Not applied

- None of the follow-ups addressed to my docs was left out.
- Observed, but in another doc: body/loop.md §3.x still says "until supervise adds it" about `input.mode`, which supervise §4.5 has had since R1. That is loop.md's edit.
- Observed, but in CONVENTIONS: CONVENTIONS §4's env row says `INIT_SESSION` "may be empty", while §3 says the kernel chooses the id. This doc implements "always set by the kernel; tolerated empty". Request to the main session: reword the §4 row to "always set by the kernel; a body treats empty as 'start a new session'".

### Remaining open questions (own, no cross-doc dependency)

- supervise §9: (1) deliver inputs to a provisional body during a long G4-rollback wait (current: yes); (2) Linux without user namespaces (current: refuse, never run unsandboxed).
- initctl §9: (1) `vendor` on the probe lane (current: no); (2) CLI output format (current: JSON on stdout).
- sandbox §9: (1) loopback TCP inside the Linux network namespace (current: no); (2) `/opt/homebrew`: PENDING OWNER ESC-2; (3) CI runner hardware: PENDING OWNER (CI).
