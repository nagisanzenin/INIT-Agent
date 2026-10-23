# LLM API provider landscape, October 2026: surfaces, wire quirks, models, and prices for the INIT Agent Root egress meter

Research date: 2026-10-20. All prices are USD per 1M tokens (MTok) unless noted. "Verified 2026-10-20" means the official page was fetched that day.

Method caveat: pages were read through a summarizing fetch tool, and a few long pages were only partly read. Before hard-coding a price into the meter, spot-check the number on the live page and keep a `source_url` and `verified_date` on every price row (the table in Q3 does this).

Staleness rule: anything sourced from a page or release older than about 2026-04-20 is flagged **[STALE?]**. Anything taken from a third-party aggregator rather than the provider is flagged **[3P]**. Anything the research could not confirm is marked **UNVERIFIED**, together with the source that was tried.

---

## Q1. Providers and their API surfaces

### Takeaway
The market now runs on four native wire protocols:
- OpenAI **Responses**
- Anthropic **Messages**
- Google **generateContent** and the newer **Interactions** API
- AWS **Converse**

On top of these sits a long tail of "OpenAI Chat Completions-compatible" endpoints. The Anthropic Messages shape has also become a second de-facto standard. DeepSeek, Kimi, Z.ai, MiniMax, Bedrock-Mantle, Vertex and Foundry all expose it.

The October 2026 frontier is:
- OpenAI: GPT-6 Astra, GPT-6.1 Sol, GPT-6 Luna, GPT-5.6 Sol/Terra/Luna
- Anthropic: Claude Fable 5.1, Opus 5.5, Sonnet 5.5, Haiku 5.5
- Google: Gemini 4 Argon, Gemini 3.8 Flash
- xAI: Grok 4.7
- DeepSeek: V4.1-Flash and V4-Pro
- Moonshot: Kimi K3
- Z.ai: GLM-5.3
- MiniMax: M3 and M3.1-Flash
- Alibaba: Qwen3.8-Max, Qwen3.7-Plus, Qwen3.8-Flash
- Mistral: Medium 3.5 and Large 4

### Cited Findings

