# Reconciliation R1: kernel-proxy-gate

```
Status: R1 · 2026-10-19 · Area: kernel
Docs covered: kernel/proxy.md, kernel/gate.md, kernel/selfmap.md, kernel/config.md, adr/002-kernel-model-proxy.md
```

Sources: every "Open questions", "Cross-doc" and "Spec deltas" section of the 26 committed docs, searched for items addressed to these docs or to contracts they own (CONVENTIONS §4).

Totals: **53 items**. 47 accepted, 4 rejected, 2 escalated to the owner.

## Ledger

| id | from doc § | question | decision | change made (your doc §) | follow-up that ANOTHER doc must make |
|---|---|---|---|---|---|
| AW-1 | awareness.md §9 item 3 | add a `timezone` key so the owner's local time can be rendered | ACCEPT (no new key) | config.md §4.3: `budget.day_starts_at_utc_offset` is also the owner's display offset | awareness.md: render local time with that offset |
| AW-2 | awareness.md §9 X2 | `/_init/stats` shape (integers, permille, micro-USD) on `proxy.sock` | ACCEPT (done in draft) | proxy.md §4.3 | — |
| AW-3 | awareness.md §9 X4 | confirm `compensates_for`, `id`, tool `name`/`input_schema`, `wit_exports`, `body_tokens`; ship the recipes | ACCEPT | selfmap.md §3.2 (top-level `body_tokens`), §4.2 schema, §4.4 recipes | supervise.md (install): copy selfmap.md §4.4 to `$INIT_HOME/kernel/docs/selfmap.md` |
| BT-1 | body/boot.md §9 Q-B5 | kernel-side model allowlist (the body's check is agent-editable) | ACCEPT | proxy.md §3.6 (price table = allowlist, frontier-only by owner pricing); config.md §4.5 | — |
| BT-2 | body/boot.md §9 Q-B6 | G3 drives the probe body with `input` frames | ACCEPT | gate.md §3.6 (instruction sent as an `input` frame) | supervise.md: let the gate send `input` frames to a probe (gate.md Q-SV3) |
| BT-3 | body/boot.md §9 Q-B7 | is the manifest what selfmap needs (WIT majors, event types)? | ACCEPT | selfmap.md §3.5, §4.2: the whole manifest minus `tools` is copied verbatim, so new fields need no schema change; WIT majors welcome, event types not needed | body/boot.md: optional `ext_api_majors` field |
| LC-1 | body/log-context.md §9 X3 | proxy recomputes `prefix_hash` per §3.10.4, `x-init-head-hash`, `last_head_hash`, cache permille | ACCEPT (done in draft) | proxy.md §3.12.1, §4.1, §4.3 | body/log-context.md: add extraction rows keyed by route family (proxy.md Q-P4a) |
| LC-2 | body/log-context.md §9 X5 | G2 continuity check on the staged log copy | ACCEPT | gate.md §3.5 criterion 7 | — |
| LC-3 | body/log-context.md §10 D8 | one exact `prefix_hash` definition, recomputed by the proxy | ACCEPT | proxy.md §3.12.1 (log-context owns it; proxy adds diagnostic `head_bytes_hash`) | — |
| PV-1 | body/provider.md §9 Q-P1 | proxy answers (a)–(f); head-hash extraction for provider id `mock` | ACCEPT | proxy.md §4.1 (`[providers.mock] preset = "deepseek"`, head object `provider = "mock"`, no keys in test mode); config.md §4.10 test example | — |
| PV-2 | body/provider.md §9 Q-P7 | how the body learns the current task id for `x-init-task` | ACCEPT (proxy side unchanged) | proxy.md §3.4 (absent header → `_none` under the same cap) | evolution.md / supervise.md: define task-id delivery to the body |
| EV-1a | evolution.md §9 Q1 | body sends `x-init-task`; proxy records it and enforces per-task caps | ACCEPT (done in draft) | proxy.md §3.4, §3.6, §3.8 | — |
| EV-1b | evolution.md §9 Q1 | gate runs carry `X-Init-Gate` set through the gate body's env | REJECT | — (proxy.md §3.4: attribution by lane socket) | evolution.md: drop the header proposal. Reason: anything the body sends it can forge or omit; a socket per lane is an unforgeable capability |
| EV-5a | evolution.md §9 Q5a | perf-only accept rule when `change_type == perf` | ACCEPT | gate.md §3.9 | evolution.md: `change_type` must be in `Validated` (gate.md Q-EV1) |
| EV-5b | evolution.md §9 Q5b | run `Repro: task:` tasks in G4 and report `repro` | ACCEPT | gate.md §3.7.2, §3.10 | — |
| EV-5c | evolution.md §9 Q5c | per-task records with evolution's metric names | ACCEPT | gate.md §4.4 (RunRecord plus explicit name and unit mapping) | — |
| EV-5d | evolution.md §9 Q5d | δ per metric, per model | ACCEPT | gate.md §3.8.2 | — |
| EV-5e | evolution.md §9 Q5e | escalate wholesale T0 rewrites to G4 | ACCEPT | gate.md §3.1 (`t0_escalated`) | — |
| EV-5f | evolution.md §9 Q5f | G4 on the provisional head vs LKG; cancel the parent's G4 | ACCEPT | gate.md §3.1, §3.12 | — |
| EV-11 | evolution.md §9 Q11 | component ids per §3.1; per-component `compensates_for` from a source marker | ACCEPT | selfmap.md §3.7, §3.9 | evolution.md: add kind `prompt/<file>/<slug>` (selfmap.md Q-E1) |
| EV-46 | evolution.md §4.6 | owner config names (final in config.md) | ACCEPT with changes | config.md §4.3, §4.7 (money as strings) | evolution.md and supervise.md: drop duplicated keys from `[evolution]` (config.md Q-C1) |
| EV-B | evolution.md §3.14 | per-gate spend default $1.50 | **ESCALATE-OWNER (E1)** | gate.md §3.11, config.md §9 Q-C2 marked pending | — |
| EV-W | evolution.md §3.14 | G4 wall-clock 60 min | **ESCALATE-OWNER (E2)** | gate.md §9 item 3 marked pending | — |
| AU-1 | ext/authoring.md §9.2 → gate | G0 builds the `ext/` workspace for wasm32-wasip2 (into `cache/target`) and runs its tests on the host | ACCEPT with change | gate.md §3.3 steps 1, 3, 6, 7 (two workspaces, built into supervise's slot target, not `cache/target`); dep scope includes `ext/Cargo.lock` (§3.1); ext crates exempt from forbid-unsafe | ext/authoring.md: say "the gate builds into supervise's build-slot target dir" |
| AU-2 | ext/authoring.md §9.2 → gate | G1 runs the inventory (ext/host.md §3.15) | ACCEPT | gate.md §3.4; selfmap.md §3.6 | — |
| HO-1 | ext/host.md §9 → selfmap | inventory procedure of §3.15 | ACCEPT | selfmap.md §3.6 (exports, imports, `api`, `describe`) | — |
| HO-2 | ext/host.md §9 → selfmap | run `describe` in a sandboxed child | ACCEPT | selfmap.md §3.6, §4.1 (`init-kernel selfmap --describe`) | — |
| HO-3 | ext/host.md §9 → selfmap | component SHA-256 and size out of the G1-compared part | ACCEPT | selfmap.md §3.6, §4.2 (`component_sha256` removed; the gate record keeps the tested binary's hash) | — |
| HO-4 | ext/host.md §9 → sandbox | a profile for `init-kernel selfmap --describe` | ACCEPT (no new profile) | gate.md §3.4 (the whole generator runs in the `build` profile) | sandbox.md §3.9: G1 executes candidate code, so it runs in `build`, not in the kernel (gate.md Q-SB1, selfmap.md Q-SB2) |
| HO-5 | ext/host.md §10.2 | Wasmtime `=48.0.6` LTS | ACCEPT | selfmap.md §3.6 (the kernel's describe host uses the body's engine version) | — |
| WI-1 | ext/wit.md §9 → selfmap | inventory from `describe` and the decoded world; `hooks` empty in 1.x | ACCEPT (done in draft) | selfmap.md §3.6, §3.9 | — |
| WI-2 | ext/wit.md §9 → gate | G1 fails if `body/wit/` differs from `kernel/wit/` byte for byte | ACCEPT | gate.md §3.4 item 3 (`gate.g1.wit_copy_drift`) | — |
| WI-3 | ext/wit.md §9 Q-W2 | general allowlisted egress through the proxy (for `wasi:http`) | REJECT (for now) | — (adr/002 §9 keeps it open) | — Reason: no observed need; a key-holding general egress path widens the blast radius |
| IC-3a | kernel/initctl.md §9 item 3 | stage weights for `pct` | ACCEPT | gate.md §4.1 | — |
| IC-3b | kernel/initctl.md §9 item 3 | quick-gate stage list on the probe lane (G0–G2) | ACCEPT | gate.md §3.1 kinds table | — |
| IC-3c | kernel/initctl.md §9 item 3 | gate-result cache used by re-promotes (3,600 s) | ACCEPT | gate.md §3.1 "Result reuse" | — |
| IC-3d | kernel/initctl.md §9 item 3 | the `gate.*` error codes | ACCEPT | gate.md §4.1 error-code table | kernel/initctl.md §4.2: register them |
| IC-4 | kernel/initctl.md §9 item 4 | `status.budget` and `status.cache_hit_rate` | ACCEPT | proxy.md §4.4 (`ProxyStatus`, integers) | kernel/initctl.md: show integers (micro-USD, permille) (proxy.md Q-I1) |
| SB-1 | kernel/sandbox.md §9 item 1 | confirm the G4 run and check placement; G0 verifies forbid-unsafe | ACCEPT | gate.md §3.3 step 3, §3.7.1 check protocol (`check` profile with `OUT`), §3.7.3 | sandbox.md §3.9: correct the G1 row (see HO-4) |
| SB-2 | kernel/sandbox.md §9 item 2 | the proxy's egress allowlist | ACCEPT | proxy.md §3.5, §9 Q-SB3 | — |
| SB-3 | kernel/sandbox.md §9 item 3 | add `[sandbox]` to config | ACCEPT | config.md §4.9 | — |
| SV-5 | kernel/supervise.md §9 item 5 (a–d) | G2 uses `probe()`; gate reports `needs_handoff`/`needs_g4`; G0 list reused for stub builds; G3 read back via sandboxed fetch | ACCEPT | gate.md §3.5, §3.1 and §4.1 (`GateResult`), §3.2 and §3.3, §3.6 | supervise.md: `ProbeOpts` (fresh session, workspace files, `input` frames) and the boot event in `ProbeOutcome` (gate.md Q-SV1 e–g) |
| SV-6 | kernel/supervise.md §9 item 6 (a–d) | proxy in-process; lanes; `panic()`; status fields | ACCEPT | proxy.md §3.1, §3.4, §4.2 (503 `kernel.panicked`), §4.4 | supervise.md: call `set_label("live")` at every boot; pass an `AlertSink` |
| SV-7 | kernel/supervise.md §9 item 7 | add `[supervise]` and `[sandbox]` to config | ACCEPT | config.md §4.8, §4.9 | — |
| SV-8 | kernel/supervise.md §9 item 8 | T1 component binaries committed or built? | ACCEPT (built by the gate) | gate.md §3.3 | — |
| SV-10 | kernel/supervise.md §9 item 10 | `body_tokens` per sha for `boot.json.selfmap` | ACCEPT | selfmap.md §3.2 (top-level `body_tokens`, read with `$G show <sha>:self.json`) | — |
| TS-2 | testing.md §9 Q2 | loopback plain-HTTP upstream in test mode | ACCEPT | proxy.md §4.1; config.md §4.2 (`proxy.test_mode`), §4.10 | testing.md: use `proxy.test_mode` |
| TS-5 | testing.md §9 Q5 | place the fault points | ACCEPT | gate.md §4.1 (three gate points); proxy.md §4.5a (`proxy.stream.mid`) | — |
| TS-8a | testing.md §9 Q8 | G4 fixture tasks follow the gate's task format | ACCEPT | gate.md §3.7.1 (task.json v1 schema) | — |
| TS-8b | testing.md §9 Q8 | G3 scripted so it costs $0 even with a real model | REJECT as default | — (gate.md §3.6: default `gate.g3.mode = "model"`; `scripted` available and used in CI) | testing.md: set `gate.g3.mode = "scripted"` in test configs. Reason: evolvability is a property of model plus body, and it costs ≈ $0.01–0.03 per gate on `deepseek-flash` |
| TS-10 | testing.md §9 Q10 | G0 runs the seed's `cargo test` with `INIT_MOCKLLM_BIN` | ACCEPT | gate.md §3.3 env additions | — |
| TS-K | testing.md §3.4 config table | key `gate.g2_ready_timeout_s` | ACCEPT (renamed) | config.md §4.6: it is `supervise.ready_timeout_s`, because G2 uses `probe()` | testing.md: rename the key |
| RM-1 | roadmap.md line 221 (owner inputs) | the DeepSeek key is "entered by the owner directly into `kernel/config.toml`" | REJECT | — (proxy.md §3.3, config.md §1 and §7: keys live only in `vault/keys/<id>`, written by `init-kernel keys set deepseek`) | roadmap.md: change the owner step. Reason: `config.toml` is agent-readable; a key there would enter the sandbox's view |

Decision counts: ACCEPT 47 · REJECT 4 (EV-1b, WI-3, TS-8b, RM-1) · ESCALATE-OWNER 2 (E1, E2).

## Escalations to the owner

**E1. Phase 0 money for gates: `gate_usd`, `day_usd` and `evolution_share`.**

Today's state:
- evolution.md proposes $1.50 per gate, $5/day and an evolution share of 0.5.
- gate.md's paired design needs $1.3–2.0 off-peak (up to about $4 at peak) on `deepseek-flash` per promote gate. That covers 66 runs at R = 2 on 16 tasks, plus one top-up to R = 3.
- With $5 × 0.5 = $2.50 a day for all gates, a $3.00 gate cap can never be reached by a single gate start.

**Recommended:** `gate_usd = "3.00"`, `day_usd = "5.00"`, `evolution_share = 0.6`, `prefer_offpeak = true` (as in config.md §4.10).

Trade-off:
- $1.50 forces either R = 1, which loses the within-gate noise floor so G4 ends `no_noise_estimate`, or about 8 tasks, which gives a margin above 25 points. Both are a gate that cannot tell a regression from noise.
- $3.00 costs one or two G4s a day out of a $15–40 Phase 0 total.

**E2. G4 wall-clock limit: 90 min (gate.md) vs 60 min (evolution.md).**

**Recommended:** 90 min at `parallel = 3–4` for Phase 0; revisit after Phase 0 measures per-run RSS and CPU (M-5).

Trade-off:
- 60 min requires `parallel ≥ 6`, which competes with the owner's own work on the 8-core M2, or fewer runs, which weakens the statistics.
- 90 min only delays LKG for a version that is already running provisionally.

## Requests to the main session (CONVENTIONS.md)

1. **§4 registry, row "Owner config `kernel/config.toml`".** Owner: **kernel/config.md**, for the file format, validation and the assembled schema. Semantics per section:
   - `[model]`, `[proxy]`, `[budget]` money keys, `[traces]`, `[providers.*]`: kernel/proxy.md;
   - `[gate]`: kernel/gate.md;
   - `[budget]` count keys and `[evolution]`: evolution.md;
   - `[supervise]`: kernel/supervise.md;
   - `[sandbox]`: kernel/sandbox.md.
2. **§4 registry, proxy row "Fixed now".** Add:
   - lanes (= proxy scopes) bound to sockets;
   - request headers `x-init-session`, `x-init-purpose`, `x-init-attempt`, `x-init-task` (read and stripped by the proxy);
   - cap refusals as 429 with `x-init-cap` and `type: init_cap_exceeded`;
   - test provider id `mock` (preset `deepseek`, `proxy.test_mode` only).
3. **§4 registry, new rows.**
   - `prefix_hash` (head hash) algorithm: owner body/log-context.md §3.10.4; the proxy recomputes it.
   - `tools_sha256` (`init-tools-v1`) and `head_bytes_hash`: owner kernel/proxy.md.
   - `/_init/stats` shape: owner kernel/proxy.md. Fixed now: integer-only (ICJ), plus the fields awareness.md reads.
   - Task / holdout / canary format (`task.json` v1), the `gate.*` error codes, and the lineage record `kind: "gate"`: owner kernel/gate.md.
   - `self.json` component ids: evolution.md §3.1. `self.json` schema and the `@init compensates-for` marker grammar: kernel/selfmap.md.
4. **§3 runtime layout table.** supervise.md owns the layout; please mirror its decisions here. These are needed by these docs:
   - `vault/keys/<provider_id>` (one 0600 file per provider);
   - `vault/traces/v1/{req/,packs/,scopes.jsonl}`;
   - `vault/gate/{pool/,jobs/,noise.json}`;
   - `cache/gate/<job>/`;
   - `state/kernel/gate/<job>/report.json`;
   - lane sockets `probe/<id>/run/{kernel,proxy}.sock`;
   - replace `cache/target/` with supervise.md's `cache/deps/<key>/` and `cache/build/slot/`.
5. **§5 global decisions.** Extend "The kernel may use tokio, hyper and rustls (proxy only)" with the other kernel dependencies these docs use:
   - syn 3.0.9 and proc-macro2 (span-locations);
   - wit-component / wit-parser 0.262.0;
   - tiktoken-rs 0.12.1;
   - wasmtime / wasmtime-wasi =48.0.6, only in the `selfmap --describe` child;
   - toml 1.1.8;
   - sha2 0.11.1;
   - tokio-tungstenite 0.30.0 (proxy).

   Also add: "the kernel's serde_json must not enable `preserve_order`" (ICJ).
6. **§5 global decisions, money.** Config money values are decimal strings. Internal accounting is integer pico-USD. Everything that reaches a prompt, a log or a note is integer micro-USD (`*_musd`).

## R2

Inputs: the "follow-up that ANOTHER doc must make" column of all six R1 ledgers, plus CONVENTIONS v0.2 §2–§6.

### Follow-ups applied

| id (source ledger) | follow-up | applied in |
|---|---|---|
| WI-3 (body-core) | G4 runs start with cwd = the task workspace | gate.md §3.7.3 (bullet "The body's cwd is the task workspace") |
| SM-1 (body-core) | manifest `tools_sha256` mismatch is now a G1 failure | selfmap.md §3.5, §3.8 check 6 (`manifest_tools_hash`); gate.md §4.1 codes |
| SR-1 (body-core) | G4 runs use `ProbeOpts.mode = normal`, so no probe round trip | gate.md §3.7.3 (pseudo-code and bullet "Mode `normal`…") |
| PX-1 (body-log-awareness) | drop "this mapping is an assumption"; log-context keys the head hash by wire format | proxy.md §3.12.1 |
| GA-2 (evolution-testing) | E2 closes: 90 min G4 | gate.md §3.7.4, §9 (question removed); config.md §4.6 |
| CF-1 (evolution-testing, kernel-supervise) | drop the four duplicated timing keys from `[evolution]` | config.md §4.7 (rejected there with a pointer to `[supervise]`), §8 C-08 |
| AU-3 (evolution-testing) / E10 (ext) | lockfile diffs touching only source-less (member) entries are not `dep` | gate.md §3.1 scope table and "What counts as a dependency change in a lockfile" |
| RS-2 (evolution-testing) | MiB-granular disk thresholds in test mode | config.md §3.1 "Disk thresholds" (`_mib` siblings in test mode), §4.6, §4.10 test example, §8 C-10; gate.md §3.7.4 |
| RS-4 (evolution-testing, optional) | test switch excluding wall-clock from `cost_x_wall` | gate.md §3.9 "Test switch" (`gate.g4.ct_include_wall`); config.md §4.6, §4.10 |
| E9 (ext) | G0 checks: ext members are cdylib-only; no body crate depends on an ext crate; soften the wit-bindgen claim | gate.md §3.3 step 3 (`gate.g0.ext_not_cdylib`, `gate.g0.ext_dependency`), §4.1 codes |
| E12 (ext) | `bindgen!` path is the per-major directory `wit/init-ext/v1` | selfmap.md §3.6 |
| BT-5 (kernel-supervise) | G3 and G4 use `ProbeHandle::send_input` | gate.md §3.6 setup, §3.7.3 |
| EV-1 (kernel-supervise) | the gate provides `init-kernel gate task import` for `/accept` | already in gate.md §3.7.1 and §4.1; no change |
| EA-3 (kernel-supervise) | use the build-slot path | already in gate.md §3.2–§3.3 (R1); no change |
| CF-2 (kernel-supervise) | state the `boot.json.model` mapping | config.md §4.1 |
| GT-4 (kernel-supervise) | pass `purpose` and `ext_set` | gate.md §3.5 procedure, §3.6 setup, §3.7.3 |
| PX-3 (kernel-supervise) | gate alerts go through the same `AlertSink` | gate.md §4.2 (record shape `{"v":1,…,"source":"gate"}`); proxy.md §4.7 (`"v":1` added) |
| CONVENTIONS §3 | layout | all paths checked: `vault/keys/`, `vault/traces/v1/`, `vault/gate/`, `cache/gate/<job>/`, `cache/build/slot/`, `probe/<id>/run/*.sock`, `state/kernel/{gate,alerts.jsonl}` |
| CONVENTIONS §3 | session id `YYYYMMDD-HHMMSS-<6hex>` | proxy.md examples (`20261019-132407-a1b2c3`) |
| CONVENTIONS §5 | wasmtime =48.0.6 | selfmap.md §3.6 (already R1) |
| CONVENTIONS §5 | ICJ, integers only | proxy.md: request records and `ScopeSummary` (`cache_permille`, integer `tps`), `Usage::cache_permille`, trace view money in `*_musd`; gate.md and selfmap.md records were already integer |
| CONVENTIONS §5 | money units | proxy.md §4.3 trace view (`*_pusd` never leaves the kernel) |
| CONVENTIONS §6 E1 (proxy/gate) | provisional default with marker | config.md §4.3 (and `evolution_share` default now 0.6), §4.10 example comment; gate.md §3.11 |
| CONVENTIONS §6 ESC-1 (supervise) | provisional default with marker | config.md §4.7a `[vendor] require_approval = false` |
| initctl.md §4.1 / supervise.md §4.9 | keys not yet in the assembled schema | config.md §4.8 (`max_concurrent_probes`, `config_error_exits`), §4.7a `[vendor]` |
| supervise.md §3.6.6 | owner answers `inconclusive` with `/approve` / `/deny` | gate.md §3.12 (diagram and owner-override bullet) |

### Could not apply

- **RS-2 for supervise's own keys.** The MiB form of `supervise.disk.*` is accepted only as a config file-format rule (config.md §3.1). The rule that supervise *uses* the MiB value is supervise.md's to state; until it does, testing.md's F-disk-full keeps using `INIT_FAULT … enospc`.
- **The G4 check needs the finished workspace after `ProbeHandle::stop()`, which deletes the probe root.** supervise.md §3.6.3 has no hook for this. It is raised as gate.md §9 Q-SV5 (a `keep_on_stop` option). Until then, the gate cannot run the check after the body is gone.

### Remaining open questions

- gate.md §9.1 and §9.2: should `margin_max` follow the pool size, and is `repro_fixed` evidence strong enough? Both wait for Phase 1 data.
- gate.md §9 Q-SV5 → kernel/supervise.md: keep `work/task/` past `stop()` for the G4 check.
- proxy.md §9.1: whether to pre-warm the provider cache before parallel G4 runs. This is a cost question; it waits for G4 cost data.
- selfmap.md §9.1: should `files` list every file? Revisit if `self.json` passes 100 KB.
- Owner: CONVENTIONS §6 E1 (proxy/gate) and ESC-1 (supervise), marked PENDING OWNER where their values appear. E2 is closed (90 min).
