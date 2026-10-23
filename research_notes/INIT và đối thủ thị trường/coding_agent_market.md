# Coding-agent and agentic-harness market, October 2026: incumbents, positioning, and the gap INIT could occupy

Scope note. Web research done 2026-10-22, plus reuse of earlier notes in this repo: `INIT v1 providers UX vision/subscription_plans.md` (2026-10-20), `install_daily_ux.md` (2026-10-20), `Agentic harness tự tiến hoá/production_harness_internals.md` and `minimal_harnesses.md` (2026-10-19), and `INIT novelty review/notes.md` (2026-10-20). Those files carry the full citations for anything marked **[repo note]**. GitHub star counts were pulled from the GitHub REST API on 2026-10-22. Labels: **(secondary)** means aggregator, blog or press summary, not the vendor. **(conflicting)** means sources disagree. **STALE** means the newest source found is more than about 4 months old. Quotes are kept under 15 words.

---

## Q0. Landscape: who the incumbents are, and how they position (pricing and model access, traction, platforms, extensions, sandbox, memory)

### Takeaway
The market has split three ways.
1. **Lab-owned harnesses that are bundled with subscriptions**: Claude Code, Codex, Antigravity (formerly Gemini CLI) and Copilot. They hold most of the users and revenue, and each one ties its flat-rate plan to its own client.
2. **Venture-scale closed agents with their own billing**: Cursor (now owned by SpaceX), Cognition (Devin plus Windsurf), Factory, Amp, Kiro and Warp. They compete on cloud and background agents, and on enterprise features.
3. **Open-source, model-agnostic harnesses.** OpenCode and Pi lead them. Cline, Kilo and Goose follow. Roo Code is dead, and Aider has largely stalled.

Every serious harness now ships MCP, skills (SKILL.md), hooks or rules, subagents, AGENTS.md support, and a markdown memory that the agent writes itself. None of them modifies its own compiled code.

### Cited Findings

