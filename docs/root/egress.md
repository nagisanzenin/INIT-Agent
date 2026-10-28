# Root egress gateway and external-action policy (R2 + R3)

> **Current policy — 2026-10-23 owner decision:** monetary limits in this document are
> superseded by observe-only accounting. Live, sealed, audit/slice and twin work does not
> refuse, stop or demote because a configured USD allowance is reached. Legacy dollar
> fields remain accepted metadata. `money.enforced = false`, `money.policy = "observe"`.
> Users retain stop/panic controls. Security, request cardinality/output validation,
> time/request/token ceilings and durable usage logging remain enforced. Independently
> authorized benchmark budgets belong to the external harness, not the product Root.
>
> Costs use recorded provider token usage and configured tariffs, not provider invoices.
> Missing usage retains the reservation estimate. `money.total_reserved_musd` exposes
> in-flight estimates. Token totals cover **settled live metered requests** only:
> `total_input_tokens` / `total_output_tokens` are null if any usage is unknown;
> `reported_input_tokens` / `reported_output_tokens` retain the known subtotal, with
> `token_totals_complete`. A legacy checkpoint without token evidence stays incomplete.
> Sealed/twin costs and tokens remain separately attributed in lane/run ledgers.
> UI/proxy clients must honor `enforced=false` and not turn legacy dollar fields into
> automatic stops. Older Root responses without this marker retain their old contract.

## Explicit compatible API providers (2026-10-23)

See [provider configuration examples](../provider-configuration.md) for paired Root/kernel
Chat, native Messages/Responses and local no-auth configurations, descriptor options and
current compatibility/accounting limits.

Owner-configured providers may set `api = "chat"`, `"responses"`, or `"messages"`.
These select metered POST endpoints respectively: `chat/completions`, `responses`,
or `messages`, with an optional `v1/` prefix. Provider IDs are arbitrary valid aliases; `host` must be an exact
class destination and `base_path` supplies a prefix such as `/v1` or `/team/v1`.
The class still needs the endpoint in `paths`, POST in `methods`, and explicit model
tariffs. Seed native Responses and Messages adapters emit `v1/responses` and
`v1/messages`: list those exact paths, use `base_path=""` for a normal `/v1`
upstream, and give the matching kernel proxy alias `preset="openai"` or
`preset="anthropic"`. For a proxy under `/team/v1`, use `base_path="/team"`
with these native adapters. The Chat adapter emits `chat/completions`; its alias
uses `preset="deepseek"` (or an explicit metered chat route), with Root
`base_path="/v1"` for an OpenAI-compatible local server. Base paths are literal
prefixes: the Root never silently strips or duplicates version components. Every request requires a model and a valid output-token bound; usage is
recorded even with a zero-price local model. An explicit dialect does not add free
GET or arbitrary POST routes. Omitting `api` preserves preset routing.

For an OpenAI-compatible local server, an owner can use:

```toml
[actions.providers]
destinations = ["127.0.0.1:11434"]
paths = ["chat/completions"]
[actions.providers.providers.localmodel]
host = "127.0.0.1:11434"
base_path = "/v1"
api = "chat"
local = true
auth = "none"
[actions.providers.models."your-local-model"]
min_level = 0
input_musd_per_mtok = 0
output_musd_per_mtok = 0
reserve_output_tokens = 4096
```

This replaces the providers class destinations; list other exact hosts/providers
when retaining remote models. `local=true` requires an explicit dialect and exact
`127.0.0.1:<port>` or `localhost:<port>`. Only local providers may set `auth="none"`;
their key file is never read and `keys` reports `not_required`. Remote traffic uses
HTTPS and existing bearer/x-api-key modes. Local traffic uses HTTP. Both live and
sealed forwarding disable redirects and ambient HTTP proxy environment settings;
a custom API proxy must be the explicitly configured destination. Test mode still
refuses remote traffic and real keys (authenticated test keys must start `test-`).
This is compatibility with these three wire formats, not native Gemini or Ollama
codecs, arbitrary OAuth schemes, or certification of every provider/model.

```
Status: v0.3 + build sync 2026-10-20 · Area: root
Depends on: CONVENTIONS.md, INIT Agent spec.md (v0.3 §13–§16, §19), adr/007-root-and-autonomy-ladder.md,
            reconciliation/v03-decisions.md, root/root.md, root/sealed-eval.md, kernel/proxy.md,
            kernel/sandbox.md, kernel/config.md, kernel/initctl.md, body/log-context.md
Defines:    the egress forwarder (wire on ROOT_HOME/run/egress.sock; key storage and injection; Root
            metering; hard money ceilings per day and total in integer micro-USD; reservations; the
            refusal format; which refusals demote); the action-class schema of the charter's [actions]
            table, its matching rules and the built-in `providers` and `crates` classes; the per-level
            model allowlist; the non-HTTP helper verbs (git push, email); sealed lanes for root/sealed-eval.md;
            the money block of ceilings.get and GET /_root/ceilings; the egress module API
            (panic, reload, validate_actions, ceilings, sealed_lane); the egress dependency list
```

---

## 1. Purpose

v0.3 moves the two things that must stay outside the agent into the Root (spec §13, §19): the **provider keys** and the **hard money ceilings**. The kernel keeps routing, caching, traces, timing, tool observations and the soft budgets (kernel/proxy.md), and from L3 the agent may evolve all of it. The Root keeps only what a mis-evolved kernel must never be able to loosen.

R2 is a **minimal metering forwarder** below the kernel proxy. The kernel proxy forwards each routed request to the Root's egress socket; the Root checks the request against the charter, reserves its worst-case cost against the hard ceilings, injects the key, forwards it over its own TLS connection and streams the answer back unchanged. R3 is the **external-action policy**: the Root reaches only destinations listed in a charter action class, each with its own limits. The agent zone's **only** network path is this socket (spec §13).

## 2. Responsibilities / non-goals

**Responsibilities**
- Serve `ROOT_HOME/run/egress.sock` (the kernel profile only) and the per-run **sealed lanes** that root/sealed-eval.md opens (§3.8).
- Hold the provider keys (`ROOT_HOME/keys/providers/<id>`) and the action-class credentials; inject them. Nothing above the Root sends or sees auth.
- Match every request against a charter action class (destination, method, path, level, volume, rate); refuse everything else.
- Meter usage per wire family for the Root's own hard accounting, and enforce the money ceilings with reservations.
- Report hits to lineage (`ceiling_hit`, root/root.md §3.10.5) with the `demotes` flag fixed by decision D6 (§3.6).
- Publish the money block, the class headroom and the per-level model table to the kernel (`ceilings.get`, `GET /_root/ceilings`).

**Non-goals**
- **Compute, disk and parallel-body quotas.** They are enforced by the Root's spawn broker and watcher (root/root.md §3.14), the one place every zone process is started. §3.9 records only the platform notes this doc contributed and the reconciliation decision.
- **Prompt rewriting, caching, traces, soft budgets, routing policy.** kernel/proxy.md.
- **The charter file format, lineage envelope, ledger, demotion procedure, process model, sandbox profiles.** root/root.md. This doc owns the `[actions]` table's schema and the semantics of the `money` keys.

