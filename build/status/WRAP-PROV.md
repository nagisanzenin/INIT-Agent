# WRAP-PROV: providers v2 core for the release (docs/providers-v2.md)

Status: **paused** (orchestrator, 2026-10-22 ~18:50 +07: the evolution test runs first; resume only if it passes).
Edits are additive. Coordination: INTEG-A owns `root/.../egress/money.rs` (twin pool), X8-KLANE owns `sup.rs`;
this lane does not edit either.

## State at pause
- Root (this lane): **no code written yet**. Reading and design done (below). Nothing uncommitted under `root/`.
- Body sub-lane WRAP-PROV-BODY (`seed/` only) was started at 18:45 and told to pause; its own section below says what
  it committed or left uncommitted.
- WRAP-UX was given the CLI contract below and told it is not built yet (its `init` keeps the DeepSeek-only path).

## Resume plan (Root, in order; each step its own commit with tests)
1. `src/catalog/{mod.rs, price.rs, sign.rs}` + `root/catalog/models.toml` (embedded, `include_str!`): serde schema
   with `deny_unknown_fields` (providers-v2 §3.2.2 subset: providers, models, price blocks with `windows`, `tiers`,
   `cache_write` (map of TTL → price; the Root bills writes at the highest TTL price, an upper bound because the tap
   does not split TTLs), `multipliers`, `changes`, `valid_until`, `from`, `account_fee_permille`), CAT-1…16 subset,
   compile to µUSD/MTok. Windows: the Root always bills the schedule's `dearest` window (= the design's behaviour while
   the CN calendar has `known_through = ""`; the conservative cheap-window grant is deferred to PV-03/PV-04).
   Process-global `catalog::current()` (embedded, or `ROOT_HOME/catalog/active.toml` once `catalog::install(rh)` ran:
   one line after `Paths::resolve` in cli.rs, and in `egress::start`). Extra schema key `id_verified = false` marks
   rows keyed by a slug because the host's wire id is unknown (Together, Fireworks, DeepInfra, OpenRouter's Claude
   ids): the charter maps the real id with `catalog = "<slug>"`. Rows from [3P] facts (Fireworks base URL, Cerebras
   prices) ship `confidence = "3p"` ⇒ disabled unless the charter provider entry says `accept_unverified = true`.
   Seed rows only from research_notes/…/llm_api_providers.md Q3 (keep `source_url`, `verified_date` 2026-10-20);
   `[STALE?]` ⇒ `status = "legacy"`; no invented ids or prices.
2. Floors (`ROOT_HOME/catalog/floors.json`): effective price = max(floor, resolved) per row and class; an increase
   raises the floor at once, a decrease stays pending until `init-root catalog approve <row>|--all` (lineage
   `catalog_approve`). `catalog status|check|diff|update --from DIR [--yes] [--approve-decreases]|--rollback`; Ed25519
   `verify_strict` over `b"init-catalog/1\0" ‖ bytes` (ed25519-dalek is already a dependency); `CATALOG_PUBKEYS` ships
   empty until the release lane embeds the maintainer key (needs from others); tests use a test key.
3. Registry in `egress/classes.rs` (additive): `[actions.providers.providers.<id>]` gains `models` (id → min_level or
   `{min_level, catalog?, context?, max_out?, <price overrides>}`), `kind = "local"|"custom"`, `server`, `endpoint`,
   `base_url`, `dialect`, `accept_unverified`; a catalog provider id with no `host` takes host, base path, auth and
   routes from the catalog. New `Class.regs` map (Provider struct untouched). Lookup order at admission: provider
   model → class model (schema 1; its charter prices override the catalog, so v0.1 bills are byte-identical) →
   `class.model`. No price ⇒ 403 `class.unpriced` (not a hit). Charter check: an enabled model with no price source is
   an error (C11). `ceilings.json` lists per-provider models so the kernel's `forwards_provider` sees them.
4. LOCAL class: exact `127.0.0.1:<port>` or `[::1]:<port>` (no hostnames, no `localhost`, no 0.0.0.0), plain HTTP,
   `Host` = the literal, no redirects; per-server allowlist (all: `POST v1/chat/completions` billed, `GET v1/models`
   free; ollama, lmstudio, llama-server, vllm also `POST v1/responses`, `POST v1/messages`); anything else
   `class.path` (demotes); auth `none` needs no key; reservation and cost 0, tokens still counted.
