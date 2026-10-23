# WP5: `kernel-proxy`, the kernel model proxy (lane X)

Status: **done** for the Slice-1 path and proxy.md §8, except the items under "Left" (2026-10-20). Spec: kernel/proxy.md v0.3, root/egress.md §3.6 and §4.1–§4.2, kernel/config.md §4.1–§4.5, and decisions D5 and D6.

## Done
- **Crate `kernel/crates/kernel-proxy/`**. It runs on its own tokio runtime with 2 worker threads and is controlled in-process through `ProxyHandle`.
  - It holds no keys, no TLS and no hard ceilings.
  - Its only upstream is the Root egress socket: plain HTTP/1.1 over Unix with a keep-alive pool (`egress.rs`).
  - Removed dependencies: `rustls`, `hyper-rustls`, `rustls-platform-verifier` and `tokio-tungstenite`. `ci/vendor.sh` was run; `vendor/` now holds 265 crates.
- **Server** (`server.rs`): HTTP/1.1 on `run/proxy.sock` and on the lane sockets (`open_scope`).
  - Sockets are mode 0600. A peer-uid check rejects other users. A stale socket at the exact path is replaced.
  - Settings: `header_read_timeout`, 64 headers maximum, 32 MiB maximum request body (413 `request.too_large`).
- **Routing** (`routes.rs`):
  - `POST /<provider>/<path>` against the preset route tables (deepseek, anthropic, openai) plus config routes.
  - Errors: 404 `route.unknown` and `route.unknown_provider`; 503 `provider.disabled` (disabled, or not forwarded by the Root); 403 `route.not_allowed`; 400 `route.query_not_allowed` and `route.bad_path`.
- **Headers** (`request.rs`):
  - The `x-init-session`, `-purpose`, `-attempt`, `-task` and `-subtask` headers are read with their grammar checks, then stripped.
  - Only the request allowlist is forwarded. No auth header is ever added.
  - The proxy sets `accept-encoding: identity`, `user-agent: init-kernel/<v>` and `anthropic-version` (Anthropic family only).
  - It also sets `x-egress-scope` (live, gate or member), `x-egress-body`, `x-egress-reservation: <req_id>` and **`x-egress-guard: epoch=<E>`** (D6). E is the epoch of the Root figure the guard used.
  - Responses pass through the §4.1 response-header allowlist. The proxy adds `x-init-req-id`, `x-init-retries`, `x-init-head-hash` and `x-init-routed`.
- **L0/L1 routing** (§3.5a):
  - L0 allows the label's default model only (400 `request.model_not_routed`).
  - From L1 the routable set is: priced in kernel config ∩ forwarded by the Root (`models[].routable` from `GET /_root/ceilings`) ∩ `model.routable`. Anything else is 400 `request.model_not_allowed`.
  - Gates snapshot the level and the routable set at `open_gate`.
  - Attribution goes into every record: `model`, `default_model`, `routed`, `subtask` and `level`. Stats carry `spend.by_model`, `spend.task.by_model` and `routing.routed_today`.
- **Pre-flight checks:** `request.bad_json`, `request.tier_not_allowed` and `request.usage_required`.
- **Soft budgets** (`budget.rs`, integer pico-USD):
  - Reservations are taken under one mutex. The limits are request, scope, gate, task (`_none` included), member, evolution, day, soft total, and the guard on the Root's own day and total spend at `(1000 − root_guard_permille)/1000`.
  - **Clamp (D6):** effective day = `min(day_usd, ceiling − request_max, soft_day_fraction × ceiling)`. It is recomputed on every Root figure, and a change raises the alert `soft_day_clamped`.
  - The day boundary is the Root's.
  - `set_budgets` covers L1+ with the evolution.md §3.14 bounds (`budget.invalid`) and persists to `state/kernel/budget.json`. A demotion to L0 resets the budgets to config.
  - Refusals are typed 429s in the family's shape: `x-init-error`, `x-init-cap`, `retry-after` (day-type caps only), and an `init` object in µUSD. Root guard refusals also carry `ceiling_musd`.