## 3. Design

### 3.1 Where the forwarder sits

```
 body ──unix──▶ kernel proxy ──unix──▶ Root egress (init-root, owner uid, unsandboxed) ──TLS──▶ provider
                routing, cache,        egress.sock: classes, level, reservations,        api.deepseek.com
                traces, soft caps,     ceilings, key injection, Root metering             api.anthropic.com
                guard (proxy §3.8)                                                         api.openai.com
                                                                                           index.crates.io …
 sealed body ──unix (sealed lane, §3.8)──▶ Root egress            (no kernel, no trace store; decision D5)
```

- **Socket.** `ROOT_HOME/run/egress.sock`, mode 0600, created by the Root (root/root.md §3.2.1). Only the `kernel` profile may connect to it (root/root.md §3.3.3); no body sandbox lists it. The kernel learns the path from `hello.paths.egress_sock` (root/root.md §4.1.3), not from an environment variable (answers kernel/proxy.md Q-RT0).
- **Wire.** HTTP/1.1 over the Unix socket, one request per connection at a time, keep-alive allowed. Requests are `<METHOD> /<provider_id>/<path>` for model providers and `<METHOD> /x/<class_id>/<host>/<path>` for every other HTTP class (§3.5). `GET /_root/ceilings` and `GET /_root/health` are answered locally, inside the same in-flight bound as forwarded requests (security review 2026-10-20 F7). The Root forwards the request body byte-for-byte and never re-serializes it.
- **Threads, not a runtime.** The egress uses std threads: one accept thread per socket, one thread per in-flight request (bounded by `egress.max_inflight`, 64), a blocking HTTP client with a keep-alive pool, and one ledger-writer thread. This matches the Root core (no async runtime, root/root.md §4.8) and fits the size budget (§6). At ≤ 64 concurrent streams, threads cost a few hundred KiB of stack in total.
- **Connection bound (security review 2026-10-20 F3).** At most `egress.max_conns` (96) kernel connections are served at once, one thread and one descriptor each. The accept thread answers one more with 503 `root.busy` (the kernel maps it to `egress.unavailable`) and closes it without spawning anything. A connection may idle 120 s between requests; once a request's first byte arrives, its head and body must follow within 30 s, and a response write may stall at most 120 s. A held-open or trickling connection therefore cannot pin a Root thread, and the egress stays near 176 descriptors (96 connections, ≤ 64 upstream sockets, the pool of 16), under launchd's default soft limit of 256.

### 3.2 Keys

**Decision:** provider keys live only in `ROOT_HOME/keys/providers/<provider_id>`, mode 0600, directory 0700, excluded from Time Machine (root/root.md §3.2.1). Loading follows root/root.md R-F4, which is v0.2's rule from kernel/proxy.md: `O_NOFOLLOW|O_CLOEXEC`, a regular file owned by the uid, no group or other bits, 1–512 bytes of UTF-8 with no control characters after stripping the trailing newline. Keys are held as a secret type with no `Debug`/`Display`/`Serialize` and are zeroed on drop. A missing or insecure key disables that provider (refusal `class.disabled`) and raises the alert `key_file_insecure`. In test mode (`root.test_mode`) only keys that start with `test-` are loaded; any other key is refused (the provider is disabled) with the alert `key_file_real`, which makes "refuses real key files" (testing.md §3.4.1) mechanical. Test mode also allows loopback destinations (`127.0.0.1:<port>` and `localhost:<port>`, plain HTTP), `crates` included, and never forwards to a non-loopback host (`class.disabled`). Production rejects loopback destinations at C11 (`validate_actions` takes a `test_mode` argument).

- `init-root keys set <provider_id>` reads the key from the TTY with echo off, writes a temp file, `fsync`s, renames, then calls `egress::reload()`. A key is never a command-line argument or environment variable. `init-root keys check` reports present, insecure or missing, never the key.
- Action-class credentials (a deploy key for a `git-push` class, SMTP auth for an `email` class) live at `ROOT_HOME/keys/actions/<class_id>` under the same rule and are set with `init-root keys set --action <class_id>`.
- **Upstream 401/403** bodies are withheld and replaced by a fixed body with code `upstream.auth` (some providers echo a masked key fragment). Alert `upstream_auth`. The kernel rewrites them too, as defense in depth (answers kernel/proxy.md Q-EG3).
- **Privacy (CONVENTIONS §1).** The Root sets `user-agent: init-root/<version>`. It never derives any header, envelope sender or commit identity from git config, `getpwuid` or the owner's email.

### 3.3 Hard money ceilings

The charter's `[money]` table holds them (root/root.md §3.12.1): `day_usd` and `total_usd` as decimal strings, `day_starts_at_utc_offset` as a fixed offset. The egress parses them exactly into **integer micro-USD** and accounts in micro-USD throughout.

| Ceiling | Charter key | Window | Refusal code |
|---|---|---|---|
| Per owner day, agent zone | `money.day_usd` | day starting at `money.day_starts_at_utc_offset` (rule of kernel/proxy.md §3.8) | `ceiling.day` |
| Lifetime total, agent zone | `money.total_usd` minus `sealed.reserve_usd` | since install | `ceiling.total` |
| Lifetime total, sealed lanes | `money.total_usd` | since install | `ceiling.total` |
| Per sealed run / audit | the budget root/sealed-eval.md passes to `sealed_lane` | the lane's life | `ceiling.lane` |

- Template values are $5.00/day and $25.00 total (**PENDING OWNER (CONVENTIONS §6 E1)**; decision D13). The soft budgets stay in kernel config.
- **The sealed reserve** keeps the last `sealed.reserve_usd` (root/sealed-eval.md §4.3) of the total for the Root's own evaluation, so the agent zone can never spend the money the sealed audit needs. Sealed lanes are not charged against the day ceiling; their spend is capped per run and per audit instead. The one exception is a lane opened with `Pool::Agent` for an L3+ sealed-half family evaluation (root/sealed-eval.md §3.6): that work is done for the agent's gate, so it is charged to the zone's day and total like any zone request.
- **Lowering a ceiling never stops anything** (D6). `egress::reload()` applies a new ceiling at the next admission. The charter validator accepts a ceiling below the current spend; requests are then refused until the day resets or the owner raises it. The kernel clamps its soft budgets to the new value from `ceilings.get` (kernel/proxy.md §3.8).

### 3.4 Reservations and metering

**Admission** (one mutex over the money state):

```text
r = req_bytes × price.input_musd_per_mtok / 10^6                    // input as if all uncached, 1 token per byte
  + min(max_out(req), 2^20) × price.output_musd_per_mtok / 10^6      // max_tokens | max_completion_tokens |
                                                                     // max_output_tokens | model.reserve_output_tokens
g = the same with ceil(req_bytes / 3) input tokens                   // the kernel guard's figure; D6 rule (c) 3 only
for c in applicable(lane):  if c.spent + c.reserved + r > c.limit → refuse c.code   // saturating sums
for c in applicable(lane):  c.reserved += r
settle(r, cost):            c.reserved −= r; c.spent += cost; epoch += 1
```

