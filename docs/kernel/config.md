# Owner configuration: `kernel/config.toml`

```
Status: v0.3 + build sync 2026-10-20 · Area: kernel
Depends on: CONVENTIONS.md, INIT Agent spec.md (v0.3 §13–§19), adr/007-root-and-autonomy-ladder.md,
            root/root.md (R5 charter format), root/egress.md (keys, hard ceilings), kernel/proxy.md,
            kernel/gate.md, kernel/selfmap.md, kernel/supervise.md (§4.9), kernel/sandbox.md (§4.3),
            evolution.md (§4.6), body/provider.md, testing.md
Defines:    the file format, location, parsing, validation and reload rules of config.toml; the full
            key schema with types and defaults (the assembled view); the split between the Root charter
            and kernel config (what moved, what stayed); the removed-key errors; the config digest; the
            Phase 0 DeepSeek-only example. Semantics of each key stay with the doc named in its section.
            The charter's format is root/root.md's.
```

---

## 1. Purpose

`$INIT_HOME/kernel/config.toml` is the **kernel's own configuration**: the default model and how routing is bounded, proxy mechanics and measurement prices, budget sub-limits, traces, the gate's statistics, supervision and sandbox settings. This doc gives the complete schema in one place, so `init-kernel` can parse it with `deny_unknown_fields`, and anyone can see every knob and its default.

**What it no longer is (v0.3).** In v0.2 this file was also where the owner set money caps and named key files. Spec v0.3 moves everything the agent must never be able to change into the Root (§13–§14, §19). Keys, the hard money ceilings, the day boundary, quotas, external-action classes, the maximum autonomy level and the purpose now live in the **charter** (R5, format owned by root/root.md; ceiling keys `ceilings.*`, root/egress.md §3.3) and the Root's key store (root/egress.md). What stays here is the kernel's **soft** layer under those ceilings: maintainer defaults that the agent may change from L1 (evolution.md §3.14). The kernel reads the charter values it needs from the Root at run time, never from this file (§2.1).

## 2. Responsibilities / non-goals

**Responsibilities:** location, format, parsing, types, defaults, validation, reload behaviour, the config digest, and the example.

**Non-goals:** the meaning of each key. That stays with its owner doc:

| Section | Semantics owned by |
|---|---|
| `[model]` (default model, routing bound), `[proxy]`, `[budget]` (money sub-limits and the Root guard), `[traces]`, `[providers.*]` (routes and measurement prices) | kernel/proxy.md |
| `[gate]`, `[gate.*]` (including `[gate.admission]`) | kernel/gate.md |
| `[budget]` (count budgets), `[evolution]` | evolution.md |
| `[supervise]` | kernel/supervise.md §4.9 |
| `[sandbox]` | kernel/sandbox.md §4.3 |

Also not here: the charter's format and every charter value (root/root.md), keys and their storage (root/egress.md).

### 2.1 Charter versus kernel config (v0.3)

**The rule.** A value goes in the **charter** if the agent must never be able to change it at any autonomy level: a security boundary, the owner's money, the purpose, the ladder's cap. A value goes in **kernel config** if it only shapes how the kernel works. Kernel config is maintainer-shipped, and from L3 the agent may change it through the gate-of-gates (§3.1).