- **Root refusals** (`x-init-egress-error`) are mapped and never retried:
  - `ceiling.*` → 429 `budget.root_*`
  - `quota.*` → 429 `budget.root_<q>`
  - `class.*` → 403 `egress.*`
  - `root.*` → 503
  - The Root's `init` is kept as `init.root`, the outcome is `root_refused`, and an alert is raised.
- **Retries** (§3.9):
  - Egress connect errors are retried and end in 503 `egress.unavailable` (`egress_down` alert after 30 s).
  - 429 (rate) and 500/502/503/504/529 are retried with backoff, honouring `retry-after-ms` and `retry-after`; waits over 20 s are passed through.
  - `x-should-retry: false` is honoured. Spend-limit 429s are not retried.
  - 401/403 become 401 `upstream.auth` with the body withheld; 402 is passed through with the alert `provider_balance`.
  - After the last attempt the response carries `x-init-retries`.
- **Metering** (`tap.rs`, written by a helper):
  - Incremental per-family parsing with the cheap pre-filter. DeepSeek is detected first by `prompt_cache_hit_tokens` and `prompt_cache_miss_tokens`; OpenAI chat, Anthropic (overwrite, never sum) and Responses are also handled.
  - It records tool calls (`ext_name`, 4 KiB cut), the normalized finish, and TTFT/TTFC.
  - `MeteredBody` forwards frames unchanged, applies idle and total timeouts, and settles on end, error, timeout or abort.
- **Cost** (`pricing.rs`): price windows (DeepSeek peak/off-peak, `default_dates`), long-context tiers, multipliers (`unpriced_modifier`), `cost_ref` at the reference window, and the no-usage estimate capped at the reservation.
- **Hashes** (`hash.rs`):
  - `prefix_hash` uses the same extraction as the body (`seed/src/context/hash.rs`); its tests are mirrored.
  - `head_bytes_hash` is `init-headbytes-v1`.
  - `tools_sha256` is `init-tools-v1`. It matches kernel-selfmap's manifest vector.
- **Traces** (`trace.rs`, written by a helper):
  - Content-addressed packs and chain encoding (`base`/`keep`/`add`, a checkpoint every 64).
  - Request records in `vault/traces/v1/req/`, rebuild of today's totals at start, `reconstruct`.
  - **Agent view:** `GET /_init/traces[/<id>/request|response]` answers only on `run/proxy.sock` and only for `vis: agent`. Money appears as `*_musd`.
- **`GET /_init/stats`** (integers only, asserted by the tests), plus `/_init/health` and `/_init/models`.
- **Observations for the gate:** `first`, `prefix_hashes`, `tools_sha256s`, `tool_round_trip` (only provider-emitted ids count), `tool_calls`, `by_model`, `last_finish`. `ScopeSummary` is returned by `scope_summary`/`close_scope`.
- **No sealed traffic (D5):** the proxy has no sealed path. `judge` mode (`StartOptions.judge`) writes no trace store and no `budget.json`.

## Tests
`cd kernel && CARGO_TARGET_DIR=target/x cargo test --offline -p kernel-proxy` passes **83 tests (72 unit + 11 integration)**. One unexplained integration failure occurred early on, before the settlement-ordering fix (the likely cause was a race between `requests_today` and the scope observations, now settled last). Since then, 20+ consecutive runs have been green.
- **Unit tests** cover:
  - P-01 (routing), P-04, P-05 and P-06 (all 15 WP2 golden fixtures plus split-invariance at every byte), P-07 (windows and cost), P-08a and P-08b (reservations and guard), P-10 (trace store: packs are 1.88% of naive size, chain depth 64), P-13 (hash invariants), P-14, P-22 (bounds) and the clamp.
- **Integration tests** (`tests/proxy.rs`): the body talks to the proxy socket, which forwards to an **in-test fake Root egress** (`tests/common/mod.rs`: `/_root/ceilings`, epochs, a peak-priced counter, typed refusals), which forwards to **`init-mockllm`** in upstream mode.
  - `slice1_forward_meter_trace`: P-02, with strict-deepseek mode finding no stripped header leaking. Cost equals the hand computation, the head hash matches, P-14, the stats and traces endpoints work, and P-16 finds no key material in `vault/`.
  - The other tests: `p03_p23_keepalive_and_egress_down`, `rt_c1_soft_cap_typed_429` (RT-C1), `p08b_root_guard_and_epoch_header`, `p09_retry_matrix`, `routing_levels_gates_and_redaction` (P-15, P-18 to P-21), `p22_set_budgets_and_clamp`, `p24_member_lanes`, `p25_judge_mode_writes_nothing`, `preflight_and_error_shapes`, `non_streaming_and_restart_rebuild`.