- Prices are the **peak** price per `(provider, model)` from the `providers` class's model table (§3.5), in integer micro-USD per million tokens, so the reservation and the settlement never under-count. The kernel measures at window prices; the guard of kernel/proxy.md §3.8 reads the Root's figure, so the two layers do not disagree about what is spent.
- **Usage parsing** reuses kernel/proxy.md §3.10.2's per-family tables by reference (`openai-chat`, `anthropic`, `openai-responses`). The Root's tap reads only `data:` lines that contain a usage object or a finish/stop reason, forwards every frame unchanged, and computes `cost = (input_total − cache_read) × input + cache_read × cache_read_price + output × output` at peak prices. **Provisional owner decision 2026-10-20 (build-deviations §3.13):** cache reads are billed at the model's `cache_read_musd_per_mtok` (built-in and template `deepseek-flash`: 6,000, kernel/config.md's peak `cache_read`); a model without that key bills them at its input price, the old safe default. The peak window stays, so the Root remains an upper bound: on S1's real usage the Root/proxy ratio falls from 3.73× (2,844 against 762 µUSD) to 2.00× (1,527), which is the peak/off-peak factor. The `end` record's `usage` is `{input, cache_read, output}`. **Cache writes (v0.5.0 L3, 2026-10-27):** the tap also reads the writes inside `input_total` (Anthropic `cache_creation_input_tokens`, split by `cache_creation.ephemeral_5m/1h_input_tokens`, an unsplit write counted as 1-hour; OpenAI `input_tokens_details.cache_write_tokens`; a Chat gateway's `prompt_tokens_details.cache_write_tokens`) and bills them at `cache_write_musd_per_mtok` and `cache_write_1h_musd_per_mtok`; a model without them bills writes at 2× its input price, the highest write price in the providers report's Table A, so the ledger never under-counts. `providers add` writes the catalog's write prices into the charter. The `usage` object gains `cache_write` and `cache_write_1h` only when they are not 0, so DeepSeek's records keep their shape. **Security review 2026-10-20 F1, F4 (provisional owner decision 2026-10-20):** admission and the sealed lane's budget use saturating sums, so a zone-chosen `max_tokens` near `u64::MAX` is refused instead of wrapping under the ceiling; the output term is clamped at 2^20 tokens (above every provider's own output limit); and the held reservation counts input at one token per byte, so the actual cost stays within it and the ceiling is a hard wall. The guard figure `g` keeps `ceil(req_bytes / 3)`, which the kernel proxy mirrors, so a guard that did its sum is never judged a bypass. A stream that ends without usage (abort, timeout, parse miss) is charged its full reservation and flagged `usage_estimated`.
- The tap also records the **finish class** (`tool_use`, `stop`, `length`, `error`) with the normalization of kernel/proxy.md §3.12, because sealed lanes need it to detect the end of a run (§3.8).
- **Ledger.** `ROOT_HOME/egress/usage/<YYYY-MM-DD>.jsonl` (0600, UTC date): a `start` record before forwarding (`id`, `lane`, `class`, `provider`, `model`, `reserved_musd`) and an `end` record after (`cost_musd`, normalized usage, `finish`, `flags`). Records hold no request or response content. `start` records are `fsync`ed one by one before the upstream request is sent (no group commit); `end` records are not synced on their own. At start the egress rebuilds day, total, sealed-reserve and class counters from these files; a `start` without an `end` is charged at its reservation. `ROOT_HOME/egress/totals.json` checkpoints the lifetime total hourly so that a rebuild reads only the tail.
- **Retries.** The Root retries only connect-phase failures: DNS, TCP or TLS errors before any request byte was written. At most 2 retries, with the backoff of kernel/proxy.md §3.9, reported as `x-egress-retries`. It never retries on a status code. Status-based retries (429, 5xx, 529) stay in the kernel, so attempts never multiply (answers kernel/proxy.md Q-EG3). Each retry reuses the reservation.
- **Money details.** Admission checks the total before the day, so the non-retryable reason surfaces first. A cost counts toward the owner day in which the request was admitted. A non-2xx upstream answer costs 0; a non-connect transport error is charged the reservation. Cost is rounded up once, over the sum of the input and output terms.

### 3.5 R3: action classes

Everything the Root may reach is an action class in the charter's `[actions]` table. Nothing else is reachable. There is no per-action human approval (spec §14 R3).

**Schema** (one TOML table per class, `deny_unknown_fields`, validated by `egress::validate_actions`, root/root.md C11):

```toml
[actions.providers]                 # built-in; present even if the charter omits it
kind = "model"                      # "model" | "http" | "git-push" | "email"
min_level = 0                       # the class is usable from this autonomy level up
destinations = ["api.deepseek.com:443"]
methods = ["GET", "POST"]
paths = ["chat/completions", "beta/chat/completions", "responses", "anthropic/v1/messages",
         "models", "user/balance"]
requests_per_day = 100_000
bytes_per_day = 8_000_000_000
rate_per_min = 600

[actions.providers.models."deepseek-flash"]   # the model allowlist; price = peak, µUSD per 1M tokens
min_level = 0
input_musd_per_mtok = 300_000
cache_read_musd_per_mtok = 6_000      # optional; absent = the input price (§3.4, provisional owner decision 2026-10-20)
# cache_write_musd_per_mtok, cache_write_1h_musd_per_mtok: optional; absent = 2× the input price (§3.4, v0.5.0 L3)
output_musd_per_mtok = 1_200_000
reserve_output_tokens = 65_536

[actions.providers.models."deepseek-v4-pro"]
min_level = 1                       # usable only once the agent may route per subtask (L1)
input_musd_per_mtok = 1_320_000
output_musd_per_mtok = 3_960_000
reserve_output_tokens = 65_536
```

| Field | Rule |
|---|---|
| table key | the class id, `^[a-z0-9][a-z0-9-]{0,31}$` |
| `kind` | `model` and `http` are supported. `git-push` and `email` are specified (§3.7), but `validate_actions` rejects them with `actions.unsupported_kind` until a Root release implements them (`SUPPORTED_ACTION_KINDS`, as `SUPPORTED_MAX_LEVEL` does for levels) |
| `min_level` | 0–4. Below it, every request of the class is refused `class.level` (root/root.md §3.13.4) |
| `destinations` | exact `host:port` strings; no wildcards, no subdomain globs, never an IP literal for `model` (loopback only in test mode, §3.2). A `model` class maps each provider id to exactly one destination through `providers` below |
| `providers` | for `kind = "model"` only: `{<provider_id> = {host = "api.deepseek.com:443", base_path = "", auth = "bearer" \| "x-api-key"}}`; DeepSeek, Anthropic and OpenAI presets fill this in (kernel/proxy.md §3.5 table) |
| `methods` | exact, uppercase |
| `paths` | the upstream path without a leading `/`: an exact string, or a prefix ending in a single `*`. Paths containing `..`, `//`, `%2e` or `%2f` (any case) are refused `class.path` before matching. For `kind = "model"` the list is further cut to the Root's **metering table** (default-deny, security review 2026-10-20 F2): each provider preset has an explicit list of `(method, path)` routes, each billed (inspected, gated by model and level, reserved, settled) or free ($0: `GET models`, `GET user/balance`, `GET v1/models`, Anthropic `POST v1/messages/count_tokens`); a provider without a preset gets the generic completion routes and `models`. A listed route without a rule (for example `embeddings`, legacy `completions`, `images/*`) is refused `class.unmetered` and never forwarded; C11 warns about such a path at charter check |
| `requests_per_day`, `bytes_per_day` | owner-day volume ceilings (request plus response bytes) |
| `rate_per_min` | token bucket: capacity `rate_per_min`, refilled at `rate_per_min`/60 per second |
| `models` | for `kind = "model"`: the **model allowlist per level** and the peak price table (answers root/root.md X5 and evolution.md's item (b)). A request whose `model` is not listed is refused `class.model`; one listed above the current level is refused `class.level`. At L0 the owner lists exactly one model at `min_level = 0` (the kernel's `model.id`); routing among more models starts at L1 |
| `response_max_bytes` | `http` only: per-response cap; default 64 MiB |