5. Quirk profiles in the Root: tap `Extra {cache_write, reported_musd}` beside `Usage` (Usage itself is untouched,
   because money.rs tests build it); OpenRouter `usage.cost` cross-check (settle at reported if > own × 1.02, alert
   `price_drift`); OpenRouter policy (`provider.allow_fallbacks == false`, `data_collection == "deny"`, no `models`,
   `route`, `plugins`) ⇒ `class.gateway`; request modifiers (`service_tier`, `speed`, `inference_geo`) neutral or
   priced, else `class.unpriced`.
6. Key store `src/keystore.rs`: backends `file` (today's `keys/providers/<id>`, the only one in test mode) and
   `keychain` via `/usr/bin/security -i` with the command line on stdin and the secret hex-encoded (`-X`), so no
   secret is ever in an argv; index `ROOT_HOME/keys/store.json`; `Cfg::load` falls back to the store when the file is
   absent. Tests use a fake `security` binary (records argv and stdin); never the owner's login keychain, and no
   scratch keychain either (`security create-keychain` edits the owner's search list).
7. CLI (`src/provcli.rs`, dispatched from cli.rs): the WRAP-UX contract below, `keys set` verification exit codes,
   `catalog …`.
8. Tests ($0): CAT rules, MF vectors that apply, sign/verify (good, flipped byte, wrong key, downgrade), floors,
   unpriced refusal, LOCAL allowlist refusals and endpoint forms, a mock upstream per family (tests/x5.rs pattern,
   `--features test-fixtures`), and the X5 tap tests (`egress::`, `extend::`, `--test x5 --test wp22`) unchanged.
9. If time: SIWC (OAuth PKCE) through the Root, honest identity; never Claude-subscription or Google CLI login.

## CLI contract for WRAP-UX (agreed 2026-10-22, not built yet)
1. `init-root providers detect --json` → `{"local":[{"kind":"ollama"|"lmstudio"|"llama-server"|"vllm","addr":"127.0.0.1:<port>","models":[{"id"}]}]}`;
   read-only `GET /v1/models` on 127.0.0.1 ports 11434, 1234, 8080, 8000, 300 ms each; no `tools` field.
2. `init-root providers configure --charter FILE --provider <id> --model <id> [--base-url https://host[:port]/path --price IN,CACHED,OUT]
   [--local 127.0.0.1:PORT --server KIND --context N] [--level 0] [--kernel-config OUT]`: catalog providers need no
   URL (unpriced ⇒ exit 2); a custom URL needs `--price` (USD/MTok decimals); `--local` is $0. Appends an
   `[actions.providers.providers.<id>]` table to FILE (never rewrites text), runs the charter check; exit 0 or 2 with
   one stderr line. OUT gets `[model]` and `[providers.<id>]` with the preset and measurement prices.
3. `init-root keys set <id>` stays TTY-only; verifies with the provider's free models route; exit 0 stored+verified,
   2 not a TTY/bad id, 3 rejected (401/403, not stored), 4 stored but unverified (unreachable or no free route),
   5 stored but no credit (402, or DeepSeek balance unavailable).

## Plan (release scope, priority order)
1. Generic OpenAI-compatible endpoint (Root: catalog providers + charter custom providers with any https base URL,
   key in the Root; quirk profiles = Root usage parser and request policy per dialect, body render/decode dialects).
2. LOCAL class (Root): exact loopback `ip:port`, per-server path allowlist, plain HTTP, `$0` owner-declared models.
3. Body codecs: live Messages and Responses renderers (API-key path) and the Chat Completions dialect profiles
   (sub-lane WRAP-PROV-BODY, `seed/` only).
4. Root-owned signed catalog `root/catalog/models.toml` (embedded) + `ROOT_HOME/catalog/active.toml` via
   `init-root catalog update`; increases automatic, decreases need `catalog approve`; unpriced ⇒ 403 `class.unpriced`
   unless LOCAL.
5. Root key store: macOS keychain through `/usr/bin/security` (secret only on stdin) with the 0600-file fallback;
   `init-root keys set <provider>`.
6. (if time) ChatGPT sign-in (SIWC, OAuth PKCE).

## Interface: zone-facing paths (deviation from providers-v2 §3.5.2, recorded)
New OpenAI-compatible providers use the kernel's existing `openai` preset paths (`v1/chat/completions`,
`v1/responses`, `v1/models`; Anthropic-compatible `v1/messages`) and the Root's `base_path` maps them upstream
(OpenRouter `/api`, Groq `/openai`, Fireworks `/inference`, Together and local servers ``). So a v0.1 kernel routes
them with `[providers.<id>] preset = "openai"`, no kernel change. DeepSeek, Anthropic and OpenAI keep today's paths.

## Interface: the published catalog view `INIT_HOME/root/catalog.json` (0644, ICJ, Root-written)
```
{"catalog_version": "2026.10.22.1", "v": 1,
 "providers": [{"id", "display", "kind": "api"|"gateway"|"local"|"custom", "dialect", "configured": bool}],
 "models": [{"provider", "model", "api": "chat_completions"|"messages"|"responses", "dialect", "path": "/<provider>/<zone path>",
             "context", "max_out", "default_max_tokens", "images", "tools", "reasoning", "replay", "effort": {"low": "low"|null, …},
             "default_effort", "caching", "billing": "tokens"|"local", "status", "wire_verified", "id_verified",
             "gateway_only": [..] (gateways only),
             "input_musd_per_mtok", "cached_input_musd_per_mtok", "cache_write_musd_per_mtok", "output_musd_per_mtok"}]}
```
No host, base path, endpoint, key or account data. The body builds `ModelInfo` for any `(provider, model)` listed
here; prices in it are hints only (the Root meters from its own catalog).

## Body (WRAP-PROV-BODY)
Status: **paused** (orchestrator PAUSE, 2026-10-22) during the reading phase. No code edits were made.
- Done and committed: nothing (no commits). Uncommitted in the working tree from this sub-lane: nothing under `seed/`.
- Tests: none run (paused before any build).
- Findings for the restart (read, not built):
  - The render seam is `context/render.rs` `head_fields` + `Renderer::msg_bytes` + `assemble`; `context/engine.rs`
    caches per-`Msg` bytes before the last assistant message. `assemble` hard-codes the `messages` array and the
    `max_tokens` key: Responses needs `input` + `max_output_tokens`, Messages needs `system` in the head fields
    (system piece empty).
  - Messages per-message cache: the adapter (`provider/anthropic.rs` `wire_messages`) merges a steer after tool
    results into the tool_result user message, so the context renderer must render a `ToolResults` `Msg` with its
    following `User` msgs (lookahead stops at a Note/Assistant; stable inside `msgs[..closed]`) and render merged
    `User` msgs as empty pieces. B3 (log-context §3.10, >15 positions) moves per request, so a marked message must
    bypass the cache.
  - Model data: `ModelInfo` is `&'static` everywhere (`for_model`, adapters, `Client::new`); catalog rows can be
    `Box::leak`ed once at boot. Boot check is `boot/env.rs` `check_model(&BootJson)` (called once, `boot/mod.rs`
    190); a catalog fallback needs `env.home`. `Target` literals: `context/model.rs` (2), `provider/models.rs`
    `target`, `context/tests.rs` `small_target`; `ModelInfo` literals only in `provider/models.rs`.
  - DeepSeek byte-identity is guarded by `golden/deepseek-six.json` and `deepseek::tests::build_matches_context_renderer`.
- Next steps (priority order of the brief): dialect table `provider/dialects.rs` (ChatDialect rows: deepseek,
  generic, openrouter, gpt-oss, deepseek-host, ollama, lmstudio, llama-server, vllm) threaded through
  `Target`/`ModelInfo` (`dialect`, `path`, `gateway_only`) into `head_fields`, `chat_messages` and `DsDecoder`;
  catalog view → leaked `ModelInfo` in `boot/env.rs`; Messages/Responses renderers + byte-equality tests; SSE fixtures.