#### Traction snapshot: GitHub REST API, 2026-10-22
| Repo | Stars | Forks | License | Last push | Note |
|---|---|---|---|---|---|
| [openclaw/openclaw](https://github.com/openclaw/openclaw) | 392.7k | 82.5k | MIT | 2026-10-22 | Personal agent built on Pi |
| [NousResearch/hermes-agent](https://github.com/NousResearch/hermes-agent) | 256.1k | 55.9k | MIT | 2026-10-22 | Self-improving personal agent |
| [anomalyco/opencode](https://github.com/anomalyco/opencode) | 219.9k | 29.4k | MIT | 2026-10-22 | Moved from sst/opencode |
| [anthropics/claude-code](https://github.com/anthropics/claude-code) | 152.8k | 26.8k | none declared | 2026-10-22 | Repo for issues and plugins. The client is closed |
| [openai/codex](https://github.com/openai/codex) | 130.4k | 20.6k | Apache-2.0 | 2026-10-22 | Rust |
| [earendil-works/pi](https://github.com/earendil-works/pi) | 117.1k | 15.0k | MIT | 2026-10-22 | Moved from badlogic/pi-mono |
| [google-gemini/gemini-cli](https://github.com/google-gemini/gemini-cli) | 107.5k | 14.8k | Apache-2.0 | 2026-10-22 | Retired for unpaid users (see Google below) |
| [OpenHands/OpenHands](https://github.com/OpenHands/OpenHands) | 91.5k | 12.2k | MIT | 2026-10-22 | |
| [cline/cline](https://github.com/cline/cline) | 70.7k | 7.7k | Apache-2.0 | 2026-10-22 | |
| [warpdotdev/warp](https://github.com/warpdotdev/warp) | 65.6k | 5.7k | AGPL-3.0 | 2026-10-22 | Client open-sourced in Apr 2026 |
| [aaif-goose/goose](https://github.com/aaif-goose/goose) | 55.5k | 6.4k | Apache-2.0 | 2026-10-22 | Moved from block/goose to the Linux Foundation's AAIF |
| [Aider-AI/aider](https://github.com/Aider-AI/aider) | 49.7k | 5.1k | Apache-2.0 | 2026-05-22 | Last release v0.86.0 on 2025-08-09 |
| [continuedev/continue](https://github.com/continuedev/continue) | 36.3k | 5.5k | Apache-2.0 | 2026-10-20 | |
| [charmbracelet/crush](https://github.com/charmbracelet/crush) | 28.7k | 2.3k | other | 2026-10-22 | |
| [QwenLM/qwen-code](https://github.com/QwenLM/qwen-code) | 28.6k | 3.2k | Apache-2.0 | 2026-10-22 | |
| [Kilo-Org/kilocode](https://github.com/Kilo-Org/kilocode) | 27.7k | 3.3k | MIT | 2026-10-22 | |
| [RooCodeInc/Roo-Code](https://github.com/RooCodeInc/Roo-Code) | 24.3k | 3.4k | Apache-2.0 | 2026-05-15 | **Archived** (last release v3.54.0) |
| [github/copilot-cli](https://github.com/github/copilot-cli) | 11.3k | 2.0k | other | 2026-10-22 | |
| [SWE-agent/mini-swe-agent](https://github.com/SWE-agent/mini-swe-agent) | 8.7k | 1.2k | MIT | 2026-10-19 | |
| [kirodotdev/Kiro](https://github.com/kirodotdev/Kiro) | 4.4k | 0.3k | none declared | 2026-10-18 | Issue tracker only |

#### Developer surveys
- Stack Overflow's April 2026 pulse survey put AI-agent use at 59%, up from 31%. Among agent users, Claude Code use rose from 41% to 55%. 69% use a single agent, and 68% prefer predictable single-agent setups. Among full-stack developers, the most common single-agent workflows are Copilot (65%) and Claude Code (50%). Daily multi-agent users pick Claude Code most often (70%). — [Stack Overflow blog, "Agents on a leash", May 27, 2026](https://stackoverflow.blog/2026/05/27/agents-on-a-leash-agentic-ai-remains-mostly-monitored-at-work)
- The main 2026 Stack Overflow survey drew 30,000+ responses from 169 countries. 65.9% of respondents use coding assistants or agents. 48% trust AI output when it is easy to verify. — [Visual Studio Magazine, Oct 6, 2026](https://visualstudiomagazine.com/articles/2026/10/06/stack-overflow-2026-survey-vs-code-leads-visual-studio-takes-third.aspx); [IT Brief (secondary)](https://itbrief.co.uk/story/stack-overflow-survey-shows-developers-trust-ai-with-caveats)
- One secondary source cites "JetBrains 2026": 46% of senior developers prefer Claude Code and 9% prefer Copilot. I did not verify this against JetBrains — [pasqualepillitteri.it (secondary)](https://pasqualepillitteri.it/en/blog/github-copilot-cursor-claude-code-ai-coding-showdown-2026)

#### Claude Code (Anthropic), closed source
- **Model access.** Three routes: a Claude subscription through claude.ai OAuth, a Console API key (`claude auth login --console`), or Bedrock, Google Cloud Agent Platform or Foundry behind a self-hosted `claude gateway` for SSO and policy — [Claude Code CLI reference](https://code.claude.com/docs/en/cli-reference). Since Oct 7, 2026, Max ($100 / $200 tiers) and Team plans include monthly API credits. Those credits do not cover Claude Code itself **[repo note: subscription_plans.md]**.
- **Traction.** Run-rate revenue passed $2.5B in Feb 2026, attributed to Anthropic's Series G announcement of Feb 12, 2026. More than half of it comes from enterprises. **STALE**: I found no newer Claude Code-specific figure — [getpanto (secondary)](https://www.getpanto.ai/blog/claude-ai-statistics). Anthropic's company-wide run rate was reportedly above $65B at the end of July 2026 (secondary, unverified) — [getpanto Anthropic stats (secondary)](https://www.getpanto.ai/blog/anthropic-ai-statistics)
- **Surfaces.** Terminal, IDE, Desktop app (macOS and x64 Windows), and cloud sessions (`--cloud`, `--teleport`). Local background agents run under a supervisor daemon (`claude agents`, `--bg`, `claude attach/stop/respawn`). There are also self-hosted cloud runners, agent teams, and `claude import codex`, which pulls in configuration from other agents — [CLI reference](https://code.claude.com/docs/en/cli-reference)
- **Extensions.** CLAUDE.md and AGENTS.md, path-scoped `.claude/rules/`, skills, hooks, subagents, MCP, and plugins with marketplaces (`claude plugin install code-review@claude-plugins-official`). Channels are a research preview — [CLI reference](https://code.claude.com/docs/en/cli-reference); [memory docs](https://code.claude.com/docs/en/memory)
- **Sandbox and permissions.** Seatbelt on macOS and bubblewrap on Linux, with an egress proxy outside the sandbox. Anthropic says this cuts permission prompts by 84%. The default interactive mode is an "Auto" transcript classifier. `--restricted` mode exists for evaluation harnesses. `--max-budget-usd` works in print mode only (details in Q3) **[repo note: production_harness_internals.md §7]**; [CLI reference](https://code.claude.com/docs/en/cli-reference)
- **Memory.** Auto memory is on by default (details in Q1) — [memory docs](https://code.claude.com/docs/en/memory)

#### OpenAI Codex (CLI, desktop app, cloud), open-source client
- **Model access.** A ChatGPT plan or an API key. Plans: Free, Go $8, Plus $20, Pro from $100. Pro tiers are listed as $100 / $200 / $500 in one source, and another says new $200 sign-ups were paused on Sep 10, 2026 **(conflicting)**. Limits are 5-hour windows plus weekly caps. Codex, ChatGPT Work and Workspace Agents share one agentic credit pool — [jetadmin (secondary)](https://www.jetadmin.io/blog/codex-pricing-current-costs-usage-limits-and-realistic-budgets-2026/); [developersdigest (secondary)](https://www.developersdigest.tech/blog/codex-usage-limits-pricing-2026); [OpenAI Help (search snippet)](https://help.openai.com/articles/20001106)
- **Traction.** Weekly active users: about 600k at the start of 2026, more than 4M by Apr 21, more than 5M on Jun 2 (an OpenAI figure, up more than 6x since the desktop app launched in February), and a reported 8M on Jul 14 (an executive post, secondary). Later "10M" figures combine Codex with ChatGPT Work — [Constellation Research](https://constellationr.com/insights/news/openai-touts-broadening-codex-usage-5-million-weekly-active-users); [gradually.ai (secondary)](https://gradually.ai/en/codex-statistics/); [FourWeekMBA (secondary)](https://fourweekmba.com/ai-ai-openai-codex-7-million-weekly-users-harness-war/)
- **Platforms and sandbox.** Seatbelt on macOS. On Linux, bubblewrap plus seccomp; the Landlock-only option is rejected for filesystem-restricted policies. Windows has dedicated sandbox crates. A guardian subagent ("Smart Approvals") handles approvals **[repo note: production_harness_internals.md §7]**
- **Memory.** AGENTS.md is capped at 32 KiB. "Memories" shipped as a preview on Apr 16, 2026. It is off by default, machine-local, and writes summaries of past sessions to `~/.codex/memories/` — [Daniel Vaughan (secondary)](https://codex.danielvaughan.com/2026/04/18/codex-built-in-memory-system-deep-dive/); [mem0 (secondary)](https://mem0.ai/blog/how-memory-works-in-codex-cli)

#### Google: Gemini CLI → Antigravity CLI, and Jules
- Google retired the open-source Gemini CLI for unpaid and Google One users on Jun 18, 2026. Its replacement is the closed-source Antigravity CLI (`agy`). Users complain about shared quotas and a new weekly cap **[repo note: install_daily_ux.md]**; [The Register](https://www.theregister.com/ai-ml/2026/05/20/bye-bye-gemini-cli-google-nudges-devs-toward-antigravity/5243605)
- Model access: Google login (free, AI Pro/Ultra, Code Assist), with third-party "piggyback" use banned, or a Gemini API key or Vertex AI **[repo note: subscription_plans.md Q4]**
- Antigravity permission modes: `request-review` (the default), `proceed-in-sandbox`, `always-proceed` and `strict`. Gemini CLI's macOS Seatbelt profiles and shadow-git checkpoints carry over **[repo note: install_daily_ux.md; production_harness_internals.md]**
- Jules: not researched this session (see Gaps).

#### Cursor (Anysphere, now SpaceX)
- **Pricing.** Hobby free, Pro $20, Pro+ $60, Ultra $200, Teams $40 per user. Each paid plan includes a usage pool, and Auto mode is unlimited. Pool sizes differ between sources **(conflicting)** — [jetadmin (secondary)](https://www.jetadmin.io/blog/cursor-pricing-explained-plans-credit-system-and-real-costs-in-2026/); [dev.to (secondary)](https://dev.to/rahulxsingh/cursor-pricing-in-2026-hobby-pro-pro-ultra-teams-and-enterprise-plans-explained-4b89). OpenAI's help page proposes winding down OpenAI models in Cursor on Nov 12, 2026 **[repo note: subscription_plans.md Q9]**
- **Traction and ownership.** ARR was about $1B in Nov 2025, $2B in Feb 2026, $3B in May 2026, and about $4B in June 2026 (secondary). SpaceX announced a $60B purchase right on Apr 21, 2026, and exercised it on Jun 16. Wikipedia says the deal closed on Aug 14, 2026; I did not confirm this against a primary source — [Wikipedia: Cursor (company)](https://en.wikipedia.org/wiki/Cursor_(company)); [tech-insider (secondary)](https://tech-insider.org/cursor-60-billion-valuation-anysphere-ai-coding-2026/)
- **Sandbox.** The agent runs terminal commands in a restricted environment on macOS, Linux and Windows; on macOS this is Seatbelt via `sandbox-exec`. Cursor chose a sandbox over allowlists so the agent can auto-run commands without network access — [Cursor blog: agent sandboxing](https://cursor.com/blog/agent-sandboxing). Two 2026 escapes are known. CVE-2026-73217 (CVSS 7.7) went through a venv Python wrapper run by the MS Python extension and was fixed in 3.1.2. CVE-2026-73218 went through Docker Desktop / Dev Containers and was fixed in 3.0.0 — [SentinelOne CVE-2026-73217](https://www.sentinelone.com/vulnerability-database/cve-2026-73217/); [CVE-2026-73218](https://www.sentinelone.com/vulnerability-database/cve-2026-73218/)
- **Memory and extensions.** See Q1 for Memories, Bugbot learned rules and the continual-learning plugin. Cursor supports the Agent Skills standard **[repo note: production_harness_internals.md §5]**
- **Cloud agents.** Up to 8 run in parallel. They reportedly create 35% of Cursor's internal merged PRs (vendor figure, secondary) — [Morph (secondary)](https://www.morphllm.com/cursor-background-agents)

#### Cognition: Devin, plus Windsurf (acquired July 2025)
- **Funding.** $400M at a $10.2B valuation in Sep 2025, then $1B at $26B in May 2026. Reports of a $40–48B round conflict — [Maginative](https://www.maginative.com/article/cognition-ai-raises-400-million-at-a-10-2-billion-valuation/); [Dealroom](https://dealroom.co/news/129831-cognition-raises-1b-at-26b-devin-maker-more-than-doubles-valuation/)
- **ARR.** $492M disclosed in May 2026, about $900M in Aug 2026, with a planned 2026 cash burn of about $800M. A $1B ARR claim dated Sep 25, 2026 is unverified — [Crypto Briefing (secondary)](https://cryptobriefing.com/cognition-900m-arr-800m-cash-burn/); [vcbeast (secondary)](https://vcbeast.com/companies/cognition)
- **Windsurf.** One secondary source says the IDE is now branded "Devin Desktop" (unverified) — [vcbeast (secondary)](https://vcbeast.com/companies/cognition)
- **Memory.** The most advanced shipped design: a git-backed memory drive with "dreaming". Details in Q1 — [Devin docs: memory](https://docs.devin.ai/product-guides/memory)

#### GitHub Copilot (agent mode, coding agent, CLI, SDK)
- **Traction.** Microsoft cited "50 million Copilot users" on its FY2026 Q4 call without defining the term (secondary). About 4.7M paid subscribers in Jan 2026 (secondary) — [Quash (secondary)](https://quashbugs.com/blog/ai-coding-assistant-market-share-statistics); [Second Talent (secondary)](https://secondtalent.com/resources/github-copilot-statistics)
- **Billing.** Usage-based since Jun 1, 2026. New individual Pro, Pro+ and Student sign-ups were paused in April 2026, and Opus was removed from Pro **[repo note: subscription_plans.md Q3]**
- **Coding agent.** You assign an issue to Copilot. It works in a sandboxed environment and opens a PR on a `copilot/` branch. CI on that PR needs human approval — [Morph (secondary)](https://www.morphllm.com/copilot-coding-agent). "Agent HQ" runs several vendors' agents side by side — [alexcloudstar (secondary)](https://alexcloudstar.com/blog/best-background-coding-agents-2026/)
- **Openness.** The Copilot SDK is GA, and there is a formal partnership with OpenCode (Jan 16, 2026) **[repo note: subscription_plans.md Q3]**

#### Amp (spun out of Sourcegraph, Dec 2025)
- Amp became an independent "frontier research lab" — [Tessl](https://tessl.io/blog/sourcegraph-spins-out-ai-coding-agent-amp-as-a-standalone-company/); [HackerNoon](https://hackernoon.com/sourcegraph-spins-out-amp-to-chase-the-ai-coding-frontier)
- **Pricing.** An ad-supported free tier launched in Oct 2025 — [Tessl](https://tessl.io/blog/amp-s-new-business-model-ad-supported-ai-coding). The ads were reportedly dropped in Mar 2026, and the free tier closed to new users in mid-2026 **(conflicting)** — [Ry Walker (secondary)](https://rywalker.com/research/amp-sourcegraph). Usage is now pay-as-you-go at zero markup over provider prices, with a $5 minimum. BYOK is enterprise-only — [tech-insider (secondary)](https://tech-insider.org/?p=18762)
- **Model access.** Users can link a Claude Pro/Max plan; Amp then runs through the Claude Agent SDK (Oct 10, 2026). ChatGPT sign-in unlocks OpenAI models. The product centres on cloud "orbs" plus a coordinating agent ("Puck") **[repo note: subscription_plans.md; install_daily_ux.md]**

#### Factory (Droids)
- **Funding.** $50M Series B (Sep 2025), $150M at $1.5B led by Khosla (Apr 2026), and a reported $200M at $5B (Sep 2026) — [Sacra](https://sacra.com/research/factory); [AF.NET summarizing Forbes (secondary)](https://af.net/realtime/ai-startup-factory-achieves-5-billion-valuation-following-200-million-funding-round/)
- **Revenue.** Sacra estimates about $60M ARR in Mar 2026, up from about $5M at the end of 2025. Factory says revenue doubled month over month for six months — [Sacra](https://sacra.com/research/factory)
- **Model access.** BYOK, included free up to an allowance on individual plans. Factory ran Terminal-Bench with permissions skipped **[repo note: subscription_plans.md Q9; production_harness_internals.md §7]**

#### Warp
- Warp open-sourced its client in Apr 2026 under AGPL, with the UI crates under MIT. The server and AI components stay proprietary. It is used by "close to a million developers", and OpenAI is the founding sponsor of the repo. Warp hosts third-party CLI agents (Claude Code, Codex, Gemini CLI, OpenCode), and steers contributions through its "Oz" cloud-agent platform — [It's FOSS](https://itsfoss.com/news/warp-goes-open-source.md); [I-Programmer](https://www.i-programmer.info/news/90-tools/18852-warp-the-open-source-agentic-development-environment-goes-open-source.html); [Help Net Security](https://www.helpnetsecurity.com/?p=368538)

#### Kiro (AWS)
- **Pricing.** Free, Pro $20, Pro+ $40, Pro Max $100, Power $200, plus Enterprise. Usage is metered in credits with per-model multipliers, from 0.05x (Qwen3 Coder Next) to 6x (Claude Fable 5.1). Overages are **off by default**. Plans now cover "teams up to 500" — [usagepricing.com (secondary, Oct 18, 2026)](https://www.usagepricing.com/blueprint/kiro); [Sep 2026 packaging change](https://www.usagepricing.com/blueprint/activity/kiro-2026-09-packaging)
- **Terms.** The free tier forbids routing usage through third-party harnesses such as OpenClaw — [usagepricing.com (secondary)](https://www.usagepricing.com/blueprint/kiro)
- **Product.** Spec-driven workflow (`.kiro/specs/`: requirements, design, tasks) and hooks fired by file events — [Medium guide (secondary)](https://medium.com/@arnab.kg.co.in/mastering-kiro-the-definitive-guide-to-spec-driven-ai-development-b14e82ff4ea5). The GPT-5.6 family was added — [Unite.AI](https://www.unite.ai/vi/openai-brings-gpt-5-6-model-family-to-awss-kiro/). There is no official user count. Customers are named only in AWS's one-year retrospective — [Kiro blog](https://kiro.dev/blog/one-year/)

#### Open-source harnesses
- **OpenCode** (Anomaly, the team behind SST)
  - Users: 8M monthly users in Jun 2026, per BetaKit, company-reported — [Let's Data Science](https://letsdatascience.com/news/opencode-reaches-eight-million-monthly-users-in-one-year-51652538). "16M monthly developers" claimed in Aug 2026 **(conflicting definitions)**.
  - Revenue: the $10 Go plan has about 301k accounts — [RuntimeWire](https://runtimewire.com/article/opencode-go-301000-active-subscribers). Reported run rate is $11M for Go alone, or about $18–25M in total **(conflicting)** — [arr.club](https://www.arr.club/opencode/opencode-hit-10m-mau-with-11m-arr-for-go-alone)
  - Product: build and plan agents, a permission engine, client/server design, models via models.dev. It scores 51.7% on TB2 with Opus 4.5. Its Claude Pro/Max plugins were removed in 1.3.0 **[repo note: minimal_harnesses.md; subscription_plans.md]**
- **Cline**
  - Funding: $32M across seed and Series A. The $27M A was led by Emergence at a $110M valuation (2025) — [Cline blog](https://cline.bot/blog/cline-raises-32m-series-a-and-seed-funding-building-the-open-source-ai-coding-agent-that-enterprises-trust); [Signalbase (secondary)](https://www.trysignalbase.com/news/funding/cline-secures-27m-in-series-a-funding-pioneering-the-future-of-autonomous-coding-agents)
  - Staff: in Jan 2026, OpenAI hired at least seven Cline staff and said it was not an acquisition (The Information, via Sacra) — [Sacra](https://sacra.com/c/cline)
  - Pricing: the Teams tier was removed in Apr 2026. Only the free open-source product and custom Enterprise remain (SSO, RBAC, limits on inference providers) — [usagepricing (secondary)](https://www.usagepricing.com/blueprint/cline); [Cline for Enterprise](https://cline.bot/blog/introducing-cline-for-enterprise)
  - Checkpoints: shadow-git checkpoints on by default **[repo note: cross_platform.md]**
- **Roo Code.** Shutdown announced Apr 21, 2026. The repo became read-only on May 15, which the GitHub API confirms as `archived: true`. Its cloud and router services closed. The team moved to "Roomote", a Slack agent. Roo points users to Cline, and Kilo publishes a migration guide — [Kilo: Roo shutdown](https://kilo.ai/compare/roo-code-shutdown-roomote); [Distill Intelligence](https://www.distillintelligence.com/news/roomote)
- **Kilo Code.** $8M seed (Dec 2025). Claims 3M+ users. GitLab holds a right of first refusal on any acquisition through Aug 2026 — [Ry Walker (secondary)](https://rywalker.com/research/kilo)
- **Aider.** The last feature release was v0.86.0 (Aug 2025; GitHub API). The leaderboards were last updated Nov 2025 (**STALE**). The project claims 6.8M pip installs. It is still community-maintained — [Ry Walker (secondary)](https://rywalker.com/research/aider)
- **Goose.** A founding project of the Linux Foundation's Agentic AI Foundation (AAIF, Dec 2025), alongside MCP and AGENTS.md — [InfoQ](https://infoq.com/news/2025/12/agentic-ai-foundation/). Block moved it because it still owned the trademarks, which hampered enterprise adoption — [The New Stack](https://thenewstack.io/block-goose-agentic-foundation/). Goose's optional sandbox is macOS-only. Providers include sign-in with a ChatGPT subscription **[repo note: cross_platform.md; install_daily_ux.md]**
- **mini-SWE-agent.** About 100 lines, bash only. With DeepSeek V3.2 at high reasoning it scores 70.0% on SWE-bench Verified at $0.448 per instance. The V3.2 Reasoner scores 60.0% at $0.028 per instance **[repo note: harness_benchmarks / swe_bench_family.md]**
- **Pi.** 117k stars. A minimal four-tool core with extensions. OpenClaw is built on it **[repo note: minimal_harnesses.md]**

### Inferences
Summary matrix, synthesized from the findings above. n/v = not verified this session.

| Harness | Model access | Traction signal | Open? | Extension model | Sandbox | Self-written memory |
|---|---|---|---|---|---|---|
| Claude Code | Claude sub / API key / clouds | >$2.5B run rate (Feb 26) | No | MCP, skills, hooks, subagents, plugins+marketplaces, agent teams | Seatbelt/bwrap + proxy, Auto classifier | Yes, on by default |
| Codex | ChatGPT plan / API key | 5M WAU (Jun 26) | Yes (Apache-2.0) | AGENTS.md, skills, MCP, plugins | Seatbelt / bwrap+seccomp / Windows | Yes, preview, off by default |
| Antigravity CLI | Google login / Gemini key | n/v | No (Gemini CLI was open) | GEMINI.md/AGENTS.md, skills, MCP | Seatbelt profiles | n/v |
| Cursor | Cursor plan (credits) | ~$3–4B ARR | No | rules, skills, MCP, plugins | Seatbelt etc., 2 CVEs | Yes (Memories, Bugbot rules) |
| Devin/Windsurf | Cognition plans | ~$0.9–1B ARR | No | knowledge, MCP | cloud VM | Yes (git memory + dreaming) |
| Copilot | Copilot plan (credits) | "50M users" | CLI no; SDK GA | MCP, agents, SDK | cloud sandbox for coding agent | n/v |
| Amp | pay-as-you-go 0% markup; Claude/ChatGPT plans | n/v | No | n/v | cloud orbs | n/v |
| Factory | BYOK / plans | ~$60M ARR est. (Mar 26) | No | n/v | n/v | n/v |
| Warp | Warp plans | ~1M devs | Client AGPL | hosts other CLI agents | n/v | n/v |
| Kiro | AWS credits | n/v | No | specs, hooks, MCP (n/v) | n/v | n/v |
| OpenCode | any provider + Go/Zen + Copilot | 8–16M MAU (claimed) | MIT | plugins, agents, MCP | permission engine | n/v |
| Cline / Kilo | BYO key | millions (claimed) | Apache/MIT | MCP, rules | checkpoints | n/v |
| Goose | any provider | 55k stars | Apache (AAIF) | MCP extensions | macOS sandbox only | n/v |
| Pi / mini-SWE | any provider | 117k / 8.7k stars | MIT | extensions / none | none built in | no |

- The lab harnesses win on distribution because they are bundled into subscriptions. The venture agents win on cloud execution and enterprise features. The open-source harnesses win on model choice and community. INIT, which is macOS-only, DeepSeek-only and has no cloud, competes in none of these three races. Its only distinctive axis is governed self-modification (Q1, Q3).

### Gaps
- Jules (Google's async agent): pricing and traction were not researched.
- Windsurf as a separate product in Oct 2026: its pricing, its memories, and whether it is now "Devin Desktop" are unverified.
- OpenHands: no funding, user or cloud-pricing data was gathered.
- Copilot's per-plan prices after Jun 1, 2026, and Antigravity CLI's tier prices, were not retrieved.
- Platform support for Kiro, Amp, Factory and Warp was not verified this session.
- Claude Code has no revenue figure newer than Feb 2026.

---

## Q1. Do incumbents already ship "the agent improves itself or its setup over time"? How close is that to INIT's self-modification?

### Takeaway
Yes, in a narrow sense. All four leaders now ship agent-written, persistent **text** that changes future behaviour:
- Claude Code: auto memory
- Devin: git-backed memory with daily "dreaming"
- Cursor: Memories and Bugbot learned rules
- Codex: Memories, in preview

Claude and Devin both add a background consolidation pass. On top of that, skills and plugin marketplaces let agents author reusable procedures.

None of them changes its own executable code, and none gates a self-change behind a held-out evaluation. These features are context injection, and the vendors themselves say memory is not enforced configuration. The closest projects to INIT are open-source or research: Hermes Agent's self-evolution, Google's Harness², MOSS and Ouroboros.

### Cited Findings

**Claude Code**
- Two memory systems. CLAUDE.md is written by the user. Auto memory is "notes Claude writes itself based on your corrections and preferences".
- Auto memory is on by default in local sessions and is stored per repository at `~/.claude/projects/<project>/memory/`, with `MEMORY.md` as the index.
- The first 200 lines or 25KB load every session. Near the limit, Claude Code prompts the agent to merge or drop stale entries. Subagents can keep their own memory.
- The docs say memory is treated "as context, not enforced configuration". To block an action, use a PreToolUse hook instead.
- `/doctor prompt-audit` checks instruction files for outdated or conflicting content.
- Source: [Claude Code memory docs](https://code.claude.com/docs/en/memory)

**Claude "Auto-dream" and "Dreaming"**
- Auto-dream was community-reported around v2.1.83 (Mar 2026). It is a background sub-agent that consolidates memory and keeps MEMORY.md under 200 lines, gated by a server-side flag. Anthropic has not documented it — [Tessl](https://tessl.io/blog/anthropic-tests-auto-dream-to-clean-up-claudes-memory/); [SFEIR (secondary)](https://institute.sfeir.com/fr/articles/claude-code-dream-auto-dream-consolidation-memoire/)
- At Code w/ Claude (May 2026), Anthropic presented "dreaming" for Managed Agents, which verifies and enriches memory between sessions — [Anthropic session page](https://claude.com/code-with-claude/session/sf-memory-and-dreaming-for-self-learning-agents)

**Devin (Cognition)**
- Memory is a "memory drive": a persistent git repository of markdown notes, with MEMORY.md given as context in every session.
- Devin saves preferences, corrections, decisions and repository notes on its own. A new memory applies only to later sessions.
- A background "dreaming" session runs about daily to merge, prune and reorganize.
- Memory is on by default, private per user, and can be toggled by an admin.
- It is built on an open spec, "Agent Memory Repo".
- Source: [Devin docs: memory](https://docs.devin.ai/product-guides/memory)

**Cursor**
- Memories are "automatically generated rules" from chat. A separate model extracts them in the background, or the agent creates them through a tool call. They can be reviewed in Settings → Rules — [Cursor docs: memories](https://docs.cursor.com/context/memories)
- "Bugbot now self-improves with learned rules" was published Apr 8, 2026. Contents not read — [plushcap index of Cursor blog (secondary)](https://www.plushcap.com/companies/cursor/blog/2026)
- A "continual-learning" plugin in cursor/plugins keeps AGENTS.md updated through a subagent. A Cursor staff prompt (Apr 13, 2026) mines past JSONL transcripts into `.cursor/rules` or `.cursor/skills` for human review — [skillselion (secondary)](https://skillselion.com/skills/cursor/plugins/continual-learning)

**Codex**
- AGENTS.md is the static, human-written layer. Memories (preview, Apr 16, 2026) summarizes past sessions into `~/.codex/memories/`. It is off by default and local to one machine. OpenAI tells teams to keep required guidance in AGENTS.md — [Daniel Vaughan (secondary)](https://codex.danielvaughan.com/2026/04/18/codex-built-in-memory-system-deep-dive/); [mem0 (secondary)](https://mem0.ai/blog/how-memory-works-in-codex-cli)

**Cross-harness convergence**
- Skills in SKILL.md format appear in 9 of 11 audited harnesses, against MCP in 8 of 11. Behavioural policy is moving "from prompt prose to configuration" **[repo note: production_harness_internals.md §5, arXiv 2609.00006]**

**Personal agents with self-authored skills**
- Hermes Agent writes a markdown skill file after tasks that take five or more tool calls, and stores skills in SQLite FTS5 for reuse — [Tensorlake (secondary)](https://www.tensorlake.ai/blog/hermes-coding-agent-learns-from-use); [fast.io (secondary)](https://fast.io/resources/hermes-ai/)
- The experimental "Hermes Agent Self-Evolution" uses DSPy and GEPA to evolve skills, tool descriptions and "the agent's own code" — [fast.io (secondary)](https://fast.io/resources/hermes-ai/)
- Hermes has no public Terminal-Bench 2.0 result. Some of its community praise came from very new accounts — [pasqualepillitteri.it (secondary)](https://pasqualepillitteri.it/en/news/3864/hermes-agent-review-community-opinions)
- Stars grew from 57k (Apr 11) to about 191k (Jun 12), and stood at 256k on Oct 22 (GitHub API) — [gradually.ai (secondary)](https://www.gradually.ai/en/hermes-agent-statistics/)

**Research and outer-loop optimizers**
- Google's Harness² (Oct 2026, 12.8k stars) rewrites harness files (prompts, memory, skills, plugins) around a frozen model inside OpenCode or Codex. Selection is score-blind by construction — [github.com/google-research/harness2](https://github.com/google-research/harness2) **[repo note: Google harness2/notes.md]**
- MOSS (May 2026) edits OpenClaw's own source, replays failures in trial containers, and swaps in place behind a health probe. Ouroboros (Aug 2026) uses a launcher with panic-stop and an external spend cap **[repo note: INIT novelty review/notes.md]**
- OpenClaw agents can edit their own config and code and trigger restarts. Issues report that a restart interrupts in-flight turns, and that an agent deadlocked rebuilding its live `dist/` **[repo note: INIT novelty review/notes.md]**

### Inferences

| Capability | Shipped by incumbents? | INIT |
|---|---|---|
| Agent-written persistent notes / rules | Yes: Claude Code (default on), Devin (default on), Cursor, Codex (preview) | Comparable, not a differentiator |
| Background consolidation ("dreaming") | Devin (documented); Claude (Managed Agents preview; Code auto-dream flagged) | Not a differentiator |
| Agent-authored skills / procedures | Hermes (automatic); Cursor (via plugin/prompt); Claude/Codex (skills are plain files the agent can write) | Not a differentiator |
| Agent edits its own harness **code** | No incumbent. OpenClaw (unsafe restarts), Hermes Self-Evolution (experimental), MOSS/Ouroboros (research) | INIT's core |
| Hot-swap of compiled code mid-session, keeping the prompt cache | None found | INIT's core |
| Sealed eval gate + statistical promotion + rollback for self-changes | None in products. Research only (Harness², MOSS health probe, Gated-QD) | INIT's core |
| Immutable root holding kill switch and money ceiling, outside the agent's reach | None in products. Ouroboros has an external spend cap (research) | INIT's core |

- In practice the incumbents are 1–2 steps away. Their learning edits text that the model reads, and the user can audit it, so a bad memory costs some adherence, not system integrity. Each vendor could add "agent-written hooks or plugins", since plugins are just files. What none of them has is the governance layer: a sealed evaluation, promotion and rollback, and an untouchable root. That layer is what makes self-modification of code tolerable.
- Expect "memory + dreaming" to become table stakes by 2027. INIT should not pitch "learns over time". It should pitch "self-changes that are measured, gated and reversible".

### Gaps
- Windsurf's "Memories" (auto-generated) was not verified against current docs.
- I found no published evaluation of whether Claude, Cursor or Devin memory improves task success. These vendors claim no measured gain.
- The content of Cursor's Bugbot learned-rules post was not read.

---

## Q2. Which harnesses are open-source and model-agnostic, how big are their communities, and what made the successful ones grow?

### Takeaway
The open-source, model-agnostic leaders by stars are OpenCode (220k), Pi (117k), OpenHands (92k), Cline (71k), Goose (56k), Aider (50k) and Kilo (28k). Personal agents built on these ideas are bigger still: OpenClaw (393k) and Hermes (256k).

Codex is open source but tied to OpenAI's models. Gemini CLI was open, but Google has de-emphasized it in favour of the closed Antigravity CLI.

Growth came from four things, in different mixes:
- Polished UX with any-provider support (OpenCode, Pi).
- A cheap bundled plan or a subscription partnership (OpenCode Go and Copilot).
- Being early with a signature idea (Aider's git auto-commit and repo map; Cline's IDE-native plan/act).
- Neutral governance (Goose to the AAIF).

Survival is fragile. Roo Code shut down, Aider has stalled, and Cline lost staff to OpenAI.

### Cited Findings
- Stars and licenses: see the Q0 table (GitHub REST API, 2026-10-22).
- **OpenCode**
  - Stars rose from about 160k (mid-2026) to about 199k (Aug 20) and 219.9k (Oct 22) — [tech-insider (secondary)](https://tech-insider.org/opencode-160k-github-stars-2026/); [RuntimeWire](https://runtimewire.com/article/opencode-go-301000-active-subscribers)
  - The Go plan ($10) has 301k accounts, about 1.9% of the claimed monthly audience — [RuntimeWire](https://runtimewire.com/article/opencode-go-301000-active-subscribers)
  - GitHub formally partnered with OpenCode for Copilot subscriptions (Jan 16, 2026). It is the only third-party harness with such a deal **[repo note: subscription_plans.md]**
  - The founders previously built SST, OpenNext, OpenAuth and models.dev — [RuntimeWire](https://runtimewire.com/article/opencode-go-301000-active-subscribers)
- **Cline.** Sacra describes the model: the free product spreads one engineer at a time, then paid tiers sell the controls security and IT need. Cline's pitch is "open source AI coding agent that enterprises trust" — [Sacra](https://sacra.com/chat/h/8fbded68-09bf-4521-bcda-3ea5c9e0e337/); [Cline blog](https://cline.bot/blog/cline-raises-32m-series-a-and-seed-funding-building-the-open-source-ai-coding-agent-that-enterprises-trust)
- **Aider** pioneered automatic git commits, repository maps and architect/editor workflows, which are now common elsewhere. It is free and model-agnostic. Feature work slowed after mid-2025 — [Ry Walker (secondary)](https://rywalker.com/research/aider)
- **Goose.** Moving to the AAIF fixed a governance and trademark problem that had hampered enterprise adoption — [The New Stack](https://thenewstack.io/block-goose-agentic-foundation/). It is described as "local-first" and MCP-based — [DevOps Digest](https://devopsdigest.com/linux-foundation-announces-the-formation-of-the-agentic-ai-foundation-aaif)
- **Pi.** A minimal core of four tools and an extension system. Its creator rejects heavy scaffolding. It became the base of OpenClaw **[repo note: minimal_harnesses.md Q1]**
- **Hermes.** Added about 134k stars in nine weeks (Apr–Jun 2026). Briefly #1 in OpenRouter daily tokens (224B in one day). Some praise came from new accounts — [gradually.ai (secondary)](https://www.gradually.ai/en/hermes-agent-statistics/); [pasqualepillitteri.it (secondary)](https://pasqualepillitteri.it/en/news/3864/hermes-agent-review-community-opinions)
- **Attrition**
  - Roo Code shut down in May 2026 — [Kilo](https://kilo.ai/compare/roo-code-shutdown-roomote)
  - OpenAI hired at least 7 Cline staff (Jan 2026) — [Sacra](https://sacra.com/c/cline)
  - Google moved free users from open Gemini CLI to the closed `agy` (Jun 2026), drawing criticism for closing a tool built on open-source contributions **[repo note: install_daily_ux.md]**
- **Openness moving the other way:** Warp open-sourced its client (AGPL, Apr 2026) — [It's FOSS](https://itsfoss.com/news/warp-goes-open-source.md). Codex's client is Apache-2.0 Rust (GitHub API).
- **Benchmarks.** On the TB2 leaderboard with Opus 4.5, open harnesses sit close to the vendor ones: Goose 54.3, Claude Code 52.1, OpenHands 51.9, OpenCode 51.7. Droid (Factory) leads at 63.1 **[repo note: minimal_harnesses.md]**

### Inferences
- On this evidence, "model-agnostic" is table stakes for open-source harnesses. Every one that grew lets users bring any provider, and OpenCode added cheap first-party plans on top. A DeepSeek-only INIT does not meet that bar for community growth. It can still credibly be a research artifact.
- Communities grew around one memorable idea and an install that takes minutes (Aider's git commits, Pi's minimalism, Hermes's "grows with you", OpenCode's TUI). For INIT, "the agent that safely rewrites itself" is a memorable idea. A roughly 4 GB Rust toolchain install is the opposite of minutes **[repo note: install_daily_ux.md]**.
- Revenue for open-source harnesses is small next to the vendors (OpenCode about $11–25M against Cursor's billions). It comes from bundled inference plans or enterprise governance, not from the harness itself.

### Gaps
- I found no rigorous, independent analysis of why OpenCode grew. The drivers above are inferred from product facts and secondary write-ups.
- Cline's current user count is claimed only (an "11 million users" migration post was not confirmed).
- OpenHands's community drivers and commercial traction were not researched.

---

## Q3. Safety and permissions: what is the state of the art, and is anyone selling "safe autonomy"?

### Takeaway
The state of the art is OS-level sandboxing of the workspace and network, plus a classifier-gated auto-approval mode:
- Seatbelt on macOS: Claude Code, Codex, Cursor, Gemini CLI/Antigravity, Goose.
- Bubblewrap plus seccomp on Linux: Codex, Claude Code.
- An egress proxy that holds credentials outside the sandbox.
- Shadow-git or snapshot checkpoints.

Spend controls are weak for individual users:
- Claude Code's `--max-budget-usd` works only in print mode and checks a client-side estimate.
- Kiro disables overages by default.
- Hard caps otherwise come from enterprise admin consoles (Coder, Cline Enterprise, Kiro), from company policy (Uber), or from third-party add-ons.

Nobody sells a tamper-proof kill switch or money ceiling as the product's identity. "Safe autonomy" is marketed in two ways: as fewer prompts (Claude Auto mode, Cursor's sandbox auto-run) and as enterprise governance (Cline).

### Cited Findings
**Claude Code**
- Seatbelt or bubblewrap jail plus a proxy; 84% fewer prompts.
- Auto mode uses a two-stage classifier that sees only user messages and tool calls. Results: 0.4% false positives on real traffic, 17% false negatives on real overeager actions, 5.7% false negatives on synthetic exfiltration.
- Checkpoints are file snapshots. Remote side effects cannot be undone.
- Source: **[repo note: production_harness_internals.md §7, citing anthropic.com/engineering]**

**Claude Code spend and kill controls**
- `--max-budget-usd` stops a run once "estimated spend" reaches the cap. It works in print mode only. "Spend can pass the cap", and spend restored from resumed runs does not count.
- `--max-turns` also works in print mode only.
- `claude stop`/`kill` and `daemon stop` control background sessions.
- `--restricted` removes command-running tools and refuses `bypassPermissions`.
- Source: [CLI reference](https://code.claude.com/docs/en/cli-reference)

**Codex**
- Bubblewrap plus seccomp, and `io_uring` is blocked in all modes. The Landlock-only option is rejected for filesystem-restricted policies. The guardian subagent can approve, deny or escalate **[repo note: production_harness_internals.md §7]**
- Roundups credit Codex with the "strongest security (OS-level sandboxing)" (vendor-adjacent sources) **[repo note: install_daily_ux.md]**

**Cursor**
- Cursor built a sandbox so the agent can run commands automatically "without network access". On macOS this is Seatbelt — [Cursor blog](https://cursor.com/blog/agent-sandboxing)
- Two 2026 escapes through local tools: a venv Python wrapper, and Docker Desktop / Dev Containers — [SentinelOne CVE-2026-73217](https://www.sentinelone.com/vulnerability-database/cve-2026-73217/); [CVE-2026-73218](https://www.sentinelone.com/vulnerability-database/cve-2026-73218/)
- Forum users report a sandbox that cannot be killed (seccomp blocking kill signals) and init failures on macOS 26 — [Cursor forum, sandbox tag](https://forum.cursor.com/tag/sandbox/45)

**Copilot coding agent.** Runs in a sandbox, and CI on agent PRs needs human approval — [Morph (secondary)](https://www.morphllm.com/copilot-coding-agent)

**Enterprise spend caps**
- Uber reportedly caps employees at $1,500 per month per AI coding tool (Bloomberg, via secondary). Tesla reportedly caps at $200 per week — [Hyrax (secondary)](https://hyrax.dev/blog/ai-coding-budget-caps-uber-enterprise-rate-limiting); [Startup Fortune (secondary)](https://startupfortune.com/uber-puts-ai-coding-agents-on-a-monthly-budget/)
- Coder ships admin dollar limits per day, week or month, with user and group overrides — [Coder docs](https://coder.com/docs/@v2.35.3/ai-coder/agents/platform-controls/usage-insights)
- A search found no native per-seat hard cap or global kill switch documented by Anthropic, GitHub or Cursor. Third-party tools fill the gap: AgentFleet, an open-source hard dollar or token cap for Claude Code; AgentGuard — [AI Weekly](https://aiweekly.co/alerts/agentfleet-adds-hard-spend-limits-to-claude-code-cli); [Kunal Ganglani](https://www.kunalganglani.com/blog/ai-agent-kill-switch-spend-limits)

**Kiro.** Overages are disabled by default and must be enabled in the console — [usagepricing.com (secondary)](https://www.usagepricing.com/blueprint/kiro)

**Cline Enterprise.** Sells SSO (WorkOS), RBAC, central billing and "the ability to limit inference providers" — [Cline for Enterprise](https://cline.bot/blog/introducing-cline-for-enterprise); [Cline security docs](https://docs.cline.bot/enterprise-solutions/security-concerns)

**Developer demand.** 60% of agent users block agents from making unapproved system changes. 68% prefer predictable single-agent setups — [Stack Overflow blog](https://stackoverflow.blog/2026/05/27/agents-on-a-leash-agentic-ai-remains-mostly-monitored-at-work)

**Benchmark practice.** Factory and Warp ran Terminal-Bench with permissions skipped **[repo note: production_harness_internals.md §7]**

### Inferences
- The kill switch and money ceiling in INIT's Root address a real, unserved gap for individual developers. Every vendor cap is one of three things: an estimate checked client-side, an admin console aimed at enterprises, or a plan-level setting the agent cannot see. None is framed as "the agent cannot raise its own budget, even by rewriting itself", and none needs to be, because no incumbent agent can rewrite itself.
- The Cursor CVEs show that the sandbox perimeter leaks through other tools the user has installed (Docker, IDE extensions). For a self-modifying agent, the Root and the sandbox must also assume the agent will try to find such paths. INIT's sealed gate should include an escape-attempt red-team set.
- "Safe autonomy" as a positioning is only lightly contested. The incumbents sell convenience, meaning fewer prompts. INIT could sell governance for a single person: hard money ceilings, a physical stop, and a signed lineage of self-changes. That is close to Cline's enterprise pitch, but for one owner.

### Gaps
- No published false-positive/false-negative rates were found for Codex's guardian or Cursor's auto-run classifier.
- Enterprise kill-switch features in Claude Enterprise, Copilot Business and Cursor Enterprise admin consoles were not read directly.
- The Uber and Tesla caps rest on secondary reports of Bloomberg.

---

## Q4. Subscription economics: which harnesses can use which subscriptions under the ToS, and why does that lock users in?

### Takeaway
Flat-rate plans are subsidized relative to API prices, so each lab keeps its plan inside its own harness.
- **Anthropic.** Third-party use of claude.ai OAuth is prohibited. Enforcement started Jan 9, 2026, and "all third-party harnesses" were cut off on Apr 4, 2026. Partial carve-outs exist: the Agent SDK and the unmodified binary, the paused credit, and API credits for Max and Team since Oct 7, 2026.
- **Google.** Consumer OAuth for third parties is prohibited (bans in Feb 2026).
- **AWS Kiro.** The free tier forbids third-party harnesses.
- **GitHub.** Copilot is open only to formally approved partners (OpenCode) and to SDK apps.
- **OpenAI** is the exception. "Sign in with ChatGPT" explicitly allows open-source and local apps.

The result: users who pay $20–200 a month to a lab are pushed toward that lab's harness, and an open-source harness must run on pay-per-token API keys, the OpenAI route, or cheap third-party coding plans.

### Cited Findings
- The full verdict matrix, with primary citations, is in **[repo note: subscription_plans.md Q0–Q3]**:
  - OpenAI SIWC: ALLOWED for open-source and local apps (preview).
  - Claude OAuth in third-party apps: PROHIBITED. Driving the unmodified `claude` binary under the user's own login: GRAY.
  - Claude Console key, including Max/Team credits: ALLOWED.
  - Copilot SDK with the app's own GitHub OAuth app: ALLOWED. Raw Copilot endpoints: GRAY.
  - Gemini consumer OAuth: PROHIBITED. Gemini API key: ALLOWED.
  - Z.ai: only in supported tools. Alibaba Coding Plan: GRAY.
  - MiniMax, Cerebras, Ollama key plans: ALLOWED.
- Anthropic: "Claude subscriptions will no longer cover usage on third-party tools" (Apr 4, 2026), and the policy "applies to all third-party harnesses" — [TechCrunch, Apr 4, 2026](https://techcrunch.com/2026/04/04/anthropic-says-claude-code-subscribers-will-need-to-pay-extra-for-openclaw-support/). It was partly walked back on Jun 15–16: the Agent SDK, `claude -p` and ACP keep working with subscriptions — [Zed blog](https://zed.dev/blog/anthropic-subscription-changes)
- Amp now lets users link a Claude Pro/Max plan, but only by running the Claude Agent SDK rather than Amp's own agent (Oct 10, 2026). This shows the lab's runtime travels with the subscription — [Amp news](https://ampcode.com/news/use-your-claude-plan)
- Kiro's free tier bars "third-party automation harnesses (such as OpenClaw)" — [usagepricing.com (secondary)](https://www.usagepricing.com/blueprint/kiro)
- Codex, ChatGPT Work and Workspace Agents draw on one shared agentic credit pool, so OpenAI's subscription is itself an agent-platform bundle — [OpenAI Help (search snippet)](https://help.openai.com/articles/20001106)
- Copilot moved every plan to metered AI credits on Jun 1, 2026, because agent workloads raised costs (secondary) — [Morph (secondary)](https://www.morphllm.com/copilot-coding-agent); **[repo note: subscription_plans.md Q3]**
- Cursor's credit pools equal the plan price; heavy users of manually selected frontier models exhaust them and pay overage (secondary) — [jetadmin (secondary)](https://www.jetadmin.io/blog/cursor-pricing-explained-plans-credit-system-and-real-costs-in-2026/)
- API-side prices **[repo note: llm_api_providers.md]**:
  - Claude Opus 5.5: $4 / $20 per million tokens.
  - Sonnet 5: $2 / $10.
  - DeepSeek V4.1-Flash: about $0.15–0.30 in, $0.60–1.20 out.
  - mini-SWE-agent with DeepSeek V3.2 Reasoner: 60% on SWE-bench Verified at about $0.03 per task **[repo note: swe_bench_family.md]**

### Inferences
- **Why lock-in works.** The subscription is the cheapest way for heavy users to buy frontier tokens. The lab captures the harness, its telemetry, its cache behaviour and its upsell (cloud agents, Work). Third-party harnesses have to sell either model choice or something the lab harness structurally won't offer.
- **INIT's DeepSeek-only, API-key model sidesteps the ToS question entirely,** and DeepSeek's price makes long self-evaluation loops affordable. The cost is that INIT cannot ride the $20–200 subscriptions most developers already pay for. The only clean bridges are OpenAI SIWC (allowed for open source) and API keys funded by Claude Max/Team credits **[repo note: subscription_plans.md]**.
- Expect more "runtime travels with the subscription" deals (Amp with the Claude Agent SDK, the Copilot SDK). These make a third-party harness a shell around the lab's agent. That is incompatible with INIT's self-modifying core loop.

### Gaps
- Whether Amp's Claude linking is "previously approved" by Anthropic is unverified.
- Current ChatGPT Pro tier structure is **(conflicting)**: $100/$200/$500 tiers, or $200 sign-ups paused.

---

## Q5. Market trends in 2026: consolidation, lock-in, background and cloud agents, multi-agent, long-running agents, pricing pressure

### Takeaway
2026 brought five shifts:
- **Consolidation.** SpaceX bought Cursor, Cognition holds Windsurf, Roo Code died, OpenAI hired away part of Cline, Goose went to a foundation, and Amp was spun out.
- **Closing of lab harnesses and subscriptions.** Gemini CLI went closed as `agy`, and Anthropic's OAuth crackdown followed.
- **Background, cloud and long-running agents went mainstream.** Claude Code background sessions and self-hosted runners, Codex cloud, Cursor cloud agents, the Copilot coding agent and Agent HQ, Amp orbs, Warp Oz and Devin all ship them. Most still return a PR for human review.
- **A move from flat-rate to metered credits**, under pressure from agent token burn: Copilot (Jun 1), Cursor pools, Kiro credits and Amp's zero-markup pass-through. Enterprises now impose budget caps.
- **Multi-agent ships but is a minority practice.** Most developers still run a single agent.

### Cited Findings
- **Consolidation and exits**
  - Cursor and SpaceX: a $60B purchase right (Apr 21), exercised Jun 16 — [Wikipedia](https://en.wikipedia.org/wiki/Cursor_(company))
  - Cognition bought Windsurf (Jul 2025) and raised $1B at $26B (May 2026) — [Dealroom](https://dealroom.co/news/129831-cognition-raises-1b-at-26b-devin-maker-more-than-doubles-valuation/)
  - Roo Code shut down — [Kilo](https://kilo.ai/compare/roo-code-shutdown-roomote)
  - OpenAI hired Cline staff — [Sacra](https://sacra.com/c/cline)
  - Goose went to the AAIF — [InfoQ](https://infoq.com/news/2025/12/agentic-ai-foundation/)
  - Amp was spun out — [Tessl](https://tessl.io/blog/sourcegraph-spins-out-ai-coding-agent-amp-as-a-standalone-company/)
  - Factory raised at $5B (Sep 2026) — [AF.NET (secondary)](https://af.net/realtime/ai-startup-factory-achieves-5-billion-valuation-following-200-million-funding-round/)
- **Revenue concentration.** Cursor at about $3–4B ARR (secondary), Claude Code at more than $2.5B (Feb), and Cognition at about $0.9–1B. Most open-source harnesses earn under $25M — sources in Q0.
- **Lock-in moves**
  - Anthropic: Jan 9 / Apr 4 restrictions — [TechCrunch](https://techcrunch.com/2026/04/04/anthropic-says-claude-code-subscribers-will-need-to-pay-extra-for-openclaw-support/)
  - Gemini CLI → Antigravity CLI — [The Register](https://www.theregister.com/ai-ml/2026/05/20/bye-bye-gemini-cli-google-nudges-devs-toward-antigravity/5243605)
  - Kiro's free-tier ban on third-party harnesses — [usagepricing.com](https://www.usagepricing.com/blueprint/kiro)
  - Counter-moves: OpenAI's SIWC for open source, and GitHub's partnership with OpenCode **[repo note: subscription_plans.md]**
  - Claude Code ships `claude import codex` to import competitors' configuration, which makes switching easier in one direction — [CLI reference](https://code.claude.com/docs/en/cli-reference)
- **Background and cloud agents**
  - Claude Code: background agents under a supervisor daemon, cloud sessions, and self-hosted environments — [CLI reference](https://code.claude.com/docs/en/cli-reference)
  - Cursor: cloud agents, up to 8 in parallel — [Morph (secondary)](https://www.morphllm.com/cursor-background-agents)
  - Copilot: the coding agent, plus Agent HQ for side-by-side agents — [alexcloudstar (secondary)](https://alexcloudstar.com/blog/best-background-coding-agents-2026/)
  - Amp: orbs, plus the coordinating agent Puck **[repo note: install_daily_ux.md]**
  - Warp: Oz cloud orchestration — [I-Programmer](https://www.i-programmer.info/news/90-tools/18852-warp-the-open-source-agentic-development-environment-goes-open-source.html)
  - Ona argues agents should run in the cloud, on schedules and triggers — [Ona newsletter, Feb 24, 2026](https://ona.com/newsletter/24-feb-2026)
  - Every major background agent opens a draft PR so existing gates stay in charge (secondary) — [alexcloudstar (secondary)](https://alexcloudstar.com/blog/best-background-coding-agents-2026/)
- **Long-running sessions.** Average coding-agent sessions reportedly grew from about 4 minutes (early 2025) to about 23 minutes (early 2026). Method unstated, so treat this as anecdotal — [alexcloudstar (secondary)](https://www.alexcloudstar.com/blog/background-ai-coding-agents-2026)
- **Multi-agent adoption.** 69% of agent users run a single agent, 17% run multiple specialized agents, and 16% run coordinated agents — [Stack Overflow blog](https://stackoverflow.blog/2026/05/27/agents-on-a-leash-agentic-ai-remains-mostly-monitored-at-work)
- **Pricing pressure**
  - Copilot moved to metered credits, paused new individual sign-ups and removed Opus from Pro **[repo note: subscription_plans.md]**
  - Cursor credit pools — [jetadmin (secondary)](https://www.jetadmin.io/blog/cursor-pricing-explained-plans-credit-system-and-real-costs-in-2026/)
  - Kiro's model multipliers run up to 6x for Claude Fable 5.1 — [usagepricing.com](https://www.usagepricing.com/blueprint/kiro)
  - ChatGPT Go at $8 and OpenCode Go at $10 are the low-end entry points — [jetadmin (secondary)](https://www.jetadmin.io/blog/codex-pricing-current-costs-usage-limits-and-realistic-budgets-2026/); [RuntimeWire](https://runtimewire.com/article/opencode-go-301000-active-subscribers)
  - Uber and Tesla cap employee spend — [Hyrax (secondary)](https://hyrax.dev/blog/ai-coding-budget-caps-uber-enterprise-rate-limiting)
- **Thinner harnesses.** Anthropic removed more than 80% of Claude Code's system prompt for the Claude 5 generation with "no measurable loss" **[repo note: awareness_runtime_principles.md]**
- **Agent-memory standards emerging.** Devin's "Agent Memory Repo" open spec — [Devin docs](https://docs.devin.ai/product-guides/memory). AGENTS.md and MCP sit under the Linux Foundation's AAIF — [InfoQ](https://infoq.com/news/2025/12/agentic-ai-foundation/)

### Inferences
- The incumbents are racing on scale: cloud fleets, parallelism, enterprise seats and bundled tokens. An indie project cannot compete there and should not try.
- The axes they underserve are single-owner governance (hard ceilings, a tamper-proof stop), auditable self-change, and running locally and privately on cheap models. These match INIT's Root, its sealed gate and DeepSeek economics.
- Metering raises cost anxiety, and enterprises are capping spend. Both make a hard, provable money ceiling more valuable in 2027 than it was in 2025.
- Two risks for INIT. First, the labs could add "agent-written hooks and plugins behind an eval" quickly; Claude Code already has hooks, plugins, auto memory and ultrareview. Second, the open-source graveyard shows that an indie harness without a funding or foundation path tends to stall (Roo, Aider). For INIT, publishing the Root, the gate and the experiments as a research artifact is a more defensible path than competing for users.

### Gaps
- The Information and Bloomberg primary articles were not read. The ARR figures for Cursor and Cognition and the Uber caps rest on secondary summaries.
- No independent market-share data covering all harnesses was found. The Stack Overflow pulse survey is the best available, and it is self-selected.
- I did not check whether Codex cloud or Jules publish background-agent usage numbers.