**Matching order** for a request on lane `l` at level `L`: route → class (provider id or `x/<class_id>`) → `destinations` → `methods` → `paths` → `min_level` (class, then model) → day volume → rate → money (§3.3, model classes only). The first failure is the refusal.

**Request policy for model classes.** The Root parses the request body only for `model`, `stream` and the `max_*` output field, plus one structural check: every `tools[]` element must be a client-executed tool (`openai-chat`: `type = "function"`; `openai-responses`: `function` or `custom`; `anthropic`: no `type`, or `custom`), and the top-level keys `mcp_servers` (Anthropic) and `background: true` (OpenAI Responses) are refused. Provider-executed tools reach the network on the provider's side, outside every class, so they are refused `class.provider_tool` unless a later class allows them. HTTP upgrades (OpenAI WebSocket mode) are not forwarded in this version, so `providers.openai.websocket = true` stays a kernel config error (kernel/proxy.md §3.15).

**Built-in classes.**

| Class | Kind | Destinations | Methods · paths | Limits | `min_level` | In safe mode |
|---|---|---|---|---|---|---|
| `providers` | model | from the charter; default DeepSeek only (spec §19) | the preset routes | 100,000 req/day, 8 GB/day, 600/min | 0 | yes, with the L0 model list |
| `crates` (decision D10) | http | `index.crates.io:443`, `static.crates.io:443` | `GET`, `HEAD` · `*` on the index, `crates/*` on static | 5,000 req/day, 2 GB/day, 300/min; responses ≤ 64 MiB | 0 (an L0 right, ESC-1; no approval) | no (safe mode disables evolution, and vendoring is part of it) |

The `crates` class carries no credential and no request body. It serves `initctl vendor` (kernel/initctl.md), which fetches the sparse index entries and the `.crate` files over `/x/crates/<host>/<path>` and builds an offline source for cargo; cargo itself cannot reach a Unix socket (cross-doc). crates.io's sparse index publishes `"dl": "https://static.crates.io/crates"` (checked 2026-10-20), so no other host is needed. The Root never follows redirects. The owner may disable a built-in class with `enabled = false`; the defaults are on.

### 3.6 Refusals, lineage and what demotes

Every refusal is HTTP 403 (class, level, model, path, destination, provider tool) or 429 (ceiling, volume, rate), with the header `x-init-egress-error: <code>` and a JSON body with a top-level `init` object in ICJ integers. The body is shaped by the route's wire family so the kernel can parse it with its normal path:

```json
{"error":{"type":"init_ceiling_exceeded","code":"ceiling.day",
          "message":"root daily ceiling reached: 5000412 of 5000000 micro-USD; resets 2026-10-20T17:00:00Z"},
 "init":{"source":"root","code":"ceiling.day","limit_musd":5000000,"spent_musd":5000412,"reserved_musd":0,
         "epoch":48113,"resets_at":"2026-10-20T17:00:00Z","retryable":true,"demotes":false,"level":0,"class":"providers"}}
```

`retry-after` is set for `ceiling.day`, `class.volume` and `class.rate`; every other refusal has `retryable: false`. kernel/proxy.md §4.2 maps these to the body's 429 `budget.root_*` and 403 `egress.*` codes and never retries them.

**What demotes** (decision D6; root/root.md §3.13.3 applies it). The egress appends `ceiling_hit{ceiling, code, limit, attempted, demotes, rule, source:"egress", class, level}` through `lineage::append` (`rule` is `a`, `b` or `c`, named in the table; there is no lineage kind `egress`):

| Refusal | `demotes` |
|---|---|
| `class.destination`, `class.method`, `class.path`, `class.unknown`, `class.provider_tool` (an R3 action-class or policy violation; rule a) | **true** |
| `class.level` (a model or class above the level) and `class.model` (a model the class does not list); a level-gated request, rule b | **true** |
| `ceiling.day`, `ceiling.total` when the kernel **bypassed its guard** (below; rule c) | **true** |
| `ceiling.day`, `ceiling.total`, `ceiling.lane` otherwise, including the peak-price gap | false |
| `class.volume`, `class.rate` | false for read-only classes (only `GET`/`HEAD`); **true** for classes with external effects (`git-push`, `email`, non-GET `http`; rule a) |
| `class.disabled` (no key, or a class disabled in safe mode), `class.unmetered` (an owner-listed model route the Root has no metering rule for; 403, a `refuse` ledger line), `upstream.*` | not a hit |

**Guard bypass, defined mechanically.** The kernel proxy sends `x-egress-guard: epoch=<E>` on every metered request, where `E` is the money `epoch` of the Root figure its guard used (`GET /_root/ceilings`, kernel/proxy.md §3.8). The egress keeps a ring of the last 1,024 epochs with their day and total spend. A money refusal counts as a bypass, and demotes, when any of these holds:
1. the header is missing or malformed;
2. `E` has left the ring, or the epoch after `E` was settled more than `egress.guard_stale_s` (30 s, three kernel poll intervals) before this admission;
3. by the Root's spend at epoch `E`, the request's reservation already did not fit under the ceiling, so the guard should have refused it.

Otherwise the guard had current data showing the request fits, and the refusal is metering lag or concurrency: logged, not demoting (answers kernel/proxy.md Q-EG4). Ordinary hits append one `ceiling_hit{demotes:false}` per ceiling or class per owner day and are otherwise counted in the ledger (a `refuse` line). Demoting `ceiling_hit` records are capped at `root.kernel_lineage_per_hour` an hour (root/root.md §3.10.5); beyond the cap the record is not written (`root.rate`, logged) and only the egress usage ledger has the refusal.

**Executed actions.** Model calls and read-only class calls are ledger records only. A request of a class with external effects (`git-push`, `email`, a non-GET `http` class) appends a lineage record `egress_action{class, verb, destination, bytes, outcome}` (a new Root-written kind, root/root.md cross-doc). Lineage records never contain content, URLs with query strings, recipient addresses or key material. An email recipient appears as the first 16 hex of `sha256(address)` (root/root.md §3.10.6 privacy).

### 3.7 Non-HTTP actions: helper verbs, not a tunnel

