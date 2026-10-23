# ROOT-S1a: Root `sealed` for the lean proof (PROOF-LEAN.md, m7-design.md §3.1.5–§3.2, §4)

Status: **done for the lean scope** (2026-10-21). Lane ROOT-S1a. The wire format below is **pinned**: GATE-V2a builds against it.

## Control link: `sealed.g4_slice` (kernel → Root, root/root.md §4.1.1 framing; PINNED)

Request (one ICJ line on fd 3; ids `k-<n>`):

```json
{"v":1,"id":"k-17","cmd":"sealed.g4_slice","args":{
  "gate_id":"j-000051",
  "candidate":{"body":"<sha256 of the body binary, 64 hex, already store.put>",
               "ext":{"set":null,"components":[]},
               "t0":null},
  "incumbent":"t9a07c2e91d04",
  "tier":{"name":"standard","n":32,"repeats":3},
  "budget_musd":3000000,
  "wall_s":5400}}
```

- `gate_id`: `^[a-z0-9._-]{1,64}$`. It seeds the task sample (`seed(gate_id, "slice")`) and names the owner-only detail file.
- `candidate.body`: required, a blob in the Root store (`store.put kind:"init-body"` first). `ext` is accepted and ignored
  (sealed runs run the body binary only, as audits do). `t0` must be null or absent (D-L2: no t0 blob / tuple v2).
- `incumbent`: a tuple id; it must equal the Root's `lkg`, else `root.args`.
- `tier`: `name` ∈ `lean|standard|strong` (copied into the record), `n` 8–64 (the slice uses min(n, |G4 partition|)),
  `repeats` 1–5 per arm.
