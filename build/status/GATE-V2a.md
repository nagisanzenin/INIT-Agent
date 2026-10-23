# GATE-V2a: G4 v2, kernel half, lean (PROOF-LEAN.md D-L3; m7-design.md §3.2, §4)

Status: **done** (2026-10-21) for the lean scope. Lane GATE-V2a. Commits: 9cb420a (types, accept_v2), 4f0e650
(control link + init-fakeroot), a4006a9 (driver and wiring), plus the pool v1 test fix (below).

## Types (committed first, for GATE-V2b and EVO-LITE)
All in `kernel/crates/kernel-gate`:
- **task.json v3** (`g4.rs`): `TaskJson.{family, template}: Option<String>`, `difficulty: Option<u8>`;
  `Limits.{max_turns, max_output_tokens, max_input_tokens}: Option<u64>` (serde default, skipped when none, so
  v2 files round-trip unchanged). `TASK_JSON_V = 3`; `load_task` accepts v1–v3 and runs `check_v3` (m7 §4.4
  ranges; v3 keys in a v1/v2 file are refused).
- **Score line** (`g4.rs`): `check_score(exit, timed_out, stdout) -> CheckScore {score_permille, tamper}`; only
  the exact line `{"score_permille":0,"tamper":true}` sets `tamper`. `score()` is unchanged in meaning.
- **RunRecord v3** (`g4.rs`): `RUN_RECORD_V = 3`; `tamper: bool`, `caps_hit: Option<String>`,
  `cache: Option<String>` (`fresh|reused|sentinel`), `model_served: Option<String>`, all `#[serde(default)]`.
- **G4 record v3** (`g4v2.rs` `V2Record`, added to `G4Outcome.v2`; `stage_record` writes the keys):
  `tier`, `preflight{runs, estimate_musd}`, `screen{n, c_pass, p_pass, net, verdict}`, `sealed{seq, status, n,
  repeats, tier, d_permille, m_permille, cpp_ratio_permille, looks_left, s_version, dup, spent_musd}` (agent view,
  re-rounded; null on the v0.3 path), `sealed_refusal` (why no slice), `cpp{ratio_permille}`,
  `milestone_required`, `accept_class` (`better | lineage_gain | tie_break:<reason>`), `divergence`. The Root
  subset and the agent report carry `tier`, `sealed`, `milestone_required`, `accept_class`.
- **Screen seam** (`g4v2.rs`): `ScreenSummary {n, c_pass, p_pass, net, c_errors, tamper, canary_trip, reject:
  Option<ScreenReject>, d_permille, delta_permille, ct, perf_gain, c_cost_musd}`, `ScreenReject {Canary, Tamper,
  ScreenErrors, ScreenInferior}`, `screen_stats(runs, &ScreenNoise, ct_include_wall)`, `screen_verdict`. GATE-V2b's
  runner fills it; the body of `kernel-supervise/src/promote/g4v2.rs::run_screen` is GATE-V2b's (a stub that
  runs only the gate's own canaries until then).
- **Slice wire** (`g4v2.rs`): `SliceArgs`, `SliceAnswer`, `SliceStatus`, `SliceRefusal`, exactly ROOT-S1a's
  pinned format (build/status/ROOT-S1a.md, 455b246). `init-proto` carries it as `KernelRequest::SealedG4Slice(Value)`.

## Decisions (orchestrator-approved 2026-10-20; m7 wins over the earlier summary)
1. `tie` goes to the tie-breaks (cheaper, costlier, smaller); the tie guard keeps `tie_negative` → `no_gain`;
   `underpowered` → `inconclusive: underpowered` only when a tie-break would accept, else `no_gain`. The CT class
   comes only from the screen (with a δ from `noise.json`); without one it is `same`.
2. Slice `health` (the candidate failed the health canary) → `reject: health`; `error:harness_health` → `error`,
   retried once like `infra` (also `checker_tamper`).
3. `error:<cause>` → `error` (never a candidate verdict), except `error:budget` → `inconclusive: budget` and
   `error:wall` → `inconclusive: timeout` (m7 §5). Refusals: `root.busy`/`root.mode`/link loss/timeout →
   `inconclusive: sealed_unavailable`; `root.sealed_quota` → `inconclusive: sealed_quota`; `root.budget` →
   `inconclusive: budget_insufficient`; `root.store` → `error: infra` (retried); `root.args`/malformed answer →
   `error: sealed`; `root.sealed_none`/`root.unknown_cmd` → the unchanged v0.3 kernel path (record `sealed: null`
   plus `sealed_refusal`), remembered per home for 1 h so a home without S pays no screen before each fallback.
