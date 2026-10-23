# Subscription-plan routes for agentic coding harnesses: auth, terms, and secure implementation for INIT (status as of 2026-10-20)

Scope note: read-only web research done 2026-10-20. "Accessed" means 2026-10-20 unless another date is given. Quotes are kept under 15 words. Primary sources are vendor docs, terms and changelogs; anything resting only on secondary coverage is labeled (secondary). Anything I could not confirm is labeled UNVERIFIED. Verdict scale: ALLOWED for a third-party harness / GRAY / PROHIBITED.

## Q0. Verdict matrix: which subscription routes can a third-party open-source harness like INIT legitimately use?

### Takeaway
As of Oct 2026 only three subscription routes are explicitly open to an unlisted open-source harness: OpenAI's "Sign in with ChatGPT" plan usage (open-source/local self-serve path, preview), API-key "coding plans" whose docs invite other tools (MiniMax; Cerebras/Ollama-style key plans), and API keys funded by subscription credits (Claude Max/Team monthly API credits). Anthropic consumer OAuth and Google consumer OAuth (Gemini CLI/Antigravity) are prohibited for third-party clients. Copilot, Kimi, Alibaba and Z.ai sit in between because each depends on official partner lists or "supported tools" lists.

### Cited Findings
- OpenAI: plan usage is self-serve for "open-source projects, personal projects that run locally" — [OpenAI cookbook](https://developers.openai.com/cookbook/articles/sign-in-with-chatgpt)
- Anthropic: "Anthropic does not permit third-party developers to offer Claude.ai login into their own applications" — [Claude Code legal and compliance](https://code.claude.com/docs/en/legal-and-compliance)
- Google: third-party tools may not "harvest or piggyback on Gemini CLI's OAuth authentication" — [Gemini CLI FAQ](https://geminicli.com/docs/resources/faq/)
- GitHub: formal Copilot partnership with OpenCode only (Jan 16, 2026) — [GitHub changelog](https://github.blog/changelog/2026-01-16-github-copilot-now-supports-opencode/)
- Z.ai: "GLM Coding Plan may only be used within officially supported tools and products" — [Z.ai usage policy](https://docs.z.ai/devpack/usage-policy)
- Alibaba: "Do not use the plan's API key for automated scripts, application backends" — [Alibaba Cloud Coding Plan, last updated Oct 11, 2026](https://www.alibabacloud.com/help/model-studio/coding-plan)

### Inferences
Summary matrix. Every row is detailed with citations in the sections below.

| Route | Auth mechanism | Verdict for INIT | Condition |
|---|---|---|---|
| OpenAI "Sign in with ChatGPT" plan usage (Plus/Pro) | OAuth 2.0 + PKCE + OIDC, loopback, dynamic client registration | **ALLOWED** (preview) | Open-source or local app; follow the SIWC rules (store:false, stream:true, weekly cap, branding) |
| Reusing Codex CLI's own OAuth client and the `chatgpt.com/backend-api/codex` endpoint | Codex CLI's PKCE flow, undocumented backend | **GRAY, avoid** | Superseded by SIWC; the backend is undocumented and unversioned |
| OpenAI Platform API key | API key | **ALLOWED** | Pay per token |
| Claude Pro/Max/Team OAuth used by INIT directly (own OAuth client, reading Claude Code's token, impersonating Claude Code headers) | OAuth PKCE (claude.ai) | **PROHIBITED** | Server-side blocked since Jan 9, 2026 (secondary); docs forbid it |
| INIT offering a "Sign in with Claude" button | — | **PROHIBITED unless previously approved** | Amp appears to have approval (inference) |
| INIT driving the unmodified official `claude` binary / Agent SDK, with the user signing in through Anthropic's own flow and INIT never touching tokens | Claude Code's own login | **GRAY** | Help Center says third-party apps draw from subscription limits, but the Agent SDK note and the April 4 statement conflict with that |
| Claude Console API key, including Max/Team monthly API credits ($100/$200/up to $500) | API key | **ALLOWED** | Credits cover the API and Agent SDK, not Claude Code |
| GitHub Copilot SDK with a user token from INIT's own GitHub OAuth App | GitHub OAuth (user token) | **ALLOWED** (official SDK) | Runs Copilot's agent runtime and draws on the user's allowance |
| Direct `api.githubcopilot.com` calls with a fine-grained PAT ("Copilot Requests") | PAT | **GRAY** | Documented for Copilot CLI, not for third parties |
| Reusing the VS Code/Copilot OAuth client ID and `copilot_internal` token exchange; Copilot proxies | Device flow | **GRAY → suspension risk** | Only OpenCode has a formal partnership |
| Gemini CLI / Antigravity Google-login OAuth (free, AI Pro/Ultra, Code Assist) | Google OAuth installed-app | **PROHIBITED** | Bans in Feb 2026 |
| Gemini API key (AI Studio) / Vertex AI | API key / GCP auth | **ALLOWED** | Google's stated "supported and secure method" |
| Qwen OAuth free tier | Device/OAuth | **N/A (discontinued Apr 15, 2026)** | — |
| Alibaba Cloud Coding Plan key (sk-sp-) | API key | **GRAY** | Interactive coding tools only; INIT is not named |
| Kimi Code membership key | API key | **GRAY** | Listed tools; honest User-Agent; personal use |
| Z.ai GLM Coding Plan key | API key | **PROHIBITED until INIT is listed** (GRAY if listed "others") | Supported-tools-only policy |
| MiniMax M Plan / Coding Plan subscription key | API key | **ALLOWED** (per usage rules; full ToS UNVERIFIED) | — |
| Cursor plan via Cursor SDK user API key | API key into Cursor's agent SDK | **GRAY / UNVERIFIED** | It is an agent surface, not raw model access |
| Windsurf, Factory, Amp own credits | — | **No exportable subscription found** | — |
| Cerebras Code, Ollama Cloud, Synthetic (key-based plans) | API key | **ALLOWED** (vendor-documented third-party integrations; terms not fully read) | — |

### Gaps
- None of the vendors publishes a third-party "subscription harness certification" that INIT could apply to, except OpenAI's interest form for commercial or hosted apps and Anthropic's unspecified "previously approved" path.

---

## Q1. OpenAI ChatGPT Plus/Pro/Business: "Sign in with ChatGPT" (SIWC) and Codex

### Takeaway
At DevDay on Sept 29, 2026, OpenAI opened an official path for third-party apps to spend a user's ChatGPT plan. Open-source and locally run apps can register themselves through dynamic OAuth client registration and call the public Responses API with the user's bearer token, within a per-app weekly cap the user sets. This is the cleanest legitimate subscription route for INIT. The earlier community approach of reusing Codex CLI's OAuth client against `chatgpt.com/backend-api/codex` is unofficial and should not be shipped.

### Cited Findings
**Policy and dates**
- SIWC launched at DevDay on Sept 29, 2026. Plus and Pro users can authorize participating apps to run eligible requests against their plan, with a weekly per-app cap — [DEV/apidog guide, Oct 2, 2026 (secondary)](https://dev.to/hassann/sign-in-with-chatgpt-for-developers-the-oauth-flow-plan-usage-and-what-it-means-for-your-api-bill-48f7); [Simon Willison DevDay live blog](https://simonwillison.net/2026/Sep/29/openai-devday-2026-live-blog/)
- OpenAI docs scope: "These docs explain ChatGPT plan usage for open-source and locally hosted apps." Paid or remotely hosted apps must complete an interest form (openai.com/form/sign-in-with-chatgpt-interest/) — [SIWC token-sharing overview](https://developers.openai.com/siwc/token-sharing-open-source)
- The cookbook says plan usage at launch covers "open-source projects, personal projects that run locally" plus selected private apps, and names Plus and Pro as eligible plans. It asks developers to review openai.com/policies and the DevKit licence (github.com/openai/sign-in-with-chatgpt-devkit) — [OpenAI cookbook](https://developers.openai.com/cookbook/articles/sign-in-with-chatgpt)
- Plan conflict: one secondary source says the OpenAI Help Center lists Go, Plus and Pro for commercial apps and says open-source tools are available to all ChatGPT users. The developer docs name only Plus and Pro. The Help Center page ([help.openai.com 20001542](https://help.openai.com/en/articles/20001542-using-your-chatgpt-plan-in-other-apps-and-sites)) returned HTTP 403 to my fetch, so this is UNVERIFIED — [eesel (secondary)](https://www.eesel.ai/blog/sign-in-with-chatgpt)
- Named open-source apps with plan usage: OpenClaw, OpenCode, Pi, T3. There were 16 launch partners in total, including Amp, Kilo Code, Warp, Conductor, Devin, Hermes Agent, Notion and Vercel — [eesel (secondary)](https://www.eesel.ai/blog/sign-in-with-chatgpt); [The New Stack via search summary (secondary)](https://thenewstack.io/sign-in-with-chatgpt/)
- An OpenAI executive reportedly said Pi and OpenCode already made up 10% of Codex traffic — [DEV Community (secondary)](https://dev.to/bd_perez/chatgpt-plus-enjoy-200-of-tokens-for-20-while-it-lasts-4j2k)

**(a) Auth flow, technical**
- OAuth Authorization Code with PKCE (S256), plus OIDC `state` and `nonce`. Authorization endpoint `https://auth.openai.com/api/accounts/authorize`; token exchange is a form POST to `https://auth.openai.com/api/accounts/oauth/token`. No client secret — [SIWC open-source sign-in](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)
- First-time registration uses `client_id=dynamic_agent_client` with `agent_name_hint` (app name) and a persistent `ext_agent_host_id`. The callback returns an issued client ID (`oaiapp_...`) that is saved and reused for later exchanges and refreshes — [SIWC sign-in](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)
- Redirect is an HTTP loopback on `127.0.0.1` (example `http://127.0.0.1:1455/auth/callback`). The docs say not to use `localhost`, and to start the listener before opening the browser — [SIWC sign-in](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)
- Scopes: `openid profile email` for identity; `offline_access resource.invoke chatgpt.tokens.use.direct` for plan usage; `resource=https://api.openai.com/v1` in both the authorize and exchange steps. The client must verify the ID token against JWKS and confirm `chatgpt.tokens.use.direct` was granted — [SIWC sign-in](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)
- The user's browser flow covers "sign-in, client registration, and consent" to use the plan. Plan usage grants no access to the user's ChatGPT conversations — [SIWC overview](https://developers.openai.com/siwc/token-sharing-open-source)

**Token lifetime and refresh**
- Access token: 1 hour (`expires_in: 3600`). Refresh token: 30 days, rotated on every refresh with a fresh 30-day lifetime. Audience is `https://api.openai.com/v1` — [SIWC token reference](https://developers.openai.com/siwc/token-sharing-open-source/token-reference)
- Refresh uses the saved issued client ID, not `dynamic_agent_client`. The docs say to "serialize refreshes for the same session so two processes do not race a rotating token". Revoke at `revocation_endpoint` before clearing tokens on sign-out. Never put tokens in URLs or logs — [SIWC accounts and sessions](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions)

**Endpoints and API format**
- Public Responses API at `https://api.openai.com/v1`, with the access token as the Bearer — [DEV/apidog (secondary)](https://dev.to/hassann/sign-in-with-chatgpt-for-developers-the-oauth-flow-plan-usage-and-what-it-means-for-your-api-bill-48f7); the audience is confirmed in [token reference](https://developers.openai.com/siwc/token-sharing-open-source/token-reference)
- Required: `store: false` and `stream: true`. No `previous_response_id` over HTTP, so the full history goes in `input`; WebSocket continuation works only within the same connection. Omitted fields include `temperature`, `max_output_tokens`, `metadata`, `user` and others. Unsupported: image generation, file search, Code Interpreter, hosted MCP, native computer use, `tool_search`. Function and custom tools are supported. The same limits apply when going "through Codex app-server", where the app must refresh the token itself — [SIWC preview limitations](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations)

**Rate and usage limits**
- Usage draws on the plan's Codex and ChatGPT Work usage. The per-app weekly cap is a percentage of total weekly usage and is a ceiling, not a reservation. Credits beyond the cap are opt-in and require a 100% cap. Plus has a shared 5-hour limit across apps; Pro does not. There is no automatic billing fallback — [DEV/apidog (secondary)](https://dev.to/hassann/sign-in-with-chatgpt-for-developers-the-oauth-flow-plan-usage-and-what-it-means-for-your-api-bill-48f7)
- Error contract: 429 `subscription_sharing_usage_limit_exceeded` (can arrive mid-stream as `response.failed`); 403 `subscription_sharing_user_not_eligible` (do not retry); 401 `subscription_sharing_invalid_user` or `invalid_grant` on refresh (user disconnected, so clear tokens) — [DEV/apidog (secondary)](https://dev.to/hassann/sign-in-with-chatgpt-for-developers-the-oauth-flow-plan-usage-and-what-it-means-for-your-api-bill-48f7)
- Users manage caps at chatgpt.com/settings/usage. Disconnecting does not notify the app — same source (secondary)

**Codex CLI (first-party) and the older unofficial route**
- Codex CLI supports "Sign in with ChatGPT for subscription access" or an API key. Credentials go in `~/.codex/auth.json`, the OS keyring, `auto`, or `ephemeral`. Tokens refresh automatically. Device-code login is in beta for headless use. The docs say: "For general OpenAI API calls, continue to use Platform API keys." — [Codex auth docs (learn.chatgpt.com, redirected from developers.openai.com/codex/auth)](https://learn.chatgpt.com/docs/auth)
- The pre-SIWC community route: plugins such as `opencode-openai-codex-auth` reused Codex's PKCE flow (`localhost:1455` callback, `auth.openai.com/oauth/token`) and called `https://chatgpt.com/backend-api/codex/responses`. One such project says this is "plausibly against OpenAI's terms" because the backend is undocumented and unversioned — [workers.iii.dev provider-openai-codex (secondary)](https://workers.iii.dev/workers/provider-openai-codex); [awesome-opencode discussion #34](https://github.com/awesome-opencode/awesome-opencode/discussions/34); [Codex auth flows write-up (secondary)](https://codex.danielvaughan.com/2026/04/01/codex-cli-authentication-flows-credential-management/)

**What harnesses ship**
- OpenCode ships a "ChatGPT Plus/Pro" browser OAuth option under `/connect`, plus manual API keys — [OpenCode providers docs](https://opencode.ai/docs/providers/)
- Devin added "ChatGPT Go, Plus or Pro (GPT share only)" around Oct 9, 2026 — [FourWeekMBA (secondary)](https://fourweekmba.com/ai-amp-opens-its-claude-code-mode-to-claude-pro-and-max-plans/)

### Inferences
- **Verdict for INIT: ALLOWED** through the SIWC open-source path, provided INIT stays open-source and local. If INIT ever offers a hosted or remote service, that service must go through the interest form.
- **Verdict for reusing Codex CLI's client ID and the backend-api endpoint: GRAY.** INIT should not ship it now that an official route exists.
- The SIWC API surface (no `previous_response_id`, no `max_output_tokens`, no `temperature`) means INIT's OpenAI adapter needs a "plan mode" that sends the full history each turn and does not rely on server-side state.
- Business, Enterprise and Edu eligibility for plan usage is not established, so INIT should treat Plus and Pro as the supported set and fall back to an API key otherwise.

### Gaps
- I could not read the OpenAI Help Center article (HTTP 403), so the per-plan eligibility details (Go, Business, Enterprise) are UNVERIFIED.
- I found no SIWC-specific developer terms. Only the general openai.com/policies and the DevKit licence are referenced.
- The exact refresh request format and the full error list are on pages I did not fetch (the "Refreshing tokens" section and errors-and-recovery).
- I found no OpenAI primary announcement post. Partner lists come from secondary sources.

---

## Q2. Anthropic Claude Pro/Max/Team/Enterprise: Claude Code OAuth and third-party harnesses

### Takeaway
Anthropic's current docs prohibit third-party developers from offering claude.ai login, or from collecting, storing or intermediating Claude subscription tokens, unless Anthropic has previously approved them. Enforcement began on Jan 9, 2026, policy text was "clarified" in Feb 2026, and subscriptions stopped covering third-party harnesses on Apr 4, 2026. A planned Agent SDK credit was announced on May 13 and paused on June 15. Since Oct 7, 2026, Max and Team plans include monthly Console API credits. The Help Center still says the Agent SDK, `claude -p` and "third-party apps" draw from subscription limits, so driving the official, unmodified Claude Code binary with the user's own login is GRAY. Direct OAuth reuse is PROHIBITED. API keys, including the new subscriber credits, are ALLOWED.

### Cited Findings
**Current terms (primary)**
- OAuth "is intended exclusively for purchasers of Claude Free, Pro, Max, Team, and Enterprise" plans, for Claude Code and native Anthropic apps — [Claude Code legal and compliance](https://code.claude.com/docs/en/legal-and-compliance)
- "Anthropic does not permit third-party developers to offer Claude.ai login into their own applications" — same page
- "developers may not collect, store, or intermediate Claude.ai credentials or session tokens"; sign-in must complete through Anthropic's own flow — same page
- The same page says the rule does not prevent "an end user from signing in to the unmodified Claude Code binary" with their own subscription, including where a platform hosts Claude Code. It also says Anthropic may enforce "without prior notice" — same page
- Hosting Claude Code requires an unmodified binary, no removal of built-in auth methods, and no paying for, reselling or intermediating usage — same page
- "Advertised usage limits for Pro and Max plans assume ordinary, individual usage" of Claude Code and the Agent SDK — same page
- Agent SDK note: "Unless previously approved, Anthropic does not allow third party developers to offer claude.ai login" or rate limits, including Agent SDK agents. The SDK is governed by the Commercial Terms. Branding must not say "Claude Code" — [Agent SDK overview](https://code.claude.com/docs/en/agent-sdk/overview)
- Consumer Terms (effective Oct 8, 2025), §3: no access "through automated or non-human means" except via an API key or "where we otherwise explicitly permit it". §2: no sharing of account credentials — [Anthropic Consumer Terms](https://www.anthropic.com/legal/consumer-terms)

**Dated policy and enforcement timeline**
- **Jan 9, 2026**: server-side checks blocked third-party tools (OpenCode, Cline, RooCode) from authenticating with Pro/Max OAuth. The error said the credential is only authorized for Claude Code. Tools had been spoofing Claude Code client headers — [paddo.dev (secondary)](https://paddo.dev/blog/anthropic-walled-garden-crackdown/); [decodethefuture (secondary)](https://decodethefuture.org/en/anthropic-blocks-third-party-tools/)
- **~Feb 19, 2026**: docs updated to say consumer OAuth tokens are for Claude Code and Claude.ai only. Anthropic called it a "clarification of existing language". Thariq Shihipar (Claude Code team) reportedly said nothing was changing for Agent SDK use with Max — [WinBuzzer (secondary)](https://winbuzzer.com/2026/02/19/anthropic-bans-claude-subscription-oauth-in-third-party-apps-xcxwbn/); [AlternativeTo (secondary)](https://alternativeto.net/news/2026/2/anthropic-officially-bans-using-subscription-authentication-for-third-party-claude-use)
- OpenCode removed its bundled Claude Pro/Max plugins. Its docs say "Anthropic explicitly prohibits this" and that the plugins are no longer bundled as of 1.3.0 — [OpenCode providers docs](https://opencode.ai/docs/providers/)
- **Apr 4, 2026 (12pm PT)**: Boris Cherny wrote that "Claude subscriptions will no longer cover usage on third-party tools like OpenClaw", and that users can still use such tools with their Claude login via discounted extra-usage bundles, or with an API key — [Boris Cherny on Threads](https://www.threads.com/@boris_cherny/post/DWsAWeND5nm/starting-tomorrow-at-pm-pt-claude-subscriptions-will-no-longer-cover-usage-on). Anthropic said the policy "applies to all third-party harnesses" — [TechCrunch, Apr 4, 2026](https://techcrunch.com/2026/04/04/anthropic-says-claude-code-subscribers-will-need-to-pay-extra-for-openclaw-support/)
- **May 13, 2026**: Anthropic announced a separate monthly "Agent SDK credit" (Pro $20, Max 5x $100, Max 20x $200; Team and Enterprise Premium per seat), billed at API rates and covering `claude -p`, GitHub Actions and third-party tools such as OpenClaw, effective June 15 — [VentureBeat, May 13, 2026](https://venturebeat.com/technology/anthropic-reinstates-openclaw-and-third-party-agent-usage-on-claude-subscriptions-with-a-catch)
- **June 15–16, 2026**: the change was paused. "ACP, `claude -p`, the Claude Agent SDK, and third-party Agent SDK apps continue to work with subscriptions as before" — [Zed blog, update June 16, 2026](https://zed.dev/blog/anthropic-subscription-changes). The Help Center says third-party app usage "still draw from your subscription limits" — [Claude Help Center 15036540](https://support.claude.com/en/articles/15036540-use-the-claude-agent-sdk-with-your-claude-plan)
- **Oct 7, 2026**: Max and Team plans include monthly API credits: Max 5x $100, Max 20x $200, Team $20 per Standard seat and $100 per Premium seat, pooled and capped at $500. Credits arrive in a linked Console org. They cover the Claude API, Managed Agents, the Agent SDK and the playground, but not Claude Code. They expire each cycle without rollover. Free, Pro and Enterprise plans are not eligible. Program terms are at anthropic.com/legal/credit-terms — [Claude Platform docs: API credits for Max and Team](https://platform.claude.com/docs/en/about-claude/api-credits-for-subscribers)
- **Oct 10, 2026**: Amp's "Claude Code mode" lets users link a Claude Pro/Max plan. It "uses the Claude Agent SDK instead of Amp's own agent". The announcement does not describe a partnership or the linking method — [Amp news](https://ampcode.com/news/use-your-claude-plan)

**(a) Auth flow, technical (Claude Code, first-party)**
- Browser OAuth on first launch, with a local callback server. If the browser cannot reach the callback (SSH, WSL2, containers), the user pastes a code instead — [Claude Code authentication](https://code.claude.com/docs/en/authentication)
- Storage: macOS Keychain (encrypted); Linux `~/.claude/.credentials.json` with mode 0600; Windows `%USERPROFILE%\.claude\.credentials.json`. `CLAUDE_CONFIG_DIR` isolates accounts, including the Keychain entry — same page
- `claude setup-token` mints a one-year OAuth token (Pro/Max/Team/Enterprise) for `CLAUDE_CODE_OAUTH_TOKEN`. It can only make model requests. Logins auto-refresh, and a warning appears 3 days before expiry — same page
- Precedence: cloud provider vars, then `ANTHROPIC_AUTH_TOKEN`, then `ANTHROPIC_API_KEY`, then `apiKeyHelper`, then `CLAUDE_CODE_OAUTH_TOKEN`, then Anthropic profiles, then `/login` subscription OAuth — same page
- Endpoints: OAuth tokens are only honored for Claude Code-identified traffic on Anthropic's API (secondary sources above). API-key traffic uses the public Messages API.

**What harnesses ship**
- OpenCode: Claude Pro/Max plugins no longer bundled since 1.3.0; API key supported — [OpenCode providers docs](https://opencode.ai/docs/providers/)
- Zed: runs Claude Code through ACP, and the official CLI in its terminal — [Zed blog](https://zed.dev/blog/anthropic-subscription-changes)
- Amp: Agent SDK mode with a linked Claude plan — [Amp news](https://ampcode.com/news/use-your-claude-plan)

### Inferences
- **PROHIBITED**: INIT running its own claude.ai OAuth client, reading Claude Code's Keychain or `.credentials.json` token, accepting a pasted `setup-token`, or sending Claude Code identity headers. Each of these is "collect, store, or intermediate" or impersonation.
- **PROHIBITED unless approved**: a "Sign in with Claude" button inside INIT.
- **GRAY**: INIT launching the user's own unmodified `claude` binary (headless `-p`/stream-json, or the Agent SDK, which runs that binary) under a dedicated `CLAUDE_CONFIG_DIR`, where the user runs `/login` themselves in Anthropic's flow and INIT never reads the credential. Support for this: the legal page's carve-out for the unmodified binary, and the Help Center line on "third-party apps". Against it: the Agent SDK note and the Apr 4 "all third-party harnesses" statement. Anthropic can change this "without prior notice". If INIT ships it, label it "uses your local Claude Code login", keep it opt-in, and never call it Claude Code.
- **ALLOWED**: a Console API key, which for Max and Team users can be funded by the monthly API credits. This is the clean "subscription-funded" Claude route for INIT.

### Gaps
- I found no Anthropic primary text that resolves the conflict between the Apr 4 "all third-party harnesses" statement and the June/Oct Help Center line "third-party apps … subscription limits".
- How Amp links a Claude account, and whether Amp is "previously approved", is UNVERIFIED.
- The Jan 9 enforcement mechanism (header fingerprinting) is documented only by secondary sources.
- A Help Center summary also mentions a "separate monthly credit for Agent SDK usage" for Pro/Max/Team/Enterprise. The Platform docs do not show it, and I treat it as UNVERIFIED and possibly stale (it may be the paused May plan).

---

## Q3. GitHub Copilot subscriptions in other tools

### Takeaway
GitHub officially supports Copilot subscriptions in OpenCode (formal partnership, Jan 16, 2026), in VS Code extensions through the Language Model API (with user consent), and in any app through the generally available Copilot SDK. The SDK embeds Copilot CLI's agent runtime and accepts tokens from the app's own GitHub OAuth App. A raw `api.githubcopilot.com` model endpoint used by an unlisted harness is not documented for third parties, and community proxies risk abuse suspension. Since June 1, 2026, Copilot is usage-billed with included usage and tighter rate limits.

### Cited Findings
- Jan 16, 2026: GitHub officially supports using Copilot Pro, Pro+, Business or Enterprise "with OpenCode" through a "formal partnership". Users run `/connect` and complete the GitHub device login flow, with "no additional AI license needed" — [GitHub changelog](https://github.blog/changelog/2026-01-16-github-copilot-now-supports-opencode/)
- OpenCode docs: GitHub Copilot uses device code at github.com/login/device; "Some models might need a Pro+ subscription" — [OpenCode providers docs](https://opencode.ai/docs/providers/)
- Copilot SDK: "generally available". Auth options: a signed-in user (stored `copilot` CLI OAuth credentials); an "OAuth GitHub App" that passes user tokens from the app's own OAuth app; env vars `COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN`; or BYOK. A subscription is required unless BYOK. Each prompt counts toward the usage allowance — [Copilot SDK README](https://cdn.jsdelivr.net/gh/github/copilot-sdk@main/README.md). For multi-user servers, pass "a per-session `gitHubToken`" — [Copilot SDK auth README](https://cdn.jsdelivr.net/gh/github/copilot-sdk@main/docs/auth/README.md)
- Copilot CLI accepts a fine-grained PAT that is user-owned and has the "Copilot Requests" permission. Classic `ghp_` tokens are ignored — [GitHub Docs: Authenticating Copilot CLI](https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/authenticate-copilot-cli)
- Endpoint format, per a vendor integration: `https://api.githubcopilot.com` is OpenAI-compatible Chat Completions, with newer models only on `/responses`. Requests without a `Copilot-Integration-Id` header are rejected; Docker uses `copilot-developer-cli`. Docker authenticates with a PAT exported as `GITHUB_TOKEN` — [Docker Agent docs](https://docs.docker.com/ai/docker-agent/providers/github-copilot/). The model list depends on the integration ID — [Pydantic AI docs (secondary)](https://pydantic.dev/docs/ai/models/github-copilot/)
- Many community tools exchange a GitHub token at `api.github.com/copilot_internal/v2/token` using the VS Code Copilot OAuth client ID. That endpoint is internal and can return 404 — [ReqLLM docs (secondary)](https://req-llm.hexdocs.pm/github_copilot.html)
- VS Code Language Model API: "Copilot's language models require consent from the user before an extension can use them". Requests can fail on quota — [VS Code docs](https://code.visualstudio.com/api/extension-guides/ai/language-model). Cline, Roo and Kilo call their support "experimental" — [Cline docs](https://docs.cline.bot/provider-config/vscode-language-model-api)
- Terms: Copilot for individuals is governed by GitHub ToS Section J (AI Features) — [GitHub additional product terms, effective Aug 27, 2026](https://docs.github.com/en/site-policy/github-terms/github-terms-for-additional-products-and-features). The ToS (effective Apr 27, 2026) has no Copilot-specific third-party client clause. §H: "excessively frequent requests to GitHub via the API may result in" suspension. §B.3: "Your login may only be used by one person" — [GitHub Terms of Service](https://docs.github.com/en/site-policy/github-terms/github-terms-of-service)
- Community answers (not authoritative) say direct backend calls from custom tools are "not currently a supported" integration — [GitHub Community #186464](https://github.com/orgs/community/discussions/186464); [#178117](https://github.com/orgs/community/discussions/178117). Proxy projects warn of suspension for scripted or bulk use — [@nick3/copilot-api on npm](https://www.npmjs.com/package/@nick3/copilot-api)
- Billing: usage-based billing has been live for all users since June 1, 2026, and "all model usage will incur AI Credit consumption". New individual sign-ups for Pro, Pro+ and Student were paused in April 2026. Opus was removed from Pro. New rate limits apply, and Pro+ has "more than 5X" Pro's limits — [GitHub Community announcement #192963](https://github.com/orgs/community/discussions/192963)

### Inferences
- **ALLOWED**: the Copilot SDK with INIT's own registered GitHub OAuth App and a user-consented token. The catch is that INIT then delegates the agent loop to Copilot's runtime, using the SDK's custom tools rather than INIT's own loop. That fits a "sub-agent backend" design, not a raw model provider.
- **GRAY**: INIT calling `api.githubcopilot.com` directly with a user-supplied fine-grained PAT ("Copilot Requests"). It is documented for Copilot CLI and used by Docker, but third parties are not explicitly authorized. INIT should send an honest integration identity and must not spoof `vscode-chat`.
- **GRAY with suspension risk**: copying OpenCode's approach (VS Code client ID plus `copilot_internal` exchange) without a partnership. INIT could ask GitHub for an OpenCode-style partnership.

### Gaps
- I did not find the client ID or integration ID that OpenCode uses under the partnership, or whether GitHub issued it a dedicated ID.
- I found no GitHub statement listing which third-party clients are "supported" beyond OpenCode, VS Code LM API consumers and SDK apps.
- Current per-plan included usage and prices after June 1, 2026 were not retrieved.

---

## Q4. Google: Gemini CLI login, Antigravity CLI, Code Assist, Google AI Pro/Ultra

### Takeaway
Google prohibits third-party software from using Gemini CLI or Antigravity OAuth logins, and enforced this with account bans in Feb 2026 (later largely reversed). Since June 18, 2026, free and Google AI Pro/Ultra users get CLI access only through the closed-source Antigravity CLI. Gemini CLI remains for Code Assist Standard/Enterprise and paid API keys. Third-party agents should use a Gemini API key or Vertex AI.

### Cited Findings
- FAQ: using third-party software to "harvest or piggyback on Gemini CLI's OAuth authentication to access our backend services" is a direct violation and may mean "immediate suspension or termination". "the supported and secure method" for third-party agents is a Vertex AI or Google AI Studio API key — [Gemini CLI FAQ](https://geminicli.com/docs/resources/faq/)
- Terms page (last updated Apr 10, 2026) names OpenClaw with Gemini CLI OAuth as an example: "Such actions may be grounds for suspension or termination of your account." Terms by auth type: personal Google login uses Google ToS; AI Pro/Ultra adds Google One Additional Terms; Code Assist Standard/Enterprise uses GCP ToS; API keys use Gemini API ToS (paid or unpaid); Vertex uses GCP ToS — [Gemini CLI terms and privacy](https://geminicli.com/docs/resources/tos-privacy/)
- May 19, 2026 announcement: from June 18, 2026, Gemini CLI "will stop serving requests" for Google AI Pro/Ultra and free users, who move to Antigravity CLI. Code Assist Standard/Enterprise, Google Cloud users and paid Gemini API keys stay "fully supported". The repo remains Apache 2.0 — [gemini-cli discussion #27274](https://github.com/google-gemini/gemini-cli/discussions/27274)
- Antigravity CLI (`agy`) is a closed-source Go binary. It uses Google OAuth through a browser, with a URL-plus-code flow on SSH — [DEV hands-on guide (secondary)](https://dev.to/arindam_1729/antigravity-cli-a-hands-on-guide-to-googles-terminal-coding-agent-5bc7); [The New Stack headline](https://thenewstack.io/google-antigravity-cli/)
- Feb 2026 enforcement: accounts using OpenClaw with Antigravity or Gemini CLI OAuth received 403 "disabled … for violation of Terms of Service". Around Feb 27 Google said it was "welcoming back" restricted users — [openclaw issue #14203](https://github.com/openclaw/openclaw/issues/14203); [PiunikaWeb, Feb 23, 2026 (secondary)](https://piunikaweb.com/2026/02/23/google-antigravity-openclaw-ban/); [MetricNexus (secondary)](https://metricnexus.ai/blog/google-banning-openclaw-antigravity-users). Antigravity CLI 403 appeals continue on Google's forum — [forum appeal thread](https://discuss.ai.google.dev/t/appeal-antigravity-cli-403-permission-denied-tos-violation-appeal-form-submitted-requesting-review/173451)
- Technical (unofficial): Gemini CLI uses a public installed-app OAuth client with a local callback. Tokens live in `~/.gemini/oauth_creds.json` (legacy) or the macOS Keychain. OAuth tokens target the internal Code Assist API `cloudcode-pa.googleapis.com/v1internal`, not the public `generativelanguage` API — [ZeroClaw commit (secondary)](https://git.hoyer.xyz/harald/zeroclaw/commit/1d8e57d388c8c81774ad23f2521157086f523db8); [agent-safehouse analysis (secondary)](https://www.agent-safehouse.dev/docs/agent-investigations/gemini-cli)

### Inferences
- **PROHIBITED**: any INIT use of Google-login OAuth tokens (free, AI Pro/Ultra, Antigravity, Code Assist) to call Google's backend.
- **ALLOWED**: a Gemini API key (AI Studio, paid or free tier) or Vertex AI credentials.
- **GAP/GRAY**: INIT spawning the official Gemini CLI or Antigravity CLI binary as a subprocess under the user's own login. No source addresses this, and Google's broad "third-party software … to access" wording makes it risky.

### Gaps
- I found no official Antigravity CLI terms page text or docs on third-party integration. The ACP and subprocess question is unanswered.
- AI Pro/Ultra quota numbers for Antigravity CLI were not retrieved.

---

## Q5. Alibaba: Qwen Code OAuth free tier, and Alibaba Cloud Model Studio Coding Plan

### Takeaway
The Qwen OAuth free tier is gone (discontinued Apr 15, 2026). Alibaba's paid Coding Plan issues a dedicated `sk-sp-` key with OpenAI- and Anthropic-compatible endpoints and lists many third-party agents. Its terms restrict it to interactive use in AI programming tools and forbid automated scripts and backends, so INIT use is GRAY.

### Cited Findings
- Qwen Code docs mark Qwen OAuth as discontinued; cached tokens may work briefly and new requests are rejected. The docs point to the Alibaba Cloud Coding Plan or an API key — [Qwen Code auth docs](https://qwenlm.github.io/qwen-code-docs/en/users/configuration/auth/); [qwen-code issue #3316](https://github.com/QwenLM/qwen-code/issues/3316)
- Shutdown date Apr 15, 2026, after the quota was cut from 1,000 to 100 requests/day — [InventiveHQ (secondary)](https://inventivehq.com/blog/qwen-code-still-free-2026-shutdown)
- Coding Plan (last updated Oct 11, 2026): supported tools include OpenClaw, Hermes Agent, Claude Code, OpenCode, Cursor, Codex, Qwen Code, Cline, Kilo CLI and others. "Do not use the plan's API key for automated scripts, application backends" or other non-interactive scenarios. Violations "may result in subscription suspension or API key revocation". Key format `sk-sp-…`. Endpoints `https://coding-intl.dashscope.aliyuncs.com/v1` (OpenAI) and `…/apps/anthropic` (Anthropic). Pro is $50/month with up to 6,000 requests per 5 h, 45,000 per week and 90,000 per month. Lite is closed to new subscriptions — [Alibaba Cloud Coding Plan](https://www.alibabacloud.com/help/model-studio/coding-plan)
- Requests count model invocations, and one task can use 5–30 of them — [emelia.io (secondary)](https://emelia.io/hub/alibaba-ai-coding-plan)

### Inferences
- **GRAY**: INIT is an "AI programming tool" but is not named. Interactive sessions with the user's own key fit the stated scope. Unattended or background INIT runs conflict with the "non-interactive scenarios" ban. INIT should label this provider "interactive only" and disable it for headless or scheduled jobs.

### Gaps
- I found no mechanism for a tool to get added to Alibaba's supported list.

---

## Q6. Moonshot Kimi Code membership

### Takeaway
Kimi Code membership issues API keys meant for third-party coding tools: Claude Code, OpenCode, Codex and Hermes Agent are documented. It forbids User-Agent tampering, and earlier benefit text limited use to personal, approved, non-automated tools. INIT use is GRAY.

### Cited Findings
- Endpoints: OpenAI-compatible `https://api.kimi.com/coding/v1` (China) and `https://api.kimi.ai/coding/v1` (overseas); Anthropic-compatible `…/coding/`. Models `k3`, `k3-256k`, `kimi-for-coding`, `kimi-for-coding-highspeed`, gated by tier. Up to 5 keys, each shown only once. The service has rate limiting with no published numbers. Third-party tools listed: Claude Code, OpenCode, Codex, Hermes Agent — [Kimi Code docs](https://www.kimi.com/code/docs/)
- "Tampering with the client identifier (User-Agent) is considered a violation" and may suspend benefits — [Kimi Code membership guide](https://www.kimi.ai/help/kimi-code/membership-guide)
- An earlier benefits page said the benefit is for personal development, and that keys used in "unapproved, automated, or derivative products or services" count as unauthorized. That URL now redirects (301) to the docs index, so the clause is UNVERIFIED as current — search snippet of [kimi.com benefits page](https://www.kimi.com/code/docs/en/benefits.html)
- An unofficial site reports a pending split of Kimi Code into a separate coding membership — [kimi-ai.chat (secondary, not Moonshot)](https://kimi-ai.chat/guide/kimi-code/)

### Inferences
- **GRAY**: allowed in spirit for personal interactive use if INIT sends an honest `INIT/x.y` User-Agent. Not allowed for automated or commercial workloads, which belong on Kimi Open Platform API keys.

### Gaps
- I did not find the current full Kimi Code terms text or per-tier quotas.

---

## Q7. Zhipu / Z.ai GLM Coding Plan

### Takeaway
The GLM Coding Plan is an API-key subscription restricted to "officially supported tools" (20+ listed). Use elsewhere risks restriction, freezing, and a ban after three violations. INIT should treat it as PROHIBITED until Z.ai lists INIT.

### Cited Findings
- "GLM Coding Plan may only be used within officially supported tools and products". "Use in unsupported tools may result in restricted benefits." Violations can bring "rate limiting, account freezing", and accounts with more than three violations "may be banned". "Account sharing or multi-user access is prohibited." — [Z.ai usage policy](https://docs.z.ai/devpack/usage-policy)
- Supported tools include ZCode, Claude Code, Codex, OpenCode, AutoClaw, OpenClaw and Cline ("Works with 20+ Agent Tools"). Limits: Lite 2,000 credits per 5 h and 10,000 per week; Pro 12,000/60,000; Max 28,000/140,000. The 5-hour quota "resets 5 hours after consumption" — [Z.ai DevPack overview](https://docs.z.ai/devpack/overview)
- The Coding API endpoint is `https://api.z.ai/api/coding/paas/v4`, separate from the general API. Plan quota cannot be used via direct API calls — [Z.ai FAQ via search snippet](https://docs.z.ai/devpack/faq). GLM-5.2 consumes 3x quota at peak — [techsy (secondary)](https://techsy.io/en/blog/glm-5-2-coding-plan)
- OpenCode ships a "Z.AI Coding Plan" option under `/connect` — [OpenCode providers docs](https://opencode.ai/docs/providers/)

### Inferences
- **PROHIBITED until listed.** The workaround is for INIT to drive a supported tool (for example, OpenCode or Claude Code configured with the GLM key) as a sub-agent, or to ask Z.ai to add INIT to the list.

### Gaps
- The Anthropic-compatible GLM endpoint and the process for getting listed were not confirmed.

---

## Q8. MiniMax Coding Plan / M Plan

### Takeaway
MiniMax's subscription ("M Plan", formerly Token/Coding Plan) issues a Subscription Key that its docs expect to be used in "other AI tools". I found no third-party-tool restriction in the usage rules, so the route is ALLOWED (full ToS UNVERIFIED).

### Cited Findings
- Usage from MiniMax's own tools and "other AI tools connected with your Subscription Key" counts toward the same limits. There is a 5-hour window for non-video models and a weekly window for all models, both starting from first use. The Subscription Key is separate from the pay-as-you-go key and not interchangeable — [MiniMax M Plan usage rules](https://platform.minimax.io/docs/m-plan/usage-rules)
- Older Coding Plan docs: the key is valid only while subscribed; the window was rolling; 1 prompt is about 15 model requests — [MiniMax Coding Plan intro](https://platform.minimax.io/docs/coding-plan/intro); [Cherry Studio docs (secondary)](https://docs.cherry-ai.com/en-us/pre-basic/providers/minimax-coding-plan)

### Inferences
- **ALLOWED** for INIT with the user's Subscription Key, subject to verifying the full ToS. Base URLs (commonly Anthropic- and OpenAI-compatible) need confirming from MiniMax docs (UNVERIFIED).

### Gaps
- Endpoints, per-tier quotas and the full terms text were not retrieved.

---

## Q9. Closed agentic coders: Cursor, Windsurf, Amp, Factory, Devin

### Takeaway
These products mostly consume subscriptions rather than export them. Cursor exposes its plan to other tools only through its own agent SDK, using a user API key that draws on the Cursor plan (secondary). Windsurf and Factory expose nothing comparable. Amp and Devin accept users' ChatGPT and/or Claude plans.

### Cited Findings
- Cursor: a user API key with the official Cursor SDK charges "the same Cursor plan/request pool". The SDK is an agent surface, not a chat-completions API — [Hermes Agent cursor provider (secondary)](https://hermes-agent.nousresearch.com/docs/plugins/cursor-provider). A Cursor forum answer says the subscription is not a generic external inference API — [Cursor forum](https://forum.cursor.com/t/can-we-use-llms-in-the-cursor-subscription-outside-cursor-by-and-api-a-key/159598). OpenAI's help page notes a planned wind-down of OpenAI models in Cursor, with a proposed date of Nov 12, 2026 — [OpenAI Help: OpenAI models in Cursor](https://help.openai.com/pt-pt/articles/20001506-using-openai-models-in-cursor)
- Factory: BYOK only. A Coder compatibility table lists Factory as not supporting ChatGPT or Claude subscriptions — [Coder commit (secondary)](https://git.dragonfruit.dev/mirror/coder/commit/ff6f5893df1209191bdc5574f92c2c8d52e94328); BYOK is free up to an allowance on individual plans — [Factory pricing docs](https://docs.factory.ai/pricing/individuals.md)
- Amp: Claude Pro/Max through Agent SDK mode (Oct 10, 2026) — [Amp news](https://ampcode.com/news/use-your-claude-plan); listed among SIWC partners (secondary, Q1)
- Windsurf: none of the sources I found mention it accepting or exporting subscriptions. Pro is $20 per month (secondary) — [Morph comparisons](https://www.morphllm.com/comparisons/windsurf-alternatives)

### Inferences
- For INIT, none of these is a model-provider route. A Cursor SDK adapter would be a "sub-agent backend" (GRAY until Cursor's SDK terms are read).

### Gaps
- I did not read Cursor's official SDK docs or terms (UNVERIFIED). Windsurf and Factory status rests on the absence of evidence.

---

## Q10. Other key-based coding plans and aggregators

### Takeaway
Several vendors sell flat-rate "coding plans" that simply issue an API key and document third-party agents such as Cline, Roo and OpenCode. These are the lowest-friction ALLOWED subscription routes. Terms were not fully read.

### Cited Findings
- Cerebras Code Pro ($50/month) and Max ($200/month) launched Aug 2025 — [Cerebras blog](https://www.cerebras.ai/blog/introducing-cerebras-code). Cline lists daily token allowances (24M Pro, 120M Max) — [Cline Cerebras docs](https://docs.cline.bot/provider-config/cerebras). The OpenCode integration requires a paid plan — [Cerebras OpenCode docs](https://inference-docs.cerebras.ai/integrations/opencode)
- Ollama Cloud: an API key from ollama.com, documented for OpenCode and Cline — [Ollama OpenCode docs](https://docs.ollama.com/integrations/opencode); [Ollama Cline docs](https://docs.ollama.com/integrations/cline)
- Synthetic: Anthropic-compatible endpoint `https://api.synthetic.new/anthropic` with `SYNTHETIC_API_KEY` — [OpenClaw provider docs (secondary)](https://docs.openclaw.ai/es/providers/synthetic)
- OpenCode Zen/Go: API key from opencode.ai/auth; Go is "a low cost subscription plan" — [OpenCode providers docs](https://opencode.ai/docs/providers/). Unconfirmed reports say free Zen keys work only in OpenCode's own clients — [80aj, Aug 14, 2026 (secondary)](https://www.80aj.com/2026/08/14/opencode-deepseek-api-restriction/); [community README claim, Sep 17, 2026 (UNVERIFIED)](https://github.com/ltmoerdani/opencode-copilot-chat)
- Mistral: Le Chat Pro ($14.99) and Team include the Vibe CLI, and subscriber keys are created under Code › Vibe CLI. Third-party use of subscription keys is UNVERIFIED — [Mistral Vibe 2.0](https://mistral.ai/fr/news/mistral-vibe-2-0); [Vibe CLI API keys docs](https://docs.mistral.ai/vibe/code/cli/api-keys-profiles)

### Inferences
- **ALLOWED** for Cerebras Code and Ollama Cloud, based on vendor-authored third-party integration docs. **GRAY** for OpenCode Zen/Go and Mistral subscription keys until their terms are read.

### Gaps
- Terms text for Cerebras, Ollama, Synthetic, OpenCode Zen/Go and Mistral subscription keys was not reviewed. Prices and quotas may be stale.

---

## Q11. (c) How should INIT implement subscription auth securely, with credentials held by a separate trusted process?

### Takeaway
Vendor guidance converges on one pattern. A privileged local process owns OAuth, token storage, refresh and requests; the UI and agent talk to it through a bridge that never exposes tokens; refreshes are serialized because refresh tokens rotate; tokens are revoked on sign-out; and client identity is honest. INIT should build a credential broker that injects auth into upstream requests, so the agent sandbox never holds or can read any credential.

### Cited Findings
- OpenAI's reference architecture: the main process handles authentication, "protected local storage", and API requests, and a bridge lets the UI call them "without receiving tokens". It uses a `127.0.0.1` callback with state, nonce and PKCE; the host ID goes in `chatgpt-host.json` and the client registration in `chatgpt-auth.json` — [OpenAI cookbook](https://developers.openai.com/cookbook/articles/sign-in-with-chatgpt)
- Write credential files atomically with mode `0600`, keep a separate record per issued client ID, never commit or log — [SIWC sign-in](https://developers.openai.com/siwc/token-sharing-open-source/sign-in). Keep tokens out of browser storage, URLs, logs, analytics and support transcripts; serialize refresh; revoke then clear — [SIWC accounts and sessions](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions)
- Codex supports `keyring`, `file`, `auto` and `ephemeral` credential stores, and warns against exposing Codex execution in untrusted environments — [Codex auth docs](https://learn.chatgpt.com/docs/auth)
- Claude Code stores credentials in the macOS Keychain or 0600 files, supports `apiKeyHelper` scripts that return keys from a vault, and isolates accounts with `CLAUDE_CONFIG_DIR` — [Claude Code authentication](https://code.claude.com/docs/en/authentication)
- The Copilot SDK supports a per-session `gitHubToken` for multi-identity servers — [Copilot SDK auth README](https://cdn.jsdelivr.net/gh/github/copilot-sdk@main/docs/auth/README.md)
- Impersonation is penalized: Kimi forbids User-Agent tampering ([Kimi membership guide](https://www.kimi.ai/help/kimi-code/membership-guide)); Anthropic blocked clients spoofing Claude Code headers ([paddo.dev (secondary)](https://paddo.dev/blog/anthropic-walled-garden-crackdown/))
- Anthropic forbids third parties to "collect, store, or intermediate" Claude.ai tokens — [legal and compliance](https://code.claude.com/docs/en/legal-and-compliance)

### Inferences
Proposed INIT design. This is my inference, not vendor text.
1. **`init-authd` broker** (separate OS process, separate user or sandbox profile where possible). It alone runs OAuth flows (SIWC PKCE loopback, GitHub device or OAuth App), reads and writes the OS keychain (macOS Keychain, libsecret, Windows Credential Manager) with a 0600 atomic-file fallback, and performs refresh with a cross-process lock (single-flight), because OpenAI refresh tokens rotate.
2. **Agent never sees secrets.** The agent sandbox calls the broker over a Unix socket or a loopback HTTP proxy with a per-session capability handle. The broker injects `Authorization`, strips auth headers from responses, and enforces per-provider budgets mirroring vendor caps (SIWC weekly cap, Copilot allowance, coding-plan 5-hour windows). The sandbox denies reads of `~/.codex`, `~/.claude`, `~/.gemini`, keychain APIs and the broker's store, and egress is restricted to the broker.
3. **Honest identity.** Use INIT's own `agent_name_hint`, User-Agent and integration IDs. Never spoof Claude Code, VS Code, Copilot or Codex identities.
4. **Per-provider modes.** OpenAI uses SIWC (native). Claude uses either a Console API key (stored in the broker) or the opt-in "local Claude Code" mode, where the broker spawns the unmodified `claude` binary under an INIT-owned `CLAUDE_CONFIG_DIR`, the user runs `/login` themselves, and INIT never reads that credential (GRAY). Copilot uses the Copilot SDK with INIT's OAuth App. Google uses a Gemini API key or Vertex only. Coding plans use API keys with an "interactive-only" flag where the vendor requires it (Alibaba, Kimi).
5. **Lifecycle.** Revoke on sign-out where the vendor supports it (OpenAI). Handle 401, 403 and 429 per vendor error contracts without OAuth retry loops. Show "Manage usage" links (chatgpt.com/settings/usage).

### Gaps
- I found no vendor guidance on remote or hosted brokers. OpenAI requires the interest form for "remotely hosted" apps, so a cloud-hosted INIT broker serving multiple users would need separate approval.
