# LLM proxies, gateways and local LLM servers (as of 2026-10-20), for INIT Agent's Root-owned metering egress proxy

Conventions: "STALE" means the source is older than about six months or has been superseded. "UNVERIFIED" means the claim could not be confirmed from a primary source in this pass. Every fact carries an inline source. Research was web-only, with no downloads and no code execution.

## 1. Aggregators and gateways: endpoint format, cost reporting (can a meter trust it?), caching passthrough, tool/reasoning quirks

### Takeaway
Two services return a per-request cost that matches what they actually bill. OpenRouter returns `usage.cost` inline on every response. Vercel AI Gateway returns `total_cost` from an asynchronous generation lookup. Cloudflare AI Gateway, LiteLLM and Portkey compute cost from token counts and their own price tables, and Cloudflare explicitly calls its figure an estimate. INIT's meter should therefore price every request itself, from the returned token counts and a price table it owns, and treat any cost a gateway reports as a reconciliation input, never as the source of truth. The landscape changed a lot in 2026: Portkey was acquired by Palo Alto Networks, Helicone by Mintlify (now in maintenance mode), TensorZero was archived, and LiteLLM's PyPI package was compromised.

### Cited Findings

#### OpenRouter
- Endpoint: OpenAI-style `POST https://openrouter.ai/api/v1/chat/completions`. A cost lookup is available at `https://openrouter.ai/api/v1/generation?id=$GENERATION_ID` — [OpenRouter API reference](https://openrouter.ai/docs/api/reference/overview)
- An Anthropic Messages-compatible surface also exists. Claude Code connects with `ANTHROPIC_BASE_URL="https://openrouter.ai/api"`, the OpenRouter key in `ANTHROPIC_AUTH_TOKEN`, and `ANTHROPIC_API_KEY=""` set explicitly empty. The docs say this only "guaranteed to work with the Anthropic first-party provider". Thinking blocks and native tool use pass through. OpenRouter strips the `[1m]` suffix before routing — [OpenRouter Claude Code integration](https://openrouter.ai/docs/guides/guides/claude-code-integration)
- Usage is now always included. The response carries `prompt_tokens`, `completion_tokens`, `completion_tokens_details.reasoning_tokens`, `prompt_tokens_details.cached_tokens`, `prompt_tokens_details.cache_write_tokens`, `cost` (the total charged to the account, in credits) and `cost_details.upstream_inference_cost` (BYOK only, otherwise 0 or null). The old `usage: {include: true}` and `stream_options.include_usage` flags are deprecated and do nothing. When streaming, usage arrives in the last SSE message — [OpenRouter usage accounting](https://openrouter.ai/docs/use-cases/usage-accounting)
- The API reference also documents `usage.is_byok` and a `cost_details.server_tool_cost` field. `finish_reason` is normalized to `tool_calls`/`stop`/`length`/`content_filter`/`error`, and the raw value is kept in `native_finish_reason`. SSE streams carry periodic "comment" payloads that clients must ignore — [OpenRouter API reference](https://openrouter.ai/docs/api/reference/overview)
- Pricing passthrough: OpenRouter passes provider prices through with no markup on inference. Buying credits costs 5.5% ($0.80 minimum), and crypto purchases cost 5%. The base currency is USD — [OpenRouter FAQ](https://openrouter.ai/docs/faq). UNVERIFIED: no page states "1 credit = $1" outright.
- BYOK: the first $25,000/month of list-price usage is free, and beyond that OpenRouter charges 5% of the normal OpenRouter cost. "Prioritized" keys are tried before OpenRouter capacity and "Fallback" keys after it. If every key fails, OpenRouter falls back to shared capacity by default; a per-key setting can forbid that. BYOK spend is excluded from guardrail and workspace budgets unless "Include BYOK spend" is on — [OpenRouter BYOK](https://openrouter.ai/docs/use-cases/byok); [OpenRouter FAQ](https://openrouter.ai/docs/faq)
- Provider routing lives in the `provider` object, with fields `order`, `allow_fallbacks` (default true), `require_parameters` (default false), `data_collection` (`allow`/`deny`), `zdr`, `enforce_distillable_text`, `only`, `ignore`, `quantizations` (e.g. `int4`, `fp8`, `bf16`), `sort` (`price`/`throughput`/`latency`), `preferred_min_throughput`, `preferred_max_latency` and `max_price`. `max_price` is a hard cap in $/M tokens, split into `prompt`, `completion`, `request` and `image`, and it blocks the request when no endpoint qualifies. A `models` array gives model fallbacks. The `:nitro` and `:floor` suffixes sort by throughput or price and also make priority or flex service-tier endpoints eligible — [OpenRouter provider routing](https://openrouter.ai/docs/features/provider-routing)
- Caching passthrough by provider:
  - Anthropic uses `cache_control`, either top-level (automatic) or on up to 4 blocks. The default TTL is 5 min, with writes at 1.25x; `ttl: "1h"` raises writes to 2x. Reads cost 0.1x.
  - OpenAI caches automatically above 1024 tokens. Writes are free before GPT-5.6 and cost 1.25x from 5.6 on. Explicit `prompt_cache_breakpoint` exists from GPT-5.6.
  - Gemini 2.5+ caches implicitly, with reads at 0.25x. DeepSeek caches automatically, with reads at 0.1x.
  - A `cache_discount` field reports savings; it is negative for Anthropic writes.
  - Sticky provider routing keyed on `session_id` (body field or `x-session-id` header) keeps caches warm and expires after 10 minutes idle. A manual `provider.order` overrides stickiness.
  - Source: [OpenRouter prompt caching](https://openrouter.ai/docs/features/prompt-caching)
- Reasoning uses a unified `reasoning` parameter with `effort` (`max`/`xhigh`/`high`/`medium`/`low`/`minimal`/`none`), `max_tokens`, `exclude` and `enabled`. The output comes back in `message.reasoning` and in a `reasoning_details[]` array whose entries have type `reasoning.text`, `reasoning.summary` or `reasoning.encrypted`, plus `signature` and `format` fields such as `anthropic-claude-v1`.
  - For tool loops, `reasoning_details` must be passed back unmodified and in order. `reasoning_content` is accepted as an alias of `reasoning`.
  - Reasoning tokens are billed as output even when `exclude: true`. If the token budget runs out during reasoning, the response can be empty with `finish_reason: "length"` and still be billed.
  - Source: [OpenRouter reasoning tokens](https://openrouter.ai/docs/use-cases/reasoning-tokens)

#### LiteLLM proxy (self-hosted, Python)
- Cost is computed by LiteLLM from its own model cost map, not reported by the provider. The docs recommend syncing pricing from GitHub. Spend is written to the `LiteLLM_SpendLogs` table and exposed through `/spend/logs`, which truncates at 10,000 rows and sets `x-litellm-spend-logs-truncated`, and through `/spend/logs/v2`.
  - A model with missing pricing is logged with `spend = 0`. LiteLLM only emits a WARNING and increments the `litellm_zero_cost_requests_total` metric.
  - Source: [LiteLLM cost tracking](https://docs.litellm.ai/docs/proxy/cost_tracking)
- Response headers:
  - Cost: `x-litellm-response-cost`, plus component headers `-input`, `-output`, `-cache-read`, `-cache-creation`, `-reasoning` (a subset of output) and `-tool-usage`, and `x-litellm-key-spend`. The component headers appear on non-streaming responses only.
  - Routing and tracing: `x-litellm-call-id`, `x-litellm-model-id`, `x-litellm-model-api-base`, `x-litellm-attempted-retries` and `x-litellm-attempted-fallbacks`.
  - Upstream headers are passed through with an `llm_provider-` prefix.
  - Source: [LiteLLM response headers](https://docs.litellm.ai/docs/proxy/response_headers)
- Security incident: on March 24, 2026, LiteLLM releases 1.82.7 and 1.82.8 on PyPI were malicious. 1.82.8 shipped `litellm_init.pth`, which ran on every Python start and stole SSH keys, `.env` files, cloud credentials, Kubernetes secrets and IMDS data, sending them to `models.litellm.cloud`. Both versions were later yanked — [FutureSearch disclosure](https://futuresearch.ai/blog/litellm-pypi-supply-chain-attack/); corroborated by [Bastion](https://bastion.tech/blog/litellm-pypi-supply-chain-attack)

#### Cloudflare AI Gateway
- Endpoints:
  - Unified OpenAI-compatible endpoint: `https://gateway.ai.cloudflare.com/v1/{account_id}/default/compat/chat/completions`, with models written `{provider}/{model}` (e.g. `anthropic/claude-sonnet-4-5`, `workers-ai/@cf/...`, `dynamic/{route}`).
  - Headers: `cf-aig-authorization: Bearer {CF_AIG_TOKEN}` for an authenticated gateway, plus the provider key in `Authorization`. Without a provider key, the key stored in the gateway (BYOK) is used.
  - STALE: the docs mark this `/compat` endpoint deprecated for single-model chat and recommend `api.cloudflare.com/client/v4/accounts/{ACCOUNT_ID}/ai/v1/chat/completions`. Existing integrations and dynamic routes still work.
  - Source: [Cloudflare unified API](https://developers.cloudflare.com/ai-gateway/usage/chat-completion/index.md)
- Cost is explicitly "an estimation based on the number of tokens sent and received". The docs point to the provider dashboard for accurate figures, and per-request custom costs can override the defaults — [Cloudflare costs](https://developers.cloudflare.com/ai-gateway/observability/costs/index.md)
- Gateway caching is an exact-match response cache, not provider prompt caching. It is controlled by `cf-aig-cache-ttl` (60 s to 1 month), `cf-aig-skip-cache` and `cf-aig-cache-key`, and reported in the `cf-aig-cache-status: HIT|MISS` response header. It covers text and image only and is volatile — [Cloudflare caching](https://developers.cloudflare.com/ai-gateway/features/caching/index.md)
- Pages also exist for BYOK, Unified Billing, dynamic routing and Claude Code integration — [Cloudflare AI Gateway llms.txt](https://developers.cloudflare.com/ai-gateway/llms.txt)

#### Vercel AI Gateway
- Base URL: `https://ai-gateway.vercel.sh/v1` with `Authorization: Bearer $AI_GATEWAY_API_KEY`. It speaks OpenAI Chat Completions, OpenAI Responses, OpenResponses and Anthropic Messages (Anthropic SDK base URL `https://ai-gateway.vercel.sh`). Models are written `creator/model`. Vercel states zero markup on provider token prices, including with BYOK — [Vercel AI Gateway](https://vercel.com/docs/ai-gateway); [REST API](https://vercel.com/docs/ai-gateway/sdks-and-apis/rest-api)
- Budgets are checked before each request but have soft-cap semantics. BYOK spend does not count toward them. A failed BYOK request can fall back to system credentials, so it gets billed to Vercel credits — [Vercel AI Gateway](https://vercel.com/docs/ai-gateway)
- Cost per request:
  - The generation ID comes back as the chat completion `id`. On streams it is injected into the first content chunk.
  - `GET /v1/generation?id=gen_<ulid>` returns `total_cost` (USD debited, surcharges included, BYOK excluded), `market_cost`, `surcharge_cost`, `gateway_cost`, `upstream_inference_cost` (non-zero only for BYOK), `is_byok`, `provider_name`, `tokens_prompt`, `tokens_completion`, `native_tokens_reasoning`, `native_tokens_cached`, `native_tokens_cache_creation` and `billable_web_search_calls`.
  - Ingestion is asynchronous: an immediate lookup returns "Usage event not found", which should be read as "not yet".
  - `GET /v1/credits` returns `balance` and `total_used` in USD. `GET /v1/models` returns per-token pricing, including `input_cache_read` and `input_cache_write`, and needs no auth.
  - Sources: [Vercel REST API](https://vercel.com/docs/ai-gateway/sdks-and-apis/rest-api); [generation lookup](https://vercel.com/docs/ai-gateway/observability-and-spend/usage)
- Caching: by default requests pass through unmodified. With `providerOptions.gateway.caching: 'auto'`, the gateway inserts `cache_control` breakpoints for Anthropic, MiniMax and Alibaba, and also accepts `cache_ttl` (`5m`/`1h`) and `cache_anchor_items`. An `x-session-affinity` header keeps a conversation on the provider holding its cache. Cached reads are reported in `prompt_tokens_details.cached_tokens` (Chat), `cache_read_input_tokens` (Messages) and `input_tokens_details.cached_tokens` (Responses) — [Vercel automatic caching](https://vercel.com/docs/ai-gateway/models-and-providers/automatic-caching)

#### Portkey (now part of Palo Alto Networks Prisma AIRS)
- Palo Alto Networks acquired Portkey to serve as the gateway layer of Prisma AIRS. Earlier reports described the deal as pending; the latest describe it as completed — [GovConWire](https://www.govconwire.com/articles/palo-alto-networks-portkey-acquisition-prisma-airs); [Intelligence Community News](https://intelligencecommunitynews.com/palo-alto-acquires-portkey/). Portkey's own docs now show the banner "Portkey is now PRISMA AIRS AI Gateway" — [Portkey docs](https://portkey.ai/docs/product/ai-gateway)
- Endpoint and headers:
  - Cloud base `https://aigw.portkey.ai/v1/...`, authenticated with `x-portkey-api-key` or `Authorization: Bearer`.
  - `x-portkey-provider` takes a Model Catalog slug such as `@openai-prod`, and models can be written `@provider-slug/model`. `x-portkey-virtual-key` is legacy.
  - Other headers: `x-portkey-config` (JSON or a saved config ID), `x-portkey-custom-host`, `x-portkey-trace-id` and `x-portkey-metadata`.
  - Custom hosts in private or reserved IP ranges are blocked by default.
  - Source: [Portkey headers](https://portkey.ai/docs/aigw/api-reference/inference-api/headers.md)
- The open-source gateway runs locally via `npx @portkey-ai/gateway` — [Portkey docs](https://portkey.ai/docs/product/ai-gateway)
- Cost appears in logs and analytics only. Without `stream_options.include_usage`, streams carry no usage, so Portkey computes no cost. A model without pricing shows 0 cents. Response headers include `x-portkey-trace-id`, `x-portkey-retry-attempt-count` and `x-portkey-cache-status` — [Portkey cost management](https://portkey.ai/docs/product/observability/cost-management); [Portkey response schema](https://portkey.ai/docs/api-reference/response-schema) (via search snippets; UNVERIFIED that no per-request cost field exists)

#### Helicone (acquired by Mintlify; maintenance mode)
- The gateway is OpenAI-compatible at `https://ai-gateway.helicone.ai` with `Authorization: Bearer $HELICONE_API_KEY`. It offers credits with "0% markup", or bring-your-own provider keys — [Helicone docs](https://docs.helicone.ai/)
- Mintlify acquired Helicone on March 3, 2026. The product is reported to be in maintenance mode, receiving security fixes and bug fixes only, with no open-source or self-host commitment stated — [Dealroom](https://ai-startups.dealroom.co/news/feed/mintlify-acquires-open-source-ai-observability-platform-helicone); [HackerNoon analysis](https://hackernoon.com/two-acquisitions-deep-two-still-standing-whats-really-happening-to-open-source-llm-observability). UNVERIFIED in primary form: Helicone's own post at [helicone.ai/blog/joining-mintlify](https://helicone.ai/blog/joining-mintlify) returned HTTP 500 when fetched.

#### Newer or other gateways
- Claude apps gateway is Anthropic's own self-hosted gateway, with SSO and OTLP telemetry. Claude Code also documents the protocol a third-party gateway must speak — [Claude Code LLM gateways](https://code.claude.com/docs/en/llm-gateway)
- Bifrost (Go) offers "20+ providers", virtual keys, hierarchical budgets, semantic cache and an MCP gateway, and claims 11 µs overhead at 5k RPS. That figure is vendor-reported, and the overview does not say how cost is computed — [Bifrost docs](https://docs.getbifrost.ai/overview)
- agentgateway is a Rust data plane for LLM, MCP and A2A traffic that started at Solo.io and is reported to now be a Linux Foundation project, Apache-2.0 licensed. The only source is a vendor comparison — [Maxim comparison](https://www.getmaxim.ai/articles/agentgateway-alternatives-5-ai-gateways-for-agent-and-llm-traffic.md) (UNVERIFIED)
- TensorZero (Rust gateway with an OpenAI-compatible `/openai/v1`) is STALE: its GitHub repository was archived on June 12, 2026 — [TensorZero GitHub](https://github.com/tensorzero/tensorzero)

### Inferences
- **Trust ranking for a meter:**
  - OpenRouter `usage.cost` is the amount debited from the OpenRouter account. It is trustworthy as "what OpenRouter charged" but includes neither the credit-purchase fee nor the BYOK provider bill.
  - Vercel `total_cost` is authoritative for Vercel credits but arrives asynchronously.
  - Cloudflare, LiteLLM, Portkey and Bifrost all derive cost from token counts and a price table, which is the same thing INIT would compute.
  - INIT should therefore always compute its own cost, from token classes × an owned price table, at the proxy. It should store any gateway-reported cost next to its own figure and alarm on divergence above a threshold.
- **Fail closed on unknown prices.** LiteLLM and Portkey silently record $0 for models they cannot price. INIT's meter must refuse to forward (or must reserve a worst-case amount) when a model has no price entry, rather than logging $0.
- **Pre-flight budget reservation.** On OpenRouter, combine `max_tokens` with `provider.max_price` to bound the worst case per request. Set `allow_fallbacks: false` or `only`, and set the BYOK "never use shared capacity" option, so routing cannot silently move to a pricier endpoint or credential. On Vercel, budgets are soft caps and BYOK can fall back to system credentials, so INIT's own cap must be the hard one.
- **Reasoning tokens.** Reasoning tokens are billed as output everywhere documented, even when hidden. A meter that prices only visible text under-counts.
- **Streaming usage.** OpenRouter puts usage in the final SSE chunk. On direct OpenAI-compatible upstreams, INIT should inject `stream_options.include_usage=true` itself so that streamed requests are priceable. If a stream ends without a usage chunk, the meter should fall back to OpenRouter's `/generation` or Vercel's `/v1/generation`, or charge a conservative estimate.
- **Supply chain.** After the LiteLLM compromise, any Python gateway dependency inside Root's trust boundary is a key-exfiltration risk. INIT should implement the proxy natively in Rust rather than embedding LiteLLM. If LiteLLM is ever used, it should run in a separate low-privilege process with pinned hashes.

### Gaps
- No page found states how OpenRouter bills client-aborted streams. The FAQ is silent on this.
- Whether Vercel's OpenAI-compatible chat response carries cost inline (as `provider_metadata.gateway.*`) and under which field name is UNVERIFIED. The docs only say that "much of this data" is in `providerMetadata`.
- Cloudflare Unified Billing and BYOK pages were found but not read in detail.
- Bifrost's and agentgateway's cost-reporting mechanisms were not documented on the pages read.
- Kong AI Gateway, Requesty, Martian/Not Diamond and Envoy AI Gateway were not researched.

## 2. Local servers: ports, OpenAI-compatible coverage, tool/reasoning quirks, KV/prompt cache, auth

### Takeaway
All the major local servers now expose OpenAI `/v1/chat/completions` with tools and streaming, and most also expose `/v1/responses` and an Anthropic-compatible `/v1/messages`. None requires authentication by default; vLLM, SGLang, llama-server, LM Studio and Jan offer optional keys. Reasoning output appears under at least four field names (`reasoning`, `reasoning_content`, `message.thinking`, inline `<think>`), so INIT must normalize it per server. Several servers expose dangerous non-inference endpoints on the same port, including model pull/create/push, built-in shell tools and dev RPC. Port collisions are common: 8080 and 1337 are each used by more than one product.

### Cited Findings

#### Ollama
- Binds `127.0.0.1:11434` by default; `OLLAMA_HOST` changes the bind. CORS is allowed from `127.0.0.1` and `0.0.0.0` by default and extended via `OLLAMA_ORIGINS`. The local server has no documented authentication. Other settings:
  - `OLLAMA_NUM_PARALLEL` (default 1); RAM grows with parallel × context.
  - `OLLAMA_KEEP_ALIVE` (default 5 min).
  - `OLLAMA_KV_CACHE_TYPE` (`f16`, `q8_0`, `q4_0`), which applies only with Flash Attention.
  - `OLLAMA_NO_CLOUD=1` gives local-only mode.
  - Source: [Ollama FAQ](https://docs.ollama.com/faq)
- The OpenAI-compatible base is `http://localhost:11434/v1/`. It serves `/v1/chat/completions`, `/v1/completions`, `/v1/models`, `/v1/embeddings` and, since v0.13.3, `/v1/responses` (stateless only).
  - Unsupported fields: `tool_choice`, `logit_bias`, `user`, `n` and logprobs.
  - `stream_options.include_usage` is supported.
  - `reasoning_effort` and `reasoning.effort` are mapped to model-defined levels.
  - An API key is required by the client but ignored by Ollama.
  - Context size cannot be set through the OpenAI API; it needs a Modelfile with `PARAMETER num_ctx`.
  - Source: [Ollama OpenAI compatibility](https://docs.ollama.com/api/openai-compatibility)
- Anthropic-compatible `/v1/messages`, usable by Claude Code via `ANTHROPIC_BASE_URL=http://localhost:11434` and `ANTHROPIC_AUTH_TOKEN=ollama`:
  - Supported: streaming, tools, thinking (`budget_tokens` accepted but not enforced) and base64 images.
  - Not supported: `cache_control`, `count_tokens`, batches, `tool_choice`, `metadata`, citations and PDF blocks.
  - Usage counts are approximations.
  - Source: [Ollama Anthropic compatibility](https://docs.ollama.com/api/anthropic-compatibility)
- Default context length conflicts between pages:
  - The context-length page says it depends on VRAM: under 24 GiB → 4k, 24–48 GiB → 32k, 48 GiB and up → 256k. It recommends at least 64000 tokens for agents and coding tools — [Ollama context length](https://docs.ollama.com/context-length)
  - The FAQ still says the default is 4096 — [Ollama FAQ](https://docs.ollama.com/faq)
- Thinking: the native `think` parameter takes `true`, `false`, `null` or a level string such as `low`/`medium`/`high` for gpt-oss. Thinking comes back separately in `message.thinking`, not inline — [Ollama thinking](https://docs.ollama.com/capabilities/thinking)
- Tools: native `/api/chat` accepts `tools`. Streaming and parallel tool calls are supported. Tool results are sent back as `{"role":"tool","tool_name":...}`. In the streaming example, accumulated `thinking` is sent back alongside the tool results — [Ollama tool calling](https://docs.ollama.com/capabilities/tool-calling)
- Security:
  - CVE-2026-7482 is a heap out-of-bounds read in the GGUF loader, rated CVSS 9.1 and fixed in 0.17.1. The advisory notes that `/api/create` and `/api/push` "have no authentication" upstream — [GitLab advisory](https://advisories.gitlab.com/golang/github.com/ollama/ollama/CVE-2026-7482/)
  - CVE-2024-28224 was a DNS rebinding flaw fixed in 0.1.29 — [GitLab advisory](https://advisories.gitlab.com/pkg/golang/github.com/ollama/ollama/CVE-2024-28224/)
  - Further 2026 CVEs (CVE-2026-5757 "Bleeding Llama" and CVE-2026-7020 path traversal) are reported by third-party databases — [ThaiCERT](https://www.thaicert.or.th/?p=13906); [GCVE](https://db.gcve.eu/vuln/CVE-2026-7020) (UNVERIFIED patch status)

#### LM Studio
- Base `http://localhost:1234/v1`, serving `GET /v1/models` and `POST /v1/responses`, `/v1/chat/completions`, `/v1/embeddings` and `/v1/completions`. It also has an Anthropic-compatible `/v1/messages` with SSE streaming and tools — [LM Studio OpenAI compat](https://lmstudio.ai/docs/developer/openai-compat); [LM Studio Anthropic messages](https://lmstudio.ai/docs/developer/anthropic-compat/messages)
- Authentication is off by default. An optional "Require Authentication for each request" setting enables Bearer API tokens with per-token permissions — [LM Studio authentication](https://lmstudio.ai/docs/developer/core/authentication)
- Tools: models with native support get a hammer badge. Others use a "default" fallback (custom system prompt plus generic format), and the docs say "results will vary". Badly formed calls end up in `message.content` instead of `tool_calls`. Streamed tool calls arrive in chunks that must be accumulated — [LM Studio tools](https://lmstudio.ai/docs/developer/openai-compat/tools)

#### llama.cpp `llama-server`
- Binds `127.0.0.1` by default (`--host`, env `LLAMA_ARG_HOST`). The master README now says the default port is **9931** (`--port`, env `LLAMA_ARG_PORT`). STALE elsewhere: the historical default was 8080, and OpenCode's docs still use `http://127.0.0.1:8080/v1` — [llama-server README](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md); [OpenCode providers](https://opencode.ai/docs/providers/)
- Features: OpenAI-compatible chat, completions, responses and embeddings, plus "Anthropic Messages API compatible chat completions".
  - `--jinja` is on by default and enables tool calling.
  - `--reasoning-format` defaults to `auto`; the options include `none`, `deepseek` (thoughts in `reasoning_content`) and `deepseek-legacy`. `--reasoning-budget N` is also available.
  - Auth: `--api-key` (comma-separated list) or `--api-key-file`. `/health` stays public.
  - Caching: `--cache-prompt` is on by default; `--cache-reuse` defaults to 0 (off). Prompt caching can make results nondeterministic.
  - Responses include `timings` (`prompt_n`, `cache_n`, `predicted_n`) and `usage.prompt_tokens_details.cached_tokens`.
  - Source: [llama-server README](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md)
- Security:
  - An experimental `--tools` flag exposes `read_file`, `grep_search`, `exec_shell_command`, `write_file` and `edit_file` "for AI agents", with the warning "do not enable in untrusted environments".
  - `--tools`/`--agent` expose file read and write over the API.
  - MCP child processes run with the server's privileges.
  - `--slot-save-path` enables slot save and restore to disk through `POST /slots/{id}?action=...`.
  - Source: [llama-server README](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md)
- Tool calling uses native handlers for Llama 3.x, Functionary, Hermes 2/3, Qwen 2.5 (+Coder), Mistral Nemo, Firefunction, Command R7B, DeepSeek R1 (WIP) and GPT-OSS (Harmony). Anything else falls back to a "Generic" format that uses more tokens. `parallel_tool_calls` is off by default. The docs warn that extreme KV quantization such as `-ctk q4_0` can "substantially degrade" tool calling — [llama.cpp function calling](https://github.com/ggml-org/llama.cpp/blob/master/docs/function-calling.md)

#### vLLM
- Serves `/v1/completions`, `/v1/chat/completions`, `/v1/responses`, `/v1/embeddings` and Anthropic `/v1/messages` plus `/v1/messages/count_tokens`.
  - Dev endpoints enabled by `VLLM_SERVER_DEV_MODE=1` must not be used in production. `/collective_rpc` is "extremely dangerous", and `/pause` and `/sleep` allow denial of service.
  - LoRA dynamic loading is for local development only.
  - Source: [vLLM online serving](https://docs.vllm.ai/en/latest/serving/online_serving/)
- Tool calling needs `--enable-auto-tool-choice` plus `--tool-call-parser`.
  - Parsers include `hermes`, `mistral`, `llama3_json`, `llama4_pythonic`, `openai`, `kimi_k2`, `glm45`, `glm47`, `qwen3_xml`, `deepseek_v3`, `deepseek_v31`, `granite4` and more.
  - `tool_choice` supports `auto`, `required` (vLLM ≥0.8.3), a named function and `none`. `--tool-strict-level` and `VLLM_ENFORCE_STRICT_TOOL_CALLING` add structural-tag constraints.
  - Without strict mode, `auto` arguments may be malformed.
  - Source: [vLLM tool calling](https://docs.vllm.ai/en/latest/features/tool_calling.html)
- Model cards specify `vllm serve ... --port 8000 --enable-auto-tool-choice --tool-call-parser qwen3_coder` for Qwen3-Coder-Next and add `--reasoning-parser qwen3` for Qwen3.6 — [Qwen3-Coder-Next card](https://huggingface.co/Qwen/Qwen3-Coder-Next); [Qwen3.6-35B-A3B card](https://huggingface.co/Qwen/Qwen3.6-35B-A3B). UNVERIFIED from the vLLM docs: that the default port is 8000, the `--api-key` path coverage, and the exact reasoning field name (`reasoning_content` vs `reasoning`).

#### SGLang
- Defaults to `--host 127.0.0.1 --port 30000`. `--api-key` covers the OpenAI-compatible server, and a separate `--admin-api-key` covers admin and control endpoints (weights update, cache flush, `/server_info`). Remote multimodal media URLs are allowed from any domain unless `--allowed-media-domains` is set — [SGLang server arguments](https://docs.sglang.io/advanced_features/server_arguments.html)
- Tool parsers include `deepseekv3`, `deepseekv31`, `deepseekv32`, `glm`, `gpt-oss`, `kimi_k2`, `llama3`, `llama4`, `mistral`, `pythonic`, `qwen`, `qwen3_coder` and `step3`. The gpt-oss parser drops analysis-channel text, which can leave content empty. Full `tool_choice` needs the default xgrammar backend — [SGLang tool parser](https://docs.sglang.io/advanced_features/tool_parser.html)

#### MLX-LM server (Apple)
- `mlx_lm.server` listens on `localhost:8080` and serves `/v1/chat/completions` and `/v1/models`. Usage has `prompt_tokens`, `completion_tokens` and `total_tokens`. The docs say it is "not recommended for production" because it has only basic security checks, and they document no auth. With `--kv-bits` set, requests are processed one at a time — [MLX-LM SERVER.md](https://github.com/ml-explore/mlx-lm/blob/main/mlx_lm/SERVER.md)
- Tool calling: there is a `tool_parsers/` module (an April 2026 issue shows a Gemma 4 parser failing on mlx_lm 0.31.2), but no documented tools API — [mlx-lm issue #1125](https://github.com/ml-explore/mlx-lm/issues/1125) (UNVERIFIED coverage)

#### Jan
- Defaults to `127.0.0.1:1337` with prefix `/v1`. The API key is optional (empty disables auth). CORS is on by default, and a "Trusted hosts" allowlist is available. "Execute Tools on Server" is off by default; when on, it runs MCP tool calls server-side — [Jan API server](https://docs.jan.ai/desktop/api-server)

#### LocalAI
- Examples use `localhost:8080`, serving `/v1/chat/completions`, `/v1/completions` and `/v1/models`, Anthropic `POST /v1/messages`, and Open Responses `POST /v1/responses`. Reasoning comes back in a `reasoning` field. Slot save and restore need an admin key when auth is on — [LocalAI text generation](https://localai.io/docs/features/text-generation/)

#### Atomic Chat (seen in Cline and Goose docs)
- Defaults to `http://127.0.0.1:1337/v1`, the same port as Jan — [Cline local models](https://docs.cline.bot/running-models-locally/overview); [Goose providers](https://goose-docs.ai/docs/getting-started/providers)

#### Reasoning-format conventions
- The gpt-oss cookbook says to expose raw CoT as `reasoning` on the message and on deltas, following OpenRouter's convention. Prior analysis must be passed back until a `final` message and dropped after it. The Responses API uses `reasoning_text` instead — [OpenAI cookbook: handle raw CoT](https://developers.openai.com/cookbook/articles/gpt-oss/handle-raw-cot)
- Qwen3.6 thinks by default (`<think>` blocks). It is disabled with `chat_template_kwargs: {enable_thinking: false}`, and `preserve_thinking: true` carries historical reasoning across turns — [Qwen3.6-35B-A3B card](https://huggingface.co/Qwen/Qwen3.6-35B-A3B)

### Inferences
- **Normalize reasoning at the proxy.** INIT should map `reasoning`, `reasoning_content`, `message.thinking` and any residual inline `<think>…</think>` into one internal reasoning channel, then re-emit per backend on the next turn according to model rules. The rules differ: gpt-oss keeps reasoning until a final answer and drops it after, Qwen3.6 and GLM need "preserve thinking" flags, and OpenRouter requires `reasoning_details` unmodified.
- **Usage gaps.** Ollama's OpenAI path supports `include_usage`, llama-server returns `usage` plus `timings` and `cached_tokens`, and MLX returns basic counts. For backends whose counts are approximate (Ollama's Anthropic path), INIT should label metered tokens "approximate".
- **Server fingerprinting.** Default ports collide: 8080 is used by MLX-LM, LocalAI and older llama-server; 1337 by Jan and Atomic Chat. The setup UX should identify a server by probing its `/v1/models` response shape, or by having the user pick the server kind, not by port alone.
- **KV reuse.** llama-server caches prompts by default, but `--cache-reuse` is off. Ollama keeps a model loaded for 5 min by default. Local prefix reuse therefore depends on keeping the prompt prefix byte-stable (the same discipline as cloud prompt caching) and keeping the model warm.

### Gaps
- vLLM's default port, the API-key scope, prefix-caching defaults and the reasoning field name were not confirmed from vLLM docs; the online-serving page did not contain them.
- SGLang's `--reasoning-parser` list and `--enable-cache-report` were not read; the page was truncated.
- MLX-LM's tools and Responses support is not documented in SERVER.md.
- How Ollama counts "VRAM" on Apple unified memory (which determines whether a Mac gets 4k, 32k or 256k by default) is UNVERIFIED.

## 3. Coding-capable open-weight models for Apple Silicon Macs (16–64 GB), Oct 2026, and harness recommendations

### Takeaway
The strongest local agentic-coding models in Oct 2026 are sparse MoEs with about 3B active parameters: Qwen3.6-35B-A3B (April 2026), Qwen3-Coder-Next 80B-A3B (Feb 2026) and GLM-4.7-Flash 30B-A3B, plus the dense Devstral Small 2 (24B) and Gemma 4 (26B-A4B and 31B). By memory:
- 32–48 GB: Qwen3.6-35B-A3B, GLM-4.7-Flash or Devstral Small 2 at Q4.
- 64 GB: Qwen3-Coder-Next at Q4, which fits only just.
- 16 GB: only gpt-oss-20b-class models or Devstral at a tight fit.

Harness vendors' published recommendations are mostly stale: Cline's dates from Aug 2025, and Aider, Goose and OpenCode give only illustrative examples. Tool-call reliability depends heavily on the server's parser and template and on context size, not only on the model.

### Cited Findings
- **Qwen3.6-35B-A3B:**
  - Size and license: 35B total / 3B active, 262,144-token native context (extensible to about 1.01M), Apache-2.0, released April 2026.
  - Thinking is on by default, and "thinking preservation" is available.
  - Benchmarks: SWE-bench Verified 73.4 and Terminal-Bench 2.0 51.5. The card's comparisons are Qwen3.5-27B (75.0 / 41.6), Gemma4-31B (TB2 42.9) and Gemma4-26B-A4B (TB2 34.2).
  - Recommended serving flags: `--reasoning-parser qwen3 --tool-call-parser qwen3_coder`.
  - Source: [HF card](https://huggingface.co/Qwen/Qwen3.6-35B-A3B)
- **Qwen3-Coder-Next:**
  - Size and license: 80B total / 3B active, 262,144 context, Apache-2.0.
  - Non-thinking only; it emits no `<think>` blocks.
  - SWE-bench Verified 70.6.
  - Serving: `--tool-call-parser qwen3_coder`, vLLM ≥0.15.0, SGLang ≥0.5.8.
  - The card positions it for coding agents and local development.
  - Source: [HF card](https://huggingface.co/Qwen/Qwen3-Coder-Next)
  - Memory: one secondary source reports about 46 GB of unified memory; another reports 52 GB at Q4 — [Morph](https://www.morphllm.com/best-open-source-coding-model-2026); [InsiderLLM](https://insiderllm.com/guides/best-local-coding-models-2026/) (UNVERIFIED, SEO-style sources)
- **GLM-4.7-Flash:**
  - Size and license: 30B-A3B MoE, MIT.
  - Benchmarks: SWE-bench Verified 59.2 and τ²-Bench 79.5. On the same table GPT-OSS-20B scores 34.0 / 47.7.
  - Serving: `--tool-call-parser glm47 --reasoning-parser glm45`. For multi-turn agentic work the card recommends turning on "Preserved Thinking" mode.
  - Source: [HF card](https://huggingface.co/zai-org/GLM-4.7-Flash)
- **Devstral Small 2:**
  - Size and license: 24B, Apache-2.0, text and image input, tools.
  - Ollama package: about 15 GB, 384K context, requires Ollama ≥0.13.3.
  - Benchmarks: SWE-bench Verified 65.8%, Terminal Bench 32.0%. The full Devstral 2 (123B) scores 72.2%.
  - Source: [Ollama library](https://ollama.com/library/devstral-small-2)
- **Gemma 4:**
  - Sizes: E2B, E4B, 26B MoE (4B active) and 31B dense, with native function calling. On Ollama the tags are `gemma4:e2b/e4b/26b/31b`.
  - Early reports (April 2026) describe Ollama's Gemma 4 tool parser routing tool calls into the reasoning field and degrading after 3–4 chained calls.
  - Sources: [Ollama gemma4](https://registry.ollama.ai/library/gemma4); [Codex + Gemma 4 write-up](https://codex.danielvaughan.com/2026/04/10/gemma-4-local-model-codex-cli) (third-party; UNVERIFIED whether these issues are fixed)
- **gpt-oss:** gpt-oss-20b needs about 14 GB and gpt-oss-120b about 66 GB, so 120b does not fit a 64 GB Mac. It is strong at tool calling but not top of coding rankings — [search synthesis of vdf.ai / how2shout](https://vdf.ai/blog/best-local-llm-for-agents-tool-calling/) (UNVERIFIED secondary)
- **Server and quantization effects on tool reliability:** extreme KV quantization degrades tool calling in llama.cpp — [llama.cpp function calling](https://github.com/ggml-org/llama.cpp/blob/master/docs/function-calling.md). Smaller Llama models often emit malformed calls, and auto-mode arguments can be malformed without strict mode — [vLLM tool calling](https://docs.vllm.ai/en/latest/features/tool_calling.html). LM Studio's "default" (non-native) tool format is unreliable — [LM Studio tools](https://lmstudio.ai/docs/developer/openai-compat/tools)
- **Context truncation:** Ollama allocates only 4k context on machines under 24 GiB. Goose's docs say 4096 is "too low" and to set `OLLAMA_CONTEXT_LENGTH`. OpenCode suggests raising `num_ctx` to about 16k–32k if tool calls fail — [Ollama context length](https://docs.ollama.com/context-length); [Goose providers](https://goose-docs.ai/docs/getting-started/providers); [OpenCode providers](https://opencode.ai/docs/providers/)
- **What harnesses recommend:**
  - **Cline** (STALE, 2025-08-28): Qwen3 Coder 30B A3B Instruct at 4-bit, MLX on Apple Silicon, via LM Studio, with 262,144 context, KV-cache quantization off, and "Use compact prompt" on (about 10% of the full prompt size, at the cost of MCP tools and Focus Chain) — [Cline blog](https://cline.bot/blog/local-models). Cline's current docs give only RAM tiers (16–32 GB small or quantized, 32–64 GB mid-size, 64 GB+ larger models) and no model names — [Cline docs](https://docs.cline.bot/running-models-locally/overview)
  - **OpenCode:** no recommended list. Its config examples use `llama2`, `google/gemma-3n-e4b` and `qwen3-coder:a3b` (illustrative and STALE). It suggests picking a Qwen-Coder or DeepSeek-Coder variant for tool calling — [OpenCode providers](https://opencode.ai/docs/providers/)
  - **Goose:** tool calling is required ("models without it can only do chat completion"). The examples use `qwen2.5` (STALE) — [Goose providers](https://goose-docs.ai/docs/getting-started/providers). A 2026 Goose blog says Ollama tool calling works through a toolshim interpreter, while built-in llama.cpp has native tool calling for some models such as Gemma 4 — [Goose blog](https://goose-docs.ai/blog/2026/04/24/use-goose-with-built-in-local-inference) (via search snippet)
  - **Aider:** no recommended model. It says to prefer `ollama_chat/` over `ollama/`. STALE: it still states that Ollama defaults to a 2k context — [Aider Ollama](https://aider.chat/docs/llms/ollama.html)

### Inferences
- **Proposed Mac tiers for INIT's model picker** (UNVERIFIED memory math; validate on the owner's machine):
  - 16 GB: gpt-oss-20b or a Qwen3.6 small variant, mostly for chat. Recommend a gateway model for real agentic work.
  - 24–36 GB: Devstral Small 2 (Q4, about 15 GB) or Qwen3.6-35B-A3B / GLM-4.7-Flash at Q4.
  - 48–64 GB: Qwen3.6-35B-A3B at Q6–Q8, or Qwen3-Coder-Next at Q4 (tight, with reduced context).
- **Treat local tool-calling as a capability INIT measures, not assumes.** A short tool-call conformance probe (parallel calls, a nested-JSON argument, 4+ chained calls, reasoning plus tool interleave) run at model-registration time would catch parser and template failures. This matters given the Gemma 4 parser and LM Studio fallback reports.
- **Set the context explicitly.** INIT should always set the context length explicitly, via `num_ctx` on Ollama's native API, Modelfile defaults or a server flag. It should refuse to run an agent loop when the effective context is below a floor such as 32k, because silent truncation corrupts tool loops.

### Gaps
- Independent, reproducible tool-calling reliability benchmarks for these models on Apple Silicon were not found. Rankings found are vendor or SEO-style.
- Names such as "Qwen3.8", "Kimi K3", "GLM-5.2" and "DeepSeek V4" appear on SEO pages but were not verified; they are large or cloud-scale anyway.
- No current recommended-model list was found from Aider or OpenCode.
- Exact quantized memory footprints (MLX vs GGUF) for each model were not verified from primary sources.

## 4. Security: letting a sandboxed agent reach a loopback LLM server without reaching other local services

### Takeaway
The pattern that state-of-the-art harnesses converge on is:
- Deny loopback by default.
- Resolve hostnames at the proxy and refuse names that resolve to loopback, link-local or host addresses (anti-DNS-rebinding and anti-SSRF).
- Allow a local server only as an explicit IP literal plus port (e.g. `127.0.0.1:11434`).
- Never use a blanket "allow local binding", which opens every localhost port.

Port-level allowlisting is not enough for local LLM servers. The same port also serves model management (`/api/pull`, `/api/create`, `/api/push` on Ollama), built-in shell tools (llama-server `--tools`), and admin or dev RPC (vLLM, SGLang). INIT's proxy therefore needs method and path allowlists as well.

### Cited Findings
- **Claude Code sandbox:**
  - Network: sandboxed commands have no direct route out and go through a local proxy with an initially empty domain allowlist.
  - Resolved-address check: after a hostname passes the allowlist, the proxy refuses it if it resolves only to local addresses (loopback, link-local such as `169.254.169.254`, or the host's own addresses). `localhost`/`*.localhost` are allowed to resolve to loopback. To allow a local server, add the IP literal with its port, e.g. `"127.0.0.1:8080"`.
  - Before v2.1.284, the proxy connected to whatever address an allowed hostname resolved to.
  - Source: [Claude Code sandboxing](https://code.claude.com/docs/en/sandboxing)
- **Claude Code sandbox, macOS:** `network.allowLocalBinding: true` lets commands "connect to any port on localhost", including every other unauthenticated local service such as debuggers.
  - On Linux and WSL2, the sandbox's localhost is private, and `allowLocalBinding` has no effect.
  - An `allowedDomains` entry for `localhost` exposes every localhost port to proxy-using commands. Claude Code sets `NO_PROXY` so that `localhost` connects directly.
  - Allowing `/var/run/docker.sock` through `allowUnixSockets` effectively grants host access.
  - Source: [Claude Code sandboxing](https://code.claude.com/docs/en/sandboxing)
- **Anthropic `sandbox-runtime` (srt):** deny-by-default `network.allowedDomains`, with `:port` suffixes and bracketed IPv6. `deniedDomains` wins. The evaluation order is deny → allow → strictAllowlist → per-command → ask callback.
  - Resolved-address denial covers loopback, link-local, multicast, cloud metadata and all host interface addresses. An IP literal in `allowedDomains` is honored as "an explicit choice".
  - `network.deniedResolvedAddresses` can add RFC1918/CGNAT/ULA ranges.
  - On macOS, the Seatbelt profile only allows connecting to the proxy's localhost port. On Linux, the network namespace is removed and traffic goes through Unix-socket-bridged proxies.
  - Unix sockets are blocked by default.
  - Source: [sandbox-runtime README](https://github.com/anthropic-experimental/sandbox-runtime)
- **OpenAI Codex:**
  - `sandbox_workspace_write.network_access` is a boolean.
  - The experimental `features.network_proxy` has `domains` allow/deny maps (deny wins; `*.`, `**.` and `*` patterns). `allow_local_binding` defaults to false, and "exact local IP or `localhost` allow rules can still permit specific targets".
  - Unix sockets have a `unix_sockets` allow/deny map, and `dangerously_allow_all_unix_sockets` defaults to false. Proxy defaults are `http://127.0.0.1:3128` and SOCKS `:8081`.
  - Source: [Codex config reference](https://learn.chatgpt.com/docs/config-file/config-reference)
- **Server-side exposure on the same port:**
  - Ollama's `/api/create` and `/api/push` are unauthenticated, and CVE-2026-7482 is reachable through model loading — [GitLab advisory](https://advisories.gitlab.com/golang/github.com/ollama/ollama/CVE-2026-7482/)
  - llama-server `--tools` exposes `exec_shell_command`/`write_file`, and `--slot-save-path` exposes disk writes via `/slots` — [llama-server README](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md)
  - vLLM's dev mode exposes `/collective_rpc`, `/pause` and `/sleep` — [vLLM online serving](https://docs.vllm.ai/en/latest/serving/online_serving/)
  - SGLang separates `--admin-api-key` for control endpoints and fetches remote media from any domain unless restricted — [SGLang server args](https://docs.sglang.io/advanced_features/server_arguments.html)
  - Jan has CORS on by default and optional server-side MCP tool execution — [Jan API server](https://docs.jan.ai/desktop/api-server)
- **DNS rebinding and browser exposure:** Ollama CVE-2024-28224 allowed remote access to the full API via DNS rebinding — [GitLab advisory](https://advisories.gitlab.com/pkg/golang/github.com/ollama/ollama/CVE-2024-28224/). A 2026 CSA research note, which was AI-generated and is not officially reviewed, describes CVE-2026-65105, a NemoClaw-launched Ollama bound to all interfaces — [CSA note](https://labs.cloudsecurityalliance.org/wp-content/uploads/2026/09/CSA_research_note_nemoclaw_dns_rebinding_model_poisoning_20260905-csa-styled.pdf) (UNVERIFIED)
- **Gateway-side precedent:** Portkey blocks private and reserved IP ranges for `x-portkey-custom-host` by default — [Portkey headers](https://portkey.ai/docs/aigw/api-reference/inference-api/headers.md). Claude Code's gateway model discovery treats any redirect as failure "so the credential can't leak to a redirect target" — [Claude Code gateway protocol](https://code.claude.com/docs/en/llm-gateway-protocol)

### Inferences
- **Recommended INIT rule set**, enforced in Root's egress proxy. The agent never connects to the LLM server directly; only Root's proxy does.
  1. Endpoints are registered by the owner as `(scheme, IP literal 127.0.0.1 or [::1], port, server-kind)`. Never accept hostnames that resolve to loopback, and never accept a `localhost` wildcard.
  2. Per server kind, allow only an explicit (method, path) set. Examples:
     - `POST /v1/chat/completions`, `POST /v1/responses`, `POST /v1/messages`, `GET /v1/models`
     - Ollama only: `POST /api/chat` and `POST /api/show` (`/api/show` is read-only metadata, used for `thinking.values`)
     - Deny `/api/pull`, `/api/create`, `/api/push`, `/api/delete`, `/slots*`, `/collective_rpc`, `/pause`, `/sleep`, admin paths and `/metrics`, unless the owner runs those through Root separately.
  3. Strip or deny request fields that trigger server-side tool execution or remote fetches, such as remote media URLs.
  4. Disable redirects.
  5. Cap request and response body sizes and enforce timeouts.
  6. Set `Host` to the literal address, to defeat rebinding-style Host confusion.
- **Detect dangerous server flags.** At registration, Root can warn when a llama-server reports `--tools`/`--agent` enabled, or when Ollama is bound to `0.0.0.0`. How to detect these flags remotely is UNVERIFIED; it may require the owner to attest them.
- **Keep the agent's sandbox clean.** The agent's own sandbox should keep loopback fully denied, including no `allowLocalBinding`. The Root proxy, outside the sandbox, is the only component holding an allow rule to the local LLM port. This preserves "Root holds the keys and meters", and it also prevents the agent from abusing a keyless local server's management endpoints.

### Gaps
- No harness was found that documents a path-level allowlist specifically for local LLM servers. Path filtering appears to be an INIT-specific requirement, based on the server-side exposures documented above.
- How to reliably fingerprint server flags (`--tools`, slot-save) over HTTP was not researched.
- Goose, OpenCode and Aider were not found to document any loopback-isolation mechanism.

## 5. How other agentic coders configure custom providers and base URLs (UX references)

### Takeaway
Every major harness converges on the same small schema:
- an id/name
- a wire protocol (OpenAI Chat, OpenAI Responses, Anthropic Messages, Ollama)
- a base URL
- a key reference (env var name or a stored-credential command, never an inline secret)
- optional extra headers
- a per-model metadata map (context window, max output, prices including cache read/write, reasoning and vision flags)

Claude Code adds useful gateway conventions: attribution headers, forwarding `anthropic-*` headers unchanged, and model discovery via `/v1/models`. Codex now accepts only `wire_api = "responses"`.

### Cited Findings
- **OpenCode** (`opencode.json` → `provider.<id>`):
  - Fields: `"npm": "@ai-sdk/openai-compatible"` for `/v1/chat/completions` (or `@ai-sdk/openai` for `/v1/responses`), `options.baseURL`, optional `options.apiKey` using `{env:VAR}` syntax, `options.headers`, and `models.<id>` with `name` and `limit.context`/`limit.output`.
  - Local examples: Ollama `http://localhost:11434/v1`, LM Studio `http://127.0.0.1:1234/v1`, llama.cpp `http://127.0.0.1:8080/v1`.
  - Credentials added via `/connect` are stored in `~/.local/share/opencode/auth.json`.
  - Gateways: OpenRouter accepts per-model `options.provider.order`/`allow_fallbacks`. Vercel accepts `order`/`only`/`zeroDataRetention`. Helicone uses baseURL `https://ai-gateway.helicone.ai` plus headers such as `Helicone-Cache-Enabled`. Cloudflare uses `CLOUDFLARE_ACCOUNT_ID`, `CLOUDFLARE_GATEWAY_ID` and `CLOUDFLARE_API_TOKEN`.
  - Source: [OpenCode providers](https://opencode.ai/docs/providers/)
- **Codex** (`config.toml` → `model_providers.<id>`):
  - Fields: `name`, `base_url`, `env_key`, `env_key_instructions`, and `wire_api`, where "`responses` is the only supported value". Also `query_params`, `http_headers`, `env_http_headers`, `request_max_retries` (4), `stream_idle_timeout_ms` (300000), `stream_max_retries` (5) and `requires_openai_auth`.
  - `auth.command`/`args`/`cwd`/`refresh_interval_ms`/`timeout_ms` supply a command-backed bearer token.
  - The built-in IDs `openai`, `ollama` and `lmstudio` are reserved. `oss_provider = "lmstudio"|"ollama"` sets the default provider for `--oss`.
  - STALE: older guides that use `wire_api = "chat"`.
  - Source: [Codex config reference](https://learn.chatgpt.com/docs/config-file/config-reference)
- **Claude Code:**
  - Connection: `ANTHROPIC_BASE_URL` plus a gateway credential (`ANTHROPIC_AUTH_TOKEN` as Bearer, `ANTHROPIC_API_KEY` as `x-api-key`, or `apiKeyHelper`). `ANTHROPIC_CUSTOM_HEADERS` adds headers.
  - Gateway requirements: serve `/v1/messages` (optionally `/v1/messages/count_tokens`) and forward `anthropic-version`/`anthropic-beta` unchanged as open lists. Do not buffer SSE, keep `ping` events, and forward error bodies unmodified, because client retry logic matches on their wording. Forward `cache_control` unchanged, or every turn is billed as uncached.
  - Attribution headers for cost attribution: `x-claude-code-session-id`, `x-claude-code-agent-id` and `x-claude-code-parent-agent-id`. Opt-in hint headers (`CLAUDE_CODE_GATEWAY_HINT_HEADERS=1`): `x-claude-code-request-class` (main/subagent/compaction/auxiliary), `x-claude-code-prompt-id` and `x-claude-code-context-compacted`.
  - Model discovery: `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1` → `GET /v1/models?limit=1000` with a 3 s timeout. Any redirect counts as failure. IDs must contain `claude` or `anthropic`. Unknown IDs are assumed to have 200K context unless suffixed `[1m]`.
  - Source: [Claude Code gateway protocol](https://code.claude.com/docs/en/llm-gateway-protocol)
  - Lock-down: managed `allowedProviders: ["customEndpoint"]` pins a single gateway base URL — [Claude Code LLM gateways](https://code.claude.com/docs/en/llm-gateway)
- **Goose:**
  - Custom providers are declarative JSON files in `~/.config/goose/custom_providers/`, with fields `name`, `engine` (`openai`/`anthropic`/`ollama`), `base_url`, `api_key_env`, `models`, `headers`, `supports_streaming` and `requires_auth`.
  - Other settings: `OLLAMA_HOST` (default `localhost:11434`), `GOOSE_PROVIDER` and `GOOSE_MODEL`.
  - Docker Model Runner uses `OPENAI_HOST`/`OPENAI_BASE_PATH`.
  - Source: [Goose providers](https://goose-docs.ai/docs/getting-started/providers)
- **Crush:**
  - Provider types: `openai`, `openai-compat`, `anthropic`, `ollama`, `llamacpp`, `lmstudio`, `omlx` and `litellm`.
  - Per-model metadata: `--context-window`, `--default-max-tokens`, `--price-input`, `--price-output`, `--price-cache-create`, `--price-cache-hit`, `--can-reason` and `--supports-images`.
  - `discover_models: true` auto-discovers models and merges them with explicit config, with explicit values winning.
  - Provider defaults come from the Catwalk database and auto-update; this can be overridden with `CATWALK_URL` or disabled with `CRUSH_DISABLE_PROVIDER_AUTO_UPDATE=1`.
  - Security note: `crush.json` is trusted code, and `$(...)` in it executes at load time.
  - Source: [Crush README](https://github.com/charmbracelet/crush)
- **Aider (LiteLLM model strings):**
  - Generic OpenAI-compatible: `openai/<model>` with `OPENAI_API_BASE`/`OPENAI_API_KEY` — [Aider OpenAI-compatible](https://aider.chat/docs/llms/openai-compat.html)
  - Ollama: `ollama_chat/<model>` with `OLLAMA_API_BASE=http://127.0.0.1:11434`, and `num_ctx` set via `.aider.model.settings.yml` `extra_params` — [Aider Ollama](https://aider.chat/docs/llms/ollama.html)
- **Gateway-side UX patterns:**
  - OpenRouter's `provider` object (order/only/ignore/max_price/zdr) — [OpenRouter provider routing](https://openrouter.ai/docs/features/provider-routing)
  - Portkey's `@provider-slug/model` inline routing — [Portkey headers](https://portkey.ai/docs/aigw/api-reference/inference-api/headers.md)
  - Vercel's `/v1/models` catalog carrying pricing, `reasoning_options` and tags, which is usable for auto-populating model metadata — [Vercel REST API](https://vercel.com/docs/ai-gateway/sdks-and-apis/rest-api)

### Inferences
- **Proposed INIT provider entry**, owned by Root:
  - `id`
  - `protocol` ∈ {openai-chat, openai-responses, anthropic-messages, ollama-native}
  - `base_url`
  - `credential_ref`, held only by Root: a keychain item or command, never in the agent's environment
  - `pricing_source` ∈ {owned-table, gateway-reported-reconcile, local-zero}
  - `models.<id>`: {context, max_output, price_in, price_out, price_cache_read, price_cache_write, price_reasoning=out, reasoning_format, tool_call_tier, verified_at}
- **Borrowed conventions:**
  - From Crush: per-model cache prices and discovery that merges with explicit config.
  - From Claude Code: forward unknown headers and fields unchanged as open lists, emit session/agent attribution headers, and fail on redirects.
  - From Codex: command-backed auth with a refresh interval.
- **Avoid two anti-patterns.** Crush's `$(...)`-in-config execution, and auto-updating the price DB from a remote source without review, would let a remote or repo-supplied file change what Root bills or executes. INIT's price table should be versioned and changes should be owner-approved.

### Gaps
- OpenCode's current recommended-models page and Aider's model-metadata JSON format (`.aider.model.metadata.json`) were not read.
- Goose's toolshim flags (`GOOSE_TOOLSHIM*`) are referenced only by third-party guides; the official providers page does not mention them (UNVERIFIED).
- Crush's full `crush.json` JSON key names for pricing (as opposed to the `crushrc` flags) were not shown on the README.
