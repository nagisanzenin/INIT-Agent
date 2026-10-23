# GATE-V2b (lean proof sprint): proxy run caps, pool cache, screen (E3), failing-run log retention

Spec: build/specs/PROOF-LEAN.md; m7-design.md §3.2.2, §3.2.7, §3.2.8, §3.3; evolution-v2.md §3.5 (E3, est), §3.12 EV-TRACE (retention).

## Done
- **Proxy per-run caps on actuals** (kernel-proxy, 4a0795c): `ProxyHandle::set_run_caps(scope_id, RunCaps{max_turns, max_output_tokens, max_input_tokens})` on gate `run` lanes. Counters update synchronously when a response ends; once a counter has reached its cap the next request gets 429 `budget.<cap>`, `x-init-cap: turns|tokens_out|tokens_in`, `retryable:false`. No reservation is used, so a first request is never refused by the token caps. Turns = requests with `x-init-purpose: turn` or no purpose header. `ScopeSummary` gains `turns`, `caps`, `caps_hit` (`turns|tokens_out|tokens_in|cost`) and `model_served` (the response's `model`, captured by the tap).
- **kernel-gate** (new files): `caps` (task v3 limits, else 12 / 24,000 / 400,000), `pool_cache` (`vault/gate/cache/<tuple12>/<key64>.jsonl`; key = sha256 ICJ{task digest, arm, model incl. served string, harness}; validity: ≤ 72 h, ≥ 1 request, selectable, incumbent-arm runs only; equal-R seeded subset; sentinel drift rule; invalidate/prune by exact path), `screen` (G4 v2 screen selection + decision, the cached-pair runner with the ⌈20%⌉ sentinel, E3 decision, run-grid assertion, `est_permille`, `go` = est ≥ 80‰, `aim`), `retain` (failing pool runs' session logs).
- **kernel-supervise**: `promote/screen.rs` (`g4_screen`, `screen_e3(env, &E3Request) -> E3Result`), `GateProxy::set_run_caps` + `GateScope::set_caps`, `g4run.rs` (caps on every G4/screen run lane, RunRecord v3 fill: `v`, `tamper` via `check_score`, `caps_hit`, `model_served`, `cache`, `turns`; keeps a pool run's `state/sessions` and retains it when the run failed), the body of `promote/g4v2.rs::run_screen` (GATE-V2a's file). init-kernel: `set_run_caps` on `KernelGate`.

## Interfaces for other lanes
- **GATE-V2a**: `run_screen(env, gate_id, &ProbeRuns) -> (ScreenSummary, Vec<RunRecord>)` = canaries first, then 12 seeded pool tasks with the pool cache; stats from `v2::screen_stats`.
- **EVO-LITE**:
  - retained logs: `kernel_gate::retain::{retained_path(home, job, run_id), list_retained(home)}` → `vault/gate/jobs/<job>/sessions/<run_id>.jsonl` (set `pool`, score < 1000 only; never holdout/canary/capability/S).
  - pool cache (read-only): `kernel_gate::pool_cache::PoolCache::open(home).all_runs() -> Vec<CacheEntry{v, key, task, at_unix, selectable, gate_id, run: RunRecord}>`; `passed_tasks(arm, now, max_age)`.
  - E3: `kernel_supervise::promote::screen::screen_e3(env, &E3Request{job, candidate, incumbent, targeted, controls_n, s_c_permille}) -> E3Result{verdict: E3Verdict, est_permille, go, aim, reused, runs}`; pure `kernel_gate::screen::{e3_decide, est_permille, go, aim}`.

## Tests ($0; temp homes under kernel/target/test-homes/{wp5,WP8}/, removed by exact path)
- `bash ci/slot.sh cargo test --offline -p kernel-proxy` (kernel/): 74 unit + 12 integration ok.
- `bash ci/slot.sh cargo test --offline -p kernel-gate --lib -- screen pool_cache retain caps`: 17 ok.
- `cargo build -p kernel-supervise -p init-kernel`: clean; clippy `-D warnings` on kernel-proxy, kernel-gate, kernel-supervise, init-kernel.
- Full unit suites after the commits: `cargo test -p kernel-gate -p kernel-supervise --lib`: 226 + 95 ok.
- Commits: 4a0795c (proxy caps), deec675 (kernel-gate modules), c48ec11 (supervise/init-kernel wiring). The `run_screen` body ships with GATE-V2a's promote/g4v2.rs commit.

| ID | Test |
|---|---|
| M7-G01 | `screen::tests::m7_g01_screen_rejects` (canary, tamper, 3 errors, net −4 reject; net −3 continues), `drivers_end_to_end` |
| M7-G07 | `kernel-proxy scope::tests::m7_g07_cap_state_refusal`, `tests/proxy.rs::m7_g07_run_caps_on_actuals`, `caps::tests::caps_from_limits` |
| M7-G08 | `pool_cache::tests::m7_g08_pool_cache_rules` (served model, harness, age, selectable, winning arm, no request, equal R), `screen::tests::cached_pairs_across_gates` (reuse across gates, sentinel, drift → invalidate + alert + fresh) |
| M7-G14 (caps part) | `caps::tests::caps_from_limits` (v2 task → no v3 caps → pool defaults) |
| EV-07 | `retain::tests::ev07_failing_pool_logs_retained_holdout_canary_not` |
| EV-21 | `screen::tests::ev21_e3_rejects` |
| EV-23 | `screen::tests::ev23_grid` |
| EV-25 (lean) | `screen::tests::ev25_lean_go_no_go` |

Not mine: M7-G02…G06, G09…G13, G15 (GATE-V2a / cut by D-L3); EV-06 (the trace reader for `trace:` refs) is EVO-LITE's kernel-evo.

## Deviations
- D1: a request with no `x-init-purpose` header counts as a turn for the cap (the doc counts only `turn`; absence must not bypass the cap). The seed body always sends the header.
- D2: the refusal code is `budget.<cap>` (`budget.turns`, `budget.tokens_out`, `budget.tokens_in`); m7-design only names the `x-init-cap` values.
- D3: `caps_hit = "cost"` for any money refusal (`budget.*`) on a lane with run caps.
- D4: the arm key's `t0_sha256` is the lineage commit sha (a finer key than the T0 digest: correct, only less reuse); `ext_set` is empty (gate runs use no ext set).
- D5: the cache stores incumbent-arm runs only, so the "winning arm" rule holds by construction (a candidate run is refused by `put`).
- D6: `screen_k` 12, reject at 3 errors / net ≤ −4, `cache_max_age_s` 72 h and `sentinel_permille` 200 are constants in `kernel_gate::{screen,pool_cache}` for now, not `[gate.g4]` config keys (config.rs is GATE-V2a's this sprint).
- D7: `incumbent_drift` goes to the kernel log, not `alerts.jsonl` (the gate env has no alert sink yet).
- D8: no agent stratum in the screen sample (L0 only; lean).

## Left
- A process-level $0 test of `screen_e3`/`g4_screen` on real probes (the logic is covered with a fake executor; the wiring compiles).
- Wire the `[gate.g4]` screen/cache keys into GateCfg; prune the cache to the `lkg`/`audited` tuples at promotion (`PoolCache::prune` exists, not yet called).