| Item | v0.2 location | v0.3 location | Owner doc | Read by the kernel? |
|---|---|---|---|---|
| Provider API keys | `vault/keys/<id>` via `init-kernel keys set`; `providers.<id>.key_file` | Root key store, `init-root keys set` | root/egress.md §3.2 | never |
| Hard money ceiling per day | `[budget] day_usd` (a kernel cap) | **charter** `ceilings.day_musd` (Root-enforced) | root/egress.md §3.3, root/root.md | yes, read-only, via `GET /_root/ceilings` (kernel/proxy.md §3.8) |
| Hard money ceiling in total (the $25 prepaid top-up) | not in config | **charter** `ceilings.total_musd` | same | same |
| Day boundary | `[budget] day_starts_at_utc_offset` | **charter** `ceilings.day_starts_at_utc_offset` | same | yes, as `day_resets_at`; every soft daily window uses it |
| Compute and disk quotas, parallel bodies | — | **charter** `quotas.*` | root/egress.md §3.9 | through the Root |
| Which hosts, providers and models may be reached; external-action classes | implicit in `[providers.*]` routes and prices | **charter** action classes and ceiling price table, enforced by the Root (R3) | root/egress.md §3.4, §3.6 | yes, the forwarded models (proposed) |
| Account headers (`organization`, `project`) | `[providers.openai]` | with the key, in the Root | root/egress.md | never |
| Maximum autonomy level, panic channels, purpose, constitution text | kernel/CONSTITUTION.md, supervise | **charter** | root/root.md (awareness.md for the text) | level: yes (R7) |
| Admission rule parameters (`admit_repeats`, `admit_k_generations`) | — | Root `[sealed]` config | root/sealed-eval.md §4.3 | yes, read-only (kernel/gate.md §3.7.5) |
| Crate approval (`[vendor] require_approval`) | config | **removed**: ESC-1 settled, no approval, an L0 right (CONVENTIONS §6) | kernel/initctl.md | — |
| Soft budgets: day (clamped to the Root's day ceiling − `request_max_usd`, D6), task, gate, quick gate, probe, request, evolution share | `[budget]` (E1 values, PENDING OWNER) | **kernel config**, maintainer defaults; agent-set from L1 with `initctl budget set` | evolution.md §3.14 (policy), kernel/proxy.md §3.8 (enforcement) | — |
| Count budgets (promotes, quick gates, attempts per task, task submissions) | `[budget]` | kernel config | evolution.md §3.14 | — |
| Default model, effort, routable bound | `[model]` | kernel config | kernel/proxy.md | — |
| Proxy mechanics, traces, measurement prices and schedules, routes | `[proxy]`, `[traces]`, `[providers.*]` | kernel config | kernel/proxy.md | — |
| Gate statistics, capability test | `[gate]` | kernel config | kernel/gate.md | — |
| Evolution policy (retirement, thinning, escalation, provisional age, agent-task stratum, fleet) | `[evolution]` | kernel config | evolution.md §4.6 | — |
| Supervision, sandbox | `[supervise]`, `[sandbox]` | kernel config | kernel/supervise.md, kernel/sandbox.md | — |

**E1/ET-E1 (CONVENTIONS §6; v03-decisions D13).** The owner's money decisions are now the charter's ceilings. The charter template uses $5.00 a day and $25.00 in total, PENDING OWNER (root/root.md, root/egress.md §3.3). The other v0.2 E1 numbers ($3.00 per gate, $0.50 per task, evolution share 0.6, a $5.00 soft day) were never ceilings: they are maintainer defaults of the soft layer here, no longer owner decisions (evolution.md §3.14). With the template's $5.00 day ceiling and `request_max_usd` $1.00, the D6 clamp makes the **effective soft day $4.00**. The day's evolution share is then $2.40: below the $3.00 gate budget, but enough for a default G4 off-peak ($1.3–2.0, kernel/gate.md §3.8.5), because the pre-flight estimate decides whether a G4 starts (kernel/gate.md §3.11). A charter day ceiling of $10.00 removes the squeeze.

## 3. Design

### 3.1 Location, ownership, format

- **Path:** `$INIT_HOME/kernel/config.toml`.
- **Who changes it, by level** (spec v0.3 §15):
  - **L0–L2.** The maintainer ships it with the kernel. The owner may edit it, but nothing requires that (spec v0.3 §18). The agent can read it (`kernel/` is read-only to the agent) and cannot write it.
  - **L1 and up.** The agent may change the `[budget]` soft values with `initctl budget set`, within evolution.md §3.14's bounds (kernel/proxy.md §3.8). The effective values live in `state/kernel/budget.json`; this file keeps the defaults.
  - **L3 and up.** The kernel is agent-evolvable, and so is its config. The agent changes it the way it changes kernel code: as `config.default.toml` in the kernel tree (`lineage/kernel.git`, evolution.md §3.15.6), through K0–K3 and the Root's gate-of-gates (kernel/gate.md §3.1). This matters because a gate parameter such as `gate.g4.margin_max` can weaken the gate as surely as code can. `$INIT_HOME/kernel/config.toml` then holds only optional owner overrides, which win key by key (§3.2). Where the kernel tree lives at L3 is a cross-doc question (§9).
- Reading is fine: it holds prices, sub-limits and gate parameters, which are awareness, and no secrets. In v0.3 it holds no ceilings either.
- **Format:** TOML 1.1, parsed with the `toml` crate 1.1.8 into `serde` structs with `#[serde(deny_unknown_fields)]`. A misspelled key is an error, not a silent default.
- **Encoding:** UTF-8, LF or CRLF.
- **Money:** decimal **strings** in USD (`"0.50"`, `"0.003"`). TOML floats are rejected for money, so `0.1` cannot turn into 0.1000000000000000055 USD. Strings have at most 6 decimals (1 micro-USD) for budget sub-limits and at most 6 decimals for per-million-token prices (1 pico-USD per token), and are parsed exactly into integers (kernel/proxy.md §3.8). The charter's money values follow the charter's own format (root/root.md).
- **Ratios and statistics** (`alpha`, `margin_max`, `evolution_share`, …): TOML floats in [0, 1] unless stated.
- **Durations:** integers with the unit in the key name (`_s`, `_ms`).
- **Disk thresholds:** integers with the unit in the key name (`_gib`, `_gb`). When `proxy.test_mode = true`, every disk-threshold key may instead be given as its sibling ending in `_mib` (integer MiB; for example `supervise.disk.min_free_run_mib`, `gate.g4.min_free_mib`), which then takes precedence. Outside test mode a `_mib` key is a validation error. This is a file-format rule; the meaning of each threshold stays with its section owner (evolution-testing RS-2, testing.md F-disk-full).
- **Paths:** relative paths are relative to `$INIT_HOME`.
- **Install template.** When the release ships no `config.toml`, `init-root install` writes this minimal one (root/root.md §4.3 step 5). `[sandbox]` comes from `ROOT_HOME/install.json` and must stay equal to it, because the Root's broker compares the two exactly and the install-mode kernel needs them for its build. Every other section takes its defaults from this doc. The owner edits the file directly.

  ```toml
  # INIT kernel config (kernel/config.md), written by `init-root install`. Edit freely; [sandbox] must
  # stay equal to ROOT_HOME/install.json, because the Root's broker compares them.
  schema_version = 2

  [sandbox]
  toolchain = "~/.rustup/toolchains/stable-aarch64-apple-darwin"   # the real install writes the resolved absolute path
  developer_dir = "/Library/Developer/CommandLineTools"
  ```

### 3.2 Load, validate, reload

```rust
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,                                   // must be 2
    #[serde(default)] pub model: ModelCfg,
    #[serde(default)] pub proxy: ProxyCfg,
    #[serde(default)] pub budget: BudgetCfg,
    #[serde(default)] pub traces: TracesCfg,
    #[serde(default)] pub providers: std::collections::BTreeMap<String, ProviderCfg>,
    #[serde(default)] pub gate: GateCfg,
    #[serde(default)] pub evolution: EvolutionCfg,
    #[serde(default)] pub supervise: SuperviseCfg,
    #[serde(default)] pub sandbox: SandboxCfg,
}
pub fn load(path: &std::path::Path) -> Result<(Config, ConfigDigest), Vec<ConfigError>>;  // all errors at once
// From L3: load(defaults = <kernel tree>/config.default.toml, overrides = $INIT_HOME/kernel/config.toml),
// merged key by key (overrides win), then validated as one Config.
```

**Validation** (beyond types). All errors are reported together, each with its TOML key path:
1. `schema_version == 2` (v0.3). A file with `schema_version = 1` is rejected with one error that names every v0.2 key it contains which moved to the charter (rule 10), so the migration is one edit.
2. `model.provider` is a key of `providers`, and `model.id` is a key of `providers.<provider>.models`. Every entry of `model.routable` names a priced model of a configured provider.
3. Every provider id matches `^[a-z][a-z0-9_-]{0,31}$` and is not `_init`.
4. Every metered model has `prices` for every window its schedule can select, and `prices.*.input` and `prices.*.output` are present.
5. Money strings match `^[0-9]{1,7}(\.[0-9]{1,6})?$`.
6. `budget.gate_usd ≤ budget.evolution_share × budget.day_usd`, otherwise no promote gate could ever start; `budget.evolution_share ≤ 0.8` (evolution.md §3.14); `budget.probe_usd ≤ budget.quick_gate_usd` (all warnings). **At run time**, on every `ceilings` event, the proxy clamps (D6; root/root.md X4): effective soft day = `min(day_usd, ceilings.day_musd − request_max_usd, evolution.soft_day_fraction × ceilings.day_musd)`, effective soft total = `ceilings.total_musd − request_max_usd`, with the alert `soft_day_clamped` (kernel/proxy.md §3.8). Nothing refuses to start, and rule 6's warning is re-evaluated on the effective values.
7. `gate.g4.min_pool ≤ gate.g4.pool_k`, `gate.g4.min_holdout ≤ gate.g4.holdout_h`, `gate.g4.repeats ≥ 2`, `gate.g4.repeats ≤ gate.g4.repeats_max ≤ 5`, `gate.g4.capability_min_tasks ≤ gate.g4.capability_max_tasks`, `evolution.tasks.agent_stratum_max ≤ ⌊gate.g4.pool_k / 2⌋`.
8. `proxy.test_mode = true` marks a test home. It allows the `_mib` disk-threshold siblings (§3.1) and a provider named `mock`, nothing else. No config can name a provider upstream: `providers.<id>.base_url` and `[proxy.test_root]` are removed. Kernel-only tests run under `init-fakeroot`, full-stack tests under the real Root with `root.test_mode` (testing.md §3.5b; kernel/proxy.md §4.1).
9. `budget.root_guard_permille` is in 0..=200.
10. **Removed keys** are errors with a pointer, not unknown-key errors:
    - `[budget] day_starts_at_utc_offset` → "the day boundary is the charter's `ceilings.day_starts_at_utc_offset` (root/egress.md §3.3)";
    - `providers.<id>.key_file` → "keys live in the Root: `init-root keys set` (root/egress.md)";
    - `providers.<id>.organization` and `project` → "account headers live with the key in the Root (root/egress.md)";
    - `[vendor]` → "removed in v0.3: ESC-1 settled, no crate approval (CONVENTIONS §6)";
    - `providers.<id>.base_url` and `[proxy.test_root]` → "the kernel never names an upstream; tests run under init-fakeroot (testing.md §3.5b)";
    - `[supervise] notify_cmd` → "alerts go through the Root (root/root.md X4)";
    - `[evolution]` timing keys → the `[supervise]` key to use (as in v0.2).
11. `providers.openai.websocket = true` is an error until root/egress.md confirms that the egress forwards WebSocket upgrades (kernel/proxy.md §3.15).

**When it is read.** At kernel start (a failure halts with `E_CONFIG`, kernel/supervise.md), on the command `init-kernel config reload` (owner channel), and at L3 on a kernel handoff that changes `config.default.toml`.

**On reload:**
- invalid → the old config stays and the errors go to `state/kernel/alerts.jsonl`;
- valid →
  - proxy settings apply to new requests (kernel/proxy.md §3.1 reload);
  - budget sub-limit changes apply at once, and a lowered sub-limit is enforced on the next admission;
  - gate settings apply to the next gate job;
  - `[supervise]` and `[sandbox]` changes need a kernel restart (`init-kernel restart`), and the reload reports which keys are pending;
  - a change of `model.id` starts the thinning baseline (evolution.md §3.12 step 0).
- **Charter changes never come through this reload.** The Root announces them, and the kernel re-reads `GET /_root/ceilings` (kernel/proxy.md §3.1). A lowered ceiling applies at the next admission.

**Config digest.** `config_sha256 = hex(SHA-256(ICJ(serde_json::to_value(&config))))`, computed from the merged, parsed struct with defaults filled in, so comments and formatting do not change it. Every gate record carries it, next to the Root-reported `charter_sha256` (kernel/gate.md §3.13). Two gates with the same pair of digests ran under the same rules.

## 4. Interfaces: schema with defaults

### 4.1 Top level and `[model]`

| Key | Type | Default | Notes |
|---|---|---|---|
| `schema_version` | int | — (required) | `2` (v0.3; §3.2 rule 1) |
| `model.provider` | string | `"deepseek"` | a `providers` key |
| `model.id` | string | `"deepseek-flash"` | the **default model**: the only model at L0, and the default route from L1 (kernel/proxy.md §3.5a). Must have prices. supervise.md §4.3 copies `model.provider`, `model.id` and `model.effort` into `boot.json.model = {provider, model, effort}`, where `model` holds `model.id` (the name body/provider.md and body/boot.md read) |
| `model.effort` | string | `"high"` | passed through `boot.json`; DeepSeek accepts `low`/`high`/`max` (`reasoning_effort`); body/provider.md Q-P6 |
| `model.routable` | array of `"<provider>/<model>"` | `[]` (= every priced model the Root forwards) | an optional narrower bound on L1+ routing (kernel/proxy.md §3.5a). It cannot widen what the Root allows |

### 4.2 `[proxy]` (kernel/proxy.md)

| Key | Type | Default |
|---|---|---|
| `worker_threads` | int | 2 |
| `max_request_bytes` | int | 33,554,432 (32 MiB) |
| `max_sse_line_bytes` | int | 1,048,576 |
| `header_read_timeout_s` | int | 30 |
| `upstream_connect_timeout_s` | int | 10 |
| `upstream_header_timeout_s` | int | 660 |
| `upstream_idle_timeout_s` | int | 330 |
| `upstream_total_timeout_s` | int | 3,600 |
| `retry_max` | int | 2 |
| `retry_max_wait_s` | int | 20 |
| `retry_budget_s` | int | 30 |
| `shutdown_grace_s` | int | 30 |
| `stats_ring` | int | 256 |
| `balance_poll_s` | int | 600 (0 = off) |
| `root_poll_s` | int | 10 (Root state refresh, kernel/proxy.md §3.1) |
| `egress_pool_max_idle` | int | 16 (kernel/proxy.md §3.7) |
| `test_mode` | bool | false (marks a test home: allows `_mib` disk thresholds and the `mock` provider id; it never changes the network path) |

`upstream_connect_timeout_s` now bounds the connect to the Root egress socket. `pool_idle_timeout_s`, `pool_max_idle_per_host`, `h2_keepalive_interval_s` and `h2_keepalive_timeout_s` are **removed**: provider connections are the Root's (kernel/proxy.md §3.7). A file containing them gets an error pointing to root/egress.md.

### 4.3 `[budget]` (soft money budgets: kernel/proxy.md §3.8; counts and policy: evolution.md §3.14)

In v0.3 every money key here is a **soft budget** under the Root's charter ceilings (`ceilings.day_musd`, `ceilings.total_musd`; root/egress.md §3.3). The values are maintainer defaults, fixed at L0; from L1 the agent may change them with `initctl budget set` (evolution.md §3.14), and the effective values live in `state/kernel/budget.json`. None is an owner decision any more.

| Key | Type | Default | Enforced by |
|---|---|---|---|
| `day_usd` | money | `"5.00"` (effective value clamped to `min(·, ceiling − request_max_usd, soft_day_fraction × ceiling)`, D6) | proxy (`budget.day`) |
| `task_usd` | money | `"0.50"` | proxy (`budget.task`, per `x-init-task`) |
| `gate_usd` | money | `"3.00"` | proxy (`budget.gate`), one promote gate |
| `quick_gate_usd` | money | `"0.50"` | proxy, one quick gate |
| `probe_usd` | money | `"0.25"` | proxy, one probe lane |
| `request_max_usd` | money | `"1.00"` | proxy (`budget.request`) |
| `evolution_share` | float | 0.6 (≤ 0.8) | proxy + serve (`budget.evolution`) |
| `reserve_bytes_per_token` | int | 3 | proxy reservation estimate |
| `root_guard_permille` | int | 20 | proxy: refuses at the Root's ceiling × (1 − 2%), against the Root's own spend (`budget.root_day`, `budget.root_total`; kernel/proxy.md §3.8) |
| `promote_attempts_task` | int | 3 | serve |
| `promotes_day` | int | 12 | serve |
| `quick_gates_day` | int | 20 | serve |
| `task_submits_day` | int | 10 | serve (`initctl task submit`, L1+; evolution.md §3.14) |

`gate_usd` defaults to $3.00 because the paired G4 at default sizing needs about $1.3–2.0 off-peak on `deepseek-flash`. `evolution_share` defaults to 0.6 so that one gate fits in a $5.00 soft day (kernel/gate.md §3.8.5, §3.11). Removed: `day_starts_at_utc_offset`, because every daily window, soft or hard, is the charter's day (root/egress.md §3.3). The display offset that awareness.md took from it (AW-1) now comes from the same place (§9).

### 4.4 `[traces]` (kernel/proxy.md §3.13)

| Key | Type | Default |
|---|---|---|
| `store_bodies` | bool | true (false keeps only the ledger records, no request or response blobs) |
| `retention_days` | int | 0 (keep forever) |
| `pack_max_bytes` | int | 268,435,456 |
| `checkpoint_every` | int | 64 |
| `writer_queue` | int | 4,096 |
| `fsync_interval_ms` | int | 1,000 |
| `max_response_bytes` | int | 16,777,216 |

### 4.5 `[providers.<id>]` (kernel/proxy.md §3.5, §3.10, §4.8)

| Key | Type | Default | Notes |
|---|---|---|---|
| `preset` | string | `<id>` if it is `deepseek`, `anthropic` or `openai`; otherwise required | route table, auth style, base URL, warm-up route |
| `enabled` | bool | true | |
| `websocket` | bool | false | `openai` preset only (proxy.md §3.15); `true` is an error until the egress forwards upgrades (§3.2 rule 11) |
| `routes` | array of tables | `[]` | extra routes: `{method, path, family = "openai-chat"\|"anthropic"\|"openai-responses"\|"none", metered = bool, auth = "bearer"\|"x-api-key"}` |
| `multipliers` | table string → money-like string | `{}` | e.g. `"service_tier:flex" = "0.5"`, `"speed:fast" = "2.0"`, `"inference_geo:us" = "1.1"` |
| `schedule` | table | none (single window `standard`) | below |
| `models` | table of tables | — (≥ 1 required for a metered provider) | below |

**`[providers.<id>.schedule]`** (time-of-day prices, e.g. DeepSeek):

| Key | Type | Notes |
|---|---|---|
| `tz` | string | fixed UTC offset for `windows`, e.g. `"+00:00"` |
| `default` | string | window name used outside every listed window |
| `windows` | array of `{days = "mon-fri"\|"sat-sun"\|"mon"…, from = "HH:MM", to = "HH:MM", use = "<window>"}` | half-open `[from, to)`; first match wins |
| `default_dates_tz` | string | offset that defines a "date" for `default_dates`, e.g. `"+08:00"` |
| `default_dates` | array of `"YYYY-MM-DD"` | whole dates priced at `default` (DeepSeek: Chinese public holidays); unknown holidays are priced by the windows, which over-estimates and is safe |

**`[providers.<id>.models."<model id>"]`**:

| Key | Type | Default | Notes |
|---|---|---|---|
| `context_window` | int | — | informational; logged against `GET /models` at start |
| `max_output` | int | — | |
| `reserve_output_tokens` | int | `max_output` | output reservation when the request sets no limit |
| `reference_window` | string | `"peak"` if a schedule exists, else `"standard"` | prices used for `cost_ref` (gate comparisons) |
| `prices.<window>.input` | money | required | USD per 1M uncached input tokens |
| `prices.<window>.cache_read` | money | `"0"` | USD per 1M cache-hit tokens |
| `prices.<window>.cache_write` | money | `"0"` | Anthropic 5-minute writes; OpenAI ≥ 5.6 writes |
| `prices.<window>.cache_write_1h` | money | `"0"` | Anthropic 1-hour writes |
| `prices.<window>.output` | money | required | USD per 1M output tokens (reasoning included) |
| `tiers` | array of `{above_input_tokens = int, prices = {<window> = {…}}}` | `[]` | long-context pricing, chosen per request |

A model without a price entry cannot be used: the proxy refuses it with `request.model_not_allowed`. **The kernel's price table is the kernel-side model allowlist.** In v0.3 it is intersected with what the Root forwards (`RootState.models`, which reflects the charter), and that intersection is the routable set (kernel/proxy.md §3.5a). The Root's allowlist is the one the agent can never change. These prices are for **measurement** (cost, `cost_ref`, allocations). The Root meters money with its own table (root/egress.md), and the kernel warns at start if the two differ (kernel/proxy.md §9 Q-EG5).

### 4.6 `[gate]` (kernel/gate.md)

| Key | Type | Default | Phase 0 example |
|---|---|---|---|
| `cargo_jobs` | int | 0 (= CPU count) | 0 |
| `g0.timeout_s` | int | 900 | 900 |
| `g3.mode` | string | `"model"` (`"scripted"` uses the mock route) | `"model"` |
| `g3.agent_timeout_s` | int | 300 | 300 |
| `g3.inner_timeout_s` | int | 300 | 300 |
| `g4.pool_k` | int | 12 | 10 |
| `g4.holdout_h` | int | 8 | 6 |
| `g4.canaries` | int | 2 | 2 |
| `g4.repeats` | int | 2 | 2 |
| `g4.repeats_max` | int | 3 | 3 |
| `g4.min_pool` | int | 8 | 8 |
| `g4.min_holdout` | int | 4 | 4 |
| `g4.min_canaries` | int | 1 | 1 |
| `g4.max_repro_tasks` | int | 5 | 5 |
| `g4.alpha` | float | 0.05 | 0.05 |
| `g4.futility_alpha` | float | 0.01 | 0.01 |
| `g4.bootstrap_b` | int | 10,000 | 10,000 |
| `g4.noise_b` | int | 2,000 | 2,000 |
| `g4.noise_history_days` | int | 30 | 30 |
| `g4.margin_factor` | float | 2.0 | 2.0 |
| `g4.margin_max` | float | 0.15 | 0.20 |
| `g4.rho_max` | float | 1.5 | 1.5 |
| `g4.parallel` | int | 4 | 3 |
| `g4.wall_s` | int | 5,400 (90 min, settled R2) | 5,400 |
| `g4.task_wall_s` | int | 900 | 900 |
| `g4.check_timeout_s` | int | 300 | 300 |
| `g4.min_free_gb` | int | 10 (test mode: `g4.min_free_mib`) | 10 |
| `g4.ct_include_wall` | bool | true (tests: false; kernel/gate.md §3.9) | true |
| `g4.prefer_offpeak` | bool | true | true |
| `g4.offpeak_max_wait_s` | int | 14,400 | 14,400 |
| `g4.cost_prior_usd` | money | `"0.05"` | `"0.03"` |
| `g4.capability_min_tasks` | int | 4 | 4 |
| `g4.capability_max_tasks` | int | 8 | 8 |

There are deliberately no keys for the admission rules (`admit_repeats`, K = `admit_k_generations`). They come from the Root's `[sealed]` config (root/sealed-eval.md §4.3) through the Root (kernel/gate.md §3.7.5), so that no kernel config, at any level, can weaken them. The agent-task stratum, the escalated re-run and the provisional age are `[evolution]` keys (§4.7).

The toolchain is `sandbox.toolchain` (§4.9). The G2 ready timeout and probe window are `supervise.ready_timeout_s` and `supervise.probe_window_s`, because G2 uses supervise's `probe()`. testing.md's `gate.g2_ready_timeout_s` maps to `supervise.ready_timeout_s`.

### 4.7 `[evolution]` (evolution.md §4.6)

| Key | Default |
|---|---|
| `retire_n_unused` | 20 |
| `retire_n_cold` | 40 |
| `retire_min_age` | 20 |
| `predict_horizon_tasks` | 20 |
| `predict_horizon_days` | 14 |
| `predict_n_min` | 5 |
| `thinning_expire_days` | 14 |
| `seed_source` | `""` (path in Phase 0, or a URL reached through a Root egress class; empty = no upstream) |
| `seed_sync_interval_s` | 86,400 (automatic upstream sync; no owner command) |
| `g4_escalate_repeats` | 4 (the one escalated re-run of an inconclusive G4; kernel/gate.md §3.12) |
| `provisional_max_age_s` | 86,400 (F16: roll back a provisional version with no verdict after 24 h) |
| `soft_day_fraction` | 0.9 (`budget.day_usd` ≤ this × the charter's day ceiling) |
| `tasks.agent_stratum_max` | 6 (≤ ⌊`gate.g4.pool_k` / 2⌋; L1+, kernel/gate.md §3.7.2) |
| `fleet.explorers` | 4 (L2+) |
| `fleet.hgm_alpha` | 0.6 (L2+) |

The new keys are evolution.md §4.6's (v0.3); their semantics are there.

The timing keys `max_provisional_depth`, `pause_timeout_s`, `probation_s` and `rollback_max_defer_s` live only in `[supervise]` (settled in R1). An `[evolution]` table that contains them is a validation error naming the `[supervise]` key to use.

### 4.7a `[vendor]`: removed in v0.3

ESC-1 is settled: crates need no approval, an L0 right (CONVENTIONS §6, spec v0.3 §19). The section is gone, and a file containing it gets the error of §3.2 rule 10. The limits that remain (crates.io only, exact versions, 20 jobs a day, full G4) are kernel/initctl.md's and kernel/gate.md's, and need no key.

### 4.8 `[supervise]` (kernel/supervise.md §4.9)

| Key | Default |
|---|---|
| `ready_timeout_s` | 90 |
| `settle_s` | 3 |
| `probe_window_s` | 15 |
| `probation_s` | 120 |
| `heartbeat_stale_s` | 30 |
| `pause_timeout_s` | 1,800 |
| `rollback_max_defer_s` | 3,600 |
| `exit_grace_s` | 10 |
| `backoff_initial_ms` | 1,000 |
| `backoff_max_ms` | 60,000 |
| `backoff_reset_after_s` | 300 |
| `crash_window_s` | 600 |
| `crash_limit` | 4 |
| `safe_crash_limit` | 2 |
| `max_provisional_depth` | 3 |
| `build_timeout_cold_s` | 3,600 |
| `build_timeout_warm_s` | 1,800 |
| `deps_cache_keys` | 3 |
| `max_concurrent_probes` | 6 |
| `config_error_exits` | 2 |
| `disk.min_free_build_gib` | 5 |
| `disk.min_free_run_gib` | 1 |
| `replay_frames` | 2,000 |
| `replay_bytes` | 262,144 |
| `git` | platform default (absolute path) |

`notify_cmd` is removed (root/root.md X4): the kernel raises alerts with the Root's `alert` request, and the Root delivers them to the owner (root/root.md §4.1.3).

### 4.9 `[sandbox]` (kernel/sandbox.md §4.3)

| Key | Default |
|---|---|
| `toolchain` | resolved at install from `rustup which rustc` (directory; read + exec in every profile; also the gate's `<toolchain bin>`) |
| `developer_dir` | `/Library/Developer/CommandLineTools` (macOS) |
| `sdkroot` | `xcrun --show-sdk-path` at install |
| `bwrap` | `/usr/bin/bwrap` |
| `extra_read` | `[]` |
| `linux_memory_max` | `""` (off) |

### 4.10 Example: DeepSeek-only Phase 0

Prices are checked against https://api-docs.deepseek.com/quick_start/pricing/ on 2026-10-19. **The money is not here.** The owner's authorization and dollar ceilings (spec §10; the owner's estimate is $15–40 in total) go into the charter once, at install, together with the key (`init-root keys set deepseek`). See root/root.md for the charter format, and §2.1 for what the kernel reads from it.

```toml
# $INIT_HOME/kernel/config.toml — kernel config, shipped by the maintainer; the agent can read but not write it
# (below L3). The hard ceilings, keys and the day boundary are in the Root charter.
schema_version = 2

[model]
provider = "deepseek"
id       = "deepseek-flash"     # DeepSeek-V4.1-Flash-1017 as of 2026-10-17; the default route
effort   = "high"
routable = []                   # L1+: every priced model the Root forwards (here flash and v4-pro)

[proxy]
worker_threads = 2

[budget]                        # soft budgets under the charter's ceilings; maintainer defaults (agent-set from L1)
day_usd             = "5.00"    # clamped to ceiling − request_max_usd: $4.00 under the template's $5.00 ceiling (D6, D13)
task_usd            = "0.50"
gate_usd            = "3.00"
quick_gate_usd      = "0.50"
probe_usd           = "0.25"
request_max_usd     = "1.00"
evolution_share     = 0.6       # so one $3.00 gate fits in a $5.00 soft day
root_guard_permille = 20

[traces]
retention_days = 0

[providers.deepseek]                  # preset "deepseek" (routes and parse families); the Root holds the key

[providers.deepseek.schedule]
tz      = "+00:00"
default = "offpeak"
windows = [
  { days = "mon-fri", from = "01:00", to = "04:00", use = "peak" },
  { days = "mon-fri", from = "06:00", to = "10:00", use = "peak" },
]
default_dates_tz = "+08:00"
default_dates    = []                 # Chinese public holidays are off-peak; list them to price them so

[providers.deepseek.models."deepseek-flash"]
context_window        = 1048576       # docs say "1M"; checked against GET /models at start
max_output            = 393216
reserve_output_tokens = 131072        # 128K, the default when reasoning_effort = "max"
reference_window      = "peak"
prices.peak    = { input = "0.30", cache_read = "0.006", output = "1.20" }
prices.offpeak = { input = "0.15", cache_read = "0.003", output = "0.60" }

[providers.deepseek.models."deepseek-v4-pro"]
context_window        = 1048576
max_output            = 393216
reserve_output_tokens = 131072
reference_window      = "peak"
prices.peak    = { input = "1.32", cache_read = "0.044", output = "3.96" }
prices.offpeak = { input = "0.66", cache_read = "0.022", output = "1.98" }

[gate]
cargo_jobs = 0

[gate.g3]
mode = "model"

[gate.g4]
pool_k         = 10
holdout_h      = 6
canaries       = 2
repeats        = 2
repeats_max    = 3
margin_max     = 0.20     # Phase 0 pools are small; a 16-task gate resolves ~17–20 points (gate.md §3.8.5)
parallel       = 3
prefer_offpeak = true
cost_prior_usd = "0.03"

[evolution]
seed_source = ""          # set when the maintainer publishes seed versions

[sandbox]
extra_read = []
```

A test configuration (testing.md; `fixtures/homes/seed-default/kernel/config.toml`) points a `deepseek`-preset provider at the mock:

```toml
schema_version = 2

[proxy]
test_mode = true

[budget]                                # ceilings come from init-fakeroot's charter fixture (testing.md §3.5b)
day_usd  = "1.00"
gate_usd = "0.50"
task_usd = "0.10"

[gate.g3]
mode = "scripted"                       # $0 G3 through the mock (testing.md)

[gate.g4]
ct_include_wall = false                 # mock wall-clock is jitter (evolution-testing RS-4)

[supervise.disk]
min_free_build_mib = 64                 # MiB siblings allowed only in test mode (§3.1)
min_free_run_mib   = 16

[model]
provider = "mock"
id       = "mock-frontier-1"

[providers.mock]
preset   = "deepseek"                   # DeepSeek routes and parsing; head-hash provider id "mock"
                                        # no base_url: init-fakeroot (or the Root) forwards to init-mockllm
[providers.mock.models."mock-frontier-1"]
max_output = 65536
prices.standard = { input = "0", output = "0" }
```

Kernel-only scenarios run this home under `init-fakeroot`, whose charter fixture supplies the ceilings and the level (testing.md §3.5b); full-stack scenarios run it under the real Root with `root.test_mode = true`.

The Anthropic and OpenAI presets need only a `[providers.<id>]` table and a `models` table with prices (kernel/proxy.md §4.8.2–4.8.3 list current prices). The charter must also let the Root forward that provider, and the key goes into the Root. For example:

```toml
[providers.anthropic.models."claude-opus-5-5"]
max_output = 131072
prices.standard = { input = "4", cache_write = "5", cache_write_1h = "8", cache_read = "0.20", output = "20" }

[providers.anthropic.multipliers]
"speed:fast" = "2.0"
"inference_geo:us" = "1.1"
```

## 5. Failure modes and recovery

| Failure | Effect | Recovery |
|---|---|---|
| Syntax error or unknown key at start | kernel halts `E_CONFIG` with every error and its key path | the maintainer or owner fixes the file; `init-kernel restart`. At L3, a broken `config.default.toml` fails G0 or G1 of the `kernel` scope before it can run |
| A v0.2 file (`schema_version = 1`, `day_starts_at_utc_offset`, `key_file`, `[vendor]` …) | one `E_CONFIG` listing every moved key and where it went (§3.2 rules 1 and 10) | write the ceilings into the charter, store the key with `init-root keys set`, delete the moved keys, set `schema_version = 2`. The soft `[budget]` money keys stay |
| Invalid edit, then reload | old config stays | errors go to `state/kernel/alerts.jsonl` |
| Price missing for `model.id` | validation error 2/4 | add the price table |
| Charter ceiling lowered below the soft day, or below today's Root spend | soft day clamped (`soft_day_clamped`), or new requests refused at once with `budget.root_day` (kernel/proxy.md §3.8) | intended; it comes from the Root, not from this file |
| Kernel price differs from the Root's for a forwarded model | warning at start; `cost_mismatch` alerts at run time | align the kernel price table (kernel/proxy.md §9 Q-EG5) |
| `model.id` changed | thinning baseline starts (evolution.md §3.12) | intended |
| Any `base_url`, `[proxy.test_root]` or `[supervise] notify_cmd` | removed-key error (rule 10) | the Root owns upstreams and alert delivery |

## 6. Performance notes (spec §9)

Parsing is a one-time cost of microseconds. The performance-relevant settings are the proxy's egress pool and Root poll interval (rank 2; the provider connection keep-alive moved to the Root), and the gate's `parallel` and `prefer_offpeak` (cost and wall-clock of G4).

## 7. Invariants and security (spec §2.1)

Spec v0.3 §19 maps the invariants onto the Root. This file's part:
- **#3 Blast radius (R2, R3).** No secrets and no ceilings live here. Keys are in the Root's key store; money, quotas and external-action classes are in the charter. A kernel config the agent changes at L3 therefore cannot raise spending or reach a new host, and `test_mode` cannot either, because the agent zone's network boundary is the Root's sandbox (kernel/proxy.md §3.7).
- **#2 Honest measuring stick (R6).** Gate parameters are kernel config. Below L3 the agent cannot write them. From L3 a change to them is a kernel change, judged by the Root's gate-of-gates, and the sealed audit still runs (kernel/gate.md §3.1). K is deliberately not a config key (§4.6). The config digest and the charter digest are in every gate record, so measurements under different rules are never confused.
- **#1 / #5 (R1, R3).** Changing this file is a maintainer or optional owner action below L3, and a gated kernel change from L3. Nothing in it is an approval point: `[vendor] require_approval` is gone. The agent can read it, which is deliberate: knowing its gate and its sub-limits is awareness.

## 8. Test plan

| # | Case | Expectation |
|---|---|---|
| C-01 | the §4.10 example parses; digest stable across reformatting and comment changes | ok; same digest |
| C-02 | unknown key `[budget] day_cap_usd` | error with key path |
| C-03 | money as float (`probe_usd = 0.25`) | error "money must be a string" |
| C-04 | `model.id` without prices | error |
| C-05 | schedule boundaries: Mon 00:59 / 01:00 / 03:59 / 04:00 UTC, Sat 02:00, a listed default date (in +08:00) | offpeak / peak / peak / offpeak / offpeak / offpeak (shared with proxy P-07) |
| C-06 | `providers.mock.base_url = "http://127.0.0.1:1"`; `[proxy.test_root]`; `[supervise] notify_cmd = []`; `supervise.disk.min_free_run_mib` without `test_mode` | each a removed-key or test-mode error naming its reason |
| C-07 | v0.2 keys: `[budget] day_starts_at_utc_offset`; `providers.deepseek.key_file`; `providers.openai.organization`; `[vendor] require_approval`; `proxy.pool_idle_timeout_s` | one error per key, each naming its new home (charter, Root key store, removed, root/egress.md); `schema_version = 1` → one summary error; `[budget] day_usd = "5.00"` alone is accepted (a soft budget) |
| C-08 | `[evolution] probation_s = 60` (a key that lives only in `[supervise]`) | error naming `supervise.probation_s` |
| C-10 | `supervise.disk.min_free_run_mib = 64` with and without `proxy.test_mode` | accepted (precedence over `_gib`) / error |
| C-09 | reload with an invalid file | old config kept; errors reported |
| C-11 | `model.routable = ["deepseek/deepseek-v4-pro"]` with and without a v4-pro price table | ok / error (rule 2) |
| C-12 | `providers.openai.websocket = true` | error (rule 11) until the egress supports upgrades |
| C-13 | L3 merge: `config.default.toml` with `gate.g4.margin_max = 0.15`, owner override 0.10 | effective 0.10; the digest covers the merged struct |
| C-14 | the test config of §4.10 under `init-fakeroot` with a $2.00 day ceiling | parses; the effective soft day is $1.00 (below ceiling − `request_max_usd`); a fixture ceiling of $1.50 clamps it to $0.50 and raises `soft_day_clamped`; the kernel keeps running (D6) |
| C-15 | the §4.10 Phase 0 example with the D13 template ($5.00 day ceiling) | effective soft day $4.00; rule 6 warns that `gate_usd` $3.00 exceeds the effective evolution share $2.40 |

## 9. Open questions

**Within this doc**
1. From L3, should owner overrides in `$INIT_HOME/kernel/config.toml` be allowed for gate statistics (`margin_max`, `alpha`), or only for operational keys (`parallel`, `notify_cmd`, disk thresholds)? Allowed for now: an owner override can only come from the owner, and the owner is above the agent. But it changes the gate outside the gate-of-gates, so the record's config digest is what makes it visible.
2. Should `schema_version = 1` files be migrated automatically (dropping moved keys with a warning)? No. Moved keys are money and key paths, and silently dropping them could hide a ceiling the owner believed was in force. One explicit error is safer.

**Cross-doc** (v0.3 round 2; items addressed to this doc are settled in reconciliation/v03-kernel-gate.md; the R1/R2 items, such as duplicate timing keys, money as strings, the `boot.json.model` mapping and test-mode names, stay settled in reconciliation/kernel-proxy-gate.md):
- **Q-EG1 → root/egress.md:** provider base URLs, account headers (OpenAI org and project), key files and the h2/pool settings removed here are yours. A price table that the Root publishes would let the kernel check its measurement prices (kernel/proxy.md Q-EG5).
- **Q-IC1 → kernel/initctl.md:** `[vendor] require_approval` is removed; `initctl vendor` keeps its fixed limits without an approval wait (ESC-1, L0 right).
- **Q-AW1 → awareness.md:** the owner's display offset (AW-1) now comes from the charter's day boundary (`ceilings.day_starts_at_utc_offset`, seen as `day_resets_at` in `/_init/stats` `root`), not from `budget.day_starts_at_utc_offset`.
- **Q-TS1 → testing.md:** fixture configs move to `schema_version = 2` and drop `base_url` and `[proxy.test_root]`; their ceilings come from `init-fakeroot`'s charter fixture.
- **Q-CV1 → CONVENTIONS.md §4, §6:** the contract-registry row says `[budget]` money keys are proxy.md's semantics; that still holds for the soft budgets, but the hard ceilings are now the charter's (root/egress.md). §6's E1 row should split: ceilings to the charter (PENDING OWNER), soft defaults to kernel config (maintainer defaults).
- **Q-EV1 → evolution.md §3.14 and §4.6:** the clamp is `min(day_usd, ceiling − request_max_usd, soft_day_fraction × ceiling)` (D6, root/root.md X4). Should `soft_day_fraction` stay, or the D6 bound alone? Both are applied until evolution.md decides.

## 10. Spec deltas

| Spec v0.2 | This doc | Why |
|---|---|---|
| §2.1 "owner-set caps" (no file named) | one TOML file, `kernel/config.toml`, strict schema, money as strings | typos and float money must not silently change limits |
| (new) | the price table doubles as the model allowlist | the proxy cannot meter a model it cannot price |

**v0.3 changes** (spec v0.3 Part II, ADR-007, CONVENTIONS §6 v0.3):

| Spec / v0.2 doc | This doc (v0.3) | Why |
|---|---|---|
| v0.2 §1 "the one file through which the owner sets what INIT may spend … which providers and keys exist" | kernel-only config; ceilings, keys, day boundary, external-action classes, max level and purpose are in the Root charter and key store; §2.1 lists what moved | spec v0.3 §13–§14, §19: control moves into a Root the agent cannot modify |
| v0.2 `[budget] day_usd`, `gate_usd`, `task_usd`, `evolution_share` (PENDING OWNER E1); `day_starts_at_utc_offset` | the hard ceilings move to the charter (`ceilings.day_musd`, `ceilings.total_musd`, `ceilings.day_starts_at_utc_offset`; root/egress.md §3.3). The `[budget]` money keys stay as **soft** budgets, maintainer defaults, agent-set from L1, with `day_usd` ≤ 0.9 × the charter's day ceiling. `day_starts_at_utc_offset` is removed | CONVENTIONS §6 v0.3: "Ceilings E1/ET-E1 move into the charter"; evolution.md §3.14; spec v0.3 §15 L1 "allocates its daily budget" |
| v0.2 `providers.<id>.key_file`, `organization`, `project`; `[proxy]` h2 and pool keys | removed (Root key store and egress) | spec v0.3 §14 R2 |
| v0.2 `providers.<id>.base_url` (https) | removed, also in tests; the only upstream is the Root egress socket, and kernel-only tests run under `init-fakeroot`; `[proxy.test_root]` removed; `proxy.test_mode` only marks a test home | spec v0.3 §13; testing.md §3.5b |
| (new, D6) | effective soft day = min(`day_usd`, ceiling − `request_max_usd`, `soft_day_fraction` × ceiling), effective soft total = ceiling − `request_max_usd`, clamped at run time, never a refusal to start | v03-decisions D6; root/root.md X4 |
| v0.2 `[supervise] notify_cmd` | removed: alerts go through the Root | root/root.md X4 |
| (D13) | the charter template is $5.00/day and $25.00 total (PENDING OWNER); the effective soft day is $4.00 and the evolution share $2.40 under it | v03-decisions D13 |
| v0.2 `[vendor] require_approval` | removed | ESC-1 settled as an L0 right; v0.3 removes human review |
| (new) | `schema_version = 2`; removed keys give pointed errors; `model.routable`; `budget.root_guard_permille`, `budget.task_submits_day`; `proxy.root_poll_s`, `egress_pool_max_idle`, `[proxy.test_root]`; `gate.g4.capability_*`; evolution.md §4.6's new `[evolution]` keys | v0.3 mechanisms in kernel/proxy.md, kernel/gate.md and evolution.md |
| v0.2 "the owner edits it; the agent cannot write it" | maintainer-shipped; owner edits optional; from L3 part of the kernel tree, changed through the gate-of-gates, with optional owner overrides | spec v0.3 §15 L3, §18 |
| v0.2 config digest in gate records | plus the Root-reported charter digest | rules now come from two places |

**Build sync 2026-10-20** (build → docs; ledger reconciliation/build-deviations.md §1.12):

| Build ref | This doc now says | Section |
|---|---|---|
| build: WP21b D4 | the Root's minimal kernel `config.toml` template is `schema_version = 2` plus `[sandbox]` (`toolchain`, `developer_dir`) taken from `install.json`; it is written only when the release ships no `config.toml`, and the other sections take their defaults | §3.1 |
| build-deviations §5 L3 | the `KernelConfig` struct comment says `schema_version` "must be 2", matching rule 1, the examples, the code and the Root's template | §3.2 |

## 11. References

Checked 2026-10-19 unless marked. Added in v0.3 (2026-10-20, this repo): spec v0.3 Part II §13–§19; adr/007-root-and-autonomy-ladder.md; CONVENTIONS.md v0.3 §4, §6.
- TOML 1.1 and the `toml` crate 1.1.8 — https://toml.io/en/v1.1.0 , https://crates.io/crates/toml
- serde `deny_unknown_fields` — https://serde.rs/container-attrs.html#deny_unknown_fields
- DeepSeek Models & Pricing — https://api-docs.deepseek.com/quick_start/pricing/
- Anthropic pricing — https://platform.claude.com/docs/en/about-claude/pricing
- OpenAI pricing — https://developers.openai.com/api/docs/pricing
- kernel/proxy.md, kernel/gate.md, kernel/supervise.md §4.9, kernel/sandbox.md §4.3, evolution.md §4.6 (this repo)