`git push` to listed remotes and email to listed addresses go through **Root-side helper verbs** (`POST /_root/action`). The Root never offers `CONNECT` or a raw-TCP allowlist. The reasons:
- A tunnel carries opaque bytes. The Root could check only the first destination; it could not bound the content, meter it or record what was sent.
- These credentials are key-equivalent. A tunnel would put them in, or pass them through, the agent zone. A helper keeps them in the Root.
- A helper can check the meaning of the action — which remote and refs, which recipient and size — and its lineage record says exactly that.

**`git-push`.** Class fields: `remotes = {<name> = "<ssh url>"}`, `allowed_refs` (prefix list), `pushes_per_day`, `max_bytes`, `force = false`. Request `{"verb":"git-push","class","remote","refs":[…]}`. The Root pushes from its own copy of the published lineage, never from a tree the zone can write, using the Root's `git` and `ssh` (root/root.md R-X) with the class's deploy key, an empty `HOME` (`ROOT_HOME/cache/git-home`), `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_SSH_COMMAND` naming the key and a Root-owned `known_hosts`, and `--no-verify`. This is the same code path as the lineage mirror (root/root.md §3.11). A GitHub deploy key is scoped to one repository and can be given write access (checked 2026-10-20), so the provider enforces the remote list too.

**`email`.** Class fields: `to` (exact addresses), `from` (fixed by the owner), `smtp = "host:port"`, `emails_per_day`, `max_bytes`, `rate_per_min`. Request `{"verb":"email","class","to","subject","text"}`: plain text only, no attachments, no extra headers. Needs an SMTP client in a later Root release.

Both kinds are rejected by `validate_actions` in the Phase 0 Root (`SUPPORTED_ACTION_KINDS = ["model","http"]`); spec §19 starts R3 with the model providers, and the Phase 0 Root supports only L0, while such classes would normally carry `min_level = 4` (spec §15).

### 3.8 Sealed lanes (decision D5)

Sealed audits and gate-of-gates runs never pass through the kernel proxy or any agent-zone process that persists data (D5; kernel/proxy.md Q-SE1). root/sealed-eval.md launches its reference body and evaluator itself; the egress gives each such run a **sealed lane**:

- `egress::sealed_lane(run_id, socket: PathBuf, budget_musd, level, pool) -> Lane` binds a Unix socket at the path root/sealed-eval.md chooses: `ROOT_HOME/run/sl-<run>.sock`, not inside the run's root, because `<run root>/run/proxy.sock` overflows macOS `sun_path` (104 bytes) for a 64-byte ROOT_HOME (the body sees it as its `INIT_PROXY_SOCK`). `Lane::close()` unbinds it and returns `{spent_musd, requests, last_finish, inflight}`. The socket is removed by its exact path.
- **Same wire as the kernel proxy's live lane** (kernel/proxy.md §4.1), so the body runs unchanged: `POST /<provider_id>/<path>` with `x-init-*` headers read and stripped, plus local `GET /_init/stats` (a minimal object with the fields awareness.md reads; other fields `null`) and `GET /_init/health`.
- **Nothing is recorded but usage.** A sealed lane writes no trace, no request or response content and no lineage record per request. Its ledger records carry only provider, model, usage, cost and the opaque `run_id`, in `ROOT_HOME/egress/usage/`, which the zone cannot read (tail Z1). So traces of S are never written anywhere the agent zone can read (D5). As built (WP24 D6): a `start` record (`id` = `<run>-<n>`, `lane:"sealed"`, `reserved_musd`) is `fsync`ed before each upstream call and the `t:"sealed"` line follows under the same `id`, so a crash keeps the request's cost (charged at its reservation); and Σ sealed spend since install, rebuilt from these lines, plus the holds stays within `sealed.reserve_usd`, else 429 `ceiling.total`. Sealed lines stay out of the zone's day and total. A lane socket serves at most 32 connections at once (503 `root.busy` beyond), with the read and write time bounds of §3.1 (security review 2026-10-20 F3).
- **Same classes and level.** A sealed lane applies the `providers` class and the model list of the level the audited system runs at, so the run behaves as it would live. Other classes are refused on sealed lanes.
- **End of run.** root/sealed-eval.md polls `Lane::status()` (`last_finish`, `inflight`, idle time) to detect the end of a task turn, the same signal kernel/gate.md §3.7.3 uses.

### 3.9 Quotas: enforced by the broker (reconciliation decision)

The original R2 brief put compute, disk and parallel-body quotas here. root/root.md implemented them in the spawn broker and watcher (§3.14), which start and reap every zone process, so the enforcement point and the charter keys (`quotas.*`) are root/root.md's. This doc keeps no duplicate keys. Contributed platform notes, adopted or recorded there:
- **Parallel bodies** are counted as broker instances of kind `body`, both platforms; refusal `root.level{quota:"parallel_bodies"}` demotes (a level-gated request, D6).
- **CPU.** Both platforms: `ru_utime + ru_stime` from `wait4` at each instance's end. For long-lived bodies, Phase 1 may sample live leaders with `proc_pid_rusage(pid, 4 /* RUSAGE_INFO_V4 */, …)` into `rusage_info_v4.ri_user_time`/`ri_system_time` (libc 0.2.191, checked 2026-10-20; the named constant `RUSAGE_INFO_V4` is UNVERIFIED in the crate docs). On Linux a hard cap needs `cpu` delegated to the user manager, which is not the default (only `memory` and `pids` are); `init-root doctor` checks `cgroup.controllers` and prints the `Delegate=cpu` drop-in. Whether `systemd-run --user --scope -p CPUQuota=` takes effect without that delegation is UNVERIFIED.
- **Disk.** `statvfs` floor plus a metadata-only walk (root/root.md R-F2). An optional hard cap on macOS: `INIT_HOME` on its own APFS volume created with `diskutil apfs addVolume <container> APFS INIT -quota <size>` (the flag exists on macOS 14.6; exact enforcement semantics UNVERIFIED). On Linux, a dedicated filesystem or project quota.
- **Live-system safety (CONVENTIONS §1).** Quota enforcement refuses spawns and egress. It never signals processes by pattern; when a zone must stop, R1 stops the instances the broker holds by id.

## 4. Interfaces

### 4.1 Requests on `egress.sock` (kernel → Root)

```
POST /<provider_id>/<upstream path> HTTP/1.1          model class
GET  /x/<class_id>/<host>/<path>    HTTP/1.1          http class (e.g. /x/crates/index.crates.io/se/rd/serde)
POST /_root/action                  HTTP/1.1          helper verbs (§3.7; refused in Phase 0)
GET  /_root/ceilings · /_root/health
headers from the kernel:  content-type, accept, anthropic-version, anthropic-beta, openai-beta, idempotency-key,
                          x-egress-scope: live|gate, x-egress-body: <instance>, x-egress-reservation: <req_id>,
                          x-egress-guard: epoch=<E>,
                          x-egress-caps: <ids> (ADR-011 §5.2), x-egress-subtask: <label> (v0.3.0, a delegate sub-task)
```