- `budget_musd`: the lane cap for the whole slice (charged to the agent pool's sealed lane). `wall_s`: 60–14,400.

The **response is asynchronous**: other requests, responses and events interleave on the link while the slice runs
(up to `wall_s` + about 60 s). Correlate by `id`. One slice at a time.

Result (`ok:true`; integers, bools and strings only):

```json
{"seq":123,"status":"superior","ratchet_signal":false,"unconfirmed":0,"ratchet_k":4,
 "tier":"standard","n":32,"repeats":3,"d_permille":200,"m_permille":220,"cpp_ratio_permille":950,
 "looks_left":null,"s_version":"s1.0","dup":false,"spent_musd":371000}
```

- `status` ∈ `superior | tie | tie_negative | inferior | inferior_early | underpowered` (a valid test),
  or a candidate tripwire `canary | tamper | leak | health`, or `error:<cause>` with
  `<cause>` ∈ `harness_health`, `runs_failed:<outcome>`, `checker_tamper`, `budget`, `wall`, `cancelled`, `internal`.
  An `error:*` is never a verdict (the kernel maps `error:budget` to `inconclusive: budget`, the rest to
  `inconclusive: sealed_unavailable` or its own rule).
  - `tamper`: a candidate run's check reported `"tamper":true` (a protected file changed); the slice stops at once.
  - `canary`: the `impossible` or the `tamper` canary scored 1000 on the candidate.
  - `leak`: the `leak` canary scored 1000, or the install GUID (`sealed/guid`) is in the candidate's body blob.
  - `health`: the candidate failed the `health` canary (the incumbent passed it).
- Rounding (what the agent may see, m7 §3.2.11): `d_permille` to the nearest 50, `m_permille` to 10,
  `cpp_ratio_permille` to 50 (half away from zero). `d_permille` and `m_permille` are 0 for tripwires and errors.
- `ratchet_signal`: exact sign-flip p ≤ 0.25 ∧ D ≥ 1/n (Root-computed). `unconfirmed`: kernel `promotion` records
  with `milestone_required:true` since the last complete `milestone` record. `ratchet_k`: the charter's
  `sealed.ratchet_k` (default 4), halved by each `not_confirmed` milestone and reset by a `confirmed` one.
- `looks_left`: always `null` and `dup` always `false` in the lean build (D-L2: no looks, no `dup`, no rotation).
- `seq`: the lineage seq of the `sealed_g4` record (also present for `error:*`).

Refusals (`ok:false`, `error.code`; nothing runs, no record): `root.sealed_none` (no S v1 drawn, or fewer than 24
valid G4 tasks), `root.busy` (a slice, audit, calibration or milestone is running), `root.budget` (`budget_musd`
below the slice estimate; `error.data.estimate_musd` says how much), `root.store` (the body blob is missing),
`root.args`, `root.mode` (safe mode). `root.sealed_quota` is never sent by the lean build.

## Records (Root-written, ICJ, integers)
`sealed_draw`, `sealed_g4`, `sealed_calibrate`, `milestone` exactly as m7-design.md §4.2, with the lean fields noted
above (`looks_left:null`, `dup:false`). No record carries task ids, templates of single tasks, seeds or content.

## Done
- **Pack install and draw** (`sealed/pack.rs`). `init-root sealed draw [--pack s1] [--pack-dir DIR]` installs the pack
  whole into `ROOT_HOME/sealed/packs/<pack>@<version>/` (files 0400/0500; an installed version is reused only if its
  tree digest matches, so a changed pack needs a new version), creates `sealed/guid` (16 random bytes), and draws S v1
  once: per family (pack.json `composition`, default D-L1 E 14+10, C 9+5, T 9+5), a seeded order that alternates
  languages, one template per slot, the reserve on templates the G4 partition does not use, plus the 4 canaries.
  Each instance: a 32-byte seed from `/dev/urandom` (kept in `sealed/seeds/<id>.json`, 0600), `gen/run` in a sealed
  instance with no network (the pack's `lib/`, `runner/`, the family's `_*` helpers and the template rebuilt in the run
  root, FORMAT.md's relative paths), a random 12-hex id written into task.json, `provenance.json` v2 with the
  draw-time check-dir digest, then §3.1.5's model-free validation (solution 3× = 1000; no-op 3× equal and ≤ 500;
  empty < 1000; in time; check dir intact; canaries: no-op stable and < 1000, honest solution stable, health = 1000).
  A failed template is replaced by a spare of its family when there is one, logged, and alerted (`sealed_template`).
  `sealed/S.json` holds the partitions; `sealed_draw{pack, pack_version, s_version, counts, skipped_templates,
  spent_musd:0}` carries counts only. `task::store` leaves the reserve out, so no audit, gog or slice runs it.
- **Sealed runs, M7 additions** (`sealed/run.rs`, `egress/lane.rs`). The lane enforces the task's v3 caps on actuals
  (`max_turns` on requests whose `x-init-purpose` is `turn` or absent, `max_output_tokens`, `max_input_tokens`): the
  next request after a cap gets 429 `x-init-cap: turns|tokens_out|tokens_in`, `type: init_cap_exceeded`; the run ends
  `cost_capped` and, with ≥ 1 request, is valid and scored. Runs report `requests, turns, input/cached/output tokens,
  caps_hit, tamper, void, wall_ms`. The check reads `{"score_permille":0,"tamper":true}` whatever the exit code, and
  hashes the check dir before and after (s1lib's `tree_digest` format, unit-tested against it): a change voids the
  run (`checker_tamper`). Sealed processes get `$TOOLCHAIN/bin` and `$DEVDIR/usr/bin` first on PATH (cargo for Rust
  checks; the CLT python3 without the xcrun shim, which cannot write its cache in the sandbox).
- **G4 slice** (`sealed/slice.rs`, control link in `sup.rs`): as pinned above. Order: GUID scan of the candidate body →
  canaries (health on both arms, the incumbent's retried once; impossible, tamper, leak on the candidate) → incumbent
  runs from the cache (exactly R per task, seeded subset) with a fresh sentinel on ⌈20%·n⌉ tasks (drift → cache dropped
  by exact path, alert `incumbent_drift`, fresh arm) → paired runs interleaved by task at `sealed.parallel`, in two
  halves with the futility look between → c712701 rules → `exact::slice_status` (ROOT-S1b) on permille sums, δ from
  the incumbent repeats smoothed with the calibrated δ_S → cost per pass → `sealed_g4` (rounded) and the owner-only
  detail `sealed/g4/<gate_id>.json`. A candidate run flagged `tamper` stops the slice at once. Each run reserves its
  whole lane cap from the slice budget first, so parallel runs never overshoot `budget_musd`. Candidate runs are never
  cached (no winner's curse); calibration runs are cached `selectable:false`, so never reused.
- **Calibrate, probe, milestone** (`sealed/slice.rs`). `sealed calibrate [--repeats 6]`: R runs of the LKG on every G4
  task, band selection (≥ 24 in band → G4 keeps only those, `selected:true`), `exact::calibrate` (pseudo-A/A, CP
  intervals, variance components, MDEs, verdict), δ_S for R = 2, 3, 5 into `noise.json` `s1`, `sealed_calibrate`;
  FA above 70‰ raises `calibrate_stop`. `sealed probe [--per-family 2]` (= `calibrate --probe N`): N throw-away
  instances per family (E split by language), R = 1 on the LKG, deleted by exact path; per-family requests, turns,
  tokens in/cached/out, cost, wall, passes and caps hits in `sealed_calibrate.probe`. `sealed milestone` (and
  `audit run --reason milestone --set reserve`): current against the last confirmed milestone's `current` (else
  genesis; `--baseline`/`--current` override), both fresh on the reserve, health on both arms and leak on the current
  first, `exact::milestone`; a complete check marks the reserve used, `partial` keeps it, a regression alerts
  `milestone_regression`. Calibrate, probe and milestone refuse to run outside a test-mode Root without `--real`.
- **Owner CLI**: `sealed draw|show|calibrate|probe|milestone|slice` (show: `--g4 <gate_id>`, `--templates`; counts per
  partition and family otherwise). Through the running Root's owner socket, or in-process under the Root lock.
- **Charter** `[sealed]`: new keys `pack` ("s1"), `ratchet_k` (4, 1–8), `milestone_repeats` (3, 2–5), `cache_max_age_h`
  (72, 0–168), `sentinel_permille` (200, 100–1000); `margin_max` default 0.20 → 0.25 (m7 §4.3).
- **Records**: the Root kinds `sealed_draw`, `sealed_g4`, `sealed_calibrate`, `milestone` (lineage/kinds.rs).

## Tests ($0: fixture pack `tests/fixtures/sealed/s1fx/`, Perl fixture body variants, init-mockllm, homes under `root/target/test-homes/m7/`)
`bash ci/slot.sh cargo test --offline -p init-root --features test-fixtures --test m7` → 3 passed (≈ 7 min):
- `m7_draw_and_g4_slice`: **M7-R01** (draw counts by family×partition, provenance v2, seeds 0600, no ids, templates or
  GUID in any record), **M7-R02** (2 broken templates skipped, logged, alerted; fewer than 24 G4 tasks → `root.sealed_none`;
  `_` helper dirs ignored), **M7-R04** (superior, tie; inferior through the control link), the canaries (impossible and
  tamper canary at 1000 → `canary`; no health → `health`; GUID in the body → `leak`), **the red-team tamper check** (a
  protected test edited; the runner's exit-3 tamper line → `tamper`, slice stopped), **M7-R08/R09 lean** (cache reuse
  with an OK sentinel; injected drift → cache dropped, `incumbent_drift`, fresh arm), **M7-R11** (turn cap on actuals:
  4 admitted, the 5th refused, runs valid and scored), **M7-R12** (an incumbent without requests →
  `error:runs_failed:done`), **M7-R13** (rounded record, owner detail by `sealed show --g4`), **M7-G13** Root half
  (`error:harness_health`), refusals (`root.args`, `root.budget`, `root.store`), and `sealed.g4_slice` end to end on the
  control link (a running Root with the fake kernel; the asynchronous answer).
- `m7_calibrate_probe_milestone`: **M7-R14** (calibration record, band selection, δ_S in noise.json, selection runs never
  reused), **M7-R18** (probe: one throw-away instance per group, deleted by exact path, per-family aggregates, S
  unchanged), **M7-R17 lean** (confirmed; reserve used up; regression halves `ratchet_k` and alerts; partial keeps the
  reserve).
- `m7_real_pack_rust_and_canaries` (S1-CORE's integration needs, on the real s1 pack's Rust E templates and the 4
  canaries, counts and bytes only): every Rust template's reference scores 1000 in the Root's check environment
  (toolchain on PATH, SDKROOT/DEVELOPER_DIR), every canary validates with the real runner, and the install GUID is in
  every generated instance (leak canary included).
- Unit: `sealed::run::tests::tree_digest_matches_s1lib` (the Root's checker digest = s1lib's reference), the `[sealed]`
  defaults test (margin_max 250‰).

**M7-R IDs:** covered R01, R02, R04 (superior, tie, inferior; tie_negative and underpowered through ROOT-S1b's unit tests
of `slice_status`), R08 (lean: reuse, `selectable:false`), R09, R10 (GUID only), R11 (turns), R12, R13, R14, R17 (lean),
R18; R03 is ROOT-S1b's. **Cut by D-L2:** R05 (`dup`), R06 and R07 (looks, rotation), R10's token and template-fingerprint
scans, R15 (`store.put kind:"t0"`, tuple v2), R17's `confirmed` pointer, rollback and demotion. **Not built (outside this
lane's lean list):** R16 (`audited` moved by a slice).

## Deviations
- **Charter defaults:** only `margin_max` moved to 0.25. `parallel` stays 2 (4 would shrink the zone's instance limit to
  `max_instances − 4` for every install), `audit_n` 24 and `audit_repeats` 2 stay; the M7 test home's charter should set
  `parallel = 4`. New keys: `pack`, `ratchet_k`, `milestone_repeats`, `cache_max_age_h`, `sentinel_permille` (the look and
  rotation keys are cut).
- **No replacement draws or template probes** at calibration (S ships with the in-band tasks if ≥ 24, else unselected,
  `selected:false`); the reserve is drawn at default difficulty.
- **Milestone:** baseline = the last confirmed milestone's `current`, else genesis (no `confirmed` pointer); a regression
  alerts `milestone_regression`, the rollback is the owner's; a used reserve is refused unless `--force` (no fresh
  reserve without rotation).
- **The candidate's ext set and t0 are not installed in sealed runs** (body binary only, like audits); `ext` is ignored.
- **`--real`:** calibrate, probe and milestone refuse to spend outside a test-mode Root without it.
- **Per-run lane cap:** each run reserves its task's whole `max_cost_musd` from the slice budget, so a slice stops
  (`error:budget`) when less than one run's cap is left instead of starting a run its lane would refuse.
- **Sealed env:** `$TOOLCHAIN/bin` and `$DEVDIR/usr/bin` lead PATH, and SDKROOT/DEVELOPER_DIR are set, for every sealed
  process (bodies too), so Rust checks build and link.
- **Lane rule:** this lane's tests draw from fixture packs in test homes and, for S1-CORE's integration check only, from
  a copy of the real pack's Rust templates and canaries; they assert counts and bytes, never instance content.

- **Root size:** `sealed/pack.rs` and `sealed/slice.rs` add 1,057 lines and the edits to run, task, mod, lane, sup,
  link, cli and kinds about 260, above m7 §6's ≈ 360 estimate (caching, calibration, probe, milestone and the owner
  detail each came out larger); the `sealed/` sub-budget is set at release as measured + 5% (root.md §4.8).

## Needs from others
- REAL-M7: run `init-root sealed draw --pack-dir root/sealed/packs/s1` in the M7 home, set `[sealed] parallel = 4`, and
  pass `--real` to `sealed probe|calibrate|milestone` on the orchestrator's go.
- GATE-V2a: map `error:budget` → `inconclusive: budget`, other `error:*` → `inconclusive: sealed_unavailable`; send
  `promotion{milestone_required:true}` on `lineage_gain` (the Root counts them for `unconfirmed`).

## Left
- Replacement draws and reserve template probes (§3.1.6), `audited` from a slice (§3.2.12), token and fingerprint scans,
  rotation, looks, `dup`, the `confirmed` pointer and milestone rollback: all cut or deferred by PROOF-LEAN.
