# Providers v2: codecs, credentials, egress classes, model catalog and pricing

```
Status: draft v0.1 · 2026-10-20 · Area: overview (cross-area design for root, body and kernel; lane DES-PROV)
Depends on: V1_PLAN.md (mandate item 3, the $5 DeepSeek cap); CONVENTIONS.md; body/provider.md;
            body/log-context.md; root/egress.md; root/root.md; kernel/proxy.md; kernel/config.md;
            kernel/sandbox.md; tui.md; research_notes/INIT v1 providers UX vision/{llm_api_providers,
            proxies_gateways_local, subscription_plans, install_daily_ux}.md (all 2026-10-20)
Defines:    (each until PV-00 folds it into its owner doc, §3.10) the model catalog format v1 and its
            price-resolution semantics; the "unpriced model ⇒ refuse" rule; the signed catalog update;
            the Root credential store and auth kinds; the supported-routes verdict table; provider
            registry v2 (route templates, gateway, LOCAL and custom classes, per-codec request policy);
            the codec set, the dialect (quirk) profile schema and the reasoning-replay table; the
            owner.sock requests keys.*, login.*, logout, models.list, model.select, providers.list,
            catalog.status; the owner-selected default model; the PV-* work packages
```

---

## 1. Purpose

V1_PLAN.md item 3 asks INIT to support every current LLM provider API, proxy or gateway, local LLM server and every subscription plan the terms allow, with current models and prices for a year after release. Today INIT speaks three wire formats, routes three hard-coded providers, prices two DeepSeek models in the charter, and keeps keys in 0600 files. This doc is the buildable design that closes the gap without moving the trust boundary:

- **Body (agent zone, evolvable):** a small set of *codecs* (one per wire family) plus data-driven *dialect profiles* for the long tail of "OpenAI-compatible" and "Anthropic-compatible" endpoints (§3.3).
- **Root (owner zone, immutable to the agent):** every credential and every auth flow (§3.4); the provider registry, host and path allowlists and request policy (§3.5); and the **model catalog with prices**, which the Root alone meters with (§3.2).
- **Kernel (agent zone, evolvable from L3):** routing and measurement read the Root's published views instead of hard-coded presets (§3.6).

The split follows one rule. What changes monthly (models, prices, promotions) is **data in a Root-owned, signed catalog**. What changes yearly (wire formats) is **code in the body**, where the agent can improve it through the gate. Anything that touches money or credentials is **in the Root**, where the agent can never reach it.

## 2. Responsibilities / non-goals

**Responsibilities**
- The catalog: schema, validation, compilation into integer prices, price resolution (windows, holiday calendars, long-context tiers, cache-write TTL classes, geo and service-tier multipliers, scheduled changes, promotions), the signed owner-run update, and the published read-only view (§3.2).
- The codec layer: OpenAI Responses, Anthropic Messages, Google Gemini (generateContent first, Interactions second), one Chat Completions codec with dialect profiles, optional Bedrock Converse; reasoning normalization and replay; tool-call quirks; streaming usage parsing (§3.3).
- The auth layer: auth kinds, the OS-keychain credential store with a 0600-file fallback, the owner UX (CLI and TUI), OAuth PKCE for ChatGPT plan usage, cloud auth (SigV4, Google, Azure), honest client identity, and the verdict table that decides which routes ship (§3.4).
- Egress classes v2: provider registry, host and (method, path) allowlists with metering rules on top of the F2 default-deny, the gateway class, the LOCAL class, custom base-URL providers (§3.5).
- Routing and selection hooks: defaults per provider, the `/model` picker, L1 per-subtask routing inputs (§3.7).
- Migration from today's code and the work breakdown (§3.9, §3.10).

**Non-goals**
- **Rejected for v1: modes that replace INIT's body loop.** "Run the user's own Claude Code binary" (the unmodified `claude` CLI or the Agent SDK under the user's Claude login), the GitHub Copilot SDK (its own agent runtime on the user's Copilot allowance) and the Cursor SDK each run *another vendor's* agent loop. INIT's self-evolving body would no longer own the loop, the context, the tools or the traces, so the gate, the measuring stick, the evolution tiers and the Root's per-request metering would not apply to the work done. They are rejected on that ground, independent of their terms-of-service status (§3.4.7: the Claude binary mode is also GRAY; the Copilot SDK itself is ALLOWED).
- **Non-chat endpoints.** Embeddings, images, audio, realtime, batch, files, fine-tuning, assistants: no metering rule, so the F2 default-deny keeps refusing them (`class.unmetered`).
- **Provider-executed tools and server-side state.** Web search, code execution, MCP connectors, `previous_response_id`, Interactions `store:true`, explicit context caches and server-side fallbacks stay refused (§3.5.3). Each one reaches the network or bills outside every class.
- **OpenAI WebSocket mode** (kernel/proxy.md §3.15 stays off).
- **Automatic network price updates.** The catalog changes only when the owner runs `init-root catalog update` (§3.2.6).
- **LAN model servers over plain HTTP.** LOCAL means loopback only; a server on another host is a custom provider and needs `https` (§3.5.6).
- **First-run wizard screens.** The onboarding lane owns them; this doc gives the Root requests they call (§4.4).

## 3. Design

### 3.1 Overview and data flow

```
 body (zone)                        kernel proxy (zone)              Root (owner uid, unsandboxed)                 upstream
 ┌──────────────────────────┐       ┌───────────────────────┐        ┌───────────────────────────────────────┐
 │ codec(api, dialect)      │ unix  │ route from registry   │ unix   │ registry v2 = catalog ∩ charter        │ TLS  api.*, gateways
 │  render ─ decode ─ replay│──────▶│ view; L0/L1 policy;   │───────▶│ class → host → (method,path) → policy  │────▶ cloud (SigV4, Google,
 │ ModelInfo ← catalog view │       │ soft budgets; traces; │        │ → level → volume → money (reserve)     │       Entra, OAuth)
 │ (read-only)              │       │ measurement prices ←  │        │ auth: keystore → inject header / sign  │ HTTP loopback only:
 └──────────────────────────┘       │ catalog view          │        │ tap: usage parser per codec → settle   │────▶ 127.0.0.1:<port>
            ▲                       └───────────────────────┘        │ catalog (signed, Root-owned) → prices  │       (LOCAL class)
            │ INIT_HOME/root/catalog.json, ceilings.json, model.json (Root-written, zone read-only, tail Z4)       │
            └──────────────────────────────────────────────────────── published views ◀─────────────────────────────┘
```

Three rules hold everywhere:
1. **The wire stays byte-transparent.** The body renders the provider's native bytes; the kernel and the Root never rewrite a body. The Root *checks* what it must (model, limits, gateway routing fields, tools), refuses on a mismatch, and injects only auth headers or a signature (§3.5.3).
2. **The Root meters from its own catalog.** No price, route or host comes from the agent zone. The kernel's prices are for measurement only (kernel/proxy.md §3.10.3).
3. **Unpriced ⇒ refuse.** A billed request for a model without a resolvable price is never forwarded (§3.2.5).

### 3.2 Model catalog and pricing (Root-owned)

#### 3.2.1 Files and ownership

| Path | Writer | Zone access | Content |
|---|---|---|---|
| `root/catalog/models.toml` (repo) | maintainer, through PV-02 and the verification lane PV-03 | — | the catalog source; embedded in `init-root` at build (`include_str!`) |
| release assets `catalog-<version>.toml`, `catalog-<version>.toml.sig` | `scripts/release.sh` | — | the same file, plus a detached Ed25519 signature (§3.2.6) |
| `ROOT_HOME/catalog/active.toml` | `init-root catalog update` (owner) | none (Z1) | the owner-applied catalog; absent ⇒ the embedded one is active |
| `ROOT_HOME/catalog/prev/<version>.toml` | the same | none | the last 5 replaced catalogs, for `catalog update --rollback` |
| `ROOT_HOME/catalog/floors.json` | Root | none | last metered price per (row, window, tier, class) and pending decreases (§3.2.6) |
| `INIT_HOME/root/catalog.json` | Root | read-only (Z4) | the published view (§4.3): models, capabilities, dialects, route templates and current prices; no hosts, params or credentials |

**Security rule.** The catalog the Root meters with is Root-owned: shipped inside the Root release, replaceable only by the owner, never read from any path the zone can write and never overridable by a kernel config, a body file, a request header or a published view. The agent could otherwise lower a price and spend past its ceilings. The body and kernel hold read-only copies for routing and measurement; nothing they do with those copies changes what the Root charges.

#### 3.2.2 Schema

One TOML 1.1 file, parsed with `toml` 1.1.8 into `serde` structs with `deny_unknown_fields`. Money values are decimal strings in USD per million tokens (MTok); they compile to integer **micro-USD per MTok** (µUSD/MTok), the unit the Root already uses (`input_musd_per_mtok`). With at most 6 decimals every price is an exact integer.

```toml
schema = 1
version = "2026.10.20.1"            # YYYY.MM.DD.n; strictly increasing (CAT-2)
verified_through = "2026-10-20"     # the oldest verified_date of any enabled row

# ── calendars and schedules ───────────────────────────────────────────────────────────────
[calendars.cn-public-holidays]
tz = "+08:00"                       # a "date" is a calendar date at this offset
known_through = ""                  # last date the lists below are complete for; "" = nothing known yet
dates = []                          # public holidays; PV-03 fills them from the State Council notice
workdays = []                       # make-up working days (weekend days declared working days)
source_url = "https://www.gov.cn/"  # PV-03 replaces with the notice URL
verified_date = ""

[schedules.deepseek]
tz = "+00:00"
default = "offpeak"
windows = [ { days = "mon-fri", from = "01:00", to = "04:00", use = "peak" },
            { days = "mon-fri", from = "06:00", to = "10:00", use = "peak" } ]
holiday_window = "offpeak"          # the provider's official rule (kernel measurement only)
dearest = "peak"                    # the window used by peak metering and every reservation
cheap = "offpeak"                   # the only window the conservative Root meter may grant (§3.2.4)
margin_s = 900                      # conservative meter: every cheap span shrinks by 15 min at each end
doubt_calendar = "cn-public-holidays"   # dates on which the Root never grants the cheap window
source_url = "https://api-docs.deepseek.com/quick_start/pricing/"
verified_date = "2026-10-20"

# ── providers ─────────────────────────────────────────────────────────────────────────────
[providers.deepseek]
display = "DeepSeek"
kind = "api"                        # api | gateway | cloud | plan | local-template
host = "api.deepseek.com:443"       # exact; templated hosts use {params} (§3.5.1)
base_path = ""                      # Root-side prefix, never seen by the zone
auth = ["bearer"]                   # allowed auth kinds (§3.4.1)
key_hint = "sk-"                    # UX hint and line-guard prefix (§3.4.3)
dialect = "deepseek"                # default Chat Completions dialect (§3.3.6)
default_model = "deepseek-flash"
tos = "allowed"                     # verdict (§3.4.7); anything else is refused at compile
routes = [                          # zone-facing paths, relative to host + base_path (§3.5.2)
  { method = "POST", path = "chat/completions",       api = "chat_completions", meter = "billed" },
  { method = "POST", path = "beta/chat/completions",  api = "chat_completions", meter = "billed" },
  { method = "POST", path = "anthropic/v1/messages",  api = "messages",  meter = "billed", dialect = "anthropic-compat" },
  { method = "POST", path = "responses",              api = "responses", meter = "billed", dialect = "responses-compat" },
  { method = "GET",  path = "models",                 meter = "free" },
  { method = "GET",  path = "user/balance",           meter = "free", balance = true },
]
source_url = "https://api-docs.deepseek.com/"
verified_date = "2026-10-20"

# ── models ────────────────────────────────────────────────────────────────────────────────
[models."deepseek/deepseek-flash"]  # key = "<provider>/<model id>"; the id may itself contain "/"
served_as = "DeepSeek-V4.1-Flash-1017"
status = "current"                  # current | legacy | preview | deprecated
context = 1_000_000
max_out = 393_216
default_max_tokens = 65_536
apis = ["chat_completions", "messages", "responses"]   # which of the provider's routes may carry this model
tools = true
forced_tool_choice = false
parallel_tools = true
images = true
reasoning = "default_on"            # none | optional | default_on | always
replay = "mandatory_with_tools"     # §3.3.8
edit_rule = "free"                  # free | closed_round (§3.3.12)
effort = { low = "low", medium = "high", high = "high", xhigh = "high", max = "max" }
default_effort = "high"
caching = "automatic"               # none | automatic | anthropic | openai_explicit | gemini_implicit | local_prefix
min_cache_tokens = 0
tier = "cheap"                      # frontier | strong | cheap | local (routing hint, §3.7)
billing = "tokens"                  # tokens | plan | local
confidence = "official"             # official | docs | 3p (§3.2.8)
wire_verified = "live"              # live | fixture | docs (§3.10, PV-19)
source_url = "https://api-docs.deepseek.com/quick_start/pricing/"
verified_date = "2026-10-20"

[models."deepseek/deepseek-flash".price]
schedule = "deepseek"
reasoning = "output"                # reasoning tokens are billed as output (every provider researched)
[models."deepseek/deepseek-flash".price.windows.peak]
input = "0.30"
cached_input = "0.006"
output = "1.20"
[models."deepseek/deepseek-flash".price.windows.offpeak]
input = "0.15"
cached_input = "0.003"
output = "0.60"
```

Further price features, shown on the rows that need them:

```toml
[models."anthropic/claude-haiku-5-5".price]
input = "0.10"
cached_input = "0.01"
cache_write = { "5m" = "0.125", "1h" = "0.20" }    # TTL classes; the usage parser reports tokens per class
output = "0.50"
multipliers = { "inference_geo:us" = "1.10", "speed:fast" = "UNPRICED" }   # "UNPRICED" ⇒ requests setting it are refused
[[models."anthropic/claude-haiku-5-5".price.tiers]]
above_prompt_tokens = 100_000       # strictly greater than
measure = "total_prompt"            # total_prompt (uncached + cache read + cache write) | uncached
whole_request = true                # the tier reprices every token of the request, cache reads included
input = "0.50"
cached_input = "0.05"
cache_write = { "5m" = "0.625", "1h" = "1.00" }
output = "2.50"

[models."google/gemini-4-argon".price]
input = "2.00"
cached_input = "0.10"               # implicit caching only
output = "10.00"
[[models."google/gemini-4-argon".price.changes]]
effective = "2026-12-01"            # a date with no time zone: §3.2.4 step 1
input = "4.00"
cached_input = "0.20"
output = "20.00"

[models."openai/gpt-5.6-sol".price]
input = "4.00"
cached_input = "0.40"
cache_write = { "30m" = "5.00" }
output = "20.00"
valid_until = "2026-11-21"          # a promotion "at least through" this date; after it the row is unpriced
```

| Key | Rule |
|---|---|
| `price.windows.<w>` | one price block per schedule window; a model with a `schedule` must price every window the schedule can yield |
| `price.cache_write` | map TTL → price: `"5m"`, `"1h"` (Anthropic, Kimi), `"30m"` (OpenAI GPT-5.6+). Absent ⇒ writes bill at `input` |
| `price.tiers` | long-context repricing; `above_prompt_tokens` strictly increasing; a tier may carry its own `windows` |
| `price.multipliers` | `"<field>:<value>" = "<factor>"` for service tiers, speed and geo (`service_tier:flex`, `service_tier:priority`, `speed:fast`, `inference_geo:us`); `"UNPRICED"` marks a value the Root must refuse; the neutral values `default`, `auto`, `standard`, `global` price at ×1 |
| `price.geo` | provider-level: `regional_endpoint = "1.10"` for Bedrock and Vertex regional endpoints, `us_data_zone = "1.10"` for Foundry; selected by the charter's endpoint params (§3.5.1) |
| `price.changes` | dated replacement price blocks, strictly increasing `effective` |
| `price.valid_until` | last day a promotional price is guaranteed; after it the row is unpriced (§3.2.5) |
| `price.from` | `"<provider>/<model>"`: copy another row's price (gateways reselling at list price, Azure deployments) |
| `price.account_fee_permille` | provider-level fee on top of every request, e.g. OpenRouter's 5.5% credit purchase fee = 55 |
| `billing = "plan"` | subscription route: money cost 0, plan counters instead; `shadow_price = "<provider>/<model>"` gives the API list price used for `cost_ref` (§3.4.4) |
| `billing = "local"` | loopback server: money cost 0; registered by the owner, never by the catalog (§3.5.5) |

#### 3.2.3 Validation (CAT rules)

`catalog::load(bytes) -> Result<Catalog, Vec<CatalogError>>` reports every error with its TOML key path. `init-root catalog check <file>` runs it; CI runs it on `root/catalog/models.toml`.

| Rule | Check |
|---|---|
| CAT-1 | `schema == 1`; unknown keys are errors |
| CAT-2 | `version` matches `^[0-9]{4}\.[0-9]{2}\.[0-9]{2}\.[0-9]{1,3}$`; an update must be strictly greater than the active version unless `--allow-downgrade` |
| CAT-3 | money strings match `^[0-9]{1,5}(\.[0-9]{1,6})?$` (or `"UNPRICED"` in `multipliers`); factors match `^[0-9]{1,2}(\.[0-9]{1,4})?$` |
| CAT-4 | every `billing = "tokens"` model has `input` and `output` in every reachable window; `cached_input ≤ input` and `cache_write ≥ input` (warnings otherwise, because a provider may really price that way) |
| CAT-5 | tiers: thresholds strictly increasing; each tier complete; a tier cheaper than its base is a warning |
| CAT-6 | schedules: `tz` matches `^[+-](0[0-9]\|1[0-4]):[0-5][0-9]$`; `days` ∈ `mon-fri`, `sat-sun`, `mon`…`sun`, `all`; `from < to` within one day; `dearest` names the window with the highest output price; `cheap`, when present, names another window, and `margin_s` ≥ 900 |
| CAT-7 | calendars: `dates` and `workdays` are `YYYY-MM-DD`, sorted, unique, disjoint, none after `known_through` |
| CAT-8 | `changes[].effective` strictly increasing; each block complete |
| CAT-9 | `valid_until ≥ verified_date` |
| CAT-10 | every model's `apis` is a subset of its provider's billed route APIs; every billed route names a usage parser this Root build has (§3.5.7) |
| CAT-11 | `source_url` is `https://`; `verified_date` is a date or `""`; `confidence` ∈ `official`, `docs`, `3p`; a `3p` row, or one with `verified_date = ""`, is **disabled** unless the charter lists it in `catalog.accept_unverified` |
| CAT-12 | routes: method uppercase; path grammar of §3.5.2; `meter` ∈ `billed`, `free`; no route matches the ToS denylist (§3.4.7) |
| CAT-13 | `host` is `name:port` or a template whose `{params}` are declared in the provider's `params`; `https` only, except `kind = "local-template"` |
| CAT-14 | model ids match `^[A-Za-z0-9._:@/+-]{1,128}$`; the key is `<provider>/<id>` with a declared provider |
| CAT-15 | `default_max_tokens ≤ max_out`; `default_effort` is a key of `effort`; `effort` values are strings or `false` (not sent) |
| CAT-16 | `tos` is `allowed` for every provider; `gray` and `prohibited` rows may not exist in a shipped catalog |

#### 3.2.4 The complete Root money formula (replaces root/egress.md §3.4's cost line)

One function, in the Root core (`catalog/price.rs`, counted with `egress/money.rs` in the hand-reviewed core, root/root.md §4.8). The kernel mirrors it for measurement from the view, with the provider's official window rule instead of the conservative one.

**Inputs.** The catalog row (after owner overrides and decrease floors, §3.2.6); the request interval `[t_admit, t_end]`; the provider-reported usage split into classes; the modifiers the request set and the response reported; the endpoint's charter params.

**Token classes** (from the usage parsers, §3.3.10):

| Class | Tokens | Price key |
|---|---|---|
| `u` | uncached input | `input` |
| `r` | cache reads | `cached_input` |
| `w5` · `w30` · `w60` | cache writes, 5-minute · 30-minute · 1-hour TTL | `cache_write."5m"` · `."30m"` · `."1h"` (absent ⇒ `input`) |
| `o` | output, reasoning included (Gemini: candidates + thoughts) | `output` |

**Steps.**

```text
cost(row, [t_admit, t_end], usage, mods, endpoint) -> Result<musd, Unpriced>
  1 change    base = row.price, replaced by the last change with effective_at(change) ≤ t_admit, and by
              any change that RAISES a price with effective_at ≤ t_end (an increase inside the request
              applies to it; a decrease applies only from the next request and only if approved, §3.2.6).
              Dated changes without a time: an increase is effective at date 00:00 UTC+14 (the earliest
              instant on Earth), a decrease at date 00:00 UTC−12 (the latest).
  2 promotion if row.valid_until and t_end > (valid_until 23:59:59 at UTC+14): Unpriced(promotion_ended)
  3 window    w = dearest, unless every condition of the conservative rule below holds, then w = cheap
  4 tier      P = Σ(u, r, w5, w30, w60) (or u alone for measure = "uncached");
              the highest tier with P > above_prompt_tokens reprices EVERY class of the request
              (whole_request = true; every researched tier is whole-request)
  5 prices    p[c] = block(base, w, tier)[c] in µUSD/MTok
  6 factor    F = ⌈ Π f ⌉ in ppm over: each request modifier, each response-reported modifier (a reported
              value overrides the requested one; unlisted ⇒ the row's largest factor, flag
              unpriced_modifier), the endpoint factor (regional_endpoint, us_data_zone, data_residency),
              and (1 + account_fee_permille/1000). Each product step rounds up to whole ppm.
              A request modifier marked UNPRICED or unlisted ⇒ refused at admission (§3.2.5).
  7 cost      cost_musd = ⌈ Σ_c tokens[c] × ⌈p[c] × F / 10^6⌉ / 10^6 ⌉     (one final rounding, as today)
```