**OpenAI: Chat Completions vs Responses**
- OpenAI recommends the Responses API. Chat Completions is "still supported" but gives less improved intelligence and performance. Chat Completions uses `reasoning_effort`, while Responses uses `reasoning.effort`. — [OpenAI reasoning guide](https://developers.openai.com/api/docs/guides/reasoning)
- **Chat Completions does not support function calling with GPT-6 Astra or GPT-6.1 Sol. Use Responses for tools.** — [OpenAI reasoning guide](https://developers.openai.com/api/docs/guides/reasoning). The model page confirms this for GPT-6.1 Sol: "Chat Completions is supported without tool calling". — [gpt-6.1-sol](https://developers.openai.com/api/docs/models/gpt-6.1-sol)
- The Astra model page lists function calling as a supported feature but does not say it works on Chat Completions. The reasoning guide says it does not. — [gpt-6-astra](https://developers.openai.com/api/docs/models/gpt-6-astra)
- GPT-6 Luna supports function calling on Chat Completions only with `reasoning_effort` set to `none`. — [gpt-6-luna](https://developers.openai.com/api/docs/models/gpt-6-luna)
- Effort values depend on the model, from `none`, `minimal`, `low`, `medium`, `high`, `xhigh` to `max`:
  - GPT-6 Astra rejects `none` with HTTP 400.
  - GPT-6.1 Sol does not support `none` or `minimal` and defaults to `medium`.
  - On GPT-5.6 and GPT-6 (Responses only), `reasoning.mode` is `standard` (the default) or `pro`. Mode is independent of effort.
  — [OpenAI reasoning guide](https://developers.openai.com/api/docs/guides/reasoning)
- Reasoning items:
  - Reasoning items in `output` are opaque. The API does not return their reasoning text.
  - In stateless mode (`store:false`, or a ZDR organization), reasoning items carry `encrypted_content` by default. `include: ["reasoning.encrypted_content"]` is now legacy and optional.
  - OpenAI "highly recommend[s]" passing back the reasoning items returned with the last function call. After several consecutive function calls, pass back all reasoning, function_call and function_call_output items since the last user message.
  - `reasoning.context` takes `auto`, `current_turn` or `all_turns`. GPT-5.6 defaults to `all_turns`. With `store:false` plus `all_turns`, you must keep every output item and replay the full history.
  - Reasoning can only be reused within the same model family.
  — [OpenAI reasoning guide](https://developers.openai.com/api/docs/guides/reasoning)
- Reasoning tokens are billed as output tokens and reported in `usage.output_tokens_details.reasoning_tokens`. `max_output_tokens` caps reasoning and visible output together. When the cap is hit, the response returns `status: "incomplete"` with `incomplete_details.reason: "max_output_tokens"`, possibly with no visible output at all. — [OpenAI reasoning guide](https://developers.openai.com/api/docs/guides/reasoning)

**OpenAI prompt caching (new rules for GPT-5.6 and later)**
- Minimum cacheable length is 1,024 tokens for GPT-5.6+. Earlier models vary.
- Cache TTL:
  - GPT-5.6+: `prompt_cache_options.ttl` accepts only `"30m"`.
  - GPT-5.5: `prompt_cache_retention` accepts `"24h"` only.
  - Older models: `"in_memory"` or `"24h"`.
- Cache writes:
  - GPT-5.6+: writes cost 1.25x the uncached input rate, and reads cost 0.1x (0.05x on GPT-6.1 Sol). The write charge is "not an additive fee". Prewarm writes are billed at the write rate.
  - GPT-5.5 and earlier: no write charge.
- Usage fields: `usage.input_tokens`, `usage.input_tokens_details.cached_tokens`, `usage.input_tokens_details.cache_write_tokens`.
- Explicit mode (GPT-5.6+): set `prompt_cache_options.mode:"explicit"` plus `prompt_cache_breakpoint:{mode:"explicit"}` on content blocks. A request can create at most 4 cache writes.
- `prompt_cache_key` is optional on 5.6+ and is used for separate cache accounting.
- Cached tokens still count toward TPM limits.
— [OpenAI prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching)

**Anthropic Messages**

Models:
- Current lineup and IDs: `claude-fable-5-1`, `claude-opus-5-5`, `claude-sonnet-5-5`, `claude-haiku-5-5`. All have a 1M context window and 128K max output.
- Thinking is "Adaptive (always on)" for Fable 5.1 and Opus 5.5, and "Adaptive" for Sonnet 5.5 and Haiku 5.5. Default effort is `high` for Fable 5.1 and Sonnet 5.5, and `medium` for Opus 5.5 and Haiku 5.5.
- Dateless IDs are pinned snapshots from the 4.6 generation on.
- On the Batches API, newer models allow up to 300K output with the beta header `output-300k-2026-03-24`.
— [Anthropic models overview](https://platform.claude.com/docs/en/about-claude/models/overview)

Thinking:
- Extended thinking (`thinking:{type:"enabled", budget_tokens}`) is not accepted on Claude 4.7 and later. Use `thinking:{type:"adaptive"}` plus `output_config.effort`. — [Anthropic errors](https://platform.claude.com/docs/en/api/errors)
- `thinking:{type:"disabled"}` returns 400 on Fable 5.1/5, Mythos 5.1/5 and Opus 5.5.
- Sonnet 5.5 rejects `disabled`. Its lowest setting is `{type:"between_tools"}`, allowed only at effort `high` or below.
- Haiku 5.5 accepts `disabled` at effort `high` or below.
— [Anthropic errors](https://platform.claude.com/docs/en/api/errors); [Anthropic thinking](https://platform.claude.com/docs/en/build-with-claude/thinking)
- Thinking display:
  - Each thinking block carries a `signature`, which is "an encrypted copy of the full reasoning that you pass back unchanged".
  - `display` defaults to `"omitted"` on the 5.x models. This returns empty `thinking` text with the signature only. `"summarized"` returns summary text. `"updates"` is beta (header `thinking-display-updates-2026-08-18`).
  - You are charged for the full thinking tokens, not the summary.
  — [Anthropic thinking](https://platform.claude.com/docs/en/build-with-claude/thinking)
- **Replay is mandatory with tools.** "When you return tool results, you must pass the thinking blocks from the assistant message back to the API, complete and unmodified." This includes `redacted_thinking` blocks and blocks with an empty `thinking` field. Modified blocks are rejected with 400. — [Anthropic thinking](https://platform.claude.com/docs/en/build-with-claude/thinking); [Anthropic errors](https://platform.claude.com/docs/en/api/errors)
- Thinking-block binding (new):
  - On Fable 5.1, Opus 5.5, Sonnet 5.5 and Haiku 5.5, a replayed thinking block is valid only while the `system`, `tools` and earlier messages are unchanged.
  - For accounts created on or after 2026-08-31, changing that history returns 400 "Invalid `signature` in `thinking` block. The block is bound to a different conversation".
  - The fix is to keep history append-only, or use beta `thinking-binding-controls-2026-08-01` with `prefix_mismatch_behavior:"drop_block"`.
  — [Anthropic errors](https://platform.claude.com/docs/en/api/errors)

Tool use quirks:
- **Forced tool use is not supported** on Opus 5.5, Sonnet 5.5, Fable 5.1 and Mythos 5.1. `tool_choice:{type:"any"}` or `{type:"tool"}` returns 400. Only `auto` and `none` are allowed.
- Assistant prefill returns 400 on Claude 4.6 and later.
— [Anthropic errors](https://platform.claude.com/docs/en/api/errors)

Prompt caching:
- Syntax is `cache_control:{type:"ephemeral"}` (5 min) or `{type:"ephemeral", ttl:"1h"}`.
- Automatic caching uses a single top-level `cache_control`.
- At most 4 breakpoints. Lookback is 20 positions per breakpoint.
- The hierarchy is tools → system → messages. Changing tools, web search, citations or `speed` invalidates everything after them. Changing thinking parameters invalidates the message blocks.
- Usage fields: `cache_creation_input_tokens`, `cache_read_input_tokens`, `input_tokens` (counts only tokens after the last breakpoint), and `cache_creation.{ephemeral_5m_input_tokens, ephemeral_1h_input_tokens}`.
- `total_input = cache_read + cache_creation + input_tokens`.
- Prompts below the minimum length are "processed without caching, and no error is returned".
- Minimum cacheable length:
  - 512 tokens: Fable 5.1/5, Mythos 5.1/5, Opus 5.5/5, Sonnet 5.5, Haiku 5.5
  - 1,024 tokens: Opus 4.8, Sonnet 5, Sonnet 4.6/4.5, Opus 4.1/4, Sonnet 4
  - 2,048 tokens: Opus 4.7, Mythos Preview, Haiku 3.5
  - 4,096 tokens: Opus 4.6/4.5, Haiku 4.5
- Caches are isolated per workspace on the Claude API. Bedrock and Google Cloud isolate per organization.
— [Anthropic prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)

Streaming and fallback:
- The SSE flow is: `message_start`; then per block `content_block_start`, `content_block_delta` (`text_delta`, `input_json_delta` with `partial_json`, `thinking_delta`, `signature_delta`), `content_block_stop`; then `message_delta`; then `message_stop`. `ping` events can appear anywhere.
- **`message_delta` usage counts are cumulative.**
- Mid-stream errors arrive as `event: error` (for example `overloaded_error`).
- Fine-grained tool input streaming is enabled per tool with `eager_input_streaming`.
— [Anthropic streaming](https://platform.claude.com/docs/en/build-with-claude/streaming)
- Server-side fallback (`fallbacks:"default"`) is beta and Claude API only. It is not available on Batches, Bedrock, Google Cloud or Foundry.
  - During fallback, a `fallback` content block marks each model boundary.
  - Per-model usage appears in `usage.iterations`. An entry of type `fallback_message` means a fallback model ran.
  - The refusal that triggered the fallback is billed in addition to the fallback request when it arrived mid-stream or falls in a billed category.
  - Refusals return 200 with `stop_reason:"refusal"` and `stop_details.category`. They are billed before any output only for categories `bio`, `frontier_llm` and `reasoning_extraction` (as of September 2026).
  — [Anthropic refusals and fallback](https://platform.claude.com/docs/en/build-with-claude/refusals-and-fallback); [Anthropic streaming](https://platform.claude.com/docs/en/build-with-claude/streaming)
- Claude 4.7 and later use a new tokenizer that produces about 30% more tokens for the same text. — [Anthropic pricing](https://platform.claude.com/docs/en/about-claude/pricing)

**Google Gemini**
- Gemini 4 Argon (`gemini-4-argon`, Stable, updated October 2026):
  - Context window of 1,048,576 tokens, unified for input and output.
  - Output is 65,536 tokens by default, or up to 1,048,576 with "long decode".
  - Thinking levels are low, medium and high.
  - Implicit caching only; "Explicit caching is not supported".
  - Supports Batch, Flex and Priority. Does not support the Live API.
  — [Gemini 4 Argon model page](https://ai.google.dev/gemini-api/docs/models/gemini-4-argon)
- **Interactions API:**
  - `POST https://generativelanguage.googleapis.com/v1beta/interactions` with header `x-goog-api-key`.
  - Generally Available "as of June 2026" and recommended for new projects.
  - "Going forward, all new models, multimodal capabilities, tools, and agentic features will launch on the Interactions API". generateContent "remains fully supported".
  - Stateful via `previous_interaction_id`, or stateless by resending history. `store=false` cannot be combined with `previous_interaction_id`.
  - Stored interactions are kept 1 day on free and 55 days on paid.
  — [Gemini Interactions API](https://ai.google.dev/gemini-api/docs/interactions)
- Thinking:
  - Default levels: Argon, 3.8 Flash and 3.6 Flash default to `medium`. 3.5 Flash-Lite defaults to `minimal`. 3.1 Pro Preview defaults to `high`.
  - Billing: "response pricing is the sum of output tokens and thinking tokens". The usage field is `total_thought_tokens` (Interactions).
  - Thought summaries come via `thinking_summaries` (`auto` or `none`).
  - Thought signatures: in stateless mode, "You MUST always resend all thought blocks exactly as they were received". This includes built-in tool call and result blocks.
  — [Gemini thinking](https://ai.google.dev/gemini-api/docs/thinking)
- Implicit caching is on by default for Gemini 2.5 and newer.
  - Minimum is 4,096 tokens for Argon, 3.8 Flash, 3.6 Flash and 3.1 Pro, and 2,048 for 2.5 Flash and Pro.
  - Cache hits appear in `usage.total_cached_tokens`.
  - Explicit caching is not supported in the Interactions API, nor on Argon and later.
  — [Gemini caching](https://ai.google.dev/gemini-api/docs/caching)
- generateContent `usageMetadata` fields:
  - `promptTokenCount`: "When `cachedContent` is set, this is still the total effective prompt size."
  - `cachedContentTokenCount`
  - `candidatesTokenCount`
  - `toolUsePromptTokenCount`
  - `thoughtsTokenCount`
  - `totalTokenCount`
  - Endpoints are `/v1beta/{model}:generateContent` and `:streamGenerateContent?alt=sse`.
  — [Gemini generateContent API reference](https://ai.google.dev/api/generate-content)
- OpenAI-compatible endpoint:
  - Base `https://generativelanguage.googleapis.com/v1beta/openai/` with `Authorization: Bearer $GEMINI_API_KEY`.
  - Supports chat completions, embeddings, images, video, batch and models. **No Responses API is mentioned.**
  - `reasoning_effort` maps to `thinking_level` (or `thinking_budget` 1,024 / 8,192 / 24,576 on 2.5). Reasoning cannot be turned off for 2.5 Pro or Gemini 3.
  - `extra_body` takes `thinking_config` and `cached_content`. Thought signatures are supported in chat completions for Gemini 3.
  - Unknown parameters "will be silently ignored". Status is still "beta".
  — [Gemini OpenAI compatibility](https://ai.google.dev/gemini-api/docs/openai)
- Gemini 2.5 models are "not deprecated" but access is limited to past users. `gemini-3-pro-preview` and `gemini-3.1-flash-lite-preview` are shut down. — [Gemini models list](https://ai.google.dev/gemini-api/docs/models)

**xAI**
- Base URL is `https://api.x.ai/v1` with a Bearer key. The Responses API (`/v1/responses`) is the main documented path.
- On grok-4.7, encrypted reasoning (`encrypted_content`) is "Always returned" on Responses. Passing it back keeps reasoning across turns. Chat Completions "has no field for the ciphertext".
- Reasoning cannot be disabled on grok-4.7, 4.6 or 4.5. Effort is low, medium, high (default) or xhigh. For grok-4.20-multi-agent, effort sets the number of agents (4 or 16).
- `presencePenalty`, `frequencyPenalty` and `stop` return an error on reasoning models.
— [xAI reasoning guide](https://docs.x.ai/docs/guides/reasoning)
- Exceeding any limit returns 429. Cached, reasoning and image prompt tokens count toward TPM. The page names no rate-limit headers. — [xAI consumption and rate limits](https://docs.x.ai/docs/key-information/consumption-and-rate-limits)
- Anthropic-SDK compatibility: xAI's integrations page claims compatibility with both the OpenAI and Anthropic SDKs, but a mirror marks the Anthropic SDK as deprecated. **UNVERIFIED** — [xAI integrations](https://docs.x.ai/api/integrations) (via search snippet)

**DeepSeek**
- Model IDs:
  - `deepseek-flash` maps to DeepSeek-V4.1-Flash-1017.
  - `deepseek-v4-pro` maps to DeepSeek-V4-Pro-0813.
  - Both have a 1M context and 384K max output.
  - Legacy `deepseek-v4-flash` and `deepseek-v4-flash-vision-exp` route to V4.1-Flash at Flash prices.
- Feature matrix:
  - JSON output, tool calls, **Responses API, Anthropic API** and chat prefix completion are supported on both.
  - FIM works in non-thinking mode only.
  - Vision is Flash only.
— [DeepSeek pricing](https://api-docs.deepseek.com/quick_start/pricing/)
- Thinking mode:
  - On by default at effort `high`. Toggle with `{"thinking":{"type":"enabled|disabled"}}` (in `extra_body` for the OpenAI SDK). `reasoning_effort` takes `low`, `high` or `max`.
  - Anthropic format uses `output_config.effort`, plus `reasoning.effort:"none"` to disable.
  - Reasoning is returned in `reasoning_content`.
  - **With `tools`, `reasoning_content` must be passed back in all later requests, even turns without a tool call. Otherwise the request fails with 400.** Without tools it is ignored.
  - `temperature` and the penalty parameters have no effect in thinking mode.
  — [DeepSeek thinking mode](https://api-docs.deepseek.com/guides/thinking_mode)

**Moonshot / Kimi**
- K3 quickstart:
  - Base URL `https://api.moonshot.ai/v1` (OpenAI-compatible), model `kimi-k3`.
  - Context window "1M-token".
  - `reasoning_effort` takes "low" / "high" / "max", default "max".
  - The docs mention an Anthropic-compatible Messages API, but the base URL is not given.
  - The docs domain moved from platform.moonshot.ai to platform.kimi.ai.
  — [Kimi K3 guide](https://platform.kimi.ai/docs/guide/kimi-k3)
- Third-party guides say K3 thinking is always on, `reasoning_content` must be replayed in multi-step tool use, and K3 fixes `temperature`, `top_p`, `n` and the penalty parameters. **[3P] UNVERIFIED** — [converge.ai K3 guide](https://enter.converge.ai/page/en-US/blog/kimi-k3-api-guide)

**Alibaba Qwen (Model Studio / DashScope)**
- OpenAI-compatible base URLs are now per workspace and per region, for example `https://{WorkspaceId}.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1` (Singapore) and `...us-east-1...` (Virginia). Auth is `Authorization: Bearer $DASHSCOPE_API_KEY`. "A Model Studio API key is bound to the region in which it was created."
- The page's Beijing endpoint is inconsistent with its own migration note.
- The page also claims `tools` works only on qwen-plus and qwen-max and not together with `stream=True`. This looks like legacy text. **[STALE?]**
— [Alibaba OpenAI compatibility](https://www.alibabacloud.com/help/en/model-studio/compatibility-of-openai-with-dashscope)
- The current text models in the Singapore region are `qwen3.8-max`, `qwen3.7-plus` and `qwen3.8-flash` (page updated 2026-10-11). — [Alibaba models](https://www.alibabacloud.com/help/en/model-studio/models)

**Z.ai (Zhipu GLM)**
- `glm-5.3`: 1M context, 128K max output.
- Base URLs:
  - General: `https://api.z.ai/api/paas/v4/`
  - Coding plan: `https://api.z.ai/api/coding/paas/v4`
  - OpenAI Responses protocol: `https://api.z.ai/api/v1`
  - Anthropic protocol: `https://api.z.ai/api/anthropic`
- "Disabling reasoning is no longer supported". Apps that send `thinking.type:"disabled"` must switch to `enabled` with `reasoning_effort:"low"`, or requests fail. `reasoning_effort` is low, high or max (default max).
— [Z.ai GLM-5.3](https://docs.z.ai/guides/llm/glm-5.3)

**MiniMax**
- `MiniMax-M3` and `MiniMax-M3.1-Flash` both have 1M context.
- Base URLs: Anthropic-compatible `https://api.minimax.io/anthropic` (recommended) and OpenAI-compatible `https://api.minimax.io/v1`.
- Thinking cannot be disabled; sending disabled returns 400.
- Effort is low, medium, high, xhigh or max. It is set via `output_config.effort` on the Anthropic path and `reasoning_effort` on the OpenAI path. M3.1-Flash defaults to max.
- On the OpenAI path, reasoning is returned in `reasoning_content`. On the Anthropic path it is a `thinking` block.
— [MiniMax text generation](https://platform.minimax.io/docs/guides/text-generation)

**Mistral**
- The current lineup includes Mistral Medium 3.5 (v26.04, "frontier-class multimodal… agentic and coding"), Mistral Small 4 (v26.03), Mistral Large 4 (v26.10, open weight) and Codestral (v25.08).
- **All Devstral models are deprecated.** Devstral 2 retired 2026-07-31.
— [Mistral models overview](https://docs.mistral.ai/getting-started/models/models_overview/)
- Medium 3.5: 256K context, GA on 2026-04-28. The page shows "$1.5 /M Tokens" and "$7.5 /M Tokens" without labeling which is input and which is output. The API ID shown is `mistral-medium-3-5+2` (sic), against slug `mistral-medium-3-5-26-04`. — [Mistral Medium 3.5 card](https://docs.mistral.ai/models/mistral-medium-3-5-26-04)
- Aggregators give the ID as `mistral-medium-2604` at $1.50 in / $7.50 out, with $0.15 cached. **[3P]** — [Eden AI](https://www.edenai.co/models/mistral-medium-2604)

**Fast inference providers**
- Groq:
  - Production models: `openai/gpt-oss-120b` and `openai/gpt-oss-20b`, plus Llama 3.x (contact sales).
  - Preview models: `qwen/qwen3.8-27b`, `minimaxai/minimax-m2.7` and `meta-models/muse-spark-open`. Preview models may be discontinued at short notice.
  - Model list endpoint is `https://api.groq.com/openai/v1/models`.
  — [Groq models](https://console.groq.com/docs/models)
- Cerebras:
  - Shared inference has only `gpt-oss-120b` (65k free / 131k paid context, about 3000 tok/s) and `qwen-3.8-27b` (64k/128k, about 1850 tok/s). Other families are available only on Dedicated.
  — [Cerebras models](https://inference-docs.cerebras.ai/models/overview)
  - The base URL `https://api.cerebras.ai/v1` comes from third-party docs. **[3P]** — [TanStack OpenAI-compatible adapters](https://tanstack.com/ai/latest/docs/adapters/openai-compatible.md)
- SambaNova: the pricing page lists MiniMax-M3, DeepSeek-V3.1/V3.2, gemma-4-31B-it, gpt-oss-120b and Meta-Llama-3.3-70B. It shows no API IDs or base URL. — [SambaNova pricing](https://cloud.sambanova.ai/plans/pricing)

**Hosts**
- Together: OpenAI-compatible at `https://api.together.xyz/v1`. — [Together OpenAI compatibility](https://docs.together.ai/docs/openai-api-compatibility)
- Fireworks `https://api.fireworks.ai/inference/v1` and DeepInfra `https://api.deepinfra.com/v1/openai` come from third-party adapter docs. **[3P]** — [TanStack OpenAI-compatible adapters](https://tanstack.com/ai/latest/docs/adapters/openai-compatible.md)

**AWS Bedrock**
- **Claude in Amazon Bedrock (Opus 4.7 and later)** runs on a new endpoint:
  - URL: `https://bedrock-mantle.{region}.api.aws/anthropic/v1/messages`. The request body is the same as Anthropic first-party, with standard SSE and model IDs like `anthropic.claude-opus-5-5`.
  - Auth: SigV4 (service `bedrock-mantle`), IAM assumed roles, or short-term bearer tokens sent in `x-api-key`. Bearer tokens last at most 12 hours and are minted with `aws-bedrock-token-generator`.
  - Not supported: Batches, Files, server tools (web search, fetch, code execution), MCP connector, `fallbacks`, Models API.
  - Global endpoint has no premium. Regional endpoint costs +10%.
  - Default quota is 2M input TPM.
  — [Claude in Amazon Bedrock](https://platform.claude.com/docs/en/build-with-claude/claude-in-amazon-bedrock)
- The legacy InvokeModel and Converse integration remains for Opus 4.6 and earlier. — [Claude in Amazon Bedrock](https://platform.claude.com/docs/en/build-with-claude/claude-in-amazon-bedrock)
- Converse API (`bedrock-runtime` only):
  - Request fields: `messages`, `system`, `inferenceConfig`, `toolConfig`, `additionalModelRequestFields`, `serviceTier:{type:"reserved"|"priority"|"default"|"flex"}`.
  - Cache checkpoints are `cachePoint` blocks.
  - Reasoning arrives as `reasoningContent{reasoningText{text, signature}, redactedContent}`. "You must include the signature and all previous messages in subsequent Converse requests".
  - Usage fields: `inputTokens`, `outputTokens`, `totalTokens`, `cacheReadInputTokens`, `cacheWriteInputTokens`.
  - Stream events: `messageStart`, `contentBlockStart`, `contentBlockDelta`, `contentBlockStop`, `messageStop` (carries `stopReason`), `metadata` (carries usage).
  — [Bedrock Converse](https://docs.aws.amazon.com/bedrock/latest/userguide/conversation-inference.html)
- Bedrock also exposes OpenAI-compatible Responses and Chat Completions:
  - `https://bedrock-runtime.{region}.amazonaws.com/openai/v1` (recommended; synchronous only; no server tools).
  - `https://bedrock-mantle.{region}.api.aws/v1`, which adds background inference, server-side tools and GPT-OSS Responses.
  — [Bedrock Responses API](https://docs.aws.amazon.com/bedrock/latest/userguide/inference-responses-api.html) (search summary; the Mantle GA date of 2026-03-03 is **[3P]**)
- **Claude Platform on AWS** is a separate, Anthropic-operated product. It uses the same API and Claude API model IDs, bills through AWS Marketplace in CCUs ($0.01 each) at standard rates, and does not offer fast mode. — [Anthropic pricing](https://platform.claude.com/docs/en/about-claude/pricing)

**Google Vertex AI (now "Agent Platform")**
- Claude on Google Cloud:
  - `POST https://aiplatform.googleapis.com/v1/projects/{p}/locations/global/publishers/anthropic/models/{model}:rawPredict`.
  - The model goes in the URL, not the body. The body carries `"anthropic_version":"vertex-2023-10-16"`.
  - Auth is `Authorization: Bearer $(gcloud auth print-access-token)` (OAuth2 / ADC).
  - Multi-region hosts are `aiplatform.us.rep.googleapis.com` and `aiplatform.eu.rep.googleapis.com`. Regional endpoints only serve Sonnet 4.6 and earlier.
  - Regional and multi-region carry a +10% premium.
  - 30 MB payload limit.
  - Not supported: Batches, code execution, web fetch, Files.
  — [Claude on Google Cloud](https://platform.claude.com/docs/en/build-with-claude/claude-on-vertex-ai)
- The streaming method name `:streamRawPredict` is not in the fetched text. **UNVERIFIED** (it is the long-standing name).
- Gemini on Vertex through an OpenAI-compatible endpoint (`.../endpoints/openapi/chat/completions`, OAuth access token). Express-mode API keys exist for Vertex, but no official confirmation was found that they work on the OpenAI-compatible route. **[3P]** — [Vertex API keys](https://docs.cloud.google.com/vertex-ai/generative-ai/docs/start/api-keys); [api7 Vertex upstream](https://docs.api7.ai/ai-gateway/build-traffic-path/provider-upstreams/google-vertex-ai)

**Azure OpenAI / Microsoft Foundry**
- v1 API:
  - Base URL `https://{resource}.openai.azure.com/openai/v1/` or `https://{resource}.services.ai.azure.com/openai/v1/`. "`api-version` is no longer a required parameter".
  - Auth is the `api-key` header, or `Authorization: Bearer` with an Entra token (scope `https://ai.azure.com/.default`).
  - The Responses API is supported.
  - Chat Completions also serves DeepSeek, Grok and MAI models.
  - The page is dated 2026-05-13, updated 2026-06-05.
  — [Azure OpenAI v1 API](https://learn.microsoft.com/en-us/azure/ai-foundry/openai/api-version-lifecycle)
- Claude in Microsoft Foundry:
  - `https://{resource}.services.ai.azure.com/anthropic/v1/messages` with `api-key` or `x-api-key`, or an Entra Bearer token (scope `https://ai.azure.com/.default`).
  - **`model` is the deployment name.**
  - **Foundry does not return `anthropic-ratelimit-*` headers.**
  - No Batches or Models API.
  - Billing is in CCUs through Azure Marketplace. The US Data Zone deployment type costs 1.1x.
  — [Claude in Microsoft Foundry](https://platform.claude.com/docs/en/build-with-claude/claude-in-microsoft-foundry)

### Inferences
- The two OpenAI facts make the Responses API mandatory for OpenAI frontier coding agents: GPT-6 Astra and 6.1 Sol do not support tools on Chat Completions, and GPT-5.6+ caching (cache-write tokens) is documented on Responses. An OpenAI adapter built only on Chat Completions cannot drive the flagship models agentically.
- Google is moving new features to the Interactions API (Argon, new tools). A generateContent adapter will keep working for now, but Interactions is the future-proof choice for Gemini.
- "Anthropic Messages compatible" has become a widely shared second protocol. One Anthropic-shaped adapter, with base-URL, auth and model-ID plug-ins, covers Anthropic, Bedrock-Mantle, Vertex (with body and URL tweaks), Foundry, Claude Platform on AWS, MiniMax (recommended path), Z.ai, DeepSeek and probably Kimi.

### Gaps
- Release dates for GPT-6 Astra, 6.1 Sol and Luna, and GPT-5.6. The model pages give only knowledge cutoffs (Astra and Sol: 2026-04-30; Luna: 2026-05-18; 5.6 Terra: 2026-02-16).
- Pricing for OpenAI `reasoning.mode:"pro"` is not on the pricing page.
- Exact Responses API SSE event strings. The streaming guide names `response.created`, `response.output_text.delta`, `response.completed` and `error`, and the SDK class list implies `response.output_item.added`, `response.function_call_arguments.delta` and `response.failed`. The 5 MB API reference was not fully read.
- Gemini generateContent rules for `thoughtSignature` on function-call parts. The old thought-signatures page now redirects to an Interactions-centric thinking guide.
- xAI max output for grok-4.7 (aggregators say 450K–500K **[3P]**), xAI tool fees, and the status of xAI's Anthropic endpoint.
- Kimi Anthropic base URL, K3 max output, and the official rules for replaying `reasoning_content`.
- SambaNova base URL and API IDs. Mistral base URL and official per-model price table (mistral.ai/pricing did not render the table).

---

## Q2. Per-provider wire details: base URL, auth, format, tool quirks, reasoning replay, SSE, usage, rate-limit headers, errors

### Takeaway
For a metering proxy, what matters is (a) where final usage appears in each stream and (b) whether the input count includes cached tokens. Usage is reported differently by each provider:
- Anthropic `input_tokens` excludes cache reads and writes, and the counts in `message_delta` are cumulative.
- Gemini `promptTokenCount` includes cached content.
- OpenAI reports `cached_tokens` and `cache_write_tokens` as details under `input_tokens`.
- Converse reports usage only in the trailing `metadata` event.

Reasoning replay is mandatory for Anthropic (with tools), DeepSeek (with tools, or 400), Bedrock Converse, and stateless Gemini. It is strongly recommended for OpenAI and xAI.

### Cited Findings

**Summary matrix** (each cell is sourced in Q1 unless a new source is given):

| Provider | Base URL | Auth | Native format | Reasoning returned as | Must replay? | Final usage location | Cache fields |
|---|---|---|---|---|---|---|---|
| OpenAI | `https://api.openai.com/v1` (well-known; not re-fetched) | `Authorization: Bearer` | Responses (and CC) | opaque reasoning items + `encrypted_content` (stateless) | Recommended; required for all_turns stateless | `response.completed` (Responses); last chunk with `stream_options.include_usage` (CC) | `input_tokens_details.cached_tokens`, `.cache_write_tokens` |
| Anthropic | `https://api.anthropic.com/v1/messages` | `x-api-key` + `anthropic-version: 2023-06-01` ([errors page curl](https://platform.claude.com/docs/en/api/errors)) | Messages | `thinking` / `redacted_thinking` blocks + `signature` | **Yes** with tools; 400 if modified | `message_start` (input) + cumulative `message_delta` | `cache_creation_input_tokens`, `cache_read_input_tokens`, `cache_creation.ephemeral_5m/1h_input_tokens` |
| Gemini (AI Studio) | `https://generativelanguage.googleapis.com/v1beta` | `x-goog-api-key` (or `?key=`) | generateContent / Interactions | thought parts / thought blocks + signatures | **Yes** in stateless mode | last chunk `usageMetadata` / interaction usage | `cachedContentTokenCount` / `total_cached_tokens` |
| Gemini OpenAI-compat | `.../v1beta/openai/` | Bearer | CC (beta) | thought signatures via compat | Gemini 3: yes | CC usage | UNVERIFIED mapping |
| xAI | `https://api.x.ai/v1` | Bearer | Responses / CC | `encrypted_content` (Responses), summaries | Recommended (4.7) | Responses usage | cached-token field UNVERIFIED |
| DeepSeek | UNVERIFIED (`https://api.deepseek.com` historically) | Bearer | CC + Anthropic + Responses | `reasoning_content` | **Yes with tools (400)** | CC usage | field names UNVERIFIED in 2026 docs |
| Kimi | `https://api.moonshot.ai/v1` | Bearer | CC (+Anthropic) | `reasoning_content` | [3P] yes for tools | CC usage | UNVERIFIED |
| Qwen | `https://{ws}.{region}.maas.aliyuncs.com/compatible-mode/v1` | Bearer | CC | `reasoning_content` (UNVERIFIED) | UNVERIFIED | needs `stream_options.include_usage` | UNVERIFIED |
| Z.ai | `https://api.z.ai/api/paas/v4/` (+ `/api/anthropic`, `/api/v1`) | Bearer | CC / Anthropic / Responses | `reasoning_content` | UNVERIFIED | CC usage | UNVERIFIED |
| MiniMax | `https://api.minimax.io/anthropic`, `/v1` | (UNVERIFIED; key) | Anthropic (recommended) / CC | `thinking` block / `reasoning_content` | UNVERIFIED | — | — |
| Groq | `https://api.groq.com/openai/v1` | Bearer | CC | — (gpt-oss) | — | CC usage | `prompt_tokens_details.cached_tokens` ([Groq caching](https://console.groq.com/docs/prompt-caching)) |
| Bedrock Converse | `bedrock-runtime.{region}.amazonaws.com` `/model/{id}/converse(-stream)` | SigV4 / bearer | Converse; AWS event-stream (binary, not SSE — long-standing; not re-verified) | `reasoningContent` + `signature` | **Yes** | `metadata` event | `cacheReadInputTokens`, `cacheWriteInputTokens` |
| Bedrock Mantle (Claude) | `bedrock-mantle.{region}.api.aws/anthropic/v1/messages` | SigV4 `bedrock-mantle` or `x-api-key` bearer | Anthropic Messages, SSE | as Anthropic | Yes | as Anthropic | as Anthropic |
| Vertex Claude | `aiplatform.googleapis.com/v1/projects/{p}/locations/global/publishers/anthropic/models/{m}:rawPredict` | Google OAuth Bearer | Anthropic body (`anthropic_version` in body) | as Anthropic | Yes | as Anthropic | as Anthropic |
| Azure OpenAI v1 | `https://{res}.openai.azure.com/openai/v1/` | `api-key` or Entra Bearer | OpenAI Responses / CC | as OpenAI | as OpenAI | as OpenAI | as OpenAI |
| Foundry Claude | `https://{res}.services.ai.azure.com/anthropic/v1/messages` | `api-key` / `x-api-key` / Entra Bearer | Anthropic | as Anthropic | Yes | as Anthropic | as Anthropic (no rate-limit headers) |

- The Azure changelog confirms CC fields that also exist on OpenAI: `max_completion_tokens`, `completion_tokens_details` with `reasoning_tokens`, and `stream_options` with `include_usage`. — [Azure OpenAI v1 API](https://learn.microsoft.com/en-us/azure/ai-foundry/openai/api-version-lifecycle)
- Gemini's OpenAI compatibility layer drops unknown fields silently. A proxy cannot rely on a 400 to detect an unsupported parameter. — [Gemini OpenAI compatibility](https://ai.google.dev/gemini-api/docs/openai)

**Rate-limit headers**
- OpenAI:
  - `x-ratelimit-limit-requests`, `x-ratelimit-limit-tokens`
  - `x-ratelimit-remaining-requests`, `x-ratelimit-remaining-tokens`
  - `x-ratelimit-reset-requests`, `x-ratelimit-reset-tokens` (durations such as `6m0s`)
  - Project-level variants: `x-ratelimit-limit-project-tokens`, `x-ratelimit-remaining-project-tokens`, `x-ratelimit-reset-project-tokens`
  - `Retry-After`, sent on temporary 429s and on 503s
  — [OpenAI rate limits](https://developers.openai.com/api/docs/guides/rate-limits)
- Anthropic:
  - `retry-after`
  - `anthropic-ratelimit-{requests,tokens,input-tokens,output-tokens}-{limit,remaining,reset}`. Reset is RFC 3339. Remaining tokens are rounded to the nearest thousand.
  - `anthropic-priority-{input,output}-tokens-*`
  - `anthropic-fast-*` for fast mode
  - `anthropic-workspace-id`
  - Only uncached input counts toward ITPM; cache reads do not, except on Haiku 3.5.
  - Monthly spend caps: Start $500, Build $1,000, Scale $200,000.
  — [Anthropic rate limits](https://platform.claude.com/docs/en/api/rate-limits)
- Foundry Claude returns none of the `anthropic-ratelimit-*` headers. — [Claude in Microsoft Foundry](https://platform.claude.com/docs/en/build-with-claude/claude-in-microsoft-foundry)
- xAI documents only the 429 itself and no headers. — [xAI consumption and rate limits](https://docs.x.ai/docs/key-information/consumption-and-rate-limits)

**Errors worth special handling**
- Anthropic:
  - 529 `overloaded_error`: retry.
  - Mid-stream `event: error` after HTTP 200.
  - 429 `rate_limit_error` with `error.details.error_code:"enforced_spend_limit_reached"` and **no `retry-after`**: do not retry; it fails until the month resets.
  - 400 `invalid_request_error` when a user-set spend limit is hit.
  - 413 `request_too_large`. Limits are 32 MB for Messages and 256 MB for Batch.
  - 504 `timeout_error`.
  - 402 `billing_error`.
  - 400 for modified thinking blocks, thinking-binding mismatch, forced `tool_choice`, prefill, `thinking.type.enabled` on 4.7+, and `disabled` where unsupported.
  - 200 with `stop_reason:"refusal"`.
  — [Anthropic errors](https://platform.claude.com/docs/en/api/errors); [Anthropic rate limits](https://platform.claude.com/docs/en/api/rate-limits)
- OpenAI:
  - 429 `slow_down` when traffic ramps too fast, even within limits.
  - 503 `server_is_overloaded`.
  - "Don't retry quota, billing, or other errors that require you to take action."
  — [OpenAI rate limits](https://developers.openai.com/api/docs/guides/rate-limits)
  - `status:"incomplete"` with reason `max_output_tokens`. — [OpenAI reasoning guide](https://developers.openai.com/api/docs/guides/reasoning)
- DeepSeek: 400 when `reasoning_content` is not replayed with tools. — [DeepSeek thinking mode](https://api-docs.deepseek.com/guides/thinking_mode)
- MiniMax: 400 when thinking is disabled. — [MiniMax text generation](https://platform.minimax.io/docs/guides/text-generation)
- Z.ai: requests with thinking disabled fail. — [Z.ai GLM-5.3](https://docs.z.ai/guides/llm/glm-5.3)
- xAI: penalty and `stop` parameters error on reasoning models. — [xAI reasoning guide](https://docs.x.ai/docs/guides/reasoning)
- Converse: changing earlier messages after a reasoning block throws an error. — [Bedrock Converse](https://docs.aws.amazon.com/bedrock/latest/userguide/conversation-inference.html)

### Inferences
- The meter should normalize every provider to one record: `{uncached_in, cache_read, cache_write_5m, cache_write_1h|30m, out_visible, out_reasoning, server_tool_calls{kind:n}, tier, geo, ctx_tier, model_served}`. Each adapter needs its own mapping:
  - Anthropic: total input is the sum of three fields.
  - OpenAI and Gemini: cached tokens are a subset of the input total. OpenAI's cached-subset semantics are long-standing; whether `cache_write_tokens` is also a subset of `input_tokens` is **UNVERIFIED**.
- Because `message_delta` usage is cumulative, the meter must take the last value and not sum deltas.
- With Anthropic server-side fallback, `model_served` can differ from the requested model. Price from `usage.iterations`.
- A "retry on 429" policy must exclude Anthropic spend-cap 429s (marked by `error_code`) and OpenAI quota and billing errors. Otherwise the proxy loops.

### Gaps
- Gemini error codes and rate-limit headers were not fetched.
- 2026 field names for DeepSeek and Kimi usage were not confirmed. DeepSeek historically used `prompt_cache_hit_tokens` and `prompt_cache_miss_tokens` (from 2025 docs, not re-verified).
- Rate-limit headers for xAI, Groq, Cerebras, Together, Fireworks, DeepInfra, Z.ai, MiniMax and Qwen were not collected.
- Converse binary event-stream framing was not re-verified in 2026 docs.

---

## Q3. Models current from about October 2025 to October 2026, with context, output and prices

### Takeaway
Frontier list prices fell or held:
- Claude Opus 5.5 is $4 / $20, down from $5 / $25 for Opus 4.5–5.
- Sonnet 5 and 5.5 are $2 / $10.
- GPT-6.1 Sol is $2 / $10.

Cheap coding-capable tiers are crowded:
- DeepSeek V4.1-Flash: $0.15–0.30 in, $0.60–1.20 out
- GLM-5.3-Flash: $0.15 / $0.50
- MiniMax-M3.1-Flash: $0.15 / $0.60
- Qwen3.8-Flash: $0.15 / $0.47
- GPT-6 Luna: $0.10 / $0.50
- Claude Haiku 5.5: $0.10 / $0.50 under 100K tokens

Several prices are **scheduled to rise**:
- Gemini 4 Argon doubles on 2026-12-01.
- Gemini 3.8 and 3.6 Flash double on 2027-01-01.
- GPT-5.6 Sol is on promotional pricing until at least 2026-11-21.

### Cited Findings

#### Machine-readable model table

Columns: in, cached_in and out are standard-tier USD/MTok. cache_write is shown as "5m/1h" for Anthropic, or as the single write rate where one is listed. "—" means no charge or none listed. "?" means not found.

| provider | model_id | context | max_out | in | cached_in | cache_write | out | notes | verified_date | source_url |
|---|---|---|---|---|---|---|---|---|---|---|
| OpenAI | gpt-6-astra | 1.05M (922K max in) | 128K | 10.00 | 1.00 | 12.50 | 50.00 | >272K in: 20/2/25/75 whole request; Batch/Flex 50%; Fast 2x; Ultrafast 60/6/75/300; effort low–max (no none) | 2026-10-20 | https://developers.openai.com/api/docs/models/gpt-6-astra |
| OpenAI | gpt-6.1-sol | 1.05M (922K) | 128K | 2.00 | 0.10 | 2.50 | 10.00 | cache read 0.05x; >272K 4/0.20/5/15; no tools on Chat Completions; Ultrafast 12/0.60/15/60; effort low–max | 2026-10-20 | https://developers.openai.com/api/docs/models/gpt-6.1-sol |
| OpenAI | gpt-6-sol | ? | ? | 2.00 | 0.20 | 2.50 | 10.00 | >272K 4/0.40/5/15 | 2026-10-20 | https://developers.openai.com/api/docs/pricing |
| OpenAI | gpt-6-luna | 1.05M (922K) | 128K | 0.10 | 0.01 | 0.125 | 0.50 | >272K 0.20/0.02/0.25/0.75; CC tools only at effort none; not recommended for coding by OpenAI | 2026-10-20 | https://developers.openai.com/api/docs/models/gpt-6-luna |
| OpenAI | gpt-5.6-sol | ? | ? | 4.00 | 0.40 | 5.00 | 20.00 | promo pricing "at least through November 21, 2026"; >272K 8/0.80/10/30 | 2026-10-20 | https://developers.openai.com/api/docs/pricing |
| OpenAI | gpt-5.6-terra | 1.05M (922K) | 128K | 2.00 | 0.20 | 2.50 | 12.00 | >272K 4/0.40/5/18; cutoff 2026-02-16; tools on CC OK | 2026-10-20 | https://developers.openai.com/api/docs/models/gpt-5.6-terra |
| OpenAI | gpt-5.6-luna | ? | ? | 0.20 | 0.02 | 0.25 | 1.20 | >272K 0.40/0.04/0.50/1.80 | 2026-10-20 | https://developers.openai.com/api/docs/pricing |
| OpenAI | gpt-5.6-cyber | ? | ? | 12.50 | 1.25 | 15.625 | 75.00 | trusted access | 2026-10-20 | https://developers.openai.com/api/docs/pricing |
| OpenAI | gpt-5.5 | ? | ? | 5.00 | 0.50 | — | 30.00 | >272K 10/1/45; cache retention 24h only; Fast 12.50/1.25/75 | 2026-10-20 | https://developers.openai.com/api/docs/pricing |
| OpenAI | gpt-5.5-pro | ? | ? | 30.00 | — | — | 180.00 | >272K 60/270 | 2026-10-20 | https://developers.openai.com/api/docs/pricing |
| OpenAI | gpt-5.4 | ? | ? | 2.50 | 0.25 | — | 15.00 | >272K 5/0.50/22.50 | 2026-10-20 | https://developers.openai.com/api/docs/pricing |
| OpenAI | gpt-5.4-mini | ? | ? | 0.75 | 0.075 | — | 4.50 | | 2026-10-20 | https://developers.openai.com/api/docs/pricing |
| OpenAI | gpt-5.4-nano | ? | ? | 0.20 | 0.02 | — | 1.25 | | 2026-10-20 | https://developers.openai.com/api/docs/pricing |
| OpenAI | gpt-5.3-codex | ? | ? | 1.75 | 0.175 | — | 14.00 | Codex Fast 3.50/0.35/28 | 2026-10-20 | https://developers.openai.com/api/docs/pricing |
| OpenAI | gpt-5.2 | ? | ? | 1.75 | 0.175 | — | 14.00 | [STALE?] older gen | 2026-10-20 | https://developers.openai.com/api/docs/pricing |
| OpenAI | gpt-5.1 | ? | ? | 1.25 | 0.125 | — | 10.00 | [STALE?] Nov-2025 gen | 2026-10-20 | https://developers.openai.com/api/docs/pricing |
| OpenAI | gpt-5-mini | ? | ? | 0.25 | 0.025 | — | 2.00 | [STALE?] | 2026-10-20 | https://developers.openai.com/api/docs/pricing |
| Anthropic | claude-fable-5-1 | 1M | 128K | 10.00 | 0.25 | 12.50 / 20.00 | 50.00 | cache read 0.025x; thinking always on (default effort high); no forced tool_choice; batch 5/25 | 2026-10-20 | https://platform.claude.com/docs/en/about-claude/pricing |
| Anthropic | claude-mythos-5-1 | ? | ? | 10.00 | 0.25 | 12.50 / 20.00 | 50.00 | limited availability (verification program) | 2026-10-20 | https://platform.claude.com/docs/en/about-claude/pricing |
| Anthropic | claude-opus-5-5 | 1M | 128K | 4.00 | 0.20 | 5.00 / 8.00 | 20.00 | cache read 0.05x; Fast mode 8/40; batch 2/10; default effort medium; no forced tool_choice | 2026-10-20 | https://platform.claude.com/docs/en/about-claude/pricing |
| Anthropic | claude-sonnet-5-5 | 1M | 128K | 2.00 | 0.10 | 2.50 / 4.00 | 10.00 | cache read 0.05x; batch 1/5; lowest thinking = between_tools | 2026-10-20 | https://platform.claude.com/docs/en/about-claude/pricing |
| Anthropic | claude-haiku-5-5 | 1M | 128K | 0.10 (≤100K prompt) / 0.50 (>100K) | 0.01 / 0.05 | 0.125/0.20 ; 0.625/1.00 | 0.50 / 2.50 | whole request repriced if prompt >100K incl. cache reads | 2026-10-20 | https://platform.claude.com/docs/en/about-claude/pricing |
| Anthropic | claude-fable-5 | 1M | ? | 10.00 | 1.00 | 12.50 / 20.00 | 50.00 | legacy | 2026-10-20 | https://platform.claude.com/docs/en/about-claude/pricing |
| Anthropic | claude-opus-5 | 1M | ? | 5.00 | 0.50 | 6.25 / 10.00 | 25.00 | Fast mode 10/50 (research preview) | 2026-10-20 | https://platform.claude.com/docs/en/about-claude/pricing |
| Anthropic | claude-sonnet-5 | 1M | ? | 2.00 | 0.20 | 2.50 / 4.00 | 10.00 | intro price made permanent; $3/$15 increase cancelled | 2026-10-20 | https://platform.claude.com/docs/en/about-claude/pricing |
| Anthropic | claude-opus-4-8 | 1M | ? | 5.00 | 0.50 | 6.25 / 10.00 | 25.00 | Fast mode 10/50 | 2026-10-20 | https://platform.claude.com/docs/en/about-claude/pricing |
| Anthropic | claude-opus-4-7 | 1M | ? | 5.00 | 0.50 | 6.25 / 10.00 | 25.00 | new tokenizer (~+30% tokens) | 2026-10-20 | https://platform.claude.com/docs/en/about-claude/pricing |
| Anthropic | claude-opus-4-6 | 1M | ? | 5.00 | 0.50 | 6.25 / 10.00 | 25.00 | [STALE?] | 2026-10-20 | https://platform.claude.com/docs/en/about-claude/pricing |
| Anthropic | claude-sonnet-4-6 | 1M | ? | 3.00 | 0.30 | 3.75 / 6.00 | 15.00 | [STALE?] | 2026-10-20 | https://platform.claude.com/docs/en/about-claude/pricing |
| Anthropic | claude-opus-4-5 | ? | ? | 5.00 | 0.50 | 6.25 / 10.00 | 25.00 | [STALE?] Nov-2025; min cache 4096 | 2026-10-20 | https://platform.claude.com/docs/en/about-claude/pricing |
| Anthropic | claude-haiku-4-5 | 200K | ? | 1.00 | 0.10 | 1.25 / 2.00 | 5.00 | [STALE?] | 2026-10-20 | https://platform.claude.com/docs/en/about-claude/pricing |
| Google | gemini-4-argon | 1,048,576 (unified) | 65,536 (1,048,576 long decode) | 2.00 → 4.00 | 0.10 → 0.20 (implicit only) | — (no explicit cache) | 10.00 → 20.00 | "→" = from 2026-12-01; Batch/Flex 1/5 → 2/10; Priority 3.60/18 → 7.20/36 | 2026-10-20 | https://ai.google.dev/gemini-api/docs/pricing |
| Google | gemini-3.8-flash | ? | ? | 0.75 → 1.50 | 0.075 → 0.15 | storage 0.50 → 1.00 /MTok/hr | 3.75 → 7.50 | "→" = from 2027-01-01; Batch/Flex 50% | 2026-10-20 | https://ai.google.dev/gemini-api/docs/pricing |
| Google | gemini-3.6-flash | ? | ? | 0.75 → 1.50 | 0.075 → 0.15 | storage 0.50 → 1.00 | 3.75 → 7.50 | same as 3.8 Flash | 2026-10-20 | https://ai.google.dev/gemini-api/docs/pricing |
| Google | gemini-3.5-flash-lite | ? | ? | 0.30 | 0.03 | storage 1.00 | 2.50 | all modalities same input price | 2026-10-20 | https://ai.google.dev/gemini-api/docs/pricing |
| Google | gemini-3.1-flash-lite | ? | ? | 0.25 (audio 0.50) | 0.025 (audio 0.05) | storage 1.00 | 1.50 | | 2026-10-20 | https://ai.google.dev/gemini-api/docs/pricing |
| Google | gemini-3.1-pro-preview | ? | ? | 2.00 (≤200K) / 4.00 | 0.20 / 0.40 | storage 4.50 | 12.00 / 18.00 | also `-customtools` variant | 2026-10-20 | https://ai.google.dev/gemini-api/docs/pricing |
| Google | gemini-3-flash-preview | ? | ? | 0.50 (audio 1.00) | 0.05 | storage 1.00 | 3.00 | | 2026-10-20 | https://ai.google.dev/gemini-api/docs/pricing |
| Google | gemini-2.5-pro | ? | ? | 1.25 / 2.50 (>200K) | 0.125 / 0.25 | storage 4.50 | 10.00 / 15.00 | [STALE?] access limited to past users | 2026-10-20 | https://ai.google.dev/gemini-api/docs/pricing |
| Google | gemini-2.5-flash | ? | ? | 0.30 (audio 1.00) | 0.03 | storage 1.00 | 2.50 | [STALE?] limited access | 2026-10-20 | https://ai.google.dev/gemini-api/docs/pricing |
| xAI | grok-4.7 | 500K | ? ([3P] 450K–500K) | 2.00 | 0.50 | — | 6.00 | ≥200K prompt: 4/1/12 on all tokens; reasoning always on | 2026-10-20 | https://docs.x.ai/docs/models |
| xAI | grok-4.6 | 500K | ? | 2.00 | 0.50 | — | 6.00 | ≥200K 4/1/12 | 2026-10-20 | https://docs.x.ai/docs/models |
| xAI | grok-4.5 | 500K | ? | 2.00 | 0.30 | — | 6.00 | ≥200K 4/0.60/12 | 2026-10-20 | https://docs.x.ai/docs/models |
| xAI | grok-4.3 | 1M | ? | 1.25 | 0.20 | — | 2.50 | ≥200K 2.50/0.40/5 | 2026-10-20 | https://docs.x.ai/docs/models |
| xAI | grok-4.20-0309-reasoning | 1M | ? | 1.25 | 0.20 | — | 2.50 | also -non-reasoning, -multi-agent-0309 same price | 2026-10-20 | https://docs.x.ai/docs/models |
| xAI | grok-build-0.1 | 256K | ? | 1.00 | 0.20 | — | 2.00 | ≥200K 2/0.40/4 | 2026-10-20 | https://docs.x.ai/docs/models |
| DeepSeek | deepseek-flash (V4.1-Flash-1017) | 1M | 384K | 0.30 peak / 0.15 off-peak | 0.006 / 0.003 | — | 1.20 / 0.60 | off-peak = all hours except 01–04 & 06–10 UTC Mon–Fri; thinking default on | 2026-10-20 | https://api-docs.deepseek.com/quick_start/pricing/ |
| DeepSeek | deepseek-v4-pro (V4-Pro-0813) | 1M | 384K | 1.32 / 0.66 | 0.044 / 0.022 | — | 3.96 / 1.98 | no vision | 2026-10-20 | https://api-docs.deepseek.com/quick_start/pricing/ |
| Moonshot | kimi-k3 | 1,048,576 | ? | 3.00 | 0.30 | 3.00 (5m) / 6.00 (1h) | 15.00 | currency shown as "$" (USD not stated); effort low/high/max, default max | 2026-10-20 | https://platform.kimi.ai/docs/pricing/chat |
| Moonshot | kimi-k2.7-code | 262,144 | ? | 0.95 | 0.19 | ? | 4.00 | | 2026-10-20 | https://platform.kimi.ai/docs/pricing/chat |
| Moonshot | kimi-k2.7-code-highspeed | 262,144 | ? | 1.90 | 0.38 | ? | 8.00 | | 2026-10-20 | https://platform.kimi.ai/docs/pricing/chat |
| Moonshot | kimi-k2.6 | 262,144 | ? | 0.95 | 0.16 | ? | 4.00 | | 2026-10-20 | https://platform.kimi.ai/docs/pricing/chat |
| Z.ai | glm-5.3 | 1M | 128K | 1.40 | 0.26 | storage "limited-time free" | 4.40 | thinking cannot be disabled | 2026-10-20 | https://docs.z.ai/guides/overview/pricing |
| Z.ai | GLM-5.3-FlashX (id case UNVERIFIED) | ? | ? | 0.37 | 0.075 | storage free (limited time) | 1.25 | | 2026-10-20 | https://docs.z.ai/guides/overview/pricing |
| Z.ai | GLM-5.3-Flash (id case UNVERIFIED) | ? | ? | 0.15 | 0.03 | storage free (limited time) | 0.50 | | 2026-10-20 | https://docs.z.ai/guides/overview/pricing |
| Z.ai | GLM-5.2 / GLM-5.1 | ? | ? | 1.40 | 0.26 | — | 4.40 | | 2026-10-20 | https://docs.z.ai/guides/overview/pricing |
| Z.ai | GLM-5 | ? | ? | 1.00 | 0.20 | — | 3.20 | | 2026-10-20 | https://docs.z.ai/guides/overview/pricing |
| Z.ai | GLM-4.7 | ? | ? | 0.60 | 0.11 | — | 2.20 | [STALE?] | 2026-10-20 | https://docs.z.ai/guides/overview/pricing |
| MiniMax | MiniMax-M3 | 1M | ? | 0.30 (≤512K) / 0.60 (>512K) | 0.06 / 0.12 | not listed | 1.20 / 2.40 | Priority tier 1.5x (`service_tier:"priority"`) | 2026-10-20 | https://platform.minimax.io/docs/guides/pricing |
| MiniMax | MiniMax-M3.1-Flash | 1M | ? | 0.15 | 0.015 | not listed | 0.60 | default effort max | 2026-10-20 | https://platform.minimax.io/docs/guides/pricing |
| Alibaba | qwen3.8-max | ≤1M (single tier) | ? | 2.00 | ? | ? (general: explicit write 125%, hit 10%) | 6.00 | thinking output same price; Singapore | 2026-10-20 | https://www.alibabacloud.com/help/en/model-studio/model-pricing |
| Alibaba | qwen3.7-plus | 1M | ? | 0.40 (≤256K) / 1.20 (256K–1M) | ? | ? | 1.60 / 4.80 | shown with "Limited-time 20% off" | 2026-10-20 | https://www.alibabacloud.com/help/en/model-studio/model-pricing |
| Alibaba | qwen3.8-flash | ≤1M | ? | 0.15 | ? | ? | 0.47 | | 2026-10-20 | https://www.alibabacloud.com/help/en/model-studio/model-pricing |
| Alibaba | qwen3-coder-flash | 1M | ? | 0.30 / 0.50 / 0.80 / 1.60 (tiers 32K/128K/256K/1M) | ? | ? | 1.50 / 2.50 / 4.00 / 9.60 | [STALE?] snapshot 2025-07-28 | 2026-10-20 | https://www.alibabacloud.com/help/en/model-studio/model-pricing |
| Mistral | mistral-medium-3.5 (id: page shows `mistral-medium-3-5+2`; [3P] `mistral-medium-2604`) | 256K | ? | 1.50 (label UNVERIFIED) | [3P] 0.15 | ? | 7.50 | GA 2026-04-28 | 2026-10-20 | https://docs.mistral.ai/models/mistral-medium-3-5-26-04 |
| Mistral | Mistral Large (version/id UNVERIFIED; likely Large 4 v26.10) | ? | ? | 0.50 | "up to 90%" off | ? | 1.50 | batch 50% | 2026-10-20 | https://mistral.ai/pricing |
| Groq | openai/gpt-oss-120b | 131,072 | 65,536 | 0.15 | 0.075 (50%) | — | 0.60 | ~500 tok/s | 2026-10-20 | https://console.groq.com/docs/models |
| Groq | openai/gpt-oss-20b | 131,072 | 65,536 | 0.075 | 0.0375 | — | 0.30 | ~1000 tok/s | 2026-10-20 | https://console.groq.com/docs/models |
| Groq | qwen/qwen3.8-27b (preview) | 131,072 | 16,384 | 0.80 | ? | — | 4.00 | preview; may vanish | 2026-10-20 | https://console.groq.com/docs/models |
| Cerebras | gpt-oss-120b | 65K free / 131K paid | ? | 0.35 [3P] | no cache discount [3P] | — | 0.75 [3P] | ~3000 tok/s | 2026-10-20 | https://www.morphllm.com/cerebras-pricing |
| Cerebras | qwen-3.8-27b | 64K / 128K | ? | 0.99 [3P] | no cache discount [3P] | — | 1.49 [3P] | ~1850 tok/s | 2026-10-20 | https://www.morphllm.com/cerebras-pricing |
| SambaNova | MiniMax-M3 (display name) | ? | ? | 0.60 | 0.06 | — | 2.40 | | 2026-10-20 | https://cloud.sambanova.ai/plans/pricing |
| SambaNova | gpt-oss-120b | ? | ? | 0.22 | — | — | 0.59 | | 2026-10-20 | https://cloud.sambanova.ai/plans/pricing |
| Together | Kimi K3 (display name) | ? | ? | 3.00 | 0.30 | — | 15.00 | | 2026-10-20 | https://www.together.ai/pricing |
| Together | Qwen3.8-2.4T-A95B | ? | ? | 2.00 | 0.25 | — | 6.00 | | 2026-10-20 | https://www.together.ai/pricing |
| Together | DeepSeek V4 Pro 0813 | ? | ? | 1.32 | 0.13 | — | 3.96 | = DeepSeek peak price | 2026-10-20 | https://www.together.ai/pricing |
| Together | DeepSeek V4.1 Flash 1017 | ? | ? | 0.30 | 0.006 | — | 1.20 | | 2026-10-20 | https://www.together.ai/pricing |
| Together | GLM-5.3 | ? | ? | 1.40 | 0.26 | — | 4.40 | | 2026-10-20 | https://www.together.ai/pricing |
| Together | MiniMax M3 | ? | ? | 0.30 | 0.06 | — | 1.20 | | 2026-10-20 | https://www.together.ai/pricing |
| Fireworks | Kimi K3 | ? | ? | 3.00 | 0.30 | — | 15.00 | Priority 1.25x; "Fast" and US variants 1.5x; batch 50% | 2026-10-20 | https://docs.fireworks.ai/serverless/pricing |
| Fireworks | DeepSeek V4.1 Flash | ? | ? | 0.30 | 0.006 | — | 1.20 | | 2026-10-20 | https://docs.fireworks.ai/serverless/pricing |
| Fireworks | GLM 5.3 | ? | ? | 1.40 | 0.26 | — | 4.40 | | 2026-10-20 | https://docs.fireworks.ai/serverless/pricing |
| Fireworks | Qwen 3.8 Max | ? | ? | 2.00 | 0.25 | — | 6.00 | | 2026-10-20 | https://docs.fireworks.ai/serverless/pricing |
| Fireworks | OpenAI GPT OSS 120B | ? | ? | 0.15 | 0.015 | — | 0.60 | | 2026-10-20 | https://docs.fireworks.ai/serverless/pricing |
| DeepInfra | DeepSeek-V4-Pro | ? | ? | 1.30 | 0.10 | — | 2.60 | cheaper output than first-party peak | 2026-10-20 | https://deepinfra.com/pricing |
| DeepInfra | DeepSeek-V4-Flash | ? | ? | 0.09 | 0.018 | — | 0.18 | V4 (not V4.1) Flash | 2026-10-20 | https://deepinfra.com/pricing |
| DeepInfra | Kimi-K3 | ? | ? | 2.85 | 0.285 | — | 14.25 | | 2026-10-20 | https://deepinfra.com/pricing |
| DeepInfra | Qwen3.8-Max | ? | ? | 1.65 | 0.206 | — | 4.951 | | 2026-10-20 | https://deepinfra.com/pricing |

- Anthropic's long-context rule: 4.6+ models (except Haiku 5.5) bill the full 1M context "at standard pricing". "A 900k-token request is billed at the same per-token rate as a 9k-token request." — [Anthropic pricing](https://platform.claude.com/docs/en/about-claude/pricing)
- DeepInfra resells Claude at `claude-sonnet-5` $3 / $15. Anthropic lists Sonnet 5 at $2 / $10. DeepInfra also lists `gemini-3.5-flash` at $1.50 / $9.00, a model that does not appear on Google's price list. Treat reseller catalogs as their own price lists and do not assume first-party parity. — [DeepInfra pricing](https://deepinfra.com/pricing); [Anthropic pricing](https://platform.claude.com/docs/en/about-claude/pricing); [Gemini pricing](https://ai.google.dev/gemini-api/docs/pricing)

### Inferences
- Coding-relevant frontier picks:
  - Claude Opus 5.5 and Fable 5.1
  - GPT-6 Astra and 6.1 Sol (through Responses)
  - Gemini 4 Argon
  - Grok 4.7 / grok-build-0.1
  - Kimi K3, kimi-k2.7-code
  - GLM-5.3
  - DeepSeek V4-Pro
  - Qwen3.8-Max
  - Mistral Medium 3.5
- Cost-efficient coders: DeepSeek V4.1-Flash, MiniMax-M3 and M3.1-Flash, GLM-5.3-Flash, Qwen3.8-Flash, GPT-5.6 Terra and Luna, Haiku 5.5. Coding quality was not verified in this research.
- A price row must be keyed by `(provider, model_id, tier, geo, ctx_bucket, effective_from)`, not just `model_id`. Google's dated price changes and OpenAI's promotional pricing mean a static table goes stale within weeks.

### Gaps
- Context and max output for gpt-6-sol, gpt-5.6-sol and gpt-5.6-luna, Gemini 3.8 and 3.6 Flash, xAI, Kimi K3 output, and Qwen.
- Release dates for most models.
- Official Cerebras rates (the pricing page renders its tables as images).
- Mistral official price table and IDs.
- Qwen cache rates.
- Bedrock and Vertex per-model rate cards for non-Anthropic models.

---

## Q4. Pricing traps for a cost meter

### Takeaway
No two providers bill alike. The meter must model:
- per-request context tiers that reprice the **whole** request (OpenAI >272K, Gemini >200K, xAI ≥200K, Haiku 5.5 >100K, MiniMax >512K, Qwen per-tier)
- cache-write premiums: Anthropic 1.25x and 2x; OpenAI GPT-5.6+ 1.25x; Kimi K3 1x and 2x
- three different cache-read multipliers
- time-of-day pricing (DeepSeek)
- service tiers: batch, flex, priority/fast, ultrafast
- geo uplifts of 1.1x
- per-call tool fees
- reasoning billed as output, even when hidden or summarized
- dated future price changes

### Cited Findings

**Context tiers (whole request repriced)**
- OpenAI: prompts above 272K input tokens pay 2x input and cache rates and 1.5x output "for the full request". This applies to GPT-5.4, 5.5, 5.6 and 6.x. — [gpt-6.1-sol](https://developers.openai.com/api/docs/models/gpt-6.1-sol); [OpenAI pricing](https://developers.openai.com/api/docs/pricing)
- xAI: at or above 200K prompt tokens, "billed at the higher rate for all tokens in the request". — [xAI models](https://docs.x.ai/docs/models)
- Gemini 3.1 Pro and 2.5 Pro: separate rates at or below 200K and above 200K. — [Gemini pricing](https://ai.google.dev/gemini-api/docs/pricing)
- Claude Haiku 5.5: prompts above 100,000 tokens pay 5x. Prompt length counts cache reads and writes, and the higher price applies "even when part of its prompt is a cache hit". — [Anthropic pricing](https://platform.claude.com/docs/en/about-claude/pricing)
- MiniMax-M3: above 512K input, rates double. — [MiniMax pricing](https://platform.minimax.io/docs/guides/pricing)
- Qwen: "The unit price is determined by the total number of input tokens in a single request." — [Alibaba pricing](https://www.alibabacloud.com/help/en/model-studio/model-pricing)

**Caching**
- Anthropic writes are 1.25x (5 min) or 2x (1 h). Reads are 0.1x, or 0.05x on Opus 5.5 and Sonnet 5.5, or 0.025x on Fable 5.1 and Mythos 5.1. The multipliers stack with Batch and data residency. — [Anthropic pricing](https://platform.claude.com/docs/en/about-claude/pricing)
- Below the minimum (512 to 4,096 tokens depending on model), caching silently does not happen. — [Anthropic prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)
- OpenAI GPT-5.6+ writes are 1.25x and reads 0.1x (0.05x on 6.1 Sol). Prewarm writes are billed. Minimum is 1,024 tokens. — [OpenAI prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching)
- Gemini:
  - Implicit cache reads are about 0.1x input (for example $0.20 vs $2.00 on 3.1 Pro).
  - Explicit caches also charge **storage per MTok per hour**: $0.50–4.50 standard, $8.10 on Priority for 3.1 Pro.
  - Implicit minimum is 4,096 tokens on new models.
  — [Gemini pricing](https://ai.google.dev/gemini-api/docs/pricing); [Gemini caching](https://ai.google.dev/gemini-api/docs/caching)
- Kimi K3 lists cache writes at $3.00 (5 min TTL), the same as base input, and $6.00 (1 h). — [Kimi pricing](https://platform.kimi.ai/docs/pricing/chat)
- Z.ai cached-input storage is "Limited-time Free" with no end date. Expect a future charge. — [Z.ai pricing](https://docs.z.ai/guides/overview/pricing)
- Cerebras: cached input is billed at the full input rate. **[3P]** — [morphllm Cerebras pricing](https://www.morphllm.com/cerebras-pricing)
- Groq: 50% off cached input on gpt-oss models. Minimum prefix is 128–1,024 tokens. TTL is about 2 hours. — [Groq prompt caching](https://console.groq.com/docs/prompt-caching)

**Tiers, batch and time of day**
- OpenAI:
  - Batch and Flex are 50%.
  - "Priority processing was renamed Fast mode on July 30, 2026". Fast is 2x on the 6.x and 5.6 families.
  - Ultrafast is 6x (Astra, 6.1 Sol).
  - Regional processing (data residency) adds 10% for models released on or after 2026-03-05. FedRAMP adds 10%.
  — [OpenAI pricing](https://developers.openai.com/api/docs/pricing)
- Anthropic:
  - Batch is 50%.
  - Fast mode: Opus 5.5 $8/$40 (2x); Opus 5 and 4.8 $10/$50. Fast mode has no Batch.
  - `inference_geo:"us"` costs 1.1x on all categories for 4.6+. Foundry US Data Zone is also 1.1x.
  - Bedrock and Vertex regional or multi-region endpoints cost +10%.
  — [Anthropic pricing](https://platform.claude.com/docs/en/about-claude/pricing)
- Gemini: Batch and Flex are 50%. Priority is 1.8x (for example Argon $3.60 / $18). — [Gemini pricing](https://ai.google.dev/gemini-api/docs/pricing)
- DeepSeek has two prices by time of day. "Peak hours are 01:00 - 04:00 and 06:00 - 10:00 UTC, Monday through Friday, excluding Chinese public holidays". Every other hour, including weekends, is billed at half. The meter needs a UTC clock **and a Chinese holiday calendar**. — [DeepSeek pricing](https://api-docs.deepseek.com/quick_start/pricing/)
- Z.ai GLM Coding Plan: off-peak and weekends consume "only 50% of the standard points". — [Z.ai GLM-5.3](https://docs.z.ai/guides/llm/glm-5.3)
- MiniMax-M3 Priority is 1.5x. — [MiniMax pricing](https://platform.minimax.io/docs/guides/pricing)
- Fireworks: Priority is 1.25x, US-region and "Fast" variants are 1.5x, and batch is 50%. — [Fireworks pricing](https://docs.fireworks.ai/serverless/pricing)
- Bedrock Converse `serviceTier` takes `reserved`, `priority`, `default` or `flex`. — [Bedrock Converse](https://docs.aws.amazon.com/bedrock/latest/userguide/conversation-inference.html)
- Qwen batch is 50%, listed only for Beijing. Some Singapore prices carry "Limited-time 20% off". — [Alibaba pricing](https://www.alibabacloud.com/help/en/model-studio/model-pricing)

**Scheduled and promotional prices**
- Gemini 4 Argon doubles on 2026-12-01. 3.8 and 3.6 Flash, Robotics-ER 2 and 3.8 TTS change on 2027-01-01. — [Gemini pricing](https://ai.google.dev/gemini-api/docs/pricing)
- GPT-5.6 Sol is on promotional pricing "at least through November 21, 2026". — [OpenAI pricing](https://developers.openai.com/api/docs/pricing)
- Claude Sonnet 5 was introduced at $2/$10 as intro pricing; this is now the standard price. — [Anthropic pricing](https://platform.claude.com/docs/en/about-claude/pricing)

**Reasoning tokens**
- OpenAI bills reasoning as output (`output_tokens_details.reasoning_tokens`). — [OpenAI reasoning guide](https://developers.openai.com/api/docs/guides/reasoning)
- Anthropic bills the full thinking tokens, not the summary, "even when the thinking text isn't returned". The billed output count will not match the visible tokens. — [Anthropic thinking](https://platform.claude.com/docs/en/build-with-claude/thinking)
- Gemini bills thinking tokens as output. — [Gemini thinking](https://ai.google.dev/gemini-api/docs/thinking)
- Several models have thinking always on or not disableable: Opus 5.5, Fable 5.1, Sonnet 5.5, Grok 4.5–4.7, GLM-5.3, MiniMax M3, and Gemini 2.5 Pro and 3.x on OpenAI-compat. Hidden output cost is unavoidable on these. — sources in Q1.

**Per-request and tool fees**
- OpenAI:
  - Web search: $10 per 1K calls plus content tokens. Non-reasoning preview: $25 per 1K. gpt-4o-mini and 4.1-mini bill a fixed 8,000 input tokens per call.
  - File search: $2.50 per 1K calls, plus storage at $0.10/GB-day.
  - Containers: $0.03–1.92 per 20-minute session, with a 5-minute minimum.
  — [OpenAI pricing](https://developers.openai.com/api/docs/pricing)
- Anthropic:
  - Web search: $10 per 1K.
  - Web fetch: free.
  - Code execution: free when used with web search or fetch `_20260209`+. Otherwise $0.05 per container-hour after 1,550 free hours/month, with a 5-minute minimum.
  - Managed Agents: $0.08 per session-hour.
  - Tool-use system prompt overhead is 286–675 tokens per request. The bash tool adds 325 tokens, `text_editor` adds 974, and the computer toolset adds about 4,500.
  - Usage reports `server_tool_use.web_search_requests`.
  — [Anthropic pricing](https://platform.claude.com/docs/en/about-claude/pricing)
- Gemini grounding (Gemini 3+): 5,000 free searches/month, then $14 per 1,000. 2.5: 1,500 RPD free, then $35 per 1,000. — [Gemini pricing](https://ai.google.dev/gemini-api/docs/pricing)

**Images and audio**
- Gemini audio input is priced 2–3.3x text (for example 3-flash-preview $1.00 vs $0.50). Image output is priced per image ($0.0336–0.24). — [Gemini pricing](https://ai.google.dev/gemini-api/docs/pricing)
- OpenAI gpt-realtime-2.1 audio is $32 in / $64 out vs text $4 / $24. Image models are priced per image-token. — [OpenAI pricing](https://developers.openai.com/api/docs/pricing)

**Refusals and fallback**
- Classifier refusals in `bio`, `frontier_llm` and `reasoning_extraction` are billed even with no output. With fallback, both the refusal and the fallback request can be billed. — [Anthropic refusals and fallback](https://platform.claude.com/docs/en/build-with-claude/refusals-and-fallback)

**Tokenizer drift**
- Claude 4.7+ produces about 30% more tokens for the same text. Comparing per-MTok prices across generations understates the real cost increase. — [Anthropic pricing](https://platform.claude.com/docs/en/about-claude/pricing)

### Inferences
- **Pre-flight estimates can misprice at a threshold boundary.** Crossing 100K, 200K, 272K or 512K reprices every token, including cache reads. The proxy should compute the tier from actual reported prompt tokens, after the response, not from its own estimate.
- **Quality guard for the meter:** reconcile every N requests against the provider's usage or cost API (Anthropic has Usage & Cost APIs, though not on Bedrock, Vertex or Foundry). Alert when the metered dollars drift more than about 2% from the invoice.
- On Bedrock, Vertex and Foundry, the bill comes from the cloud (CCUs or AWS/GCP pricing). The proxy's dollar figure is an estimate unless the platform rate card is loaded separately.

### Gaps
- Pricing for OpenAI `reasoning.mode:"pro"`.
- xAI tool fees and image-input pricing.
- Bedrock flex and priority multipliers.
- Qwen cache percentages.
- Whether Gemini Interactions bills differently from generateContent (nothing suggests it does).
- Whether OpenAI's `cache_write_tokens` is counted inside `input_tokens`.

---

## Q5. What a generic OpenAI-compatible adapter can cover, and which providers need native adapters

### Takeaway
A single "OpenAI Chat Completions" adapter with per-provider quirk profiles covers the long tail:
- DeepSeek, Kimi, Qwen, Z.ai, MiniMax, xAI (basic), Groq, Cerebras, SambaNova, Together, Fireworks, DeepInfra
- Gemini-compat (beta)
- Azure v1 and Bedrock OpenAI endpoints

It is **not** enough for the providers INIT most needs for coding agents. Four native adapters are needed:
1. **OpenAI Responses**: tools on GPT-6 Astra and 6.1 Sol, reasoning items and encrypted replay, cache-write usage. Also reused by xAI, Azure v1, Bedrock, Z.ai and DeepSeek.
2. **Anthropic Messages**: thinking signatures, `cache_control`, usage semantics, `usage.iterations`. Also reused by Bedrock-Mantle, Vertex (with URL and body tweak), Foundry, Claude Platform on AWS, MiniMax, Z.ai and DeepSeek.
3. **Gemini generateContent/Interactions**: thought signatures, implicit-cache usage, new features ship only on Interactions.
4. **Bedrock Converse**: SigV4, binary event-stream, `cachePoint`, `reasoningContent`. Needed only for non-Anthropic Bedrock models, since Claude on Bedrock now has a Messages endpoint.

### Cited Findings
- Chat Completions cannot do tool calling on GPT-6 Astra and 6.1 Sol. Luna allows it only at effort `none`. — [OpenAI reasoning guide](https://developers.openai.com/api/docs/guides/reasoning); [gpt-6.1-sol](https://developers.openai.com/api/docs/models/gpt-6.1-sol); [gpt-6-luna](https://developers.openai.com/api/docs/models/gpt-6-luna)
- xAI's encrypted reasoning works only on Responses; "Chat Completions… has no field for the ciphertext". — [xAI reasoning guide](https://docs.x.ai/docs/guides/reasoning)
- The Gemini OpenAI-compat layer is beta, has no Responses endpoint, and silently ignores unknown parameters. — [Gemini OpenAI compatibility](https://ai.google.dev/gemini-api/docs/openai)
- New Gemini features launch only on Interactions, and explicit caching is unavailable there. — [Gemini Interactions API](https://ai.google.dev/gemini-api/docs/interactions); [Gemini caching](https://ai.google.dev/gemini-api/docs/caching)
- Multi-protocol providers:
  - DeepSeek supports the Responses API and the Anthropic API. — [DeepSeek pricing](https://api-docs.deepseek.com/quick_start/pricing/)
  - Z.ai exposes CC, Responses (`/api/v1`) and Anthropic (`/api/anthropic`). — [Z.ai GLM-5.3](https://docs.z.ai/guides/llm/glm-5.3)
  - MiniMax recommends its Anthropic endpoint, which "Supports thinking blocks, interleaved thinking". — [MiniMax text generation](https://platform.minimax.io/docs/guides/text-generation)
- Quirks the CC profile must encode per provider:
  - Field that carries reasoning: `reasoning_content` (DeepSeek, MiniMax, Kimi, Z.ai).
  - Replay rules: DeepSeek returns 400 when it is not replayed with tools.
  - Effort vocabulary: DeepSeek and Z.ai `low/high/max`; MiniMax `low…max`; Gemini maps to `thinking_level`.
  - Thinking toggle form: DeepSeek `extra_body.thinking`. Disabling is impossible on Z.ai and MiniMax.
  - Ignored or erroring sampling parameters: DeepSeek `temperature` is ignored; xAI `stop` and penalties error.
  - Cached-token field: Groq `prompt_tokens_details.cached_tokens`.
  - `stream_options.include_usage` is needed to get usage when streaming. — [Alibaba OpenAI compatibility](https://www.alibabacloud.com/help/en/model-studio/compatibility-of-openai-with-dashscope); [Azure OpenAI v1 API](https://learn.microsoft.com/en-us/azure/ai-foundry/openai/api-version-lifecycle)
  — sources for the remaining items in Q1 and Q2
- Auth plug-ins needed:
  - static Bearer
  - `x-api-key` + `anthropic-version`
  - `api-key` (Azure)
  - `x-goog-api-key` or `?key=`
  - Google OAuth (Vertex)
  - Entra ID token, scope `https://ai.azure.com/.default`
  - AWS SigV4 (service `bedrock` or `bedrock-mantle`)
  - Bedrock short-term bearer sent in `x-api-key`
  — [Claude in Amazon Bedrock](https://platform.claude.com/docs/en/build-with-claude/claude-in-amazon-bedrock); [Claude on Google Cloud](https://platform.claude.com/docs/en/build-with-claude/claude-on-vertex-ai); [Claude in Microsoft Foundry](https://platform.claude.com/docs/en/build-with-claude/claude-in-microsoft-foundry); [Azure OpenAI v1 API](https://learn.microsoft.com/en-us/azure/ai-foundry/openai/api-version-lifecycle)
- Vertex Claude differs from first-party in two ways: the model is in the URL, and `anthropic_version: "vertex-2023-10-16"` is in the body. — [Claude on Google Cloud](https://platform.claude.com/docs/en/build-with-claude/claude-on-vertex-ai)
- Foundry uses the deployment name as `model` and returns no rate-limit headers. — [Claude in Microsoft Foundry](https://platform.claude.com/docs/en/build-with-claude/claude-in-microsoft-foundry)

### Inferences
**Recommended Root proxy design (a proposal)**
- Build **three wire codecs**: OpenAI Responses, Anthropic Messages, and OpenAI Chat Completions with quirk profiles.
- Add **one Google codec**, Interactions-first with a generateContent fallback.
- Add an optional **Converse codec** for non-Anthropic Bedrock models.
- Keep auth as an orthogonal plug-in layer.
- Route each provider to the richest protocol it supports. For example, send MiniMax, Z.ai and DeepSeek through the Anthropic codec when tools and thinking are used, because replay and caching semantics are explicit there.
- The meter needs a **usage normalizer per codec** (Q2) and a **price resolver** keyed by `(provider, model, tier/speed, geo, ctx_bucket, utc_time, effective_date)`.
- Quality guard: golden-transcript tests per codec. Record real SSE streams that include thinking, tools, cache hits and errors, and assert both the round-trip replay and the metered cost.

**Why not "everything through OpenAI-compat":**
- It loses Anthropic cache-write and TTL accounting.
- It loses encrypted reasoning replay on OpenAI and xAI.
- It cannot call tools on the GPT-6 flagships.
- Its Gemini support is beta and silently drops parameters.

### Gaps
- Whether Kimi, Qwen, Groq, Cerebras and the hosts accept and return reasoning replay fields in their OpenAI-compat streams.
- Whether OpenRouter-style aggregators matter for INIT. They were out of scope and not researched.
- Exact OpenAI Responses SSE event names in 2026. Use the official reference and recorded fixtures before coding.