- Every other header is dropped, including any auth header. The Root adds the route's auth header from the class, `user-agent: init-root/<version>`, `accept-encoding: identity`, and the OpenAI account headers if the charter's provider entry sets them.
- `x-egress-scope`, `x-egress-body` and `x-egress-reservation` are labels for the ledger only, never inputs to a ceiling or class decision.
- `x-egress-subtask` (v0.3.0) marks a `delegate` sub-task's request. The Root accepts it only in the `x-init-subtask` grammar (≤ 64 bytes, `[a-zA-Z0-9][a-zA-Z0-9._-]*`) and uses it only to count the request apart in the harm sentinel's turn record (`sub_requests`, `sub_cost`: H10 and H5b test the main loop's share, ADR-011 §3.1). It is never a ceiling input and, like every `x-egress-*` header, never reaches the provider.
- Response: the provider's status, allowlisted headers (kernel/proxy.md §4.1) and the streamed body, unchanged, plus `x-egress-req-id`, `x-egress-retries`, `x-egress-reserved-musd` and `x-egress-epoch`. The final cost is not a header (it is known only at the end of a stream); the kernel reads it from `GET /_root/ceilings`, which it re-reads after every completed request (declines kernel/proxy.md Q-EG2b, with reason).

### 4.2 The money block: `ceilings.get` and `GET /_root/ceilings`

root/root.md's `ceilings.get` (§4.1.3) and the published `INIT_HOME/root/ceilings.json` include this block from `egress::ceilings()`. `GET /_root/ceilings` on the egress socket returns the same object, fresh, for the kernel's guard. ICJ: integers, micro-USD.

```json
{"v":1,"epoch":48113,"level":0,
 "money":{"day_limit_musd":5000000,"day_spent_musd":1234567,"day_reserved_musd":0,
          "day_starts_at":"2026-10-19T17:00:00Z","day_resets_at":"2026-10-20T17:00:00Z",
          "total_limit_musd":25000000,"total_spent_musd":4100000,"sealed_reserve_musd":3000000},
 "models":[{"provider":"deepseek","model":"deepseek-flash","min_level":0,
            "input_musd_per_mtok":300000,"output_musd_per_mtok":1200000,"routable":true},
           {"provider":"deepseek","model":"deepseek-v4-pro","min_level":1,
            "input_musd_per_mtok":1320000,"output_musd_per_mtok":3960000,"routable":false}],
 "classes":[{"id":"providers","kind":"model","min_level":0,"enabled":true,
             "requests_today":1290,"requests_limit":100000,"bytes_today":512000000,"bytes_limit":8000000000,
             "rate_tokens":597,"rate_limit":600},
            {"id":"crates","kind":"http","min_level":0,"enabled":true,
             "requests_today":0,"requests_limit":5000,"bytes_today":0,"bytes_limit":2000000000,
             "rate_tokens":300,"rate_limit":300}]}
```

- `models` is the read-only Root price and level table (answers kernel/proxy.md Q-EG2a and Q-EG5). `routable` is `min_level ≤ level`. The kernel builds its routable set from it and warns at start when it would route to a model the Root does not list. The kernel's windowed measurement prices stay in kernel config; they may differ from these peak prices.
- No destinations beyond class ids and provider ids, no credentials, no per-request content. Sealed-lane spend appears only as the aggregate `total_spent_musd`.
- `GET /_root/health` returns `{"level":0,"ok":true,"panicked":false,"test_mode":false,"v":1}`: the level, `ok` (false while panicked), whether the egress is panicked, whether the Root runs in test mode, and the version.

### 4.3 Module API (called by root/root.md and root/sealed-eval.md)

```rust
pub fn start(cfg: &Charter, keys: &Path, lineage: LineageHandle) -> io::Result<Egress>;
impl Egress {
    pub fn panic(&self);                     // at once: refuse new requests (503 root.panicked), shut every kernel connection;
                                             // an in-flight upstream connection closes at its next upstream frame (ureq has no
                                             // cancel handle), not within 100 ms; close sealed lanes; reservations settle in full
    pub fn resume(&self);
    pub fn reload(&self, cfg: &Charter) -> Result<(), Vec<CharterError>>; // keys + classes + money; in-flight keep the old
    pub fn ceilings(&self) -> MoneyBlock;    // §4.2
    pub fn set_level(&self, level: u8, safe_mode: bool); // class/model gating; safe mode = L0 models, `providers` only
    pub fn sealed_lane(&self, run_id: &str, socket: &Path, budget_musd: u64, level: u8, pool: Pool)
        -> io::Result<Lane>;                 // Pool::Sealed (audits, gate-of-gates: total + reserve, no day ceiling)
                                             // Pool::Agent (L3+ sealed-half family evals: the agent's day and total)
}
pub fn validate_actions(t: &toml::Table) -> Vec<CharterError>;  // root/root.md C11
```

A panic stops no upstream request at once, so the spend after it is bounded by one reservation per in-flight request, each charged in full.

`validate_actions` checks the schema of §3.5: unknown keys, unsupported kinds, id grammar, `host:port` form, path grammar, non-negative limits, that every `providers` model has both prices, that at least one model has `min_level = 0`, and that `crates` (if enabled) lists only its two built-in hosts.

### 4.4 Files and CLI

| Path | Mode | Writer | Content |
|---|---|---|---|
| `ROOT_HOME/keys/providers/<id>` | 0600 | owner via `init-root keys set` | one provider key |
| `ROOT_HOME/keys/actions/<class_id>` | 0600 | owner via `init-root keys set --action` | a class credential |
| `ROOT_HOME/egress/usage/<date>.jsonl` | 0600 | egress | §3.4 ledger, no content |
| `ROOT_HOME/egress/totals.json` | 0600 | egress | hourly checkpoint of the lifetime total |

```
init-root keys set [--action] <id>       read a secret from the TTY (no echo), store it, egress reload
init-root keys check                      present / insecure / missing; never prints a secret
init-root egress stats [--day DATE]       owner view of money, classes and lanes
```

## 5. Failure modes and recovery

| Failure | Effect | Recovery |
|---|---|---|
| Root down or restarting | the kernel cannot reach `egress.sock`; it returns 503 `egress.unavailable` | launchd/systemd restarts the Root (root/root.md); counters are rebuilt from the ledger before the socket is bound |
| Ledger cannot be written (disk) | the egress cannot record a `start` | **fail closed**: every request is refused 503 `root.ledger` until a write succeeds; alert |
| Key missing or insecure | that provider is disabled | `class.disabled` (not a ceiling hit); `init-root keys set` |
| Provider 401/403 | the key is rejected | body withheld, `upstream.auth`, alert |
| Money ceiling reached | refusals `ceiling.*` | the day resets; a total needs a charter edit. Demotion only for a guard bypass (§3.6) |
| Ceiling lowered below today's spend | requests refused until reset | nothing refuses to start (D6); the kernel clamps its soft budgets |
| Price table stale (provider raised prices) | the Root under-counts | peak prices and the one-token-per-byte input reservation leave margin (cache reads are billed at their own peak price since the provisional owner decision 2026-10-20, §3.4); the owner's prepaid balance is the outermost cap; `init-root egress stats` shows the provider balance where a free balance route exists (DeepSeek `user/balance`) |
| Connection flood on `egress.sock` (a mis-evolved kernel) | beyond 96 open connections each new one gets 503 `root.busy`; held or trickling connections time out (120 s idle, 30 s per request) | nothing to do: `owner.sock`, panic and the Root's other threads keep working (F3); panic closes every held connection |
| Sealed lane left open (Root crash during an audit) | a socket file remains in `ROOT_HOME/run` | root/sealed-eval.md deletes listed run roots by exact path at Root start (`sealed::clean_stale` from the supervisor's start, WP24 D8; it signals nothing) |

## 6. Performance notes (spec §9)

- **Rank 1, prompt cache.** Bytes are forwarded unchanged, so the body's byte-stable prefix reaches the provider intact.
- **Rank 2, round trips.** The provider keep-alive pool lives in the Root, which outlives body and kernel restarts, so a restart still pays no TLS handshake. The blocking client speaks HTTP/1.1 only (UNVERIFIED that ureq 3.4.2 has no h2; it does not document it): each concurrent stream uses its own pooled connection instead of h2 multiplexing. That costs one TLS handshake per extra concurrent stream when the pool is cold, and nothing once it is warm.
- **Added latency.** One Unix-socket hop, a minimal JSON parse of three fields, and one `fsync` of the `start` record before forwarding (one `fsync` per record, no group commit). Target: ≤ 3 ms p50 added before the upstream request; ≤ 20 µs per forwarded frame.
- **Size.** The budget is ≤ 330 lines (root/root.md §4.8):

| Part | Lines |
|---|---|
| socket server and HTTP/1.1 framing (httparse), chunked relay | 60 |
| classes: schema, `validate_actions`, matching, level and model gating, tool check | 70 |
| money: reservations, ceilings, epochs, guard check, ledger, rebuild | 70 |
| upstream client (ureq agent per destination), key injection, connect retries | 40 |
| usage and finish tap, three families (kernel/proxy.md §3.10.2) | 50 |
| refusals, `ceiling_hit` appends, money block, sealed lanes, panic and reload | 40 |
| **total** | **330** |

**As built, with the budget that replaces the table above.** This is a provisional owner decision of 2026-10-20 that the owner may override (build-deviations.md §3.1, root/root.md §4.8). The egress measures 1,681 lines at cdb49e8 by root.md §4.8's method (1,649 at dd5c358). Its sub-budgets are:
- `egress/money.rs`: 270 measured, budget 290. It is part of the Root's hand-reviewed core.
- the rest (`egress.rs`, `classes.rs`, `http.rs`, `lane.rs`, `tap.rs`): 1,411 measured, budget 1,490.

The table above is the v0.3 design estimate.

**Dependencies** (X6; versions from docs.rs, 2026-10-20):

| Crate | Version | Features | Why |
|---|---|---|---|
| `ureq` | 3.4.2 | `default-features = false`, `features = ["rustls"]` (no gzip: the stream must be relayed byte-for-byte) | blocking HTTPS client with a keep-alive pool; no async runtime; rustls with the `ring` provider by default |
| `rustls` (via ureq) | as resolved by ureq 3.4.2 | `ring` | TLS; `ring` avoids aws-lc-rs's C/CMake build (kernel/proxy.md §3.7 reasoning) |
| `httparse` | 1.10.1 | — | parses the kernel's HTTP/1.1 request head on the Unix socket; no runtime dependencies |

Rejected: `tokio` + `hyper` + `hyper-rustls` (the v0.2 kernel stack). It gives h2 multiplexing, but adds an async runtime the Root core does not otherwise need and does not fit 330 lines. ureq's redirect setting (`max_redirects(0)`) is UNVERIFIED by name in 3.4.2; the Root also refuses any 3xx by class rule, so redirects are never followed either way.

## 7. Invariants and security (spec §2.1)

| Invariant | How the egress keeps it |
|---|---|
| #3 Keys never in the sandbox | keys live in `ROOT_HOME/keys/`, which the zone cannot even stat (tail Z1); auth is injected below the kernel |
| #3 Spend the agent cannot bypass | hard ceilings and reservations in the Root; the sealed reserve stops the zone from spending the auditor's money |
| #3 External actions bounded | exact destinations, methods and paths per class; provider-executed tools refused; non-HTTP actions only through helpers; no tunnel |
| #2 Honest measuring stick | sealed lanes record no content and bypass the kernel (D5) |
| #4 Lineage | `ceiling_hit` and `egress_action` records, with no content, addresses or keys |
| #5 Human where irreversible | set once in the charter; no per-action approval |

## 8. Test plan (new IDs, proposed to testing.md; mock providers only, no live-system signals)

| # | Case | Expectation |
|---|---|---|
| EG-01 | kernel → egress → mock provider; a body-sent `authorization` header | dropped; the key comes from the Root; response bytes identical |
| EG-02 | 20 concurrent requests whose reservations exceed the day ceiling | admitted spend never exceeds the ceiling; the rest get 429 `ceiling.day`, `retry-after` |
| EG-03 | guard bypass rules 1–3 against an ordinary hit (fresh epoch, fits) | rules 1–3 → `ceiling_hit{demotes:true}`; ordinary → `demotes:false` |
| EG-04 | `ceiling.total` with the sealed reserve | the zone is refused at `total − reserve`; a sealed lane is still admitted up to `total` |
| EG-05 | ceiling lowered below today's spend, then `reload` | refusals until reset; nothing exits; `ceilings.get` shows the new limit |
| EG-06 | Root restart mid-day (test Root in a scratch `ROOT_HOME`) | counters rebuilt from the ledger before the socket binds |
| EG-07 | usage tap on kernel/proxy.md P-04/P-05/P-06 fixtures | Root `cost_musd` equals the hand computation at peak prices |
| EG-08 | connect failure vs a 503 from the mock | connect → ≤ 2 Root retries; 503 → passed through, no Root retry |
| EG-09 | upstream 401 with a key fragment in the body | body withheld, `upstream.auth` |
| EG-10 (F2) | a listed billable route with no metering rule (`embeddings`, `completions`, `GET chat/completions`); `GET user/balance`; a completion | 403 `class.unmetered`, never forwarded, not a hit; the balance read is forwarded with the key at `reserved_musd: 0`, cost 0; the completion is reserved and metered (unit `every_forwarded_model_route_is_metered_or_free`, `unmeterable_routes_are_refused`; `tests/rsec.rs`) |
| EG-11 (F3, F7) | 136 connections held with a partial request head | 96 served, 40 answered 503 `root.busy` with no new thread; `/_root/health` is `root.busy` too; `owner.sock` answers and panic closes every held connection; an idle or trickling read times out (unit `connection_reads_are_bounded_in_time`, `local_answers_are_behind_the_inflight_bound`; `tests/rsec.rs`) |
| R3-01 | path, method, destination and unknown-class refusals | 403 with the right code; `ceiling_hit{demotes:true}` |
| R3-02 | model above the level (`deepseek-v4-pro` at L0) | 403 `class.level`, demotes |
| R3-03 | a request with a provider-executed tool, or `mcp_servers` | 403 `class.provider_tool` |
| R3-04 | `crates`: a sparse-index GET and a `.crate` GET; a POST; another host | allowed; 403 `class.method`; 403 `class.destination` |
| R3-05 | `crates` volume and rate exhausted | 429, `demotes:false` (read-only class) |
| R3-06 | a `git-push` or `email` class in the Phase 0 charter | `validate_actions` → `actions.unsupported_kind` |
| R3-07 | safe mode | only `providers` with the L0 model list |
| SL-01 | sealed lane: a body run against a mock provider | `/_init/stats` answered; no trace or content anywhere; ledger usage only; `close()` reports spend and finish |
| A6+ | from inside the body sandbox, connect `egress.sock` | refused by the OS (only the `kernel` profile lists it) |

## 9. Open questions

**Within this doc**
1. Should `crates` limits scale with the vendor quota (kernel/initctl.md: 20 jobs per day)? 5,000 requests per day covers about 20 jobs of ~250 index and crate fetches each; revisit with real vendor data.
2. h2 for the Root's client. Not needed at ≤ 64 streams; revisit if a provider limits concurrent connections.

**Cross-doc**
- **root/root.md:** add the Root-written lineage kind `egress_action`; accept `ceiling_hit{source:"egress"}` with the `demotes` values of §3.6; `ceilings.get` embeds `egress::ceilings()`; call `egress::set_level` on every level and mode change; tail composition for sealed run roots (root/sealed-eval.md); `keys/actions/` beside `keys/providers/`.
- **kernel/proxy.md:** send `x-egress-guard: epoch=<E>` with the epoch its guard used; read `models` from `GET /_root/ceilings` for the routable set and warn on missing models; map `class.provider_tool`, `class.model`, `class.level` and `ceiling.lane` (the last only on sealed lanes, never seen by the kernel).
- **kernel/initctl.md:** `vendor` fetches through `/x/crates/<host>/<path>` and builds a cargo `local-registry` (or a `directory` source after `cargo vendor` offline); cargo cannot reach a Unix socket.
- **testing.md:** a mock egress in `init-testkit` (`/_root/ceilings`, typed refusals, epochs, a peak-priced counter), as kernel/proxy.md Q-TS1 asks.
- **CONVENTIONS.md §3, §5:** the `vault/keys/` row and the "Keys" global decision move to `ROOT_HOME/keys/providers/` and `init-root keys set`; the TLS crates move from the kernel to `init-root` as listed in §6.

## 10. Spec deltas

| Spec v0.3 | This doc | Why |
|---|---|---|
| §14 R2 "hard money ceilings (per day and total)" | integer micro-USD with reservations; a sealed reserve inside the total | the backstop is precise, and the zone cannot starve the audit |
| §14 R2 "compute and disk quotas … counts parallel bodies" | enforced by the Root broker (root/root.md §3.14), not the egress | the broker starts and reaps every zone process |
| §15 "an attempt to exceed a ceiling or a policy class" | class and level violations demote; a money refusal demotes only when the kernel bypassed its guard (D6) | metering lag and peak-price estimates must not demote |
| §14 R3 "git push … email" | helper verbs; no CONNECT; unsupported in the Phase 0 Root | a tunnel cannot be bounded or logged |
| §19 "R3 starts with egress limited to the model providers" | plus the built-in read-only `crates` class (D10) | crate adds are an L0 right (ESC-1) |
| (new) | provider-executed tools refused | they reach the network outside every class |
| (new) | sealed lanes for root/sealed-eval.md (D5) | sealed runs must not pass through the kernel |

**Build sync 2026-10-20** (build → docs; ledger reconciliation/build-deviations.md §1.4 and §1.3):

| Build ref | This doc now says | Section |
|---|---|---|
| build: WP22 D2, D3 | test mode loads only `test-` keys (others refused, alert `key_file_real`); loopback destinations are allowed in test mode only, `crates` included, and a test Root never forwards off-host | §3.2, §3.5 (`destinations`) |
| build: WP22 D4 | the total is checked before the day; a cost counts toward the day it was admitted; a non-2xx answer costs 0 and a non-connect transport error costs the reservation; rounding happens once over the sum; `start` records are fsynced one by one (no group commit) and `end` records are not synced on their own | §3.4, §6 |
| build: WP22 D5 | `panic` shuts kernel connections at once; an in-flight upstream connection closes at its next upstream frame, not within 100 ms, and is charged its full reservation, so the spend bound is one reservation per in-flight request | §4.3 |
| build: WP22 D6, D7, D9 | an unlisted model (`class.model`) demotes under rule b; safe-mode refusals are `class.disabled` and not a hit; there is no `egress` lineage kind (only `ceiling_hit{…, code, rule, class, level}` and `egress_action`, plus a `refuse` ledger line); ordinary hits are recorded once per ceiling or class per owner day; `/_root/health` returns `{level, ok, panicked, test_mode, v}` | §3.6, §4.2 |
| build: WP23 D4 | demoting `ceiling_hit` records are capped at `root.kernel_lineage_per_hour`; beyond the cap only the egress usage ledger has them | §3.6 |
| build: WP24 D5 | the sealed lane socket is `ROOT_HOME/run/sl-<run>.sock`, because `<run root>/run/proxy.sock` overflows `sun_path` for a 64-byte ROOT_HOME | §3.8, §5 |

## 11. References

All checked 2026-10-20.
- Spec v0.3 §13–§16, §19; ADR-007; reconciliation/v03-decisions.md (D5, D6, D10, D13).
- root/root.md §3.2, §3.3, §3.10.5, §3.12, §3.13.3–§3.14, §4.1.3, §4.8; kernel/proxy.md §3.5, §3.8, §3.9, §3.10.2, §3.12, §4.1, §4.2; kernel/initctl.md `vendor`.
- crates.io sparse index `config.json` (`"dl": "https://static.crates.io/crates"`) — https://index.crates.io/config.json ; index layout — https://doc.rust-lang.org/cargo/reference/registry-index.html ; `local-registry` and `directory` sources — https://doc.rust-lang.org/cargo/reference/source-replacement.html
- `ureq` 3.4.2 (blocking, keep-alive pool, rustls with `ring` by default, gzip a default feature) — https://docs.rs/ureq/latest/ureq/ ; `httparse` 1.10.1 — https://docs.rs/httparse/latest/httparse/
- `libc` 0.2.191 `proc_pid_rusage`, `rusage_info_v4` — https://docs.rs/libc/latest/aarch64-apple-darwin/libc/fn.proc_pid_rusage.html , https://docs.rs/libc/latest/aarch64-apple-darwin/libc/struct.rusage_info_v4.html
- systemd user delegation (`memory`, `pids` by default; `cpu` needs a drop-in) — https://wiki.archlinux.org/title/Control_groups
- macOS 14.6 `diskutil apfs addVolume … -quota` (local usage text)
- GitHub deploy keys (one repository, optional write) — https://docs.github.com/en/authentication/connecting-to-github-with-ssh/managing-deploy-keys
- Anthropic MCP connector (`mcp_servers`) — https://docs.anthropic.com/en/docs/agents-and-tools/mcp-connector ; OpenAI remote MCP tool — https://developers.openai.com/api/docs/guides/tools-remote-mcp