- **Modifiers covered:** service tier (`service_tier:flex` ×0.5, `service_tier:priority`, `speed:fast` ×2, OpenAI `ultrafast` as its own price block), geo (`inference_geo:us` ×1.1 for Claude 4.6+; Bedrock and Vertex regional endpoints +10%; Foundry US Data Zone ×1.1; OpenAI regional processing +10%, declared by the charter param `data_residency = true` because responses do not report it), gateway account fees, batch (never used: batch routes are not in the registry).
- **Thresholds** are transcribed as "strictly above": OpenAI "> 272K" → `above_prompt_tokens = 272_000`; xAI "≥ 200K" → `199_999`; Haiku 5.5 "> 100K" → `100_000`; MiniMax "> 512K" → `512_000`. A "K" whose meaning (1,000 or 1,024) the page does not state is read as 1,000, the earlier and therefore dearer threshold (PV-02 rule).
- **Tier selection** uses the provider-reported prompt total after the response, never an estimate (crossing 100K, 200K, 272K or 512K reprices the whole request, cache reads included).

**Time-of-day pricing: the conservative rule** (`meter.window = "conservative"`, the template default; `"peak"` is the v0.1 behaviour). The Root grants the `cheap` window only when **all** of these hold, and otherwise bills the `dearest` window, so it stays an upper bound on what the provider charges:
1. **Inside one shrunken span.** The official cheap spans (for DeepSeek: every UTC instant outside 01:00–04:00 and 06:00–10:00 Monday–Friday, weekends included) are each shrunk by `margin_s` (900 s) at both ends: 04:15–05:45 and 10:15–00:45 on weekdays, Friday 10:15 to Monday 00:45 over a weekend. The whole interval `[t_admit, t_end]` must lie inside one shrunken span; a request that touches a peak window anywhere is peak, however short the touch.
2. **No holiday doubt.** Neither the UTC date nor the +08:00 date of `t_admit` or `t_end` is in the doubt calendar's `dates` or `workdays`. DeepSeek bills Chinese public holidays off-peak, but which clock and date it uses is not stated, and make-up working days blur "Monday through Friday", so the Root never grants the cheap window on those dates (an over-count on holidays, accepted).
3. **Known calendar.** Both dates are ≤ the calendar's `known_through`. The seed catalog ships `known_through = ""`, so the Root bills peak on every date until PV-03 fills and verifies the calendar; v0.1 behaviour is therefore unchanged until then.
4. **Trusted clock.** The Root keeps a clock check from the `Date` header of every upstream HTTPS response (any provider): the newest sample is at most 6 h old, its skew `|Date − wall clock|` is ≤ 60 s, and since that sample the wall clock has advanced within 2 s of the monotonic clock. Before the first sample after a Root start, or after any failed condition, the clock is "in doubt" and the request is billed peak (flag `clock_doubt`).

The 15-minute margin covers clock error well beyond the 60-s skew bound and any ambiguity in which instant the provider classifies by (admission, first token or completion).

**Reservation** (admission, before any usage): dearest window; the highest price among `base` and every change effective within `[t_admit, t_admit + 1 h]`; the tier implied by `in_est = req_bytes` (one token per byte, F4; tokens ≤ bytes for every request INIT sends, so a request whose bytes are under a threshold cannot be repriced above it); the requested modifiers and endpoint factor; and, for the input term, the dearest input-side class, because a 1-hour cache write costs twice the input price:

```text
r = ⌈ ( in_est × max(p.input, p.cache_write_*) + min(max_out(req), 2^20) × p.output ) × F / 10^12 ⌉
```

**Worked vectors** (exact integers; they are the MF tests of §8):

| Id | Row and usage | Expected `cost_musd` |
|---|---|---|
| MF-01 | `claude-opus-5-5` ($4 / $0.20 read / $5 5m / $8 1h / $20), `inference_geo:us`; u 10,000, r 200,000, w5 5,000, w60 2,000, o 3,000 | 199,100 |
| MF-02 | `claude-haiku-5-5`; o 1,000; (a) u 29,999 + r 70,001 = 100,000; (b) u 30,000 + r 70,001 = 100,001 | (a) 4,200 (base) · (b) 21,001 (whole request at the tier) |
| MF-03 | `gpt-6.1-sol` > 272K ($4 / $0.20 / $5 30m / $15); `input_tokens` 300,000 incl. 250,000 cached and 10,000 written ⇒ u 40,000; o 2,000 | 290,000 |
| MF-04 | `gpt-6-astra` > 272K ($20 / $2 / $25 / $75), same usage, `service_tier:flex` ×0.5 | 850,000 |
| MF-05 | `deepseek-flash`; prompt 10,000 (hit 8,000, miss 2,000), o 500; (a) peak; (b) off-peak granted | (a) 1,248 · (b) 624 |
| MF-06 | `gemini-4-argon`; prompt 50,000 incl. 40,000 cached, candidates 1,000 + thoughts 3,000; (a) ending 2026-11-30T09:59:59Z; (b) admitted 10:00:00Z | (a) 64,000 · (b) 128,000 |
| MF-07 | `claude-sonnet-5-5` ($2 / $4 1h / $10); u 1,000, w60 20,000, o 500 | 87,000 (the old input-price formula gives 47,000: −46%) |
| MF-08 | `grok-4.7` (≥ 200K: $4 / $1 / $12); r 100,000, o 1,000; (a) u 99,999; (b) u 100,000 | (a) 255,998 · (b) 512,000 |
| MF-09 | `openrouter/anthropic/claude-sonnet-5-5`, fee 55‰; u 1,000, o 1,000; reported cost 0.0130 credits | own 12,660; settled 13,000 (reported > own × 1.02), `price_drift` |
| MF-10 | reservation for `claude-sonnet-5-5`: 40,000-byte request, `max_tokens` 64,000 | r = 800,000 |

#### 3.2.5 Unpriced model ⇒ refuse

A billed request is refused **403 `class.unpriced`** before anything is forwarded when any of these holds:
- the model has no catalog row and no owner price (charter `price` override or custom-provider price, §3.5.6);
- the row is disabled (CAT-11) or past `valid_until`;
- the request sets a modifier the row marks `UNPRICED` or does not list (`service_tier: "priority"` on a row without it);
- the route's usage parser is unknown to this Root build.

`class.unpriced` is not a demoting hit (it is like `class.unmetered`, root/egress.md §3.6). The Root alerts `price_expiring` daily from 14 days before a `valid_until`, and `catalog_stale` weekly for enabled rows verified more than 120 days ago. A response that *reports* a modifier the row does not price (for example an upstream that silently upgraded the service tier) is settled at the row's largest listed multiplier and flagged `unpriced_modifier`, the rule kernel/proxy.md §3.10.3 already uses.

`billing = "plan"` and `billing = "local"` rows are priced, explicitly, at zero money; they are never "unpriced".

#### 3.2.6 Update mechanism (owner-run, signed, no agent network)

```
init-root catalog status                       version, source (embedded|active), verified_through, rows expiring or stale
init-root catalog diff <file>                  models added/removed, every price change with its % delta
init-root catalog update --from <dir> [--yes] [--allow-downgrade]
init-root catalog update --rollback            restore the newest prev/<version>.toml
init-root catalog approve <provider>/<model> | --all    approve pending price decreases
```

- **Increases apply automatically; decreases need the owner's approval.** A lower price lets the agent spend more under the same ceiling, so it is an owner decision; a higher one only makes the meter stricter. The Root keeps `ROOT_HOME/catalog/floors.json` (0600): for each (row, window, tier, class) the effective price it last metered with, plus the decreases still pending. Whenever a new price would be lower than its floor — a catalog update, a Root upgrade that brings a new embedded catalog, or a scheduled `price.changes` entry reaching its date — the Root keeps metering at the floor until the owner approves that (row, change). `catalog update` lists every decrease in its diff and asks for each (`--approve-decreases` approves them all in a non-interactive run); `catalog approve` approves pending ones later; each approval appends lineage `catalog_approve{row, change, from, to}`. Pending decreases raise `price_decrease_pending` once a day. Increases, including scheduled ones, apply at their effective instant with no prompt. Charter price overrides below the catalog are owner approvals by construction (the charter apply flow, with the `charter.price_below_catalog` warning).

- **Signature.** `catalog-<version>.toml.sig` is a 64-byte Ed25519 signature (hex, one line) over `b"init-catalog/1\0" ‖ file bytes`. `init-root` embeds up to two catalog public keys (`CATALOG_PUBKEYS`, a rotation slot) and verifies with `ed25519-dalek` 3.0.0 `verify_strict`, the crate the Root already uses for lineage. The maintainer's signing key never lives in the repo or in `INIT_HOME`; `scripts/release.sh` signs with it from `~/.config/init-agent/catalog-signing.ed25519`, a path on the sandbox deny list (kernel/sandbox.md §3).
- **Source.** `--from <dir>` reads the two files the owner downloaded. While the repo is private there is no network fetch at all. Once it is public, `--fetch <version>` may download the two assets from the release page through a Root-internal fetch with exact hosts (`github.com:443` and the one asset-redirect host GitHub uses, checked at build) and no other redirect; this is an owner command, never reachable from the zone.
- **Apply.** Validate (CAT-1…16), show the diff (with each decrease marked for approval), ask `apply? [y/N]` (non-TTY needs `--yes`), move the active file to `prev/`, write the new one with `write_atomic`, append lineage `catalog{version, sha256, source, added, removed, repriced}`, call `egress::reload()`, republish the views. In-flight requests and open sealed lanes keep the catalog they were admitted under; a gate keeps the catalog version it opened with (kernel/proxy.md §3.5a snapshot).
- **Owner overrides.** The charter may set `[providers.<id>.price."<model>"]` blocks (§4.2). They survive catalog updates; `charter check` shows each override next to the catalog price and warns `charter.price_below_catalog` when it is lower.
- **No auto-update.** Neither the Root daemon nor anything in the zone checks for new catalogs. `catalog_stale` and `price_expiring` alerts tell the owner when to run the command (anti-pattern from the research: a remote price DB that changes what the Root bills without review).

#### 3.2.7 Published view and its readers

`INIT_HOME/root/catalog.json` (ICJ, rewritten on every reload) carries, per provider: `id`, `display`, `kind`, `dialect`, `default_model`, the zone-facing `routes` (method, path template, api, dialect, meter); per model: every capability key of §3.2.2, the **current** resolved price vector (µUSD/MTok, window in force now and at peak), tiers, cache-write classes, multipliers, `billing`, `confidence`, `wire_verified`, `status`, `valid_until`. It carries no host, base path, endpoint params, owner price overrides (only their effect), keys or account data.

| Reader | Uses it for | Authority |
|---|---|---|
| body | `ModelInfo` (context, max_out, effort map, replay rule, caching kind, dialect, route path) | none: wire hints only |
| kernel | route table, measurement prices, `cost_ref`, `/_init/models` | measurement only |
| TUI (through `models.list`, never the file) | the `/model` picker | none |
| Root | never reads it back | — |

#### 3.2.8 Seed and verification

- **PV-02** transcribes the table in `llm_api_providers.md` Q3 into `root/catalog/models.toml`, one row per (provider, model id), keeping each row's `source_url` and `verified_date`, with these flags:
  - `confidence = "3p"` for every number marked **[3P]** (Cerebras prices, the Mistral model id, Fireworks and DeepInfra base URLs) and for Kimi's `$` with no stated currency; such rows ship disabled (CAT-11);
  - `status = "legacy"` for every row marked **[STALE?]**; legacy rows stay priced so old sessions keep working, but `/model` hides them;
  - `price.changes` for Gemini 4 Argon (2026-12-01) and Gemini 3.8/3.6 Flash (2027-01-01); `valid_until = "2026-11-21"` for GPT-5.6 Sol's promotion;
  - every "?" (unknown context or max output) becomes a conservative value the body can live with: `context = 128_000` and `max_out = 16_384`, marked `"ctx_known": false` in the view, until PV-03 finds the number;
  - non-Anthropic Bedrock and Vertex rate cards were not researched, so those models have no rows and are refused as unpriced (§3.2.5) until PV-03 adds them.
- **PV-03 (release gate).** A dedicated lane re-fetches every `source_url` within 7 days of the release tag, re-reads each number, sets `verified_date`, and fixes or disables rows that changed. CI check `catalog-fresh` fails a release whose enabled rows have `verified_date` older than 14 days at tag time. The same lane fills the CN holiday calendar from the official notice and verifies the base URLs and wire facts marked UNVERIFIED in §3.3.
- **Scope of "current for a year".** The catalog is data, so the year is met by releases of the catalog, not of the code: the verification lane runs at least monthly after v1.0, and `price_expiring` warns the owner before a promotion ends.

### 3.3 Codec layer (body)

#### 3.3.1 Codecs, dialects and the trait

A **codec** is one wire family: request rendering, stream decoding, error classification and replay. A **dialect** is a data profile that adjusts a codec to one provider's quirks without new code. The pair `(api, dialect)` replaces today's `Api`-only dispatch (`provider::for_model`).

| `api` (log JSON) | Codec | Framing | Dialects |
|---|---|---|---|
| `responses` | OpenAI Responses | SSE | `openai`, `siwc` (ChatGPT plan, §3.4.4), `xai`, `azure`, `bedrock-openai`, `responses-compat` (Z.ai `/api/v1`, DeepSeek, Ollama, LM Studio, vLLM, llama-server, Vercel) |
| `messages` | Anthropic Messages | SSE | `anthropic`, `bedrock-mantle`, `vertex`, `foundry`, `anthropic-compat` (MiniMax, Z.ai, DeepSeek, Kimi, OpenRouter, Vercel, Synthetic, Ollama, LM Studio, llama-server, vLLM) |
| `gemini_generate` | Gemini generateContent | SSE (`?alt=sse`) | `ai-studio`, `vertex` |
| `gemini_interactions` | Gemini Interactions | SSE | `ai-studio` (PV-14) |
| `chat_completions` | Chat Completions | SSE | §3.3.6: `openai`, `deepseek`, `kimi`, `qwen`, `glm`, `minimax`, `xai`, `mistral`, `gpt-oss`, `openrouter`, `generic`, and the local servers `ollama`, `llama-server`, `vllm`, `sglang`, `lmstudio`, `mlx-lm`, `jan`, `localai` |
| `converse` | Bedrock Converse (optional, PV-15b) | AWS event stream (binary) | `bedrock` |

The `Provider` and `Decoder` traits of body/provider.md §3.4 stay as they are; only construction changes:

```rust
// seed/src/provider/mod.rs (v2). std + serde_json only.
pub fn for_model(info: std::sync::Arc<ModelInfo>) -> Result<Box<dyn Provider>, ProviderError> {
    let d = dialect::lookup(info.api, &info.dialect)                 // built-in table, §3.3.6
        .ok_or_else(|| ProviderError::Protocol(format!("unknown dialect {}", info.dialect)))?;
    Ok(match info.api {
        Api::ChatCompletions    => Box::new(chat::Chat::new(info, d)),
        Api::Messages           => Box::new(messages::Messages::new(info, d)),
        Api::Responses          => Box::new(responses::Responses::new(info, d)),
        Api::GeminiGenerate     => Box::new(gemini::Generate::new(info, d)),
        Api::GeminiInteractions => Box::new(gemini::Interactions::new(info, d)),
        Api::Converse           => Box::new(converse::Converse::new(info, d)),
    })
}

/// Built from INIT_HOME/root/catalog.json at boot (no longer a static table).
pub struct ModelInfo {
    pub provider: String, pub id: String, pub api: Api, pub dialect: String,
    pub route: String,                 // "/deepseek/chat/completions" or a {model} template, expanded
    pub context_window: u64, pub ctx_known: bool, pub max_output: u32, pub default_max_tokens: u32,
    pub effort: std::collections::BTreeMap<Effort, Option<String>>, pub default_effort: Effort,
    pub images: bool, pub forced_tool_choice: bool, pub parallel_tools: bool,
    pub replay: Replay, pub carrier: Carrier,          // §3.3.8
    pub edit_rule: EditRule,                          // Free | ClosedRound (§3.3.12)
    pub cache: CacheProfile,                          // log-context §3.10.0, values from caching + dialect
    pub billing: Billing,                             // Tokens | Plan | Local
}
```

- `Api` keeps its three existing variants and their JSON strings, so every existing log reads unchanged; three variants are added. log-context must treat an `api` string it does not know as "a different api" (its blocks are dropped on replay, never a parse error), so a rolled-back body can still read a log written by a newer one (cross-doc, §9).
- **Boot check** (body/boot.md §3.4): the `boot.json` model must be in the catalog view, `routable` in `ceilings.json`, and its `(api, dialect)` known to this body build; otherwise exit 78 as today.
- **Fallback table.** A body that finds no view (unit tests, `init-mockllm` homes, a v0.1 Root) uses the built-in table: today's seven rows of body/provider.md §3.3, unchanged.

#### 3.3.2 How today's provider code maps onto v2

| Today | v2 | Change |
|---|---|---|
| `model.rs` `Api {ChatCompletions, Messages, Responses}` | same enum + `GeminiGenerate`, `GeminiInteractions`, `Converse`; new `dialect: String` beside it | additive |
| `provider/models.rs` `SEED_MODELS` (7 static rows), `lookup` | `provider/catalog.rs`: parse the view into `Arc<ModelInfo>`; `SEED_MODELS` kept as the fallback table | data source |
| `provider/deepseek.rs` (Chat Completions build + decoder) | `provider/chat/{mod.rs, dialects.rs}`: the same code, parameterized; `deepseek` is the first dialect and must render **byte-identical** requests (golden `testdata/golden/deepseek-six.json`, T-PROV-1) | refactor |
| `provider/anthropic.rs` | `provider/messages/`: + host dialects (model in URL for Vertex, deployment names for Foundry, no binding beta for non-Anthropic hosts) | extend |
| `provider/openai.rs` | `provider/responses/`: + `siwc` plan mode, `xai`, compat dialects | extend |
| — | `provider/gemini/{parts.rs, generate.rs, interactions.rs}`, `provider/converse/{mod.rs, eventstream.rs}` | new |
| `ReasoningRule {PassBackPlain, SignedModelBound, EncryptedItems}` | `Replay` × `Carrier` (§3.3.8) | generalized |
| `effort_wire(api, e)` | the model's catalog `effort` map | data |
| `mentions_overflow`, `base_kind` | per-dialect overflow patterns and error-code tables (§3.3.6 column "errors") | data |
| `handoff.rs` id sanitizer | + per-dialect id rules (Mistral, §3.3.9) | extend |
| `transport.rs`, `http.rs`, `sse.rs` | unchanged, plus `Framing::{Sse, AwsEventStream}` chosen by api | small |
| `context/render.rs` `head_fields`: Chat Completions only; Messages and Responses return "not built yet (WP10)" | one renderer per api with the same per-message byte cache (log-context §6 rank 7); this closes the WP10 gap | **largest body item (PV-12)** |
| `context/hash.rs` head object per api (3) | + Gemini (`systemInstruction` + `tools`) and Converse (`system` + `toolConfig`) | extend (log-context owns the algorithm) |

#### 3.3.3 OpenAI Responses

body/provider.md §3.8.3 stays authoritative for the `openai` dialect. Dialect deltas:

| Dialect | Endpoint (zone-facing) | Differences |
|---|---|---|
| `openai` | `/openai/v1/responses` | as today; GPT-6 Astra and 6.1 Sol are Responses-only in the catalog (`apis = ["responses"]`), because Chat Completions has no tools on them |
| `siwc` | `/openai-chatgpt/v1/responses` | `store:false`, `stream:true` mandatory; **omit** `max_output_tokens`, `temperature`, `metadata`, `user`; function and custom tools only; full history every turn (no `previous_response_id`, already INIT's rule). The Root refuses any omitted field (§3.5.3) |
| `xai` | `/xai/v1/responses` | `encrypted_content` is always returned and replayed; effort `low`/`medium`/`high`/`xhigh` (Max → `xhigh`); never send `stop`, `presence_penalty`, `frequency_penalty` |
| `azure` | `/azure/openai/v1/responses` | `model` is the **deployment name** (charter maps it to a catalog row, §4.2); no `api-version` query |
| `bedrock-openai` | `/bedrock/openai/v1/responses` | synchronous only per research; streaming UNVERIFIED → non-streaming fallback (`stream:false`, decoder reads one JSON body) |
| `responses-compat` | provider route | `include` and `prompt_cache_breakpoint` omitted (unknown fields may 400); reasoning items replayed only if the server returned them |

#### 3.3.4 Anthropic Messages and its host dialects

body/provider.md §3.8.2 stays authoritative for `anthropic`. Host dialects:

| Dialect | Model goes in | Body differences | Headers | Notes |
|---|---|---|---|---|
| `anthropic` | body `model` | as today, incl. `thinking.block_binding` | `anthropic-version`, `anthropic-beta: thinking-binding-controls-2026-08-01` | Opus 5.5, Sonnet 5.5, Fable 5.1: no forced `tool_choice`; Sonnet 5.5's lowest thinking is `between_tools` |
| `bedrock-mantle` | body `model` (`anthropic.claude-opus-5-5`) | same body | same, no binding beta (support UNVERIFIED) | SigV4 service `bedrock-mantle` (§3.4.5); no Batches, server tools, `fallbacks` |
| `vertex` | **URL**: `…/models/{model}:streamRawPredict` | no `model`; `"anthropic_version":"vertex-2023-10-16"` in the body | no `anthropic-version` header | streaming method name UNVERIFIED (research); 30 MB payload cap |
| `foundry` | body `model` = **deployment name** | same body | `anthropic-version` | no `anthropic-ratelimit-*` headers, so the transport relies on `retry-after` and backoff only |
| `anthropic-compat` | body `model` | no `cache_control` where the host ignores or rejects it (Ollama rejects; DeepSeek ignores), no `block_binding`, no betas; effort through `output_config.effort` (DeepSeek, MiniMax) | `anthropic-version` | notes become user text (`NotePlacement::TextAfterToolResults`), because mid-conversation `system` is Anthropic-only |

#### 3.3.5 Google Gemini

**generateContent** (PV-13), zone-facing route `/google/v1beta/models/{model}:streamGenerateContent?alt=sse` (AI Studio, `x-goog-api-key` injected by the Root, never `?key=`) or `/vertex/google/models/{model}:streamGenerateContent?alt=sse` (Vertex, OAuth bearer; the Root holds the `v1/projects/{project}/locations/{location}/publishers` prefix).

```json
{"contents":[{"parts":[{"text":"Fix the failing test."}],"role":"user"},
             {"parts":[{"text":"","thought":true,"thoughtSignature":"Ck8B…"},
                       {"functionCall":{"args":{"command":"cargo test"},"id":"fc_1","name":"bash"},"thoughtSignature":"Cm0B…"}],
              "role":"model"},
             {"parts":[{"functionResponse":{"id":"fc_1","name":"bash","response":{"output":"test add ... FAILED"}}}],"role":"user"}],
 "generationConfig":{"maxOutputTokens":65536,"thinkingConfig":{"includeThoughts":false,"thinkingLevel":"high"}},
 "systemInstruction":{"parts":[{"text":"<head.core>"},{"text":"<head.self_>"}]},
 "toolConfig":{"functionCallingConfig":{"mode":"AUTO"}},
 "tools":[{"functionDeclarations":[{"description":"…","name":"read","parametersJsonSchema":{"…":{}}}]}]}
```

- **Replay (mandatory).** Every `model` turn is stored as its native `Content` (`Thinking.native` holds the whole parts array) and replayed verbatim, `thoughtSignature`s included, to the same model. Research: in stateless mode "You MUST always resend all thought blocks exactly as they were received". History edits follow `closed_round` (§3.3.12); that stripping signatures of closed turns before an epoch is accepted is UNVERIFIED (PV-03), and a 400 naming a signature triggers the flatten fallback.
- **Tool ids.** `functionCall.id` / `functionResponse.id` are used when present; otherwise the decoder synthesizes `gc_<n>_<i>` (`n` = assistant messages so far) and the renderer matches responses by order. Whether Gemini 3+ always returns ids is UNVERIFIED.
- **Schemas.** `parametersJsonSchema` (full JSON Schema) is sent; if the conformance fixture shows it unsupported on a model, the dialect switches to `parameters` with a sanitizer that drops `additionalProperties` (UNVERIFIED, PV-03).
- **Effort.** `thinkingLevel`: Low → `low`, Medium → `medium`, High/XHigh/Max → `high`. Reasoning cannot be turned off on Gemini 3+ (research).
- **Usage** (`usageMetadata`, last chunk wins): `input = promptTokenCount + toolUsePromptTokenCount` (`promptTokenCount` already includes cached content); `cache_read = cachedContentTokenCount`; `output = candidatesTokenCount + thoughtsTokenCount` (Gemini bills "the sum of output tokens and thinking tokens"; `candidatesTokenCount` excludes thoughts). This sum is the one meter trap specific to Gemini.
- **Finish.** `STOP` → `ToolUse` if any `functionCall` was emitted, else `EndTurn`; `MAX_TOKENS` → `MaxTokens`; `SAFETY`, `RECITATION`, `BLOCKLIST`, `PROHIBITED_CONTENT`, `SPII` → `Refusal{category}`; `MALFORMED_FUNCTION_CALL` → `Stream{InvalidRequest}` (not retried; the loop reports it); others → `Protocol`.
- **Not used:** the Gemini OpenAI-compatible endpoint (beta, silently drops unknown parameters), explicit `cachedContents` (storage-billed and stateful), built-in tools.

**Interactions** (PV-14), route `/google/v1beta/interactions`: `store:false`, full history each turn, thought blocks resent exactly, usage from `total_*` fields (`total_cached_tokens`, `total_thought_tokens`). The stream event names were not captured by the research, so PV-14 starts by recording them into fixtures from the official reference. Shared `gemini/parts.rs` maps content both ways. **Default:** a model uses `gemini_generate` unless its catalog row lists only `interactions` (a model launched on Interactions only, which Google says new features will be). Open question Q5.

#### 3.3.6 Chat Completions and dialect profiles

One codec (today's `deepseek.rs`, generalized). A dialect is a static Rust table row in `chat/dialects.rs` (body code, so the agent can grow it through the gate); the catalog names the dialect per provider and may override it per model (a host serving DeepSeek models uses `deepseek-host`, its gpt-oss models `gpt-oss`).

```rust
pub struct ChatDialect {
    pub id: &'static str,
    pub reasoning_in: ReasoningField,   // where the stream carries reasoning
    pub reasoning_out: ReasoningField,  // where replay puts it (None = not replayed)
    pub think_tags: bool,               // split inline <think>…</think> out of content
    pub thinking_toggle: Toggle,        // DeepSeekObject | TemplateKwargs | AlwaysOn | None
    pub effort_param: EffortParam,      // ReasoningEffort | OpenRouterReasoning | None
    pub max_tokens_field: &'static str, // "max_tokens" | "max_completion_tokens"
    pub usage: UsageShape,              // Openai | DeepSeek | OpenRouter | LlamaCpp
    pub include_usage: bool,            // send stream_options.include_usage = true
    pub never_send: &'static [&'static str],   // params that error or are ignored
    pub tool_choice: &'static [&'static str],  // accepted values; [] = never send the field
    pub parallel_tool_calls: Option<bool>,     // Some(v) = send it explicitly
    pub synth_call_ids: bool,           // server may omit tool_call ids
    pub call_id_rule: IdRule,           // Verbatim | Alnum9 (Mistral)
    pub extra: &'static str,            // fixed JSON object merged at render (OpenRouter `provider`, Qwen kwargs)
    pub overflow: &'static [&'static str],     // context-overflow message patterns
}
pub enum ReasoningField { None, ReasoningContent, Reasoning, ReasoningDetails }
```

| Dialect | Providers | Reasoning (in → replay) | Thinking / effort | Usage | Tool and param quirks | Evidence |
|---|---|---|---|---|---|---|
| `openai` | OpenAI CC, Azure CC, Bedrock-openai CC | none returned | `reasoning_effort` per model | `prompt_tokens` incl. cached, `prompt_tokens_details.cached_tokens`; `include_usage` | `max_completion_tokens`; **no tools on GPT-6 Astra / 6.1 Sol** (the catalog omits `chat_completions` from their `apis`); GPT-6 Luna tools only at effort `none` | docs |
| `deepseek` | DeepSeek | `reasoning_content` → `reasoning_content`, **mandatory with tools** | `thinking:{type:"enabled"}`; `low`/`high`/`max` | `prompt_cache_hit_tokens` / `_miss_tokens`, always on the last chunk | never `temperature`, `top_p`, penalties, `tool_choice`, `stream_options`, `user_id`; foreign tool calls → flatten fallback (body/provider.md §3.10) | **live** |
| `deepseek-host` | DeepSeek models on Together, Fireworks, DeepInfra | `reasoning_content` → same (UNVERIFIED that hosts accept it; conformance fixture) | `reasoning_effort` | `openai` + `include_usage` | as `deepseek` | 3p |
| `kimi` | Moonshot (`api.moonshot.ai/v1`), Kimi on hosts | `reasoning_content` → same, treated as mandatory with tools ([3P]) | always on (K3); `low`/`high`/`max`, default `max` | `openai` shape (cached field UNVERIFIED); `include_usage` | never `temperature`, `top_p`, `n`, penalties ([3P]); call ids kept verbatim | docs + 3p |
| `qwen` | Alibaba Model Studio (per-workspace, per-region base URL) | `reasoning_content` (UNVERIFIED) → same | `enable_thinking` for hybrid models (UNVERIFIED for 3.8); `reasoning_effort` if accepted | `openai` + **`include_usage` required** | the "no tools with stream" note is [STALE?]: conformance fixture decides; key bound to its region | docs |
| `glm` | Z.ai general API (`/api/paas/v4`) | `reasoning_content` → same (preserved thinking) | **cannot disable**: always send `thinking:{type:"enabled"}`; `low`/`high`/`max` | `openai` + `include_usage` | the Coding-plan base URL is on the ToS denylist (§3.4.7) | docs |
| `minimax` | MiniMax CC path (`/v1`) | `reasoning_content` → same | **cannot disable** (400); `low`…`max` | `openai` + `include_usage` | the catalog prefers the `messages` route ("recommended" by MiniMax) | docs |
| `xai` | xAI CC (fallback only) | none on CC ("no field for the ciphertext") | `low`…`xhigh` | `openai` | never `stop`, penalties; the catalog prefers `responses` | docs |
| `mistral` | Mistral | none in v1 | none | `openai` | tool call ids must be 9 alphanumerics (`IdRule::Alnum9`: deterministic `base62(sha256(id))[..9]`, UNVERIFIED in 2026) | 3p |
| `gpt-oss` | Groq, Cerebras, Fireworks, SambaNova and local gpt-oss | `reasoning` → `reasoning`, **kept until the final answer, dropped after it** | `reasoning_effort` `low`/`medium`/`high` | `openai`; Groq cached via `prompt_tokens_details.cached_tokens` | Harmony models; `parallel_tool_calls` explicit | docs |
| `openrouter` | OpenRouter | `reasoning_details[]` → replayed **unmodified and in order** (mandatory in tool loops) | `reasoning:{effort}` | always included: `cost`, `cache_write_tokens`; deprecated `usage`/`include_usage` flags never sent | renders the fixed `provider` object of §3.5.4 and `transforms: []`; ignores SSE comments; maps `native_finish_reason` | docs |
| `generic` | any custom OpenAI-compatible endpoint | `reasoning_content` or `reasoning`, whichever arrives → not replayed | none unless the owner's charter entry sets `effort_param` | `openai` + `include_usage` | conservative: no `tool_choice`, no `parallel_tool_calls` | — |
| `ollama` | Ollama `/v1` | `reasoning` (UNVERIFIED) → same | `reasoning_effort` mapped by Ollama | `openai` + `include_usage` (supported) | **no `tool_choice`, `logit_bias`, `user`, `n`**; context fixed at registration (§3.5.5) | docs |
| `llama-server` | llama.cpp | `reasoning_content` with `--reasoning-format deepseek` | none | `openai` + `timings`, `cached_tokens` | `parallel_tool_calls` off by default; non-native templates use the token-heavy generic format | docs |
| `vllm`, `sglang` | vLLM, SGLang | `reasoning_content` or `reasoning` (UNVERIFIED) → per model family | per model family | `openai` + `include_usage` | needs `--enable-auto-tool-choice` + parser; `required` only on vLLM ≥ 0.8.3 | docs |
| `lmstudio`, `jan`, `localai`, `mlx-lm` | those servers | `reasoning` where emitted; `think_tags` on | none | `openai` | LM Studio non-native tools may land in `content`; MLX-LM documents no tools API, so its models need a passing conformance probe (§3.5.5) | docs |

Rules common to every dialect:
- Requests are rendered through `serde_json::Value` (sorted keys, ICJ) exactly as today, so the same projection yields the same bytes.
- `extra` objects are constants per (dialect, model); they never carry a time, an id or a random value.
- A dialect may only remove fields or add constant fields relative to `openai`; anything else is a new codec.
- **Conformance probe** (PV-18 fixtures; live only for DeepSeek and the owner's local servers): parallel calls, a nested-JSON argument, four chained calls, reasoning interleaved with tools, a 400 shape, a max-tokens stop. A dialect row whose probe never ran is `wire_verified = "docs"` in the catalog, and `/model` marks it "not live-tested".

#### 3.3.7 Bedrock Converse (optional, PV-15b)

Needed only for non-Anthropic, non-OpenAI models on Bedrock (Claude uses `bedrock-mantle` Messages; gpt-oss and OpenAI-family models use `bedrock-openai`). Route `/bedrock/model/{model}/converse-stream`; SigV4 service `bedrock`.
- Request: `messages`, `system`, `inferenceConfig{maxTokens}`, `toolConfig{tools:[{toolSpec}]}`, `additionalModelRequestFields` (reasoning config per model family); never `serviceTier` other than `default` unless priced.
- Framing: AWS event stream. Each message is a 12-byte prelude (total length u32 BE, headers length u32 BE, CRC32 of the first 8 bytes), headers (`:event-type`, `:message-type`, `:content-type`), a JSON payload and a CRC32 over the whole message. Both CRCs are checked (`Protocol` on mismatch). The framing is long-standing and was not re-verified in 2026 docs (research gap); PV-15b fixes it from the official reference and one recorded fixture.
- Events: `messageStart`, `contentBlockStart`, `contentBlockDelta` (`text`, `toolUse.input`, `reasoningContent{text, signature, redactedContent}`), `contentBlockStop`, `messageStop{stopReason}`, `metadata{usage}`. Reasoning replay is mandatory ("include the signature and all previous messages").
- Usage only in `metadata`: `inputTokens`, `outputTokens`, `cacheReadInputTokens`, `cacheWriteInputTokens`.

#### 3.3.8 Reasoning normalization and replay rules

Every provider's reasoning arrives in the one normalized block body/provider.md already defines (`Thinking{provider, model, text, native}`). `native` holds the exact provider object for replay; `text` is display-only. Two enums replace `ReasoningRule`:

```rust
pub enum Replay { Mandatory, MandatoryWithTools, Recommended, DropAfterFinal, None }
pub enum Carrier {
    PlainField(&'static str),  // reasoning_content | reasoning: the text is the replay
    SignedBlock,               // Anthropic thinking / redacted_thinking with signature; Converse reasoningContent
    EncryptedItem,             // OpenAI / xAI reasoning item with encrypted_content
    ThoughtSignature,          // Gemini parts carrying thoughtSignature
    ReasoningDetails,          // OpenRouter reasoning_details[] (array kept whole and in order)
    InlineTags,                // <think>…</think> split out of content; never replayed
}
```

| Provider · route | Carrier | Replay | Source | Consequence in INIT |
|---|---|---|---|---|
| Anthropic and `bedrock-mantle`, `vertex`, `foundry` | SignedBlock | **Mandatory** with tools; unmodified; bound to the unchanged prefix on 5.x models | Anthropic thinking and errors pages | today's rules (binding beta, epoch strip allowed, strip-and-retry once) |
| MiniMax, Z.ai, DeepSeek, Kimi on `anthropic-compat` | SignedBlock | treated as **Mandatory** (interleaved thinking; exact rule UNVERIFIED) | MiniMax, Z.ai docs | replay unchanged; no binding beta; strip-and-retry once on a 400 naming the signature |
| OpenAI Responses (`openai`, `azure`, `bedrock-openai`) | EncryptedItem | **Recommended**; effectively mandatory with `store:false` and `reasoning.context = all_turns` (GPT-5.6 default) | OpenAI reasoning guide | replay every item since the last user message, as today; same model family only |
| `siwc` | EncryptedItem | as OpenAI | SIWC preview limits | full history each turn |
| xAI Responses | EncryptedItem | **Recommended** ("always returned" on grok-4.7) | xAI reasoning guide | replay |
| Gemini (both APIs) | ThoughtSignature | **Mandatory** in stateless mode | Gemini thinking guide | replay whole `Content`; `strip_thinking_before_epoch = false` |
| Bedrock Converse | SignedBlock | **Mandatory** (signature + all previous messages) | Bedrock Converse docs | replay; no history edits |
| DeepSeek CC | PlainField(`reasoning_content`) | **MandatoryWithTools**: every later request, even turns without a tool call; 400 otherwise | DeepSeek thinking guide | as today; never stripped |
| Kimi CC | PlainField(`reasoning_content`) | MandatoryWithTools ([3P]; treated as mandatory because the safe side only costs input tokens) | 3P guide | as DeepSeek |
| GLM, Qwen CC | PlainField(`reasoning_content`) | **Recommended** (preserved thinking) | Z.ai, Qwen cards | replay |
| OpenRouter | ReasoningDetails | **Mandatory** in tool loops, unmodified, in order | OpenRouter reasoning docs | replay the array whole |
| gpt-oss (any host, local) | PlainField(`reasoning`) | **DropAfterFinal**: keep until the final answer, drop after it | OpenAI gpt-oss cookbook | the renderer drops reasoning of turns that ended `EndTurn` |
| Local servers with `<think>` | InlineTags | None | — | displayed, logged, never replayed |

Rules derived from the table:
1. **INIT replays whenever the model accepts it** (Mandatory, MandatoryWithTools and Recommended alike). Recommended replay improves quality and keeps the cached prefix byte-stable.
2. **History edits** follow the codec's `edit_rule` (§3.3.12): `closed_round` codecs strip every reasoning block before an epoch and close an open tool round first; `free` codecs keep reasoning on every retained message (`strip_thinking_before_epoch = false`).
3. **Handoff** (body/provider.md §3.10) is unchanged: thinking is kept only for the same provider and model. ThoughtSignature and ReasoningDetails are model-bound like signatures. A foreign assistant turn into a MandatoryWithTools target goes through the DeepSeek flatten fallback, generalized to every PlainField-mandatory dialect.
4. **Recovery.** A 400 that names the reasoning carrier (signature, `reasoning_content`, thought signature) triggers the codec's one-shot fallback (strip for SignedBlock, flatten for PlainField), emits `Warning`, and is logged as `error{code:"reasoning_replay"}`.
5. **Billing.** Reasoning is billed as output everywhere researched, including hidden or summarized reasoning, so every usage parser counts it in `output` (§3.3.10).

#### 3.3.9 Tool-call quirks

| Quirk | Where | INIT handling |
|---|---|---|
| No forced `tool_choice` (`any`/`tool` → 400) | Anthropic Opus 5.5, Sonnet 5.5, Fable 5.1; DeepSeek thinking mode (`required`/named → 400) | `forced_tool_choice = false` in the catalog; the loop never forces (seed never does) |
| No tools on Chat Completions | GPT-6 Astra, GPT-6.1 Sol; GPT-6 Luna except at effort `none` | catalog `apis = ["responses"]` |
| `tool_choice` unsupported | Ollama `/v1`; Ollama `/v1/messages` | dialect `tool_choice = []` (field never sent) |
| Arguments as string vs object | CC and Responses: string (replayed verbatim, `raw_arguments`); Messages and Gemini: object; Converse: object | unchanged rule of body/provider.md §3.2 |
| Ids missing | some local servers; Gemini (UNVERIFIED) | `synth_call_ids`: `call_<n>_<i>`, deterministic |
| Id format | Mistral 9 alphanumerics (UNVERIFIED); Anthropic pattern UNVERIFIED | `IdRule` applied in the renderer *and* the handoff sanitizer, as a pure function of the message list |
| Malformed calls in `content` | LM Studio non-native tools; small local models | not parsed as calls; the conformance probe gates the model (`tool_calling = "measured"`) |
| Parallel calls off by default | llama-server | `parallel_tool_calls: true` sent explicitly where the dialect allows it |
| Gemini malformed call | `finishReason = MALFORMED_FUNCTION_CALL` | not retried; the loop reports it like `invalid_json` |
| Tool results with images | CC and Responses accept image parts; Gemini `inlineData`; Converse `image` blocks | `images` capability flag; otherwise the handoff placeholder text |
| Strict schemas | OpenAI Responses normalizes to strict when `strict` is absent | `strict: false` always sent (as today) |

#### 3.3.10 Streaming usage parsing (cumulative vs delta)

The rule for every parser: **usage objects are snapshots; the last value of each field wins; never sum across events**, except where a dialect declares delta semantics (none of the researched providers does). The Root's tap (§3.5.7) and the kernel's tap implement this table independently; the body's decoder uses it only for display.

| Codec · dialect | Where final usage is | Semantics | `input` (all prompt tokens) | `cache_read` | `cache_write` (by TTL) | `output` (incl. reasoning) |
|---|---|---|---|---|---|---|
| `messages` | `message_start.message.usage`, then each `message_delta.usage` | **cumulative**: overwrite present fields | `input_tokens + cache_creation_input_tokens + cache_read_input_tokens` | `cache_read_input_tokens` | `cache_creation.ephemeral_5m/1h_input_tokens`; absent ⇒ all 5m | `output_tokens` |
| `responses` | `response.completed` / `.incomplete` / `.failed` `response.usage` | single final object | `input_tokens` | `input_tokens_details.cached_tokens` | `input_tokens_details.cache_write_tokens` → 30m class (subset of `input_tokens`: UNVERIFIED; the parser clamps `uncached = input − read − write` at 0) | `output_tokens` |
| `chat_completions` `openai`, `qwen`, `glm`, `kimi`, `minimax`, `generic`, local | last chunk with non-null `usage` (requires `include_usage`) | snapshot | `prompt_tokens` | `prompt_tokens_details.cached_tokens` (0 if absent) | 0 | `completion_tokens` |
| `chat_completions` `deepseek` | last chunk (always sent) | snapshot | `prompt_tokens` (= hit + miss) | `prompt_cache_hit_tokens` | 0 | `completion_tokens` |
| `chat_completions` `openrouter` | last SSE chunk | snapshot | `prompt_tokens` | `prompt_tokens_details.cached_tokens` | `prompt_tokens_details.cache_write_tokens` → 5m class | `completion_tokens`; plus `cost` (cross-check, §3.5.4) |
| `gemini_generate` | every chunk's `usageMetadata` | running total (last wins; UNVERIFIED that every chunk carries it) | `promptTokenCount + toolUsePromptTokenCount` | `cachedContentTokenCount` | 0 | `candidatesTokenCount + thoughtsTokenCount` |
| `gemini_interactions` | final interaction usage | snapshot | `total_input_tokens` (name UNVERIFIED) | `total_cached_tokens` | 0 | output + `total_thought_tokens` |
| `converse` | `metadata` event | single | `inputTokens + cacheReadInputTokens + cacheWriteInputTokens` (UNVERIFIED whether `inputTokens` excludes cache; the parser takes the larger reading) | `cacheReadInputTokens` | `cacheWriteInputTokens` → 5m | `outputTokens` |

A stream that ends without usage (abort, cut, server without `include_usage`) is charged its full reservation by the Root and flagged `usage_estimated` (root/egress.md §3.4, unchanged). For `billing = "local"` rows the money cost is 0 either way and token counts are labelled approximate.

#### 3.3.11 Note placement and cache profile per api

| api · dialect | `note_placement` | `breakpoints` | `min_cacheable_tokens` |
|---|---|---|---|
| `messages` `anthropic`, `bedrock-mantle`, `vertex`, `foundry` | `MidSystemMessage` | `Anthropic{max:4}` | catalog (512 on 5.x models) |
| `messages` `anthropic-compat` | `TextAfterToolResults` | `None` (or `Anthropic{max:4}` where the host honours `cache_control`: OpenRouter, Vercel) | catalog |
| `responses` | `DeveloperMessage` | `OpenAiExplicit{max:4}` on GPT-5.6+, else `None` | 1,024 |
| `gemini_*` | `UserMessage` (a `user` text part) | `None` (implicit caching) | 4,096 on new models |
| `chat_completions` | `UserMessage` | `None`; OpenRouter → Anthropic upstream: `cache_control` on content parts (`Anthropic{max:4}`) | catalog |
| `converse` | `UserMessage` | `None` in v1 (`cachePoint` later) | — |

#### 3.3.12 History edits per codec (masking, compaction, head reload)

**The problem.** log-context edits history at *epochs*: masking replaces old tool output and large tool inputs with `[init:masked]` placeholders (log-context §3.11), compaction replaces a span with a summary (§3.12), and a head reload rewrites `system` (§3.8.2). On Claude 5.x (Fable 5.1, Opus 5.5, Sonnet 5.5, Haiku 5.5; Anthropic, Bedrock Mantle, Vertex, Foundry) a thinking block is bound to the exact earlier `system`, `tools` and messages. Replacing an old tool output with a placeholder, or compacting, therefore **modifies** the history every later thinking block is bound to; accounts created on or after 2026-08-31 get a 400. Stripping all earlier thinking blocks is allowed, but **an open tool round must return its own thinking block unmodified** (mandatory replay with tools). log-context's epochs start "at turn boundaries, after results are appended" (§3.10.2): that point can be *inside* an open round, where the newest assistant message still has `tool_use` blocks whose results this request returns. Its thinking block is bound to the pre-edit prefix, so neither keeping it nor dropping it gives a valid request. History edits must follow each codec's rule.

**Definitions.** A *step boundary* is any point between two model requests. A tool round is *open* when the newest assistant message on the path ended `ToolUse`, so the next request returns its results; it is *closed* when the newest assistant message ended without tool calls, or an owner message started a new turn.

**The rule per codec** (`ModelInfo.edit_rule`, from the catalog row; published in the view):

| `edit_rule` | Applies to | At an epoch |
|---|---|---|
| `free` | Chat Completions dialects with PlainField carriers (`deepseek`, `kimi`, `glm`, `qwen`, `minimax` CC, `gpt-oss`, local) and carriers without replay | edits anywhere at a step boundary; every retained assistant message keeps its reasoning per the replay rule (DeepSeek's `reasoning_content` is never stripped; log-context `strip_thinking_before_epoch = false`) |
| `closed_round` | Messages (all dialects: binding on Claude 5.x; unknown, so treated the same, on `anthropic-compat` hosts), Responses (encrypted items; "keep every output item" with `store:false` and `all_turns`), Gemini (thought signatures; binding rules UNVERIFIED), Converse ("changing earlier messages after a reasoning block throws an error"), OpenRouter `reasoning_details` | (1) strip **every** reasoning block before the epoch (all of them is valid); (2) if the round is open, apply **round-close** first, so no tool round is open across the edit; (3) everything after the epoch is append-only, so new reasoning binds to the new prefix |

**Round-close** (a pure, deterministic transform recorded at the epoch). The open round's assistant message `A` (its text and `k` tool calls) and its results `R` are rendered, from the epoch on, in the flattened form body/provider.md §3.10 already defines for DeepSeek handoffs:
- `A` → one assistant message containing only text: `A`'s own text, then one line per call, `[called <name> <raw_arguments>]`, no `tool_use` and no reasoning;
- `R` → one user message: `[result of <name> (<call id>)]\n<content>` per result, in call order (masking placeholders apply to them as to any other result), followed by any steer text;
- the request then ends with that user message, so no `tool_result` answers a `tool_use` and no thinking block has to be returned.

The model loses only the structured form of its last step (it keeps the text of the calls and their results) and the reasoning of that step, at an epoch it would have lost anyway. The flattened bytes become part of the new epoch's prefix and are reused unchanged by every later request, so the cache stays valid after the one miss the epoch already causes.

**Where it is enforced** (cross-doc, log-context; ties to its §3.7 bounded view and the epoch rules of §3.10.2, §3.11 and §3.12):
- `View` (log-context §3.7) gains `round_closed: Option<u64>`, the id of the assistant event the epoch closed, set from the epoch event (`mask` and `compaction` events record `round_closed`; a head reload records it in its reload event). The projection renders that round flattened for every request of the epoch, and log-context's P2 rule (computed at the epoch, replayed verbatim) holds.
- `boundary_rebuild_check` (log-context §3.11) reads `edit_rule` from `ModelInfo`. For `closed_round` with an open round it emits the epoch with `round_closed = <A.id>`; for `free` it behaves as today.
- The compaction fork (log-context §3.12.3 step 3) is unchanged: it appends a prompt to the current request and edits nothing, so it is valid under every rule, including inside an open round. Round-close applies to the projection *after* the compaction event.
- The Anthropic binding beta stays as the detector: `prefix_mismatch_behavior: "error"` in tests and `"drop_block"` in production; with these rules every `input_transformations` entry is a bug and is logged `error{code:"thinking_dropped"}`. Hosts without the beta (Bedrock Mantle, Vertex, Foundry: support UNVERIFIED) rely on the rule alone; their 400 on a bound block triggers the codec's one-shot fallback: strip all reasoning and round-close, then retry once.
- **Handoff** (provider or model switch) is an edit too: it strips foreign reasoning (body/provider.md §3.10) and, when the target is `closed_round` and the round is open, applies round-close.

Tests: HE-01…06 (§8).

### 3.4 Auth layer (Root only; the agent never sees a credential)

#### 3.4.1 Auth kinds

Each provider entry lists its allowed kinds (catalog `auth`); the charter picks one when a provider allows several. Injection happens in the Root after every class, policy and money check, on the Root's own TLS connection. Every kind's secret material lives in the credential store (§3.4.2); every kind's token endpoint is a Root-internal destination (§3.4.5), never a zone-reachable class.

| Kind | Header or signature the Root adds | Secret held | Refresh | Providers |
|---|---|---|---|---|
| `bearer` | `authorization: Bearer <key>` | API key | — | OpenAI, DeepSeek, xAI, Moonshot, Z.ai, MiniMax, Alibaba, Mistral, Groq, Cerebras, Together, Fireworks, DeepInfra, SambaNova, OpenRouter, Vercel, Ollama Cloud, local servers with `--api-key` |
| `x-api-key` | `x-api-key: <key>` | API key | — | Anthropic (incl. Max/Team monthly API credits, which are Console keys), Foundry Claude (alt.) |
| `api-key` | `api-key: <key>` | API key | — | Azure OpenAI v1, Foundry |
| `goog-api-key` | `x-goog-api-key: <key>` (never `?key=` in the URL) | API key | — | Gemini (AI Studio) |
| `oauth_pkce` | `authorization: Bearer <access token>` | OAuth state: refresh token, issued client id, host id | access token 1 h, refresh token 30 d **rotating**; single-flight refresh (§3.4.4) | `openai-chatgpt` (Sign in with ChatGPT plan usage) |
| `aws_sigv4` | `authorization: AWS4-HMAC-SHA256 …`, `x-amz-date`, `x-amz-content-sha256`, `x-amz-security-token` when a session token exists | access key id, secret, optional session token | static; or `command` (AWS `credential_process` JSON) | Bedrock (`bedrock` for Converse and `bedrock-runtime` OpenAI routes; `bedrock-mantle` for Claude) |
| `google` | `authorization: Bearer <access token>` | service-account key (`client_email`, `private_key`, `token_uri`) or ADC user credentials (`client_id`, `client_secret`, `refresh_token`) | access token ~1 h, refreshed at 300 s margin | Vertex (Gemini and Claude publishers) |
| `azure_entra` | `authorization: Bearer <access token>` | tenant, client id, client secret | client-credentials grant, scope `https://ai.azure.com/.default` | Azure OpenAI v1, Foundry (alternative to `api-key`) |
| `command` | per `format`: a bearer token, or AWS `credential_process` JSON for SigV4 | the argv (not secret; charter) | `refresh_s` (default 3,000) | escape hatch for SSO and interactive cloud logins (§3.4.5) |
| `none` | nothing | — | — | LOCAL servers without auth |

Kinds that exist in the wild but are **not** implemented, because their routes are not ALLOWED (§3.4.7): Claude.ai OAuth, Gemini CLI / Antigravity Google login, Copilot `copilot_internal` exchange, Codex CLI's client against `chatgpt.com/backend-api`.

#### 3.4.2 Credential store

One trait, four backends, chosen by charter `keys.store` (default `"auto"`).

```rust
// root/crates/init-root/src/keys/mod.rs
pub trait Backend {
    fn name(&self) -> &'static str;                                // "keychain" | "secret-service" | "wincred" | "file"
    fn get(&self, item: &ItemId) -> io::Result<Option<Secret>>;    // Secret: no Debug/Display/Serialize, zeroed on drop
    fn put(&self, item: &ItemId, s: &Secret) -> io::Result<()>;
    fn delete(&self, item: &ItemId) -> io::Result<bool>;
    fn probe(&self) -> io::Result<()>;                             // put + get + delete of a canary item
}
pub struct ItemId { pub kind: &'static str, pub provider: String } // account string "<kind>/<provider>", e.g. "api/deepseek"
```

| Backend | Platform | Mechanism | Notes |
|---|---|---|---|
| `keychain` | macOS | the Apple-signed `/usr/bin/security` tool: `find-generic-password -s dev.init.root -a <account> -w` (secret on stdout, read through a pipe) and `add-generic-password -U -s dev.init.root -a <account> -l "INIT Root · <provider>" -w` with `-w` last so the secret is read from the pipe, never from argv (prompt-from-stdin behaviour UNVERIFIED; test KS-02 checks it on a scratch keychain) | Items created by `security` trust `security` in their ACL, so a rebuilt, unsigned `init-root` gets **no access prompt after every upgrade**. With the FFI route (`security-framework`), the item ACL names the creating binary's code identity, and an ad-hoc-signed Root would prompt after each release until the owner buys a Developer ID (V1_PLAN pending decision). The zone cannot reach the keychain: its profiles deny `mach-lookup` to every XPC service, including `com.apple.SecurityServer`, and deny `~/Library/Keychains` (kernel/sandbox.md §3, measured) |
| `secret-service` | Linux | `secret-tool store --label="INIT Root · <provider>" service dev.init.root account <account>` (secret on stdin) and `secret-tool lookup service dev.init.root account <account>` (stdout); needs libsecret's `secret-tool` and an unlocked collection (UNVERIFIED stdin behaviour; KS-04) | Headless hosts usually have no Secret Service, so `auto` falls back to `file`. The zone must not reach the session bus: the Linux zone mounts no `$XDG_RUNTIME_DIR` (bus socket, `keyring/control`); test S-L-KS (cross-doc kernel/sandbox.md) |
| `wincred` | Windows | `CredWriteW` / `CredReadW` / `CredDeleteW` with `CRED_TYPE_GENERIC`, target `dev.init.root/<account>`, `CRED_PERSIST_LOCAL_MACHINE`, through `windows-sys` (`Win32_Security_Credentials`; version UNVERIFIED) in `sys.rs` | Blob limit 2,560 bytes (`CRED_MAX_CREDENTIAL_BLOB_SIZE`, UNVERIFIED): larger items (a service-account key, ~1.9 KB reduced) fit; anything larger goes to `file`. Whether an AppContainer zone process can call `CredReadW` for the user is UNVERIFIED: test W-KS-1 must show it fails before `wincred` is allowed |
| `file` | all | `ROOT_HOME/keys/providers/<id>` and `ROOT_HOME/keys/oauth/<id>.json`, 0600 in 0700, R-F4 (today's rule, unchanged) | Always available; the only backend in `root.test_mode` (so tests never touch the owner's keychain) |

- **`auto`** = the platform backend if its `probe()` succeeds at start, else `file`, with alert `keystore_fallback` once.
- **Index.** `ROOT_HOME/keys/store.json` (0600) lists `{account, backend, kind, set_at}` per item, never a secret, so `keys check` knows where to look and `keys move --to keychain|file` can migrate.
- **Load.** Secrets are read at start and on `keys.reload` (and at OAuth refresh), held in memory as `Secret`, never written to logs, alerts, lineage, `charter.json` or any view. A backend error disables that provider (`class.disabled{reason:"key_unavailable"}`, alert `key_unavailable`), as a missing key does today.
- **Lineage.** `credential{provider, kind, action, backend}` with `action` ∈ `set`, `delete`, `login`, `logout`, `refresh_failed`, `moved`. No secret, no account email, no token claims.
- **Today's files keep working.** An existing `ROOT_HOME/keys/providers/deepseek` is found by the `file` backend whatever `keys.store` says; `keys move` migrates it on request.

#### 3.4.3 Owner UX: CLI, TUI and the line guard

```
init-root keys set <provider> [--kind api_key|aws|azure-client|google-sa|google-adc] [--from-file <path>] [--no-verify]
init-root keys check [--json]          backend, present | missing | insecure | locked | expired, per provider; never a secret
init-root keys delete <provider>       revoke where supported, then delete
init-root keys move --to keychain|file
init-root login <provider> [--manual] [--port N] [--no-browser]
init-root logout <provider>
```

- **`keys set`** reads each field from the TTY with echo off (never argv or env; exit 2 if not a TTY). `--from-file` reads a service-account JSON or an ADC file the owner names, keeps only the fields of §3.4.1, and prints a reminder that the source file is still on disk. Verification: if the provider has a free route (`GET models`), the Root calls it once with the new credential at $0; 401/403 → `keys.rejected`, the item is **not** stored; no free route → stored, `verified: "skipped"`.
- **TUI `keys.set` request** (owner.sock, §4.4). The TUI gets `/key <provider>`: it opens a masked single-line prompt, sends one `keys.set` request frame, overwrites its buffer and keeps nothing. The Root never logs the frame, never relays it to the kernel, never puts it in the replay ring or the owner queue, and accepts it only from a non-sandboxed same-uid peer (the existing owner.sock check). This is a trust-model change to tui.md T-2 ("no keys"), which becomes "never holds a key at rest; may carry one key once, in one `keys.set` frame" (open question Q2). The CLI path stays the reference path.
- **TUI `/login <provider>`** sends `login.start`; the Root daemon (a LaunchAgent in the owner's GUI session) opens the browser itself with `/usr/bin/open` (Linux `xdg-open`, Windows `explorer.exe`) and returns the URL, which the TUI shows as a notice so the owner can open it by hand. Completion arrives as a `notice{code:"login"}`.
- **Line guard.** Before an owner `line` is queued, echoed or relayed, the Root checks it for credentials: (1) any substring equal to a stored secret, always; (2) a hand-written prefix scan (the Root has no regex crate) equivalent to `(?:^|\s)(sk-(?:ant-|or-|proj-)?|xai-|gsk_|AIza|AKIA|ASIA|ghp_|github_pat_|glpat-|hf_)[A-Za-z0-9_\-]{20,}` and JWT shapes (`eyJ` + two dots, > 100 chars). A match is not forwarded, echoed, queued or logged; the owner gets `notice{level:"warn", text:"That looked like a credential, so it was not sent to the agent. Use /key <provider>."}`. False positives cost one rephrase; a leaked key costs a revocation.

#### 3.4.4 OAuth PKCE: Sign in with ChatGPT plan usage (provider `openai-chatgpt`)

The research verdict is **ALLOWED** (preview) for open-source and locally run apps through OpenAI's self-serve path. INIT ships it as a separate provider id, so plan traffic never mixes with API-key traffic.

**Sign-in** (`init-root login openai-chatgpt`, or `login.start` from the TUI):
1. Load or create the persistent host id: 16 random bytes from `/dev/urandom`, hex, item `oauth/openai-chatgpt/host` (not derived from hardware, user name or email).
2. Bind the loopback listener on `127.0.0.1:1455` (default; `--port`; whether any loopback port is accepted is UNVERIFIED, RFC 8252 says it should be). Start it **before** opening the browser. Never `localhost`.
3. Build the authorize URL: `https://auth.openai.com/api/accounts/authorize` with `response_type=code`, `client_id` = the saved issued id (`oaiapp_…`) or `dynamic_agent_client` on first use, `agent_name_hint=INIT Agent`, `ext_agent_host_id=<host id>`, `redirect_uri=http://127.0.0.1:1455/auth/callback`, `scope=openid profile email offline_access resource.invoke chatgpt.tokens.use.direct`, `resource=https://api.openai.com/v1`, `state` and `nonce` (16 random bytes each, base64url), `code_challenge = base64url(SHA-256(verifier))`, `code_challenge_method=S256`, verifier = 32 random bytes base64url (RFC 7636).
4. Accept exactly one `GET /auth/callback` within 300 s; check `state`; answer a fixed "You can close this tab" page; close the listener. The zone cannot reach loopback (its profiles allow only Unix sockets), so it cannot inject a callback.
5. Exchange: form POST to `https://auth.openai.com/api/accounts/oauth/token` (`grant_type=authorization_code`, `code`, `code_verifier`, `redirect_uri`, `client_id`, `resource`). Save the issued client id from the callback.
6. Verify the ID token's RS256 signature against the issuer's JWKS (discovery document path UNVERIFIED; PV-08 reads it from the docs), check `iss`, `aud`, `exp`, `nonce`, and that the granted scope includes `chatgpt.tokens.use.direct`. Keep no claims: the ID token (which contains the email) is dropped after the check (CONVENTIONS privacy rule).
7. Store `{v:1, client_id, access, access_exp, refresh, refresh_exp, scope}` as item `oauth/openai-chatgpt`, append `credential{action:"login"}`, `egress::reload()`.

`--manual` (SSH, no local browser): print the URL; the owner opens it elsewhere, then pastes the full redirect URL from the address bar into the prompt (or the TUI sends `login.complete`); steps 4–7 run on the pasted URL. Whether OpenAI accepts this for SIWC is UNVERIFIED.

**Refresh.** A timer thread refreshes when `access_exp − now < 300 s`; requests never wait on a refresh unless the token has actually expired. Refresh is **single-flight**: one mutex per OAuth item; a second refresher waits on a condvar for at most 30 s. Because the refresh token rotates, the new pair is **written to the store before** the in-memory token is swapped. A crash between the provider's answer and the write loses the session (the owner signs in again), but never leaves two processes holding different "current" refresh tokens. The CLI never refreshes while the daemon runs (it asks the daemon over owner.sock); with the daemon down it takes `flock` on `ROOT_HOME/keys/oauth/openai-chatgpt.lock`.

**Sign-out** (`init-root logout openai-chatgpt`): POST the refresh token to the issuer's `revocation_endpoint`, then delete the item, then `credential{action:"logout"}`. A failed revocation is reported, and the local tokens are deleted anyway.

**Plan mode** (dialect `siwc`, §3.3.3; Root policy, §3.5.3):
- `billing = "plan"`: no money is reserved or charged. The Root still parses usage and keeps per-provider counters (requests, tokens) per owner day and per rolling 7 days.
- Charter `[providers.openai-chatgpt] requests_per_day` (default 1,500) is a hard volume cap, so an evolving agent cannot drain the owner's whole plan; the per-app weekly cap the owner sets at chatgpt.com/settings/usage is the vendor's own outer bound.
- `cost_ref` for the gate and the self-model uses `shadow_price` (the API list price of the same model), so a candidate cannot look cheaper by routing to the plan.
- Errors: 429 `subscription_sharing_usage_limit_exceeded` (also mid-stream as `response.failed`) → kernel code `plan.exhausted`, not retried, alert `plan_exhausted` with the "Manage usage" link; 403 `subscription_sharing_user_not_eligible` → provider disabled, alert, not retried; 401 `subscription_sharing_invalid_user`, or `invalid_grant` on refresh → tokens deleted, alert `login_required`.
- Eligible plans per the developer docs: Plus and Pro. Other plans are UNVERIFIED; the error contract handles them.

#### 3.4.5 Cloud auth: SigV4, Google, Azure, command

- **SigV4** (`auth/sigv4.rs`, ~150 lines). Canonical request: method, URI-encoded path (each segment encoded once; `:` in Bedrock model ids encoded as `%3A`), sorted query, signed headers `host;x-amz-content-sha256;x-amz-date` (+ `x-amz-security-token`), `hex(SHA-256(body))`. String to sign `AWS4-HMAC-SHA256\n<amzdate>\n<date>/<region>/<service>/aws4_request\n<hex(SHA-256(canonical))>`. Signing key `HMAC(HMAC(HMAC(HMAC("AWS4"+secret, date), region), service), "aws4_request")`. HMAC-SHA256 is written over `sha2` 0.11.1 (RFC 2104, ~20 lines) rather than adding a crate; tests use the RFC 4231 vectors and AWS's published SigV4 test suite. Service names: `bedrock` (Converse, `bedrock-runtime` host), `bedrock-mantle` (Claude Messages), region from the charter. The Root already buffers the request body (it parses it for the class checks), so hashing it adds no extra pass; SigV4 needs a clock within 5 minutes, and a 403 that names the signature time is reported as `upstream.auth{hint:"clock"}`.
- **Google** (`auth/google.rs`). Service account: JWT `{"alg":"RS256","typ":"JWT"}` `{iss: client_email, scope: "https://www.googleapis.com/auth/cloud-platform", aud: token_uri, iat, exp: iat+3600}`, signed with `ring::signature::RsaKeyPair::from_pkcs8` + `RSA_PKCS1_SHA256` (ring 0.17 as resolved through rustls; exact version UNVERIFIED), POST `grant_type=urn:ietf:params:oauth:grant-type:jwt-bearer`. ADC user credentials: POST `grant_type=refresh_token` to `https://oauth2.googleapis.com/token`. Express-mode Vertex API keys are not used (unconfirmed on the routes INIT needs).
- **Azure Entra** (`auth/azure.rs`): POST `https://login.microsoftonline.com/<tenant>/oauth2/v2.0/token`, `grant_type=client_credentials`, scope `https://ai.azure.com/.default`. The `api-key` kind is the recommended default for Azure (simpler, no token endpoint).
- **Root-internal auth destinations.** `auth.openai.com:443`, `oauth2.googleapis.com:443`, `login.microsoftonline.com:443`: a fixed list compiled into the Root, used only by owner commands and refresh threads, never reachable through `egress.sock`, logged in `ROOT_HOME/egress/auth.jsonl` (time, provider, endpoint, status; no content). No redirects followed.
- **`command`** (opt-in): charter `[providers.<id>.auth] kind = "command"`, `argv` (no shell), `format = "bearer" | "aws-credential-process"`, `refresh_s`, `timeout_ms` (10,000). The Root runs it as the owner, outside every sandbox, with a cleared environment plus `HOME`, the install-time `PATH`, `PYTHONNOUSERSITE=1` and `PYTHONDONTWRITEBYTECODE=1`; stdout is the secret (never logged), stderr is truncated to 1 KiB and scanned by the line guard before logging. Charter rule: `argv[0]` must be absolute, a regular file owned by root or the owner, not group- or world-writable, and **outside the owner's home, `INIT_HOME` and every workspace**, because from v0.1.1 the zone may write the workspace and, in full mode, the home (ADR-009). `charter check` warns when `command` auth and `sandbox.allow_full = true` are both on: a tool that loads code from the home could still be tampered with in full mode (residual risk, §7).

#### 3.4.6 Honest client identity

- The Root sets `user-agent: init-root/<version>` on every upstream request (root/egress.md §3.2, unchanged) and nothing that names another client.
- SIWC registration uses `agent_name_hint = "INIT Agent"`. OpenRouter requests may carry `X-Title: INIT Agent` (honest attribution, no referer).
- The kernel → Root header allowlist (root/egress.md §4.1) never includes identity headers, so `x-claude-code-*`, `anthropic-beta: oauth-*`, `originator`, `Copilot-Integration-Id`, `x-stainless-*` and similar are dropped before the Root sees them; the Root also refuses an `anthropic-beta` value outside its allowlist (§3.5.3). A test asserts no shipped dialect renders such a header (AU-09).
- Kimi's terms treat User-Agent tampering as a violation; INIT never sets a foreign one, for any provider.

#### 3.4.7 Supported routes (verdicts from subscription_plans.md)

**Shipped in v1 (ALLOWED):**

| Route | Provider id | Auth | Notes |
|---|---|---|---|
| Pay-per-token API keys: OpenAI, Anthropic Console, Gemini AI Studio, xAI, DeepSeek, Moonshot (Kimi Open Platform), Z.ai general API, MiniMax pay-as-you-go, Alibaba Model Studio pay-as-you-go, Mistral, Groq, Cerebras, Together, Fireworks, DeepInfra, SambaNova | `openai`, `anthropic`, `google`, `xai`, `deepseek`, `moonshot`, `zai`, `minimax`, `alibaba`, `mistral`, `groq`, `cerebras`, `together`, `fireworks`, `deepinfra`, `sambanova` | key kinds | rows with [3P] base URLs ship disabled until PV-03 verifies them |
| Claude Max/Team monthly API credits | `anthropic` | `x-api-key` | they arrive as Console API credit, so this is the API-key route |
| Sign in with ChatGPT plan usage (Plus, Pro; preview) | `openai-chatgpt` | `oauth_pkce` | §3.4.4; valid while INIT stays open-source and local; a hosted INIT would need OpenAI's interest form |
| Vertex AI (Gemini and Claude), Bedrock (Claude via Mantle, OpenAI-compatible, Converse), Azure OpenAI v1 and Foundry | `vertex`, `bedrock`, `azure` | `google`, `aws_sigv4`, `api-key` / `azure_entra` | cloud bills are estimates at catalog rate cards; models without a researched rate card are unpriced |
| Gateways: OpenRouter, Vercel AI Gateway; self-hosted Cloudflare AI Gateway, LiteLLM, Portkey through the custom class | `openrouter`, `vercel`, custom | `bearer` | §3.5.4 |
| MiniMax M Plan subscription key | `minimax-plan` | `bearer` | ALLOWED per its usage rules; full terms UNVERIFIED (PV-03 reads them before enabling) |
| Cerebras Code, Ollama Cloud (key-based plans) | `cerebras-code`, `ollama-cloud` | `bearer` | vendor-documented third-party integrations; terms not fully read; Ollama Cloud base URL UNVERIFIED |
| Local servers | owner-registered ids | `none` / `bearer` | §3.5.5 |

**Not shipped:**

| Route | Verdict | Reason (research) |
|---|---|---|
| Reusing Codex CLI's OAuth client against `chatgpt.com/backend-api/codex` | GRAY | undocumented, unversioned backend; superseded by SIWC |
| Driving the unmodified `claude` binary / Agent SDK with the user's own Claude login | GRAY, and **rejected** | Anthropic's statements conflict (the Apr 4, 2026 "all third-party harnesses" vs the Help Center); and it replaces INIT's body loop (§2 non-goals) |
| GitHub Copilot SDK with INIT's own OAuth App | ALLOWED, but **rejected** | it runs Copilot's agent runtime instead of INIT's body loop (§2 non-goals) |
| `api.githubcopilot.com` with a fine-grained PAT; the `copilot_internal` token exchange | GRAY (suspension risk) | documented for Copilot CLI only; formal partnership exists only with OpenCode |
| Alibaba Coding Plan `sk-sp-` keys; Kimi Code membership keys | GRAY | interactive-only or listed-tools-only terms; INIT is not listed |
| OpenCode Zen/Go keys, Mistral Vibe subscription keys, Cursor SDK plan keys | GRAY | terms unread or not a model API |
| Spawning Gemini CLI or Antigravity CLI under the user's login | GRAY | Google's wording covers "third-party software … to access" |
| Claude Pro/Max OAuth by INIT (own client, reading Claude Code's token, `setup-token`, Claude Code headers); a "Sign in with Claude" button | **PROHIBITED** | "does not permit third-party developers to offer Claude.ai login"; enforced server-side since Jan 9, 2026 |
| Gemini CLI / Antigravity Google-login OAuth | **PROHIBITED** | explicit in the Gemini CLI FAQ and terms; bans in Feb 2026 |
| Z.ai GLM Coding Plan key | **PROHIBITED** until Z.ai lists INIT | "may only be used within officially supported tools" |
| Qwen OAuth free tier | N/A | discontinued Apr 15, 2026 |

**Enforcement.** The Root compiles a ToS denylist of `(host, path prefix, verdict, reason)`: `chatgpt.com` + `backend-api/`, `api.z.ai` + `api/coding/`, `coding-intl.dashscope.aliyuncs.com`, `api.kimi.com` + `coding/`, `api.kimi.ai` + `coding/`, `api.githubcopilot.com`, `cloudcode-pa.googleapis.com`, `api.anthropic.com` with an OAuth bearer. A catalog row (CAT-16) or a charter custom provider (§3.5.6) that matches is refused (`charter.route_denied`, with the reason). There is no owner override in v1.0 (open question Q3). Verdicts are re-read by PV-03 before each catalog release, because vendors change them often.

### 3.5 Egress classes v2 (Root)

#### 3.5.1 Provider registry = catalog ∩ charter

At start and on every reload the Root compiles the **registry**: for each provider the charter enables (`[providers.<id>]`, §4.2), the catalog entry (or the charter's own entry, for `local` and `custom` kinds) with its params expanded:

```rust
pub struct Provider {
    pub id: String,
    pub kind: Kind,                       // Api | Gateway | Cloud | Plan | Local | Custom
    pub dest: String,                     // exact "host:port" after params, e.g. "bedrock-mantle.us-east-1.api.aws:443"
    pub scheme: Scheme,                   // Https | Http (Local only)
    pub base_path: String,                // Root-side prefix after params, never published
    pub auth: AuthKind,
    pub routes: Vec<Route>,               // §3.5.2
    pub models: BTreeMap<String, ModelGate>, // model id → {min_level, price: Compiled | Plan | Local, catalog_row}
    pub geo_factor_ppm: u32,              // from the endpoint params (regional_endpoint, us_data_zone)
    pub limits: Limits,                   // requests_per_day, bytes_per_day, rate_per_min (provider-level, optional)
}
```

- Params are charter values (`region`, `resource`, `project`, `location`, `workspace`) checked by grammar (`^[a-z0-9][a-z0-9-]{0,62}$`; `project` also allows digits-first) before expansion, so no param can inject a host or a path.
- `dest` is exact: no wildcard, no subdomain glob, no IP literal except for `Local` (loopback only). This keeps root/egress.md's destination rule.
- The class `providers` stays the one model class in the charter's `[actions]` table (its schema, volume, rate and `min_level` rules are unchanged); its `destinations`, `methods`, `paths` and `models` keys are **derived** from the registry in charter schema 2 and are errors there (they remain valid in schema 1, §3.9).
- Matching order (root/egress.md §3.5) gains two steps: route → **provider** → class → destination → **route template** → method → level (class, then model) → volume → rate → **policy (§3.5.3)** → money.

#### 3.5.2 Route templates and the metering table (F2 default-deny, extended)

The egress wire is unchanged: `<METHOD> /<provider_id>/<zone-facing path>[?query]`. The Root forwards to `scheme://dest/base_path/zone-facing path`.

**Path template grammar.** A route `path` is `/`-separated segments; each segment is a literal (`[A-Za-z0-9._~-]+`, plus the literal tail `:action` such as `:streamGenerateContent`), or `{model}`, or `{model}:<action>`. `{model}` matches one segment of `[A-Za-z0-9._@-]+` with `%3A` decoded to `:` and no other percent-decoding. The existing traversal refusal (`..`, `//`, `%2e`, `%2f`) runs first. A route may fix one exact query string (`query = "alt=sse"`); any other query on a POST is refused `class.path`.

**Metering table.** The registry's routes *are* the F2 table: every forwarded `(method, path)` is `billed` (inspected, gated by model and level, reserved, settled with the route's usage parser) or `free` ($0, never inspected for a model, e.g. `GET models`, `POST …:countTokens`, `POST v1/messages/count_tokens`, `GET user/balance`). Anything else is refused `class.unmetered` and never forwarded, exactly as today. Free routes may not carry a request body larger than 64 KiB, except `count_tokens`-style routes, which get the normal 32 MiB limit.

**Model source.** For a billed route the model comes from the body's `model` field or, for templates, from `{model}`. When both exist they must be equal (`class.model` otherwise). The model must be in `Provider.models` (`class.model`) at a level the system has reached (`class.level`); both refusals demote as today (rule b).

Route sets per api (zone-facing paths, appended to each provider's own base):

| api | Billed | Free |
|---|---|---|
| `chat_completions` | `POST chat/completions` (+ provider-specific prefixes such as `beta/`) | `GET models` |
| `responses` | `POST responses` | `GET models` |
| `messages` | `POST messages` (Vertex: `POST {model}:streamRawPredict`, `POST {model}:rawPredict`) | `POST messages/count_tokens`, `GET models` |
| `gemini_generate` | `POST models/{model}:streamGenerateContent?alt=sse`, `POST models/{model}:generateContent` | `POST models/{model}:countTokens`, `GET models` |
| `gemini_interactions` | `POST interactions` | — (`GET interactions/{id}` is stateful and refused) |
| `converse` | `POST model/{model}/converse-stream`, `POST model/{model}/converse` | — |

#### 3.5.3 Request policy per api

The Root parses the request JSON it already buffers and applies these checks after the model and level gates. Every refusal is 403 with a `class.*` code; policy refusals demote under rule a (they are attempts at an external action outside every class), except `class.unpriced` and `class.unmetered`, which are configuration states.

| Check | Applies to | Code |
|---|---|---|
| Tools are client-executed only: CC `type = "function"`; Responses `function`, `custom`; Messages no `type` or `custom`; Gemini only `functionDeclarations` (no `googleSearch`, `googleSearchRetrieval`, `codeExecution`, `urlContext`, `fileSearch`, `computerUse`); Converse only `toolSpec` | all billed routes | `class.provider_tool` (existing, extended) |
| No `mcp_servers`, `container`, `fallbacks` (Messages); no `background: true`, `conversation`, `previous_response_id` (Responses); no `cachedContent` (Gemini); no `previous_interaction_id` (Interactions); no `guardrailConfig` (Converse) | per api | `class.provider_tool` |
| `store` must be `false` (Responses, Interactions) | per api | `class.policy` |
| No remote media: image and file parts must be inline (`data:` URLs, base64 `source`, `inlineData`, Converse `bytes`); refuse `http(s)://` image URLs, Anthropic `source.type = "url"`, Gemini `fileData`, Converse `s3Location` (a remote URL makes the provider, or a local server such as SGLang, fetch an attacker-chosen host) | all billed routes | `class.remote_fetch` (new; demotes) |
| `n` / `candidateCount` absent or 1 (the reservation assumes one completion) | all | `class.policy` |
| Modifiers (`service_tier`, `speed`, `inference_geo`, Converse `serviceTier`, `performanceConfig`) are neutral or priced (§3.2.5) | all | `class.unpriced` |
| `anthropic-beta` values ⊆ {`thinking-binding-controls-2026-08-01`, `thinking-display-updates-2026-08-18`} | Messages | `class.header` (new; demotes) |
| CC dialects with `include_usage = true` carry `stream_options.include_usage: true` when streaming (otherwise the stream would be unmeterable and charged in full) | chat | `class.unmetered` |
| Plan routes (`siwc`): `stream: true`, `store: false`; no `max_output_tokens`, `temperature`, `metadata`, `user`; no hosted tools | `openai-chatgpt` | `class.policy` |
| Gateway routing object (§3.5.4) | `openrouter` | `class.gateway` (new; demotes) |

`class.remote_fetch`, `class.header` and `class.gateway` join the rule-a list of root/egress.md §3.6.

#### 3.5.4 Gateway class (OpenRouter, Vercel AI Gateway)

A gateway forwards to one of many upstream endpoints, possibly at different prices, and by default falls back to others. The Root makes that deterministic and keeps its own price authoritative.

- **Catalog rows are per (gateway, model, upstream).** E.g. `openrouter/anthropic/claude-sonnet-5-5` with `gateway = { only = ["anthropic"] }` and `price.from = "anthropic/claude-sonnet-5-5"` (OpenRouter passes provider prices through with no inference markup), plus provider-level `account_fee_permille = 55` (5.5% credit purchase fee). A model id with a routing suffix (`:nitro`, `:floor`, `:online`) has no row, so it is refused as unpriced: those suffixes change the service tier or add web search.
- **Required routing object** (rendered by the `openrouter` dialect as a constant, checked by the Root): `provider = {allow_fallbacks: false, only: <the row's list>, require_parameters: true, data_collection: "deny", max_price: {prompt: <row input in USD/MTok>, completion: <row output>}}`, `transforms: []` (no prompt compression; OpenRouter's default for some models is UNVERIFIED), no `models` array, no `route`, no `plugins`. `data_collection: "allow"` is accepted only if the charter sets `[providers.openrouter] data_collection = "allow"`. `max_price` is OpenRouter's own hard cap, so even a gateway-side routing change cannot cost more than the row.
- **Own price is the meter; reported cost is a cross-check.** The Root settles each request at `resolve(row) × tokens` (§3.2.4). It also reads `usage.cost` (OpenRouter credits, assumed 1 credit = $1 per its USD base currency; UNVERIFIED) and stores it as `reported_musd` on the `end` record. If `reported > own × 1.02`, the settlement is raised to `reported` (a gateway figure can raise a settlement, never lower it) and `price_drift{provider, model, own_musd, reported_musd}` is alerted at most once an hour per model; if `reported < own × 0.98`, only the alert fires (the catalog may be stale-high). The 2% threshold is the research's reconciliation guard.
- **Vercel AI Gateway** (`vercel`, OpenAI CC, Responses and Anthropic Messages at `https://ai-gateway.vercel.sh`): own pricing only. Its `total_cost` arrives asynchronously from `GET /v1/generation?id=` and is not fetched in v1. Its budgets are soft and BYOK can fall back to system credentials, so INIT's ceiling is the hard one. Caching stays pass-through (no `providerOptions.gateway.caching`).
- **Self-hosted gateways** (Cloudflare AI Gateway, LiteLLM, Portkey, Bifrost) are custom providers (§3.5.6) with owner-declared prices. `charter check` warns that LiteLLM 1.82.7–1.82.8 shipped a credential stealer (Mar 2026) and that INIT's own key should not be shared with such a gateway.

#### 3.5.5 LOCAL class (loopback LLM servers)

```toml
[providers.qwen-local]                    # charter; the owner registers it with `init-root local add`
kind = "local"
server = "ollama"                          # ollama | llama-server | vllm | sglang | lmstudio | mlx-lm | jan | localai | openai-compatible
endpoint = "127.0.0.1:11434"               # exact IPv4 or [::1] literal + port; never a hostname, never 0.0.0.0
api = "chat_completions"                   # or "messages" / "responses" where the server supports it
auth = "none"                              # or "bearer" with a key in the store
models = { "init-qwen3.6-35b-a3b-ctx64k" = { min_level = 0, context = 65536, max_out = 16384 } }
```

- **Destination.** Exactly the registered `ip:port`, plain HTTP, `Host: <ip>:<port>` (the literal, defeating Host confusion), no redirects. Hostnames, `localhost`, wildcard binds and non-loopback addresses are refused at `charter check` (`charter.local_endpoint`). Plain HTTP is allowed for this class only; test mode keeps its existing loopback rule for mocks.
- **(method, path) allowlist per server kind**; everything else is refused `class.path` (demotes):

| Server | Allowed | Explicitly denied examples (the reason the list exists) |
|---|---|---|
| all | `POST v1/chat/completions`; `GET v1/models` (free) | — |
| `ollama` | + `POST v1/responses`, `POST v1/messages` | `api/pull`, `api/create`, `api/push`, `api/delete`, `api/copy`, `api/blobs/*` (unauthenticated model management, CVE-2026-7482 reachable through model loading); `api/generate`, `api/chat` (unmetered here) |
| `llama-server` | + `POST v1/responses`, `POST v1/messages` | `slots*` (disk writes with `--slot-save-path`), the `--tools` / `--agent` endpoints (`exec_shell_command`, `write_file`), `POST props`, `completion`, `infill`, `lora-adapters` |
| `vllm` | + `POST v1/responses`, `POST v1/messages` | `collective_rpc`, `pause`, `sleep`, `wake_up`, `reset_prefix_cache`, `v1/load_lora_adapter`, `start_profile` (dev mode) |
| `sglang` | — | `update_weights*`, `flush_cache`, `server_info`, `get_server_info`, profiling |
| `lmstudio` | + `POST v1/responses`, `POST v1/messages` | `api/v0/*` |
| `mlx-lm`, `jan`, `localai`, `openai-compatible` | as "all" | everything else |

- **The agent sandbox still denies loopback.** Zone profiles allow network only to Unix sockets (kernel/sandbox.md); the Root, outside the sandbox, is the only component with a rule to the local port. The agent can therefore never reach a keyless server's management endpoints, even though the server itself has no auth.
- **Context length is set explicitly.** Registration requires `context ≥ 32,768` (the floor; silent truncation corrupts tool loops) and verifies it where a read-only endpoint reports it: Ollama `POST api/show` (Root-internal only, never zone-reachable), llama-server `GET props` (`n_ctx`), vLLM `GET v1/models` (`max_model_len`); endpoint names and fields UNVERIFIED (PV-16). Ollama ignores context settings on its OpenAI path, so `init-root local add --server ollama --context 65536` creates a derived model with the context baked in, `init-<model>-ctx64k`, by calling `POST api/create` with `{"from": <model>, "parameters": {"num_ctx": 65536}}` **as an owner command** (request shape UNVERIFIED), or tells the owner to set `OLLAMA_CONTEXT_LENGTH`. A mismatch between the declared and the reported context refuses the registration; an unverifiable server is registered with `ctx_known = false` and the body budgets compaction on the declared value.
- **Fingerprint.** `GET v1/models` must answer in the shape the declared `server` kind produces; default ports collide (8080: MLX-LM, LocalAI, older llama-server; 1337: Jan, Atomic Chat), so the kind is never guessed from the port. `init-root local scan` lists what answers on the known default ports (read-only `GET v1/models` on loopback).
- **Exposure check.** `init-root doctor` connects to the registered port on each of the host's non-loopback addresses; a successful connect warns `local_exposed` ("your <server> listens on the LAN"). Dangerous server flags (`--tools`, `VLLM_SERVER_DEV_MODE`, Jan's server-side MCP) cannot be detected over HTTP; `local add` asks the owner to confirm they are off, and the path allowlist makes them unreachable through INIT either way.
- **Tool-call conformance probe.** `local add` runs the §3.3.6 probe at $0 against the server. A model that fails is registered with `tool_calling = "failed"` and is not selectable as the default (`/model` explains why); the owner may override with `--accept-probe-failure`.
- **Billing and limits.** `billing = "local"`: no money; tokens counted (labelled approximate where the server's counts are, e.g. Ollama's Anthropic path); the class volume and rate limits apply. Header timeout 600 s (model load), idle 300 s.
- **UX hints.** The catalog's `local_recommendations` table (no prices) lists the research's tiers for the picker: 16 GB gpt-oss-20b (chat-grade); 24–36 GB Devstral Small 2 or Qwen3.6-35B-A3B / GLM-4.7-Flash at Q4; 48–64 GB Qwen3.6-35B-A3B at Q6–Q8 or Qwen3-Coder-Next at Q4 (memory figures UNVERIFIED).

#### 3.5.6 Custom base-URL providers

```toml
[providers.corp-gw]
kind = "custom"
base_url = "https://llm.corp.example:8443/v1"     # https only; exact host:port; path becomes base_path
api = "chat_completions"                            # one of the six apis
dialect = "generic"                                 # any dialect of that api
auth = "bearer"
models = { "corp-coder-1" = { min_level = 0, context = 131072, max_out = 16384,
                              price = { input = "1.00", cached_input = "0.10", output = "3.00" } } }
```

- The route set is the api's standard set (§3.5.2) under `base_path`; the usage parser is the dialect's.
- **Every model needs a price** (`price` block, or `price.from` naming a catalog row, or `billing = "local"`, which is refused for non-loopback hosts). No price ⇒ the model is never routable (§3.2.5).
- Destinations matching the ToS denylist are refused (`charter.route_denied`); IP literals and plain HTTP are refused (`charter.custom_endpoint`); a LAN server must present TLS.

#### 3.5.7 Metering pipeline v2

```
admission: route → provider → class → dest → template → method → level → volume → rate → policy
           → resolve(row, t, in_est) → reserve r (§3.2.4)            billing=plan: r=0, plan counters; local: r=0
stream:    forward frames unchanged; tap = usage parser of the route (§3.3.10); finish class
end:       usage → resolve(row, t_admit[, t_end], prompt_total, reported modifiers) → cost
           gateway: cross-check reported cost (§3.5.4)
           settle(r, cost); ledger end {cost_musd, usage{input_uncached, cache_read, cache_write_5m,
           cache_write_30m, cache_write_1h, output, reasoning}, window, tier, factor_ppm, billing,
           reported_musd?, catalog_version, flags}
```

- The Root's `Usage` grows from `{input, cache_read, output}` to the kernel's class split (kernel/proxy.md §3.10.2), so cache writes are billed at their TTL price instead of the input price. The tap reads only lines containing usage, finish or error needles (unchanged design), plus binary `metadata` frames for Converse.
- Parsers in the Root: `chat:openai`, `chat:deepseek`, `chat:openrouter`, `messages`, `responses`, `gemini`, `interactions`, `converse`. A dialect maps to one of them; a dialect whose usage shape needs a new parser needs a Root release (CAT-10), which is deliberate: the agent cannot teach the meter.
- `ceilings.json` `models[]` gains `billing`, `cached_input_musd_per_mtok`, `window` (the one in force) and `catalog_version`; the kernel's guard keeps reading the Root's spend, not recomputing it.

### 3.6 Kernel proxy changes

- **Routes from the view.** `Preset` (three hard-coded tables, `routes.rs`) is replaced by the registry routes published in `catalog.json` for every provider listed in `ceilings.json` `models`. The kernel table stays a stricter subset of the Root's (kernel/proxy.md §3.5) and keeps its family per route. `{model}` templates are matched with the Root's grammar; `query_not_allowed` now permits the route's fixed query.
- **Model source.** For templated routes the kernel's request parser takes `model` from the path (kernel/proxy.md §3.6).
- **Families.** `openai-chat`, `anthropic`, `openai-responses`, `none` plus `gemini`, `gemini-interactions`, `converse`; each with the §3.3.10 mapping, first-token and tool-call rules (Gemini: first part with text or `functionCall`; Converse: first `contentBlockDelta`).
- **Measurement prices** come from the view (current-window and peak vectors, tiers, multipliers) when `[providers.<id>.models]` is absent in `config.toml`; a config table, when present, overrides for measurement only and is warned about at start if it differs from the view (Q-EG5 behaviour, unchanged). `cost_ref` uses peak, or `shadow_price` for plan rows.
- **Default model.** The kernel takes the default from `INIT_HOME/root/model.json` (§3.7) when present, else `model.id`. A change triggers a handoff boot at the next idle boundary (body/provider.md §3.10).
- **Gate snapshot.** `open_gate` also records `catalog_version`; both arms are measured under it.
- **New proxy error codes** mapped from the Root: `egress.unpriced`, `egress.remote_fetch`, `egress.header`, `egress.gateway`, `egress.policy`, `plan.exhausted`; none is retried.

### 3.7 Model routing and selection UX hooks

- **Defaults per provider** (catalog `default_model`, re-checked by PV-03): `deepseek` → `deepseek-flash`; `anthropic` → `claude-sonnet-5-5`; `openai` → `gpt-6.1-sol`; `openai-chatgpt` → `gpt-6.1-sol` (plan eligibility UNVERIFIED); `google` → `gemini-4-argon` (price doubles 2026-12-01; the picker says so); `xai` → `grok-4.7`; `moonshot` → `kimi-k3`; `zai` → `glm-5.3`; `minimax` → `MiniMax-M3`; `alibaba` → `qwen3.8-max`; `mistral` → Mistral Medium 3.5 (id 3p, disabled until verified); `groq`, `cerebras` → `gpt-oss-120b`; `together`, `fireworks`, `deepinfra` → their DeepSeek V4 Pro row; `openrouter` → `anthropic/claude-sonnet-5-5`; LOCAL → the registered model. The system default stays `deepseek/deepseek-flash` at effort `high` (CONVENTIONS §6).
- **Owner-selected default** (new). `model.select{provider, model, effort}` on owner.sock (and the line `/model <provider>/<model> [effort]`) is owner authority, so it is a Root request. The Root checks the model is in the registry with `min_level = 0`, priced, `ready` (credential present, conformance not failed), writes `ROOT_HOME/state/model.json`, publishes `INIT_HOME/root/model.json`, appends lineage `model_select{provider, model, effort}`, and sends the kernel a `model` event. At L0 the egress forwards **only** this model (it replaces root/egress.md's "the owner lists exactly one model at `min_level = 0`"); safe mode uses it too. If the body cannot boot the new model (exit 78), the kernel reverts to the previous default and alerts `model_rejected`.
- **`/model` picker (TUI).** `/model` with no argument sends `models.list` (§4.4) and renders a picker: provider groups, model, context, `$in / $out` per MTok (current window), a dot for `ready`, badges `plan`, `local`, `not live-tested` (`wire_verified ≠ "live"`), `price changes <date>`, `ends <valid_until>`; the current default marked; unselectable rows dimmed with the reason (no key, level, probe failed). Enter sends `model.select`; a second column picks effort from the model's `effort` keys. `init-root attach` users type `/model` and get the same list as a text notice.
- **L1 per-subtask routing** (kernel/proxy.md §3.5a, unchanged in mechanism). The routable set becomes: `ceilings.json` models with `routable: true` (registry ∩ charter level ∩ credential present) ∩ codecs this body build implements ∩ `model.routable` from kernel config. The body's router (evolvable, seed policy: no routing) gets the catalog's `tier`, prices, context, `tools`, `forced_tool_choice`, `images` and `replay` to choose with. **Hook for loop.md:** a subtask routed to a different *provider* or *model* runs in its own context (a child message list), never as a mid-conversation switch, because a switch drops thinking and every cached prefix (§3.3.8 rule 3).

### 3.8 Root size and dependencies

All of this is Root code by necessity: credentials, auth flows, prices and usage parsing for metering must sit outside the agent's reach. Estimates (non-blank, non-comment lines, root/root.md §4.8 method):

| Module | Content | Estimate |
|---|---|---|
| `catalog/` (schema, CAT rules, compile, view, update, signature) | §3.2 | 560 |
| `catalog/price.rs` (**core**) | resolver: changes, windows, calendars, tiers, multipliers, reservation | 180 |
| `egress/registry.rs` | providers, params, templates | 260 |
| `egress/policy.rs` | §3.5.3 | 220 |
| `egress/tap.rs` (delta) | class split, Gemini, Interactions, Converse frames, OpenRouter cost | +260 |
| `egress/local.rs` | LOCAL class, probes, `local add/scan`, exposure check | 220 |
| `keys/` | trait, four backends, index, move, line guard | 420 |
| `auth/` (static, SigV4, Google, Azure, command, token cache) | §3.4.1, §3.4.5 | 560 |
| `auth/oauth.rs` | §3.4.4 | 380 |
| `link.rs` (delta) | the owner.sock requests of §4.4 | +150 |
| **total** | | **≈ +3,210** |

Under the provisional rule of root/root.md §4.8 the budget grows by these lines (measured + 5% at release), and the core subtotal by `catalog/price.rs` (+190). The open choice is recorded as Q4. New direct dependencies: none beyond `ring` (already in the tree through rustls; now used directly for RS256) and, on Windows only, `windows-sys` for `wincred` (version UNVERIFIED). HMAC, base64(url), JWT assembly and the AWS event-stream CRC32 are hand-written and vector-tested.

### 3.9 Migration plan

Each stage ships on its own and keeps the DeepSeek path working; the stage's gate is the full existing suite plus the named new tests.

| Stage | What changes | Compatibility rule | Gate |
|---|---|---|---|
| S1 Catalog in the Root | embedded catalog; `catalog/price.rs`; view published; Root `Usage` class split | a schema-1 charter's `[actions.providers.models.*]` prices **override** the catalog for those models (owner overrides, §3.2.6), so v0.1 behaviour is byte-for-byte the same; `meter.window = "conservative"` with the seed calendar's `known_through = ""` bills peak on every date, as v0.1 does | EG-01…11, R3-*, CAT-T* |
| S2 Registry and routes | registry from catalog ∩ charter; `PRESETS`, `ROUTES` and the `providers` part of `BUILTIN` removed (the built-in `crates` class stays); policy checks; charter schema 2 (`[providers.*]`, `[keys]`, `[meter]`, `[catalog]`) | schema 1 is still accepted and mapped: `destinations`/`paths` → `[providers.deepseek]` with today's routes; `charter check` prints the schema-2 equivalent; the template moves to schema 2 | EV2-*, EG-10 (F2) unchanged |
| S3 Keystore | backends, index, `keys move`, line guard, `keys.set` | today's `keys/providers/<id>` files are read by `file` whatever `keys.store` says | KS-* |
| S4 Kernel v2 | routes and measurement prices from the view; families; `model.json` | `[providers.<id>]` in config schema 2 keeps working (prices = measurement overrides); config schema 3 makes `models` optional | P-01…09 unchanged; KP-* |
| S5 Body codec framework | `Api` + dialect; `ModelInfo` from the view; per-api renderers in log-context; replay table | the `deepseek` dialect renders **byte-identical** requests (golden test), so live sessions keep their cache; unknown `api` strings in logs read as "other api" | T-PROV-1…6, CDX-* |
| S6 New codecs, dialects, auth kinds, LOCAL, gateways, SIWC | PV-07…PV-16 | additive | per WP |
| S7 UX hooks | `/model`, `/key`, `/login`, `/providers` | the TUI keeps working against an older Root (requests answer `root.unknown_cmd`; the TUI hides the features) | UX-* |
| S8 Release gates | PV-03 verification, PV-19 live DeepSeek, PV-20 security review | — | §8 |

### 3.10 Work packages

Agent-hours are design estimates for one build agent per WP, including its $0 tests; they exclude review. "Live" spend is Root-metered against the owner's $5 DeepSeek cap (V1_PLAN item 5).

| WP | Title | Area | Depends on | Agent-h | $0 tests (and live, where any) |
|---|---|---|---|---|---|
| PV-00 | Fold this doc into body/provider.md, root/egress.md, root/root.md, kernel/proxy.md, kernel/config.md, tui.md, kernel/sandbox.md, testing.md; ledger `reconciliation/providers-v2.md`; CONVENTIONS §4 rows | docs | — | 4 | doc lint |
| PV-01 | Catalog engine: schema, CAT-1…16, compile to µUSD, `price.rs` resolver, reservation, view writer, signed update, `catalog status/diff/update/rollback` | root | — | 12 | CAT-T01…12 (unit: windows, holidays at tz edges, tiers, dated changes (d) rule, promos, multipliers, rounding; signature good/bad/downgrade) |
| PV-02 | Seed catalog data from llm_api_providers.md Q3 with the §3.2.8 flags; providers, routes, schedules, calendars | data | PV-01 schema (this doc §3.2.2 is enough to start) | 5 | `catalog check` clean; CAT-T13 every research row present or listed as excluded |
| PV-03 | Verification lane: re-fetch every `source_url` and verdict; fill CN holidays; verify every UNVERIFIED wire fact in §3.3–3.5 against official references; record fixtures from official examples | data/research | PV-02 | 8 | `catalog-fresh` CI check; no spend |
| PV-04 | Root meter v2 (the complete formula of §3.2.4): class-split `Usage`, settlement via `price.rs`, peak and conservative windows with the clock check, decrease floors and `catalog approve`, plan and local billing, gateway cross-check, new ledger fields, `ceilings.json` fields | root (core) | PV-01 | 16 | MT-01…10 (fixtures per parser: hand-computed costs, MF-01…10, tier at threshold ±1, cache-write TTL classes, Gemini thoughts sum, OpenRouter drift both ways, unpriced modifier) |
| PV-05 | Registry and egress classes v2: params, templates, `{model}` gating, fixed queries, policy table §3.5.3, ToS denylist, charter schema 2 and schema-1 mapping, custom providers, gateway object check | root | PV-01 | 16 | EV2-01…14; EG-01…11 and R3-* unchanged |
| PV-06 | Credential store: trait, `file`/`keychain`/`secret-service`/`wincred`, index, `keys set/check/delete/move`, verification probe, line guard, `keys.set` request | root | — | 12 | KS-01…08 (keychain tests use a scratch keychain file created by the test, never the login keychain; Secret Service only in a disposable VM; Windows in CI VM) |
| PV-07 | Auth kinds: static headers, SigV4, Google SA/ADC, Azure Entra, `command`; token cache; Root-internal auth destinations | root | PV-06 | 14 | AU-01…08 (RFC 4231 HMAC, AWS SigV4 test-suite vectors, RFC 7515 A.2 RS256, mock token endpoints in `init-testkit`) |
| PV-08 | SIWC OAuth PKCE: login (browser and manual), dynamic registration, JWKS check, single-flight rotating refresh, revoke on logout, plan mode policy and counters, error contract | root | PV-07 | 12 | OA-01…09 against a mock authorization server (`init-mockllm --oauth`, testing.md); RFC 7636 App. B PKCE vector |
| PV-09 | Kernel proxy v2: routes and prices from the view, templated model extraction, new families, `model.json` default, gate catalog snapshot, new error mappings | kernel | PV-01 (view format), PV-05 (route contract) | 12 | KP-01…08; P-01…09 unchanged |
| PV-10 | Body codec framework: `Api` + dialect, `ModelInfo` from the view with fallback table, `Replay`/`Carrier`, handoff generalization, boot check | body | PV-01 (view format; can use a fixture view) | 12 | CDX-01…06; T-PROV-1…6; golden DeepSeek bytes unchanged |
| PV-11 | Chat Completions dialects (§3.3.6) incl. id rules, `extra` objects, think-tag splitting, usage shapes | body | PV-10 | 16 | CDX-10…24, one fixture set per dialect (stream, tool loop with replay, 400 shape, max-tokens stop) |
| PV-12 | Live renderers for Messages and Responses in log-context (closes WP10), host dialects (§3.3.3–3.3.4), `siwc` plan mode, codec-aware history edits and round-close (§3.3.12) | body | PV-10 | 17 | CDX-30…38; HE-01…06; A2 cache-prefix checks per api at $0 (mock `prefix`) |
| PV-13 | Gemini generateContent (AI Studio and Vertex), `gemini/parts.rs` | body | PV-10 | 12 | CDX-40…46 (doc-derived fixtures: thought signatures, function ids, thoughts usage, finish reasons) |
| PV-14 | Gemini Interactions (optional for v1.0) | body | PV-13, PV-03 (event names) | 8 | CDX-47…50 |
| PV-15a | Bedrock Mantle Messages and OpenAI-compatible routes | body + root | PV-07 (SigV4), PV-12 | 4 | CDX-51…52; AU SigV4 for `bedrock-mantle` |
| PV-15b | Bedrock Converse codec + AWS event-stream framing in body, kernel and Root taps (optional; v1.1 recommended) | body + kernel + root | PV-15a | 12 | CDX-53…58 (CRC errors, split frames, reasoning signature replay) |
| PV-16 | LOCAL class UX: `local add/scan/remove`, probes, context enforcement, Ollama derived model, exposure check, conformance probe | root | PV-05, PV-11 | 12 | LOC-01…10 against `init-mockllm --server <kind>` fakes; live only against the owner's own local server if one is running, $0 |
| PV-17 | TUI and Root UX hooks: `models.list`, `model.select`, `providers.list`, `login.*`, `/model` picker, `/key`, `/login`, `/logout`, `/providers`, Root line handling | tui + root | PV-06, PV-08, PV-09 | 10 | UX-01…08 (TUI-* harness; `keys.set` never in replay, history or logs) |
| PV-18 | Mocks and fixture corpus (incl. a strict thinking-binding mode for HE tests): `init-mockllm` renders Messages/Responses/Gemini/Interactions/Converse and the CC dialects; `--oauth` and token-endpoint mocks; `--server <kind>` local fakes; conformance runner | testkit | PV-10 shapes (start in parallel) | 15 | self-tests; every CDX fixture replays through both body decoder and Root/kernel taps |
| PV-19 | Live DeepSeek validation (the only live provider) on its **three** surfaces, checks L1–L7 of §3.10.1 | live | PV-04, PV-09, PV-11, PV-12 | 4 | **≤ $0.40 Root-metered, off-peak; no new check after $0.25**; recorded `.sse` files become fixtures |
| PV-20 | Security review (R-SEC style) of PV-04…08, PV-16: key paths, line guard, policy bypasses, OAuth state/PKCE, `command` hardening, LOCAL path allowlist | review | PV-04…08, PV-16 | 8 | findings fixed in place |

**Totals:** v1.0 scope (all but PV-14 and PV-15b) ≈ **221 agent-hours**; with both optional WPs ≈ **241**.

**Parallel plan** (seven lanes; critical path ≈ 48 h: PV-06 → PV-07 → PV-08 → PV-17; PV-19 (ready at ≈ 40 h) and PV-20 (ready at ≈ 38 h) finish inside it, then PV-00's final fold, ≈ 52 h in all):

| Lane | Sequence |
|---|---|
| Root catalog and meter | PV-01 → PV-04 |
| Root egress | (after PV-01) PV-05 → PV-16 |
| Root auth | PV-06 → PV-07 → PV-08 |
| Body core | PV-10 → PV-12 → PV-13 → (PV-14) |
| Body dialects and mocks | PV-18 → PV-11 → PV-15a → (PV-15b) |
| Kernel and TUI | (after PV-01, PV-05 contract) PV-09 → (after PV-08) PV-17 |
| Data | PV-02 → PV-03 (finishes within 7 days of the release tag) |

Then PV-19 and PV-20, then PV-00's final fold. **Honesty rule for the release notes:** only DeepSeek's three surfaces are verified live. Every other provider, dialect and auth flow is verified against recorded or doc-derived fixtures and mock servers, and the catalog says so per row (`wire_verified`), which `/model` shows.

#### 3.10.1 Live validation on DeepSeek (PV-19)

DeepSeek serves Chat Completions, Responses (`/responses`) and Anthropic Messages (`/anthropic/v1/messages`) with the owner's existing key, so **three of the four codec families** (`chat_completions`, `responses`, `messages`; not Gemini) can be tested live inside the $5 cap. Each surface is driven by the real body, kernel and Root with `deepseek-flash` at effort `high`.

| Check | Per surface (×3) | Verifies |
|---|---|---|
| L1 text turn | 1 request, ~4k input (INIT head), ≤ 300 output | request bytes accepted; stream decode; usage parse in body, kernel and Root agree |
| L2 tool loop | a scripted task needing 4 `bash` calls; ~5 requests | tool-call assembly, reasoning replay on every later request (DeepSeek's mandatory rule on CC; the carrier the other two surfaces really return, recorded as fact for the catalog) |
| L3 cache | the L2 conversation's next turn | `cache_read > 0`; Root bills it at the cache-read price |
| L4 max-tokens stop | `max_tokens = 64` | `MaxTokens` mapping per surface |
| L5 history edit | a forced mask epoch inside an open round (test hook `INIT_TEST_FORCE_EPOCH`), on the surface's `edit_rule` | `free` on CC keeps `reasoning_content`; round-close on Messages and Responses is accepted |
| L6 error shape | omit `reasoning_content` on a CC tool turn (CC only); a malformed body on the other two | the 400 is classified, not retried, costs 0 |
| L7 handoff | CC → Messages surface in one session (same model, other api) | thinking dropped; the flatten fallback where DeepSeek requires it (V-DS-3) |

- **Budget.** About 30 requests in all, ≈ 150k input tokens (mostly cached after L1) and ≈ 12k output: ≈ $0.06 at peak list prices, ≈ $0.03 off-peak. Hard cap **$0.40 Root-metered** (a charter `money.day_usd = "1.00"` for the run, as in build-deviations §3.17), **no new check after $0.25**, one retry per check at most.
- **Timing.** Run inside a conservative off-peak span so the run also validates §3.2.4: the Root must grant off-peak (after a fresh `Date` sample) and its total must stay ≥ DeepSeek's charge.
- **Reconciliation.** `GET user/balance` before and after the whole batch; the balance delta must be ≤ the Root's total and within the kernel's measured cost ± the balance's display precision (the field's resolution is UNVERIFIED; if it is coarser than the run's cost, only the upper-bound check is asserted).
- **Outputs.** Every stream is saved as an `.sse` fixture under `seed/src/provider/testdata/deepseek-{chat,responses,messages}/`; the catalog's DeepSeek rows and the three dialects get `wire_verified = "live"`. Everything else stays `fixture` or `docs`.


## 4. Interfaces

### 4.1 Catalog provider entries for the other kinds

§3.2.2 shows an `api` provider. The other kinds add `params`, templated hosts and multiple apis:

```toml
[providers.vertex]
kind = "cloud"
params = ["project", "location"]                   # charter supplies values; grammar §3.5.1
host = "aiplatform.googleapis.com:443"             # "global"; multi-region hosts via `host_by_param`
host_by_param = { location = { us = "aiplatform.us.rep.googleapis.com:443", eu = "aiplatform.eu.rep.googleapis.com:443" } }
base_path = "v1/projects/{project}/locations/{location}/publishers"
auth = ["google"]
geo = { regional_endpoint = "1.10" }               # applied when location ≠ "global"
routes = [
  { method = "POST", path = "anthropic/models/{model}:streamRawPredict", api = "messages", dialect = "vertex", meter = "billed" },
  { method = "POST", path = "google/models/{model}:streamGenerateContent", query = "alt=sse", api = "gemini_generate", dialect = "vertex", meter = "billed" },
]
tos = "allowed"

[providers.openai-chatgpt]
kind = "plan"
host = "api.openai.com:443"
base_path = "v1"
auth = ["oauth_pkce"]
oauth = { issuer = "https://auth.openai.com", authorize = "/api/accounts/authorize", token = "/api/accounts/oauth/token",
          scopes = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct",
          resource = "https://api.openai.com/v1", dynamic_client = "dynamic_agent_client", redirect_port = 1455 }
routes = [ { method = "POST", path = "responses", api = "responses", dialect = "siwc", meter = "billed" } ]
tos = "allowed"

[providers.openrouter]
kind = "gateway"
host = "openrouter.ai:443"
base_path = "api/v1"
auth = ["bearer"]
dialect = "openrouter"
account_fee_permille = 55
routes = [ { method = "POST", path = "chat/completions", api = "chat_completions", meter = "billed" },
           { method = "GET",  path = "models", meter = "free" } ]
tos = "allowed"

[server_kinds.ollama]                              # LOCAL templates (no host; the charter gives the endpoint)
default_port = 11434
dialect = "ollama"
apis = ["chat_completions", "responses", "messages"]
ctx_probe = { method = "POST", path = "api/show" }  # Root-internal only
fingerprint = { path = "v1/models", expect = "object=list" }
```

### 4.2 Charter schema 2 (cross-doc: root/root.md §3.12 owns the file; root/egress.md owns `[providers]` and `[actions]`)

| Key | Type | Default | Meaning |
|---|---|---|---|
| `schema_version` | int | `2` (1 still accepted, §3.9) | |
| `providers.<id>.enabled` | bool | `true` | the provider is in the registry |
| `providers.<id>.models` | table: model id → int (min_level) or `{min_level, catalog?, context?, max_out?, price?}` | — (≥ 1 model) | the per-level allowlist (replaces `actions.providers.models`); `catalog` maps an Azure or Foundry deployment name to a catalog row for capabilities and price |
| `providers.<id>.params` | table of strings | `{}` | `region`, `resource`, `project`, `location`, `workspace` (§3.5.1) |
| `providers.<id>.auth` | string or table | the first of the catalog's `auth` | `{kind = "command", argv, format, refresh_s, timeout_ms}` for `command` |
| `providers.<id>.price."<model>"` | price block | — | owner override (§3.2.6) |
| `providers.<id>.requests_per_day` · `bytes_per_day` · `rate_per_min` | int | none (plan providers: `requests_per_day = 1500`) | provider-level limits, under the class limits |
| `providers.<id>.data_collection` | `"deny"` \| `"allow"` | `"deny"` | gateways only |
| `providers.<id>.kind` · `server` · `endpoint` · `api` · `dialect` · `base_url` | | — | `local` and `custom` entries (§3.5.5, §3.5.6) |
| `keys.store` | `"auto"` \| `"keychain"` \| `"file"` | `"auto"` | §3.4.2 (`keychain` means the platform store) |
| `meter.window` | `"conservative"` \| `"peak"` | `"conservative"` (orchestrator direction 2026-10-20, replacing the provisional "keep the peak window" of build-deviations §3.13; owner may override) | §3.2.4 conservative rule |
| `catalog.accept_unverified` | [string] | `[]` | `"<provider>/<model>"` or `"<provider>/*"`: enable `3p` / unverified rows |

Validation additions: C16 `[providers]` (registry compile errors, `charter.route_denied`, `charter.local_endpoint`, `charter.custom_endpoint`, params grammar, every enabled model priced); C17 `command` argv rule (§3.4.5); C18 at L0 the owner-selected default (§3.7) must be one of the `min_level = 0` models, else the first such model is selected and `charter check` says so.

Template (replaces root/root.md §3.12.2's `[actions.providers]` block):

```toml
[providers.deepseek]          # schema: root/egress.md (providers v2); prices: the Root catalog
models = { "deepseek-flash" = 0, "deepseek-v4-pro" = 1 }

[actions.providers]           # volume limits for all model traffic; hosts and paths come from the registry
requests_per_day = 100000
bytes_per_day = 8000000000
rate_per_min = 600

[keys]
store = "auto"                # OS keychain where available, else 0600 files in ~/.init-root/keys

[meter]
window = "peak"
```

### 4.3 Published views (zone read-only)

- `INIT_HOME/root/catalog.json`: §3.2.7. Top level `{"catalog_version", "providers":[…], "models":[…], "server_kinds":[…], "v":1}`; prices in µUSD/MTok integers; ICJ.
- `INIT_HOME/root/model.json`: `{"effort":"high","model":"deepseek-flash","provider":"deepseek","selected_at":"…","v":1}`.
- `INIT_HOME/root/ceilings.json` `models[]` gains `billing`, `cached_input_musd_per_mtok`, `window`, `catalog_version`, `ready` (credential present and usable).
- `INIT_HOME/root/charter.json`: `[providers.*]` published with ids, model ids, min levels and kinds only; params, endpoints, auth and price overrides dropped (prices appear only through `ceilings.json`).

### 4.4 owner.sock requests (cross-doc: root/root.md §4.2)

Request frames `{"v":1,"id","cmd","args"}` as today; accepted only from a non-sandboxed same-uid peer.

| `cmd` | `args` | `result` | Errors |
|---|---|---|---|
| `keys.set` | `{"provider", "kind": "api_key"\|"aws"\|"azure_client"\|"google_sa"\|"google_adc", "fields": {"key"} \| {"access_key_id","secret_access_key","session_token"?} \| {"tenant","client_id","client_secret"} \| {"json"}}` | `{"provider","stored_in","verified":"ok"\|"skipped"}` | `root.args`, `keys.unknown_provider`, `keys.format`, `keys.rejected`, `keys.store`. **Never logged, relayed, queued or replayed.** |
| `keys.delete` | `{"provider"}` | `{"deleted":bool,"revoked":bool\|null}` | `keys.unknown_provider` |
| `keys.list` | `{}` | `[{"provider","kind","stored_in","state":"present"\|"missing"\|"insecure"\|"locked"\|"expired","expires_at"?}]` | — |
| `login.start` | `{"provider","manual"?:bool}` | `{"url","expires_in_s":300}`; completion as `notice{code:"login"}` | `login.unsupported`, `login.busy`, `login.port` |
| `login.complete` | `{"provider","redirect_url"}` | `{"ok":true}` | `login.state`, `login.exchange`, `login.scope` |
| `logout` | `{"provider"}` | `{"revoked":bool}` | `keys.unknown_provider` |
| `providers.list` | `{}` | `[{"id","display","kind","enabled","ready","auth","models":n,"tos":"allowed","wire_verified"}]` | — |
| `models.list` | `{}` | `{"default":{"provider","model","effort"},"level":n,"catalog_version","models":[{"provider","model","display","context","max_out","price":{"input_musd","cached_musd","output_musd","window"},"billing","tier","min_level","selectable":bool,"why_not"?,"ready":bool,"wire_verified","status","changes"?,"valid_until"?,"effort":[…]}]}` | — |
| `model.select` | `{"provider","model","effort"?}` | `{"default":{…},"applies":"next_idle"}` | `model.unknown`, `model.unpriced`, `model.not_ready`, `model.level`, `model.probe_failed` |
| `catalog.status` | `{}` | `{"version","source":"embedded"\|"active","verified_through","expiring":[…],"stale":[…]}` | — |

Lines the Root handles (added to root/root.md §4.2's table): `/model`, `/model <provider>/<model> [effort]`, `/providers`, `/login <provider>`, `/logout <provider>`. `/key …` as a *line* is refused by the Root with a notice pointing to the TUI prompt or `init-root keys set` (a key must never travel in a line).

### 4.5 CLI (added to root/root.md §4.3)

| Command | Exit |
|---|---|
| `keys set <provider> [--kind …] [--from-file <path>] [--no-verify]` · `keys check [--json]` · `keys delete <provider>` · `keys move --to keychain\|file` | 0; 2 not a TTY or invalid; 3 rejected by the provider |
| `login <provider> [--manual] [--port N] [--no-browser]` · `logout <provider>` | 0; 1 failed; 2 unsupported |
| `catalog status` · `catalog check <file>` · `catalog diff <file>` · `catalog update --from <dir> [--yes] [--approve-decreases] [--allow-downgrade]` · `catalog update --rollback` · `catalog approve <provider>/<model> \| --all` | 0; 1 signature or validation failure; 2 refused |
| `local add <id> --server <kind> --endpoint <ip:port> --model <name> --context <n> [--api …] [--key] [--accept-probe-failure]` · `local scan` · `local remove <id>` | 0; 1 probe failed; 2 invalid |
| `models [--json]` · `providers [--json]` | 0 |

`local add`, `local remove` and `keys set` for a provider that is not yet in the charter **append** a `[providers.<id>]` block to the owner's working `charter.toml` (never rewrite existing text), show the diff and run the `charter check --apply` flow. Every other charter change stays an owner edit.

### 4.6 Egress wire additions (root/egress.md §4.1)

- Zone-facing paths may contain a `{model}` segment and one fixed query (§3.5.2).
- Headers from the kernel unchanged (`anthropic-beta` values now filtered, §3.5.3).
- Response headers gain `x-egress-billing: tokens|plan|local` and `x-egress-catalog: <version>`.
- New refusal codes: `class.unpriced` (403, not a hit), `class.remote_fetch`, `class.header`, `class.gateway`, `class.policy` (403, rule a), `plan.exhausted` (429, not a hit, `retryable:false`).

### 4.7 Ledger and lineage

- Egress `end` records add `usage.{input_uncached, cache_read, cache_write_5m, cache_write_30m, cache_write_1h, output, reasoning}`, `window`, `window_reason` (`cheap`, or why not: `margin`, `peak_overlap`, `holiday_doubt`, `calendar_unknown`, `clock_doubt`, `meter_peak`), `tier`, `factor_ppm`, `billing`, `reported_musd`, `floored` (a pending decrease kept the old price), `catalog_version`. `start` records add `catalog_version`.
- `ROOT_HOME/egress/auth.jsonl`: `{"at","endpoint","provider","status","v":1}` per Root-internal auth call.
- New Root-written lineage kinds: `catalog{version, sha256, source, added, removed, repriced}`, `catalog_approve{row, change, from, to}`, `credential{provider, kind, action, backend}`, `model_select{provider, model, effort}`. None carries a secret, a token claim, an email or a URL with a query.

### 4.8 Kernel config (kernel/config.md)

`schema_version = 3`: `[providers.<id>]` is optional; `preset` is replaced by the registry; `models`, `schedule` and `multipliers` become measurement overrides (absent = the view); `routes` may only narrow (a route not in the view is a config error). `model.provider`/`model.id` remain the fallback default when `INIT_HOME/root/model.json` is absent. Schema 2 files keep loading with a deprecation notice.

### 4.9 Files

| Path | Mode | Writer | Content |
|---|---|---|---|
| `ROOT_HOME/catalog/active.toml`, `prev/<version>.toml` | 0600 | owner via `catalog update` | §3.2.1 |
| `ROOT_HOME/keys/store.json` | 0600 | Root | backend index, no secrets |
| `ROOT_HOME/keys/oauth/<id>.json`, `<id>.lock` | 0600 | Root | `file` backend OAuth state; refresh lock |
| `ROOT_HOME/state/model.json` | 0600 | Root (owner request) | owner-selected default |
| `ROOT_HOME/local/<id>.json` | 0600 | Root | last probe result per LOCAL provider (kind, context, probe outcome) |
| `ROOT_HOME/egress/auth.jsonl` | 0600 | Root | §4.7 |
| `INIT_HOME/root/catalog.json`, `model.json` | 0644 | Root | views (§4.3) |

## 5. Failure modes and recovery

| Failure | Effect | Recovery |
|---|---|---|
| `active.toml` missing, unreadable or invalid at start | the embedded catalog is used | alert `catalog_invalid`; `catalog update --rollback` or a fixed file |
| Catalog update with a bad signature, a downgrade or a CAT error | nothing changes | the command exits 1 with the errors |
| A promotion passes `valid_until`; a row is disabled | requests for it get 403 `class.unpriced` | `price_expiring` alerts from 14 days before; `catalog update`, or a charter price override |
| Clock in doubt (no fresh `Date` sample, skew > 60 s, wall-clock jump) | the Root bills peak (`clock_doubt`), an over-count | nothing to do; the next upstream response brings a sample; `doctor` reports a large skew |
| A catalog or scheduled change lowers a price | the Root keeps the old price (`floored`) | `catalog approve`; alert `price_decrease_pending` |
| Provider raised prices without a catalog update | the Root under-counts by the increase | the owner's prepaid balance is the outermost cap; the `balance` free route (DeepSeek) and gateway cross-checks (`price_drift`) surface it; PV-03's monthly lane |
| Keychain locked or Secret Service missing | that provider is disabled (`key_unavailable`) | unlock and `keys check`, or `keys move --to file`; `auto` falls back at start |
| Keychain prompts after an upgrade (FFI fallback only) | the Root waits on a GUI prompt | the default `/usr/bin/security` backend avoids it (§3.4.2) |
| OAuth refresh fails with `invalid_grant` / 401 | tokens deleted; provider disabled | alert `login_required`; `init-root login openai-chatgpt` |
| Token endpoint unreachable | the current access token is used until it expires, then the provider is disabled | the refresh thread retries with backoff 30 s → 10 min |
| Crash between a refresh answer and the store write | the rotated refresh token is lost | sign in again (by design, §3.4.4) |
| Plan cap reached (`subscription_sharing_usage_limit_exceeded`) | 429 `plan.exhausted`, not retried | alert with the usage link; the owner switches default (`/model`) or waits |
| Clock skew > 5 min | SigV4 and JWT requests fail | `upstream.auth{hint:"clock"}`; `init-root doctor` compares with a provider `Date` header |
| Local server down or restarting | 503 `upstream.connect`; the kernel's normal retry policy applies | none needed |
| Local server reports a smaller context than registered | registration refused, or at start the provider is disabled with `local_ctx_mismatch` | re-register with the real context |
| Local server exposed on the LAN | warning only | `doctor` explains `OLLAMA_HOST` / `--host` |
| Gateway reports a cost above the Root's | settlement raised to the reported figure; `price_drift` | catalog update |
| A dialect sends a field the host rejects (400) | the request fails, not retried | the conformance fixture catches it before release; the agent may fix the dialect through the gate (body code) |
| Body cannot boot the owner-selected model (exit 78) | the kernel reverts to the previous default | alert `model_rejected` |
| A secret-shaped line | not sent; notice | the owner rephrases or uses `/key` |

## 6. Performance notes (spec §9)

- **Rank 1, prompt cache.** Every renderer keeps log-context's per-message byte cache and ICJ ordering; dialect `extra` objects and gateway routing objects are constants per model, so prefixes stay byte-stable. The `deepseek` dialect is byte-identical to today (golden test). A provider or model switch still costs a full cache miss, which is why L1 cross-model subtasks get their own context (§3.7).
- **Rank 2, round trips.** Credentials are read at start and on reload, never per request (one `security` or `secret-tool` call costs ~10–30 ms, UNVERIFIED). OAuth, Google and Entra tokens refresh on a timer 300 s before expiry, off the request path. Keep-alive pools are per destination, including loopback endpoints.
- **Added Root latency** stays within root/egress.md's targets (≤ 3 ms p50 before the upstream request): price resolution is O(windows + tiers + changes) ≤ 20 comparisons on a precompiled row; template matching is one split; the body JSON is already parsed. SigV4 adds one SHA-256 over the body (≈ 1 ms per MiB with the opt-level-3 `sha2` build) plus four HMACs.
- **Tap cost.** Unchanged needle pre-filter; Gemini chunks carry `usageMetadata` in every chunk, so the Root parses one small JSON object per chunk on Gemini routes (bounded by the 1 MiB line cap).
- **Catalog size.** About 150 model rows; compile < 5 ms; the view is ≈ 150 KB, written on reload only.

## 7. Invariants and security (spec §2.1)

| Invariant | How v2 keeps it |
|---|---|
| #3 Keys never in the sandbox | credentials only in the Root's store; the zone cannot reach the macOS keychain (`mach-lookup` denied, `~/Library/Keychains` denied), the Linux session bus (`$XDG_RUNTIME_DIR` not mounted, test S-L-KS), or Windows Credential Manager (test W-KS-1, else `file`); auth injected below the kernel; OAuth tokens never leave the Root; the TUI carries a key at most once, in one unlogged `keys.set` frame (Q2); the line guard blocks credential-shaped owner lines |
| #3 Spend the agent cannot bypass | the meter's catalog is Root-owned and signed, never read from a zone-writable path (never a T0 file); price decreases need the owner's approval, increases apply at once; the off-peak price is granted only under the conservative rule, so the meter stays an upper bound; kernel prices are measurement-only; unpriced ⇒ refuse; reservations price the dearest window, tier and change, and the input term at `max(input, cache_write)`; gateway fallbacks off, `only` pinned, `max_price` set, reported costs can only raise a settlement; plan routes have a hard daily request cap |
| #3 External actions bounded | registry hosts exact; route templates and the F2 metering table; provider-executed tools, server-side state and remote media URLs refused; LOCAL limited to exact loopback `ip:port` and inference paths, so model-management, shell-tool and admin endpoints of local servers are unreachable; no redirects; Root-internal auth destinations are a fixed list |
| #2 Honest measuring stick | `cost_ref` from peak or shadow list prices, so routing to a plan or off-peak does not look cheaper; gates snapshot `catalog_version`; sealed lanes keep the catalog they opened with; `wire_verified` is shown, so untested providers are not presented as tested |
| #4 Lineage | `catalog`, `credential`, `model_select` records with no secret, claim, email or query URL |
| #5 Human where irreversible | catalog updates, credentials, logins, provider enablement and the default model are owner actions (CLI or owner.sock), never agent actions |
| Honest identity (ToS) | `init-root/<version>` UA; `agent_name_hint = "INIT Agent"`; identity headers of other clients never rendered or forwarded; GRAY and PROHIBITED routes refused by a compiled denylist |
| Privacy (CONVENTIONS §1) | keys never in URLs (`x-goog-api-key` header, not `?key=`); the ID token's email is never stored; `data_collection: "deny"` by default on gateways; Responses and Interactions `store:false` enforced |

**Residual risks** (accepted, listed for PV-20):
- `command` auth runs an owner tool outside every sandbox; with full mode on, a tool that loads code from the home could be tampered with by the agent. Mitigations: argv[0] outside the home, env hardening, a `charter check` warning; recommendation: prefer the native kinds.
- Keychain items created through `/usr/bin/security` can be read by any same-uid process that can talk to `securityd`, the same exposure as today's 0600 files; the zone is excluded by its profiles.
- Cloud and gateway prices are estimates of the owner's real bill (CCU or marketplace billing, credit fees); the provider-side spend limits the owner sets remain the outermost guard.

## 8. Test plan

New IDs, proposed to testing.md. All run at $0 against mocks, fixtures or scratch stores; nothing touches the owner's keychain, login items or live processes (CONVENTIONS §1 live-system safety). Existing suites (EG-*, R3-*, SL-01, P-01…09, T-PROV-1…6, TUI-*) must pass unchanged at every stage (§3.9).

| ID | Case | Expectation |
|---|---|---|
| CAT-T01 | conservative rule, verified calendar, trusted clock, weekday: requests `[00:44:00Z, 00:44:59Z]`, `[00:44:30Z, 00:45:01Z]`, `[04:14:59Z, 04:20Z]`, `[04:15:00Z, 05:45:00Z]`, `[05:45:00Z, 05:45:01Z]`, `[10:15Z, 10:20Z]`; `[00:30Z, 04:30Z]` (spans a peak window) | off-peak, peak, peak, off-peak, peak, off-peak; peak |
| CAT-T02 | weekend span: Fri 10:15Z, Sat 02:00Z, Mon 00:44Z; Mon 00:46Z | off-peak ×3; peak |
| CAT-T02b | holiday doubt: a listed holiday as a +08:00 date whose UTC date differs (16:00Z the day before); a listed make-up workday on a Saturday; a weekend date after `known_through`; `known_through = ""` | peak ×4 (the kernel's measurement still uses the official off-peak rule for the holiday) |
| CAT-T02c | clock: no `Date` sample since start; sample 7 h old; skew 61 s; a 3-s wall-clock jump against the monotonic clock; then a fresh good sample | peak ×4 with `clock_doubt`; then off-peak again |
| CAT-T02d | `meter.window = "peak"` | peak always (v0.1 numbers) |
| CAT-T03 | Haiku 5.5 tier: prompt total 100,000 and 100,001 tokens incl. cache reads | whole request at base, then at tier prices |
| CAT-T04 | Argon change (step 1): requests at 2026-11-30T09:59:59Z and 10:00:00Z | old, then new price; reservation 1 h before the change already uses the new price |
| CAT-T05 | GPT-5.6 Sol at 2026-11-21T09:59:59Z and 10:00:00Z | priced, then `class.unpriced` |
| CAT-T06 | multipliers: `inference_geo:us`, `speed:fast` marked UNPRICED, unlisted `service_tier:priority`, neutral `auto` | ×1.10; refused; refused; ×1 |
| CAT-T07 | rounding: every class rounded up per µUSD/MTok, cost rounded once | equals hand computation to the µUSD |
| CAT-T08 | each CAT rule violated once | the rule's error with its key path |
| CAT-T09 | signature: valid; one flipped byte; wrong key; downgrade; `--allow-downgrade` | applied; refused ×3; applied |
| CAT-T10 | update while a request is in flight and a sealed lane is open | both settle under the old version; the next admission uses the new one |
| CAT-T11 | view contents | no host, param, override, key or account field; prices equal the Root's resolution |
| CAT-T12 | `3p` row without and with `accept_unverified` | disabled (`class.unpriced`); enabled |
| CAT-T13 | seed completeness | every Q3 research row is in the catalog or in PV-02's exclusion list with a reason |
| MT-01…10 | Root settlement from fixtures per parser (§3.3.10): Anthropic cumulative deltas, Responses cache-write subset, DeepSeek hit/miss, OpenRouter cost drift ±, Gemini thoughts sum, Converse metadata, a stream without usage, a plan route, a local route, cache-write TTL classes | hand-computed costs; drift rules; plan and local cost 0 with counters |
| EV2-01 | templated route `models/{model}:streamGenerateContent?alt=sse`; same path with `?alt=json`; model with `%3A`; with `%2F` | forwarded; `class.path`; decoded `:`; `class.path` |
| EV2-02 | body `model` ≠ path model | `class.model` |
| EV2-03…08 | each §3.5.3 policy row violated once (provider tool per api, remote image URL per api, `store:true`, `n:2`, foreign `anthropic-beta`, missing `include_usage`) | the listed code; demotion flag per §3.5.3 |
| EV2-09 | OpenRouter: missing `allow_fallbacks:false`, `only` ≠ row, `max_price` above row, `transforms` absent, `:nitro` suffix, `data_collection:"allow"` without charter | `class.gateway` ×5, `class.unpriced` |
| EV2-10 | ToS denylist: catalog row and charter custom provider for each denied route | `charter.route_denied` with the reason |
| EV2-11 | params injection: `region = "us-east-1.evil.com"`, `project = "../x"` | charter error; never compiled |
| EV2-12 | schema-1 charter (today's template) | identical registry behaviour and costs to v0.1 (EG-07 numbers) |
| EV2-13 | custom provider: http URL, IP literal, unpriced model, priced model | errors ×3; routable |
| EV2-14 | free route body > 64 KiB | `class.policy` |
| KS-01 | `file` backend: today's key file found under every `keys.store` value | loaded |
| KS-02 | `keychain` on a scratch keychain created by the test (`security create-keychain <scratch>/t.keychain-db`, explicit keychain path in every call) | put/get/delete; the secret never appears in any argv (checked by recording argv through a wrapper) |
| KS-03 | `keys move` file → keychain → file | identical secret; index updated; source deleted by exact path |
| KS-04 | `secret-service` in a disposable VM with an unlocked test collection | as KS-02; `auto` falls back to `file` when no bus exists |
| KS-05 | `keys.set` over owner.sock from the TUI harness | stored; the frame absent from Root logs, replay ring, owner queue, kernel traffic and alerts |
| KS-06 | line guard: a stored secret inside text; each prefix pattern; a 19-char tail (no match); a JWT shape | not sent ×3, sent, not sent |
| KS-07 | `keys set` verification: mock 200, 401, no free route | stored; not stored (exit 3); stored `skipped` |
| KS-08 | test mode | only `file`; real-looking keys refused as today |
| AU-01 | HMAC-SHA256 RFC 4231 cases 1–7 | exact |
| AU-02 | SigV4 against AWS's published test-suite vectors, plus a Bedrock path with `:` in the model id | exact signatures |
| AU-03 | RS256 JWT against RFC 7515 A.2; Google SA flow against the mock token endpoint | exact; token cached until 300 s before expiry |
| AU-04 | ADC refresh and Entra client-credentials against mocks | header injected; refresh off the request path |
| AU-05 | `command`: argv[0] under the home, group-writable, relative; a 15 s hang; stderr containing a key | charter errors ×3; timeout → provider disabled; stderr redacted |
| AU-06 | injected headers per kind; body-sent `authorization`, `x-api-key`, `api-key`, `x-goog-api-key` | Root's header only; body's dropped |
| AU-07 | no key ever in a URL: every route of every kind | no `key=` query |
| AU-08 | Root-internal destinations unreachable through `egress.sock` | `class.unknown` |
| AU-09 | no shipped dialect renders an identity header of another client | static scan + render of every fixture |
| OA-01 | PKCE: RFC 7636 Appendix B verifier → challenge | exact |
| OA-02 | full login against the mock AS (dynamic registration, issued id saved, scope granted) | tokens stored; ID token dropped after checks |
| OA-03 | callback with a wrong `state`; two callbacks; none within 300 s | refused; second ignored; timeout, listener closed |
| OA-04 | ID token with a bad signature, wrong `aud`, wrong `nonce`, missing scope | refused, nothing stored |
| OA-05 | 20 concurrent requests across the refresh boundary | exactly one refresh; new refresh token stored before use |
| OA-06 | crash injected after the token response, before the store write (`INIT_FAULT root.oauth.after_exchange`) | next start: provider disabled, `login_required`; no token pair in two places |
| OA-07 | logout: revoke ok; revoke fails | deleted both times; result says `revoked:false` on failure |
| OA-08 | plan policy: `max_output_tokens` present, `store:true`, hosted tool | `class.policy` ×3 |
| OA-09 | error contract: 429 sharing limit (HTTP and mid-stream `response.failed`), 403 not eligible, 401 invalid user | `plan.exhausted`; disabled; tokens deleted; none retried |
| KP-01…08 | kernel: view-driven routes; templated model extraction; new families' usage and first-token; config override warning; `model.json` default and handoff; gate snapshot of `catalog_version`; new error mappings; schema-2 config still loads | as §3.6 |
| CDX-01…06 | body framework: view parse; fallback table; boot check exit 78 for unknown dialect; unknown `api` in an old log; replay table per carrier; handoff per carrier | as §3.3 |
| CDX-10…24 | one fixture set per CC dialect (stream with reasoning, 4-call tool loop with replay, 400 shape, max-tokens stop, overflow message) | decoded messages and re-rendered bytes match goldens; DeepSeek golden unchanged |
| CDX-30…38 | Messages and Responses renderers in log-context; host dialects; `siwc` field omission | byte goldens; per-message cache reuse (T-CTX-4 style) |
| CDX-40…50 | Gemini generate and Interactions fixtures | signatures replayed verbatim; ids synthesized when absent; usage sum |
| CDX-51…58 | Bedrock Mantle and OpenAI routes; Converse framing (CRC mismatch, split prelude, reasoning signature) | as §3.3.7 |
| LOC-01 | endpoint forms: `localhost:11434`, `0.0.0.0:11434`, `192.168.1.5:11434`, `127.0.0.1:11434`, `[::1]:8080` | refused ×3, accepted ×2 |
| LOC-02 | per server kind, every denied path in §3.5.5 and every allowed one | `class.path` / forwarded |
| LOC-03 | from inside the body sandbox, connect to the mock server's loopback port | refused by the OS |
| LOC-04 | context: declared 65,536 vs probe 4,096; unverifiable server | refused; registered `ctx_known:false` |
| LOC-05 | Ollama derived model creation is an owner command only; the same `api/create` from the zone | created by the Root; `class.path` |
| LOC-06 | fingerprint mismatch (declared vllm, mock answers as Ollama) | refused |
| LOC-07 | exposure check with a mock listening on a non-loopback test address | `local_exposed` warning |
| LOC-08 | conformance probe pass and fail | selectable; `probe_failed` with reason |
| LOC-09 | SGLang-style remote image URL in a local request | `class.remote_fetch` |
| LOC-10 | `Host` header and redirects | literal `ip:port`; 3xx not followed |
| UX-01…08 | `models.list` content and reasons; `model.select` at L0 and the egress forwarding only it; revert on exit 78; `/model` picker render; `/key` masked prompt (no echo, no history); `/login` URL notice; an older Root answering `root.unknown_cmd`; `/key` sent as a line refused | as §3.7, §4.4 |
| S-L-KS, W-KS-1 | Linux zone cannot reach the session bus; Windows AppContainer cannot `CredReadW` | refused (cross-doc kernel/sandbox.md; platform lane P) |
| HE-01 | `closed_round` (Messages fixture, binding beta `error` mode): a mask epoch while a round is open | request carries no thinking, the open round flattened, ends with a user message; the mock in strict binding mode accepts it |
| HE-02 | the same without round-close (negative control) | the strict mock answers 400 "bound to a different conversation" |
| HE-03 | `free` (DeepSeek strict mock): mask epoch inside an open round | `reasoning_content` kept on every retained assistant message; accepted |
| HE-04 | compaction fork inside an open round on `closed_round` | the fork request edits nothing (byte-prefix equal to the current request); the next request after the compaction event applies round-close |
| HE-05 | replay: three requests after the epoch | identical flattened bytes each time (cache-stable); `round_closed` recorded in the epoch event |
| HE-06 | handoff into a `closed_round` model with an open round | foreign reasoning stripped and the round closed |
| CAT-T14 | decreases: a catalog update lowering one price and raising another; a scheduled decrease reaching its date; a rollback to a cheaper catalog; `catalog approve` | increase applied at once; decreases metered at the floor until approved, `price_decrease_pending`; after approval the new price and a `catalog_approve` record |
| MF-01…10 | the worked vectors of §3.2.4 | exact `cost_musd` and reservation |
| LIVE-DS-01…07 (PV-19) | checks L1–L7 of §3.10.1 on Chat Completions, `/responses` and `/anthropic/v1/messages` | as listed there; Root ≥ DeepSeek's charge; total ≤ $0.40 Root-metered; streams saved as fixtures |

## 9. Open questions

**For the owner** (each with the recommended answer, taken as the provisional default so the build need not wait):

| Id | Question | Recommended |
|---|---|---|
| Q1 | Price decreases need the owner's approval (§3.2.6), increases apply automatically. Should a *scheduled* decrease that the owner already approved at `catalog update` time apply on its date without a second prompt? | Yes: approval is per (row, change), given once at update time with the diff in front of the owner; `catalog approve` covers decreases left pending. |
| Q2 | May the TUI carry a key once, in a `keys.set` frame (a change to tui.md T-2)? | Yes, with the §3.4.3 constraints (masked, one frame, never logged, relayed, queued or replayed; peer check). `init-root keys set` stays the reference path. |
| Q3 | May the owner override a GRAY verdict (for example an Alibaba or Kimi coding-plan key for interactive use)? | No in v1.0. Revisit per vendor once its terms name INIT or explicitly allow third-party harnesses; PV-03 re-reads verdicts each catalog release. |
| Q4 | The Root grows by ≈ 3,200 lines (§3.8), against the "minimal Root" principle. Accept in-process modules, or split OAuth and cloud auth into a helper binary? | Accept in-process modules with their own sub-budget lines (measured + 5% at release) and `catalog/price.rs` in the hand-reviewed core. A helper binary would sit in the same trust zone, so it adds a protocol without reducing what the owner must trust. |
| Q5 | Gemini default API and Converse timing. | `gemini_generate` as the default, `gemini_interactions` per model when a model is Interactions-only (PV-14 in v1.0 if time allows); Bedrock Converse (PV-15b) in v1.1, because Claude on Bedrock uses Mantle Messages and OpenAI-family models use Bedrock's OpenAI routes. |

**Cross-doc** (to settle in `reconciliation/providers-v2.md` through PV-00):
- **root/egress.md:** registry v2 replaces `PRESETS`, `ROUTES`, the `providers` part of `BUILTIN` and the charter's model price tables (§3.5); new refusal codes and their demotion flags (§4.6); class-split usage and the catalog resolver (§3.5.7); "the owner lists exactly one model at `min_level = 0`" becomes "the egress forwards only the owner-selected default at L0" (§3.7); `keys/` layout and backends (§3.4.2).
- **root/root.md:** charter schema 2 keys and rules C16–C18 (§4.2); `ROOT_HOME/catalog/`, `keys/store.json`, `keys/oauth/`, `state/model.json`, `local/` (§4.9); owner.sock requests and lines (§4.4); CLI (§4.5); lineage kinds `catalog`, `credential`, `model_select`; alerts `catalog_invalid`, `catalog_stale`, `price_expiring`, `price_drift`, `keystore_fallback`, `key_unavailable`, `login_required`, `plan_exhausted`, `model_rejected`, `local_exposed`, `local_ctx_mismatch`; size budget lines (§3.8); fault point `root.oauth.after_exchange`; alerts `price_decrease_pending`; `floors.json`.
- **kernel/proxy.md, kernel/config.md:** §3.6 and §4.8.
- **body/provider.md:** §3.3 replaces §3.1 (layout), §3.3 (`ModelInfo`, `ReasoningRule`) and extends §3.8 and §3.10.
- **body/log-context.md:** per-api renderers (PV-12, PV-13), head-hash objects for Gemini and Converse, unknown `api` strings read as "other api", `NotePlacement` per dialect (§3.3.11); the history-edit rule (§3.3.12): `View.round_closed` (§3.7), `round_closed` on `mask`, `compaction` and reload events (§4.2 schema), `boundary_rebuild_check` reads `edit_rule` (§3.11), and §3.10.2's claim that an epoch "after results are appended" is never inside an open round is corrected (it is, and round-close handles it).
- **tui.md:** T-2 amendment (Q2); `/model`, `/key`, `/login`, `/logout`, `/providers` (§3.7, §4.4); the TUI starts parsing these commands itself, a change to its "parses nothing except `/panic`" rule.
- **kernel/sandbox.md:** Linux zone without `$XDG_RUNTIME_DIR` (S-L-KS); Windows AppContainer credential test (W-KS-1); `~/.config/init-agent/catalog-signing.ed25519` stays on the deny list.
- **testing.md:** the §8 IDs; `init-mockllm` modes `--oauth`, `--server <kind>`, Gemini, Interactions and Converse rendering (PV-18).
- **CONVENTIONS.md §4:** registry rows for the catalog format (this doc until folded into root/egress.md), the credential store (root/egress.md), dialect profiles (body/provider.md).
- **The onboarding lane (install/daily UX design):** the first-run "model provider" step calls `providers.list`, `keys.set`, `login.start` and `model.select`; env-key auto-detection must hand the key to the Root through `keys.set`, never through the agent zone.

## 10. Spec deltas

| Current text | This doc | Why |
|---|---|---|
| V1_PLAN Phase C "model catalog with prices as a T0 data file" | a Root-owned, signed catalog (§3.2.1); never T0 or agent-writable | a writable price table would let the agent lower prices and spend past its ceilings |
| build-deviations §3.13 (provisional owner decision) "keep the peak window" | `meter.window = "conservative"`: off-peak only inside the official window shrunk by 15 min at both ends, never on CN holiday or make-up-workday dates, never with an unknown calendar or a doubtful clock; `"peak"` remains available (orchestrator direction 2026-10-20; owner may override) | peak billing halves what the $5 cap buys on DeepSeek; the conservative rule keeps the Root an upper bound |
| CONVENTIONS §5 "Providers: DeepSeek first; Anthropic Messages and OpenAI Responses follow" | six wire APIs with dialects; every ALLOWED route of §3.4.7 | V1_PLAN item 3 |
| CONVENTIONS §5 "Keys … `init-root keys set`" (files) | OS keychain where available, 0600 files as fallback; OAuth and cloud credentials in the same store | the expected security level for 2026 harnesses (research) |
| root/egress.md §3.5 prices in the charter's `[actions.providers.models]` | prices in the Root-owned signed catalog; the charter selects models and levels and may override prices | prices change monthly; the owner should not hand-maintain them, and the agent must never touch them |
| root/egress.md §3.4 cost line `(input − cache_read) × input + cache_read × cache_read_price + output × output` at peak | the complete formula of §3.2.4: cache writes by TTL, whole-request long-context tiers, service-tier, speed, geo and endpoint factors, gateway fees, dated changes, the conservative window; reservation at `max(input, cache_write)` | the old line under-counts every non-DeepSeek provider (MF-07: −46% on 1-hour cache writes) |
| log-context §3.10.2 "epochs start only at turn boundaries, after results are appended" | codec-aware history edits with round-close (§3.3.12) | on Claude 5.x an edit inside an open round cannot produce a valid request otherwise |
| root/egress.md §3.2 loopback destinations only in test mode | plus the production LOCAL class: exact loopback `ip:port`, inference paths only | local LLM support without opening loopback to the agent |
| root/egress.md §3.5 "at L0 the owner lists exactly one model" | the owner selects the L0 default with `/model` among the `min_level = 0` models | daily UX without charter edits |
| kernel/proxy.md §3.5 three presets | registry routes from the published view | data-driven providers |
| tui.md T-2 "no keys" | no key at rest; one key in transit per `keys.set` (Q2) | in-app onboarding |
| body/provider.md §3.3 seed model table (static) | catalog view, static table as fallback | models change monthly |
| root/root.md §4.8 size budget ≈ 7,500 | + ≈ 3,200 lines in new sub-budget lines; core + ≈ 190 (Q4) | credentials, auth and metering must be Root code |

## 11. References

Internal (read 2026-10-20): V1_PLAN.md; CONVENTIONS.md; body/provider.md; body/log-context.md; root/egress.md; root/root.md; kernel/proxy.md; kernel/config.md; kernel/sandbox.md; tui.md; ADR-009; code at HEAD 0be7b68: `seed/src/provider/*`, `seed/src/context/render.rs`, `root/crates/init-root/src/egress/{classes,tap}.rs`, `kernel/crates/kernel-proxy/src/{routes,pricing,tap}.rs`.

Research notes (all fetched by the research lanes on 2026-10-20; this lane did not re-fetch them, and PV-03 re-verifies every number and wire fact before release):
- `research_notes/INIT v1 providers UX vision/llm_api_providers.md`: provider surfaces, wire quirks, the Q3 price table, pricing traps, the codec recommendation.
- `research_notes/INIT v1 providers UX vision/proxies_gateways_local.md`: OpenRouter, Vercel, Cloudflare, LiteLLM, Portkey; local servers and their dangerous endpoints; the loopback isolation pattern; custom-provider UX of other harnesses.
- `research_notes/INIT v1 providers UX vision/subscription_plans.md`: the route verdict matrix; SIWC flow, tokens and errors; credential-broker guidance.
- `research_notes/INIT v1 providers UX vision/install_daily_ux.md`: keychain-first storage, `/model`, the `keys.set` proposal, keychain prompts after unsigned upgrades.

Primary sources cited through those notes (accessed 2026-10-20 by research):
- OpenAI reasoning guide https://developers.openai.com/api/docs/guides/reasoning ; prompt caching https://developers.openai.com/api/docs/guides/prompt-caching ; pricing https://developers.openai.com/api/docs/pricing ; SIWC https://developers.openai.com/siwc/token-sharing-open-source (sign-in, token reference, profiles and sessions, preview limitations)
- Anthropic thinking https://platform.claude.com/docs/en/build-with-claude/thinking ; errors https://platform.claude.com/docs/en/api/errors ; prompt caching https://platform.claude.com/docs/en/build-with-claude/prompt-caching ; streaming https://platform.claude.com/docs/en/build-with-claude/streaming ; pricing https://platform.claude.com/docs/en/about-claude/pricing ; Bedrock https://platform.claude.com/docs/en/build-with-claude/claude-in-amazon-bedrock ; Vertex https://platform.claude.com/docs/en/build-with-claude/claude-on-vertex-ai ; Foundry https://platform.claude.com/docs/en/build-with-claude/claude-in-microsoft-foundry ; Claude Code legal and compliance https://code.claude.com/docs/en/legal-and-compliance
- Gemini Interactions https://ai.google.dev/gemini-api/docs/interactions ; thinking https://ai.google.dev/gemini-api/docs/thinking ; caching https://ai.google.dev/gemini-api/docs/caching ; generateContent https://ai.google.dev/api/generate-content ; pricing https://ai.google.dev/gemini-api/docs/pricing ; Gemini CLI FAQ https://geminicli.com/docs/resources/faq/
- DeepSeek pricing https://api-docs.deepseek.com/quick_start/pricing/ ; thinking mode https://api-docs.deepseek.com/guides/thinking_mode
- xAI reasoning https://docs.x.ai/docs/guides/reasoning ; Kimi K3 https://platform.kimi.ai/docs/guide/kimi-k3 ; Z.ai GLM-5.3 https://docs.z.ai/guides/llm/glm-5.3 and usage policy https://docs.z.ai/devpack/usage-policy ; MiniMax https://platform.minimax.io/docs/guides/text-generation ; Alibaba https://www.alibabacloud.com/help/en/model-studio/compatibility-of-openai-with-dashscope ; Groq https://console.groq.com/docs/models
- Bedrock Converse https://docs.aws.amazon.com/bedrock/latest/userguide/conversation-inference.html ; Azure OpenAI v1 https://learn.microsoft.com/en-us/azure/ai-foundry/openai/api-version-lifecycle
- OpenRouter provider routing https://openrouter.ai/docs/features/provider-routing ; usage accounting https://openrouter.ai/docs/use-cases/usage-accounting ; reasoning tokens https://openrouter.ai/docs/use-cases/reasoning-tokens ; Vercel AI Gateway https://vercel.com/docs/ai-gateway
- Ollama OpenAI compatibility https://docs.ollama.com/api/openai-compatibility ; context length https://docs.ollama.com/context-length ; llama-server https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md ; vLLM https://docs.vllm.ai/en/latest/serving/online_serving/ ; SGLang https://docs.sglang.io/advanced_features/server_arguments.html ; Claude Code sandboxing https://code.claude.com/docs/en/sandboxing

Standards (long-standing; vectors used by tests): RFC 7636 (PKCE, Appendix B), RFC 8252 (loopback redirects), RFC 7515 (JWS, Appendix A.2 RS256), RFC 2104 and RFC 4231 (HMAC-SHA256), AWS Signature Version 4 and its published test suite.

UNVERIFIED in this doc and assigned to PV-03 or the named WP: `/usr/bin/security` and `secret-tool` reading the secret from a pipe; `windows-sys` version and the Credential Manager blob limit; the AppContainer credential behaviour; `ring` exact version; SIWC's JWKS path, port flexibility and manual-paste acceptance; Gemini function-call ids, `parametersJsonSchema` and per-chunk usage; Interactions event names and usage field names; Converse framing in 2026 docs and its `inputTokens` semantics; `cache_write_tokens` as a subset of `input_tokens`; Kimi, Qwen, GLM, MiniMax, vLLM and Ollama reasoning field names and replay rules; the Mistral tool-id rule; OpenRouter credit = USD and its `transforms` default; Ollama `api/create` parameters, llama-server `props`, vLLM `max_model_len`; the Vertex streaming method name; base URLs marked [3P] in the research; whether Bedrock Mantle, Vertex and Foundry accept the binding beta; whether Gemini accepts closed turns without their thought signatures; the resolution of DeepSeek's `user/balance`.