- **Lint:** `cargo clippy --offline -p kernel-proxy --lib -- -D warnings` is clean. `--all-targets` is currently blocked by lints in WP3's uncommitted `init-testkit` files (a dev-dependency), not by kernel-proxy. All kernel-proxy files are rustfmt-clean (`--edition 2024`).

## Deviations
1. **`x-egress-guard` uses egress.md's format, `epoch=<E>`.** proxy.md §3.8's `day=…;total=…;seen=…;r=…` is dropped: the Root (WP22) parses the epoch, and a malformed header demotes.
2. **The guard's Root view** is `day_spent + day_reserved` from the last figure, plus root-priced estimates of requests the kernel settled after that figure was requested. This over-counts in-flight requests (the safe direction). The **soft total** is checked against the Root's lifetime figure, because the kernel keeps no lifetime ledger.
3. **No `models` table in `/_root/ceilings`** means "the Root forwards every priced model", so the Root's own allowlist still decides.
4. **`route.bad_path` is checked before the route lookup**, so `..` gets a 400 instead of a 403, as P-01 expects.
5. **Neutral tiers.** `service_tier`/`speed` values `default`, `auto` and `standard` (and `global`/`not_available` reported in responses) are priced at ×1 without a configured multiplier, instead of being refused.
6. **Member lanes.** `open_scope` also accepts sockets under `$INIT_HOME/run/fleet/` (supervise.md §3.14). Member scope ids look like `member:<bid>`. A member's share is `OpenScope.cap_musd`.
7. **Unmetered routes** (`GET models`, `user/balance`, `count_tokens`) are forwarded and passed through with no budget and no trace record.
8. **Estimate input.** The no-usage estimate uses the response bytes forwarded as `content_bytes_seen`, capped at the reservation.
9. **Cancelled requests.** A request whose handler is cancelled before it completes (the body disconnected while waiting for headers) is settled at its full reservation and writes no `end` record.
10. **Extra end-record field `resp_type`** (the response content type) serves `/_init/traces/<id>/response`.
11. **Gate snapshot after a demotion (P-21).** A gate keeps routing L1 models, but the real Root refuses them at L0 (`class.level`, which demotes). In practice a demotion cancels the running G4 (RT-L8).
12. The tap and trace helpers' own deviations (pre-filter until first content, line-bound semantics, chain reset rules, summary merge rules) are recorded as doc comments in `tap.rs` and `trace.rs`.

## Needs from others
- **WP4 (supervise):**
  - Call `Proxy::start(&ProxyConfig, StartOptions{home, egress_sock: hello.paths.egress_sock, label, judge:false}, alert_sink)`.
  - Relay `ceilings` events (`set_ceilings`) and level changes (`set_level`), and give the body id (`set_body_id("live", boot_id)`).
  - Open and close lanes for gate and fleet work.
  - `ProxyConfig` is the `[model]`/`[proxy]`/`[budget]`/`[traces]`/`[providers.*]` subset plus `soft_day_fraction`, so the config loader should embed these structs.
- **WP3:** the integration tests use an in-test fake egress because `init-fakeroot` was uncommitted when they were written. Switching them to `init-fakeroot` once it is committed is a small change in `tests/common/mod.rs`.
- **WP22 / docs owner:** update proxy.md §3.8's `x-egress-guard` text to `epoch=<E>` (deviation 1).
- **WP15/WP18:** run one Slice-1 test through the real `init-root` egress. The header and ceilings formats match WP22.md.

## Left
- The balance poll (`providers.*.balance` is `null`).
- WebSocket mode: a config error by design until the egress forwards upgrades.
- `traces.retention_days`.
- `cost_mismatch`/`root_cost_musd`: the Root publishes no per-request cost (egress.md §4.1).
- P-11 (WebSocket), P-12 (overhead benchmark), P-17 (live).
- The `init-kernel traces show|stats` CLI (owned by the `init-kernel` binary).