4. **D-L2 rule:** the lean Root runs the candidate's body binary only (no t0 blob, ext ignored). The S path is
   taken only when LKG..candidate touches body paths alone (t2, dep; no `skills/`, `SELF.md`, `GENESIS.md`,
   `ext/` path in the whole provisional chain). t1 and t0_escalated changes take the v0.3 kernel path
   (`promote/g4v2.rs::s_path_paths_ok`, with a code comment citing D-L2).
5. `promotion{lkg:true}` carries `milestone_required:true` after a `lineage_gain` accept (read from the job's
   `vault/gate/jobs/<job>/result.json`); ROOT-S1a counts these as the span's unconfirmed accepts.
6. Every accept is tagged with its class in the G4 record and Root lineage (`accept_class`), so a tie-break accept
   never counts as "sticks".

## Deviations
- **Pre-flight per-run estimate:** `gate.g4.cost_prior_usd` when set, else m7 §3.2.6's $0.005, not config.md's
  $0.05 default (at $0.05 no tier fits a $3 gate budget). The kernel does not know the Root's incumbent cache, so
  the estimate assumes fresh runs (m7's "fresh" column): it may pick a weaker tier, never a dearer one.
- **Escalation floor:** the first G4's tier per candidate sha is kept in process memory; after a kernel restart the
  escalated re-run's floor is `strong` (one up from `standard`).
- **cost_guard on the S path** uses the screen's cost × time effect when its δ is known; otherwise `better` rests on
  the Root's cost per pass (≤ 1.5) alone.
- **Slice budget:** `budget_musd` = `budget.gate_usd` − the kernel runs of this G4 so far (the proxy exposes no
  per-gate spent; G0–G3 spend of a sync G4 is not subtracted; the Root's ceilings still bind).
- `repro_fixed` never fires on the S path (no repro runs in the lean screen).

## Tests (M7 IDs, $0, no model)
Covered (kernel side): M7-G02, G03, G04, G05, G06, G12, G13 (kernel mapping), G14, G15 (kernel side: ratchet
branches; the "re-promote reuses the record" part is cut with `dup`, D-L2), plus the pinned wire format, the agent
view rounding (M7-R13 kernel side), the refusal mapping, the D-L2 rule, the driver over a scripted slice link, and
init-fakeroot's `sealed.g4_slice` script, and the **real pool v1** (POOL-LITE): `pool.py import` into a test home,
all 20 tasks load in the kernel as task.json v3, one screen-style run of `pool-c-squeeze-ws` on init-mockllm scores
1000 as a valid RunRecord v3, and the shared runner's tamper line (exit 3) reads as `tamper` (test
`gate_v2a_pool_v1_loads_as_v3_and_one_screen_run_scores` in kernel-supervise/tests/promote.rs).
Commands (all green, 2026-10-21): `cargo test -p kernel-gate --lib` (226), `-p kernel-supervise --lib` (95),
`-p kernel-supervise --test promote --test serve_seam` (G4 v0.3 flows unchanged through the new routing),
`-p init-proto`, `-p init-testkit --lib fakeroot`; `cargo clippy -D warnings --all-targets` on kernel-gate,
init-proto, init-testkit, kernel-supervise, init-kernel.
GATE-V2b: M7-G01 (screen), G07 (scope caps), G08 (pool cache).
Cut (D-L3): M7-G09 (fallback with v2 rules and `gate judge`), G10 and G-21b (`gate simulate --rule v2`), G11 (gog
v2 enumeration); M7-L01…L04 (D-L6).

## Needs from others
- GATE-V2b: the body of `run_screen` (canaries + screen), the g4run.rs wiring of RunRecord v3 fields.
- ROOT-S1a: nothing beyond the pinned format.

## Notes for others
- Rust pool or S checks run under a test home inside `kernel/target/` are taken for members of the kernel cargo
  workspace (`kernel/Cargo.toml`); pool.py's scratch must stay in the system temp dir in tests, and a kernel-side
  Rust pool check under such a home would fail the same way (not an issue under a real `~/.init`).
- Probe lane sockets must fit 103 bytes: World homes under `kernel/target/test-homes/w8b/` need 1-letter names.

## Left
- An end-to-end run of `sealed.g4_slice` against the real `init-root` (ROOT-S1a's side) — REAL-M7's $0 dry run on
  init-mockllm is the place; nothing in this lane is blocked on it.
- The escalated re-run's tier floor is process-local (see Deviations).
- Cut, not built (D-L3/D-L6): judge mode, `gate simulate --rule v2`, the gog v2 enumeration, the fallback with v2
  rules, the capability path.
