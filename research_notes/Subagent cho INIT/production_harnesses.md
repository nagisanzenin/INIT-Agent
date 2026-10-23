# How production agent harnesses implement sub-agents (state as of October 2026)

Research date: 2026-10-22. Each harness is described on the same dimensions so a comparison table can be built: (a) spawn mechanism/API, (b) context isolation, (c) tool and permission scoping, (d) communication, (e) result return, (f) filesystem isolation, (g) concurrency, nesting and recursion, (h) cost multipliers, (i) failure modes. Sources dated more than about 6 months before October 2026 (before about April 2026) are marked **[OLD: date]**. Claims from secondary sources or search snippets are marked **[secondary]** or **[unverified]**. Official docs that carry no date were read on 2026-10-22 and are treated as current.

---

## Q1. Claude Code (Anthropic): subagents, forks, background agents, agent teams, dynamic workflows, Agent SDK

### Takeaway
Claude Code has the richest stack of the harnesses surveyed, with five layers. (1) Markdown-defined subagents are spawned through the `Agent` tool, start with a fresh context (forks inherit the conversation and the prompt cache), run in the background by default, nest up to 3 layers deep, and are capped at 20 concurrent. (2) Experimental agent teams add a shared task list with file locking and file-based mailboxes for peer-to-peer messaging. (3) Dynamic workflows are model-written JavaScript orchestration scripts: up to 16 concurrent agents and 1,000 agents per run, deterministic and resumable. (4) Per-subagent git worktrees. (5) The Agent SDK exposes the same primitives, with hard depth, concurrency and USD caps. Topology is chosen by the model by default. Humans configure the roster and the caps.

### Cited Findings

**(a) Spawn mechanism / API**
- Claude delegates through the **`Agent` tool**, renamed from `Task` in v2.1.63. `Task(...)` still works as an alias. Delegation is chosen from each subagent's `description`. A subagent can also be invoked with `@agent-<name>`, or run as the whole session with `claude --agent <name>`. — [Claude Code docs: Subagents](https://code.claude.com/docs/en/sub-agents)
- Definitions are Markdown files with YAML frontmatter. Only `name` (max 256 chars, no `:`) and `description` are required. Optional fields: `tools` (allowlist), `disallowedTools` (denylist, applied first), `model` (`sonnet|opus|haiku|fable|<id>|inherit`), `effort` (`low…max`), `permissionMode`, `maxTurns`, `autoCompactWindow` (100k–1M, v2.1.296+), `skills` (first 32 names preloaded), `mcpServers`, `hooks`, `memory` (`user|project|local`), `background`, `isolation: worktree`, `omitClaudeMd`, `initialPrompt`, `color`, `experimental.cacheTtl`. — [Subagents](https://code.claude.com/docs/en/sub-agents)
- Scope precedence: managed settings > `--agents` CLI JSON > `.claude/agents/` (project) > `~/.claude/agents/` (user) > plugin `agents/`. Edits are picked up within seconds (hot reload). — [Subagents](https://code.claude.com/docs/en/sub-agents)
- Built-ins: **Explore** (read-only, Write/Edit denied, skips CLAUDE.md and git status, one-shot and not resumable, thoroughness levels quick/medium/very thorough); **Plan** (read-only, used in plan mode); **general-purpose** (all subagent tools); `claude` (catch-all); `statusline-setup` (Sonnet); `claude-code-guide` (Haiku). — [Subagents](https://code.claude.com/docs/en/sub-agents)
- Model resolution order: per-invocation `model` parameter set by Claude > frontmatter `model` > `CLAUDE_CODE_SUBAGENT_MODEL` > main model. `CLAUDE_CODE_SUBAGENT_MODEL_FORCE=1` (v2.1.257+) pins every subagent to one model. Before v2.1.251 the environment variable came first. — [Subagents](https://code.claude.com/docs/en/sub-agents)
- Agent SDK: `agents={name: AgentDefinition(description, prompt, tools, disallowedTools, model, skills, memory, mcpServers, maxTurns, background, effort, permissionMode, …)}` passed to `query()`. Programmatic definitions override filesystem definitions with the same name. Claude may call the built-in `general-purpose` agent even when nothing is defined. `CLAUDE_AGENT_SDK_DISABLE_BUILTIN_AGENTS=1` removes it. — [Agent SDK: Subagents](https://code.claude.com/docs/en/agent-sdk/subagents)
- Source-code study: agent definitions are JSON/Zod schemas loaded from a priority chain. Built-ins include Explore, Plan and VerificationAgent. In "coordinator mode" a Lead spawns Workers via AgentTool and SendMessageTool, and Workers get a 16-tool whitelist (`ASYNC_AGENT_ALLOWED_TOOLS`) and report back via `<task-notification>` XML. The coordinator workflow runs Research → Synthesis → Implementation → Verification. — [Barbaste et al., "Harness Engineering…", arXiv 2609.00006, Sept 2026](https://arxiv.org/html/2609.00006v1)

**(b) Context isolation**
- A non-fork subagent starts with a **fresh context**. Its initial context contains its own system prompt (not Claude Code's), the task message Claude writes, CLAUDE.md (skipped by Explore/Plan/`omitClaudeMd`), a git status snapshot, preloaded skills, and, if it has `SendMessage`, a roster of sibling named agents. Only the final summary returns to the parent. — [Subagents](https://code.claude.com/docs/en/sub-agents)
- A **fork** (`/subtask`, or Agent tool with `subagent_type: "fork"` from v2.1.302) inherits the full conversation, system prompt, tools and model. Because its prefix is identical, its first request **reuses the parent's prompt cache**, which makes a fork cheaper than a fresh subagent for same-context tasks. A fork cannot spawn further forks. Fork mode has been on by default in interactive sessions since v2.1.232, and everywhere since v2.1.302. — [Subagents](https://code.claude.com/docs/en/sub-agents)
- The source-code study describes fork isolation across six dimensions: a linked AbortController, a cloned file-state cache, suppressed permission prompts for async agents, fresh denial tracking, and the parent's rendered system prompt frozen at fork time so the cache still hits. — [arXiv 2609.00006](https://arxiv.org/html/2609.00006v1)

**(c) Tool / permission scoping**
- Always removed for subagents: `AskUserQuestion`, `EndConversation`, `EnterPlanMode`, `ExitPlanMode` (unless in plan mode), `ScheduleWakeup`, `WaitForMcpServers`, `Workflow`, and `Agent` at the depth limit. A main thread run via `--agent` can restrict which agents it may spawn with `tools: Agent(worker, researcher)`. Specific agent types can be denied with `permissions.deny: ["Agent(Explore)"]`. — [Subagents](https://code.claude.com/docs/en/sub-agents)
- **Background subagents get a smaller built-in tool set**: Read, Grep, Glob, LSP, Bash, PowerShell, Edit, Write, NotebookEdit, WebFetch, WebSearch, TodoWrite, Skill, ToolSearch, EnterWorktree, ExitWorktree, Monitor, TaskStop, SendMessage, Artifact, SubagentHandback, plus all MCP tools. The same definition can therefore get different tools in the foreground and in the background. — [Subagents](https://code.claude.com/docs/en/sub-agents)
- Permission inheritance: if the parent is in `bypassPermissions`, `acceptEdits` or auto mode, the subagent inherits that mode and its own `permissionMode` is ignored. Plugin subagents ignore `hooks`, `mcpServers` and `permissionMode`. — [Subagents](https://code.claude.com/docs/en/sub-agents)
- Background and forked subagents get only the static-rules and LLM-classifier permission layers. Prompts they would have raised resolve to denials, and cases the layers cannot resolve escalate to the parent. — [arXiv 2609.00006](https://arxiv.org/html/2609.00006v1)
- Security of returns: final subagent reports are scanned for instruction-like text (v2.1.210+). Imitated control tags such as `<system-reminder>` are neutralized, `Human:`/`Assistant:` turn markers are escaped, and a `[harness: …]` marker line is prepended. Text is never removed. — [Agent SDK: Subagents](https://code.claude.com/docs/en/agent-sdk/subagents)
- Teams: messages between agents are labeled as coming from another Claude session, not from the user. A teammate cannot approve a permission prompt or supply consent, and cannot relay a denied action to another teammate to bypass the check. In auto mode a classifier reviews every inter-agent message before delivery. — [Agent teams](https://code.claude.com/docs/en/agent-teams)

**(d) Communication**
- Subagents: a one-shot return to the caller. Named subagents can message each other with `SendMessage`, and a finished subagent can be resumed with `SendMessage` to its ID or name. Full history is kept. Transcripts are stored at `~/.claude/projects/{project}/{sessionId}/subagents/agent-{agentId}.jsonl` and deleted after `cleanupPeriodDays` (default 30). — [Subagents](https://code.claude.com/docs/en/sub-agents)
- **Agent teams** (experimental, `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`) have four parts: a team lead, teammates (full independent Claude Code sessions), a **shared task list** (`~/.claude/tasks/{team-name}/`, states pending/in progress/completed, dependency blocking, **file locking for claims**), and a **mailbox** (one JSON file per agent at `~/.claude/teams/{team}/inboxes/{agent}.json`, with automatic delivery). Teammates message each other directly by name. There is no broadcast: one message is sent per recipient. — [Agent teams](https://code.claude.com/docs/en/agent-teams)
- Team quality hooks: `TeammateIdle`, `TaskCreated` and `TaskCompleted`. Exit code 2 blocks the event or keeps the agent working. — [Agent teams](https://code.claude.com/docs/en/agent-teams)
- Cross-session messaging lets separate sessions on one machine, another machine or the cloud message each other. `crossSessionInbound: hold` defers delivery. — [Run agents in parallel](https://code.claude.com/docs/en/agents); [Costs](https://code.claude.com/docs/en/costs)

**(e) Result return**
- Subagent: a final summary only. Background subagents deliver results later as a completion notification. When a subagent reaches `maxTurns`, its output is marked partial and it can be resumed. — [Subagents](https://code.claude.com/docs/en/sub-agents)
- SDK: the Agent tool returns `status: "async_launched"` for background runs and `status: "completed"` when Claude waits. An API error that ends a subagent early is never delivered as its result. — [Agent SDK: Subagents](https://code.claude.com/docs/en/agent-sdk/subagents)
- Teams: an idle notification carries the teammate's final answer to the lead. — [Agent teams](https://code.claude.com/docs/en/agent-teams)
- Workflows: script variables hold intermediate results, and only the final return value reaches Claude's context. `agent(prompt, {schema})` returns validated JSON, with up to 5 validation retries (`MAX_STRUCTURED_OUTPUT_RETRIES`). — [Workflows](https://code.claude.com/docs/en/workflows)

**(f) Filesystem isolation**
- `isolation: worktree` gives the subagent a temporary git worktree **branched from the default branch, not the parent's HEAD**. The worktree is auto-removed if unchanged, and Bash/PowerShell commands are checked to stay inside it. Forks can also pass `isolation: "worktree"`. — [Subagents](https://code.claude.com/docs/en/sub-agents)
- **Agent teams do not isolate teammates in worktrees.** The docs tell users to partition files per teammate, because two teammates editing one file overwrite each other. — [Run agents in parallel](https://code.claude.com/docs/en/agents); [Agent teams](https://code.claude.com/docs/en/agent-teams)
- `/batch` is a bundled skill that splits one large change across **5 to 30 worktree-isolated subagents**. — [Run agents in parallel](https://code.claude.com/docs/en/agents)
- Agent view sessions move into their own worktree before editing files. Cloud sessions and "Projects" threads run in cloud VMs (Projects is a public beta on Pro/Max). — [Run agents in parallel](https://code.claude.com/docs/en/agents)

**(g) Concurrency, nesting, recursion**
- `CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS` defaults to **20** running subagents. Above it the spawn returns `Concurrent subagent limit reached`. There is no cap on total subagents per session. Ultracode sessions are exempt. — [Subagents](https://code.claude.com/docs/en/sub-agents)
- **Nesting:** subagents can spawn subagents up to **3 layers** below the main conversation by default (`CLAUDE_CODE_MAX_SUBAGENT_SPAWN_DEPTH`; `1` disables nesting). At the limit the `Agent` tool is withheld. History: v2.1.172–2.1.216 allowed **5 fixed layers**; v2.1.217–218 defaulted to 1; v2.1.219 set the default to 3. — [Subagents](https://code.claude.com/docs/en/sub-agents)
- Subagents spawned from the main conversation now start in the **background by default**. A community changelog dates this to v2.1.198 (July 1, 2026) **[secondary]**. `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1` forces waiting. Ctrl+B backgrounds a running subagent. — [Subagents](https://code.claude.com/docs/en/sub-agents); [community changelog](https://claudemarket.ai/blog/claude-code-changelog)
- Agent teams: **no nested teams** (teammates cannot spawn teammates); **one team per session**; the lead is fixed; there is no hard teammate cap, and the docs suggest "3-5 teammates" with "5-6 tasks per teammate". An in-process teammate's own subagents must run in the foreground. Agent teams cannot be spawned in `-p` or Agent SDK sessions. — [Agent teams](https://code.claude.com/docs/en/agent-teams)
- Community changelog: v2.1.178 (June 15, 2026) removed `TeamCreate`/`TeamDelete`, so every session has one implicit team and teammates are spawned with the Agent tool's `name` parameter **[secondary]**. The official docs agree that a named Agent call launches a teammate when teams are enabled. — [community changelog](https://claudemarket.ai/blog/claude-code-changelog); [Agent teams](https://code.claude.com/docs/en/agent-teams)
- **Dynamic workflows** (GA May 28, 2026): Claude writes a JS script using `agent()`, `parallel()`, `pipeline()`, `phase()`, `log()` and the `args` global. Limits: **16 concurrent agents** by default (`CLAUDE_CODE_WORKFLOW_MAX_CONCURRENT_AGENTS`, 1–256); **4,096 items** per `parallel`/`pipeline` call; **1,000 agents per run**. Scripts have no filesystem or shell access, no `import()`, and no mid-run user input. `Date.now()`, `Math.random()` and `new Date()` throw so that relaunches replay deterministically. A stalled agent restarts at most 5 times (6 attempts). On a usage limit the run pauses (twice at most, resets within 24 h). — [Workflows](https://code.claude.com/docs/en/workflows); [Anthropic blog, May 28 2026](https://claude.com/blog/introducing-dynamic-workflows-in-claude-code)
- Workflow size guideline (advisory): `small` <5 agents, `medium` <10 (default), `large` <50, `unrestricted`. A `Large workflow` warning appears above 25 agents or a projected 1.5M tokens. **Ultracode** (`/effort ultracode`) makes Claude plan a workflow for every substantive task, disables the warning and the 20-subagent concurrency check, and sets effort to xhigh. — [Workflows](https://code.claude.com/docs/en/workflows)
- Agent SDK caps: depth (`CLAUDE_CODE_MAX_SUBAGENT_SPAWN_DEPTH`, default 3), concurrency (default 20), and **spend** (`maxBudgetUsd`, no default). At the spend cap the SDK refuses new spawns with `Budget limit reached`, **stops running background subagents**, and ends the query with `error_max_budget_usd`. — [Agent SDK: Subagents](https://code.claude.com/docs/en/agent-sdk/subagents)
- "Claude Opus 5 delegates to subagents more readily than earlier models." With the `claude_code` preset on Opus 5, the system prompt tells Claude not to call the Agent tool unless asked. — [Agent SDK: Subagents](https://code.claude.com/docs/en/agent-sdk/subagents)

**(h) Cost**
- "Agent teams use approximately **7x more tokens** than standard sessions when teammates run in plan mode." Team token use scales roughly linearly with team size. — [Costs](https://code.claude.com/docs/en/costs)
- Subagent and workflow-agent caches use a separate TTL bucket: 5 minutes by default, or `subagentPromptCacheTtl: 1h` at a higher write price. Workflow fan-outs hold sibling agents with a matching prefix for up to 5,000 ms (`CLAUDE_CODE_WORKFLOW_PREFIX_STAGGER_MS`) so their first requests read the first agent's cache. — [Workflows](https://code.claude.com/docs/en/workflows); [Agent teams](https://code.claude.com/docs/en/agent-teams)
- A startup warning fires when custom subagent descriptions together exceed **15,000 tokens**. — [Subagents](https://code.claude.com/docs/en/sub-agents)
- Enterprise baseline (not specific to subagents): about $13 per developer per active day and $150–250 per developer per month, under $30 per active day for 90% of users. — [Costs](https://code.claude.com/docs/en/costs)
- Anthropic's research system **[OLD: June 13, 2025]**: an Opus 4 lead with Sonnet 4 subagents beat single-agent Opus 4 by **90.2%** on an internal research eval. Agents use about **4×** chat tokens and multi-agent systems about **15×**. Token usage explains **80%** of the variance on BrowseComp. Observed failures: spawning 50 subagents for simple queries, duplicated work, endless searching. The post calls multi-agent a poor fit for "most coding tasks" with few parallelizable parts. — [Anthropic Engineering](https://www.anthropic.com/engineering/multi-agent-research-system)
- Third-party cost anecdotes: a 49-subagent run at an estimated $8k–15k, and a 23-subagent project at about $47k over 3 days **[secondary, unverified; CloudZero via SSOJet]**. — [SSOJet/Security Boulevard, June 7 2026](https://securityboulevard.com/2026/06/6-coding-agents-that-actually-run-sub-agents-in-parallel/)
- Workflow showcase: Bun's Zig→Rust port with hundreds of agents and two reviewers per file reached 99.8% of tests passing, about 750k lines of Rust, in 11 days (not yet in production). — [Anthropic blog, May 28 2026](https://claude.com/blog/introducing-dynamic-workflows-in-claude-code)

**(i) Failure modes / complaints**
- GitHub issue **#68619** (opened 2026-06-15, still open), "[CRITICAL] Subagent spawning … infinite recursion, infinite token usage…": subagents spawning more than 50 levels deep, 1.2M+ tokens in about 30 minutes, children spawned to work around a permission denial, and interruption discarding the whole tree's work. It was filed during the 5-layer era (v2.1.172–216), before the default changed to 3 **[user report]**. — [anthropics/claude-code#68619](https://github.com/anthropics/claude-code/issues/68619)
- Issue 45272 (Apr 8 → closed May 26, 2026): Claude launched 3 background agents before the user had answered its own question **[mirror]**. Issue 71423: a coordinator occasionally spawns unrequested subagents that ignore session skills and tools, burning 300–400k tokens **[mirror]**. — [claudeissues 45272](https://claudeissues.com/issue/45272-agent-launches-background-sub-agents-without-user-approval-wasting-tokens); [claudeissues 71423](https://claudeissues.com/issue/71423-bug-unauthorised-launch-of-extremely-inefficient-subagents-that-wasted-tokens-i)
- Documented limitations of teams: no session resumption for in-process teammates; task status can lag and block dependents; shutdown is slow; the lead may start implementing instead of waiting, or may stop early; teammates stop on errors instead of recovering; permission prompts from teammates pile up at the lead. — [Agent teams](https://code.claude.com/docs/en/agent-teams)

### Inferences
- Anthropic's own sequence (subagents → background default → nesting at 5, cut back to 1, then 3 → teams → workflows → ultracode) suggests that model-chosen recursive fan-out was too costly without hard caps. The answer was deterministic scripts plus Root-like caps (depth, concurrency, USD) rather than removing model autonomy.
- Forks with a shared prompt cache and workflow prefix staggering show that the dominant cost lever is cache reuse across siblings, not only smaller models.
- The report-injection scanning and the "a teammate cannot grant consent" rule treat other agents' outputs as untrusted input. This is directly relevant to a self-modifying harness.

### Gaps
- No official per-version release-date table was retrieved. The v2.1.198 and v2.1.178 dates come from a community changelog.
- No official Anthropic number for subagent (non-team) token multipliers. The 7× figure applies to plan-mode teams only.
- Whether #68619's root cause (recursion through permission-denial workarounds) is fixed beyond the depth default is not confirmed.

---

## Q2. OpenAI Codex: CLI subagents, cloud parallel tasks and best-of-N, Agents SDK

### Takeaway
Codex CLI ships model-callable multi-agent tools (`spawn_agent`, `send_input`, `resume_agent`, `wait`, `close_agent`), enabled by default in current releases. The defaults are conservative: **6 concurrent threads and depth 1**, verified in source on 2026-10-22. Children inherit the parent's sandbox and approval policy, can optionally fork the parent history, and can be fanned out per CSV row. Codex cloud isolates each task, and each best-of-N attempt, in its own container and returns diffs or PRs for human selection.

### Cited Findings
- **(a)** The tools are `spawn_agent`, `send_input`, `resume_agent`, `wait` and `close_agent`. The handler directory in the Codex repo contains exactly `spawn.rs`, `send_input.rs`, `resume_agent.rs`, `wait.rs` and `close_agent.rs`. `SpawnAgentArgs` has the fields `message | items`, `agent_type`, `model`, `reasoning_effort` and `fork_context: bool`. — [openai/codex spawn.rs (main, read 2026-10-22)](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/handlers/multi_agents/spawn.rs)
- **(g) Defaults:** `DEFAULT_AGENT_MAX_DEPTH: i32 = 1` and `DEFAULT_AGENT_MAX_THREADS: Option<usize> = Some(6)`. When the depth limit is hit, the spawn returns the message **"Agent depth limit reached. Solve the task yourself."** — [openai/codex config/mod.rs](https://github.com/openai/codex/blob/main/codex-rs/core/src/config/mod.rs); [spawn.rs](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/handlers/multi_agents/spawn.rs)
- The official docs say "Current Codex releases enable subagent workflows by default", with `agents.enabled` default `true`. Global `[agents]` keys are `enabled`, `max_concurrent_threads_per_session` (legacy alias `max_threads`), `default_subagent_model`, `default_subagent_reasoning_effort` and `interrupt_message`. Built-in agents are `default`, `worker` and `explorer`. — [Codex docs: Subagents (learn.chatgpt.com, undated)](https://learn.chatgpt.com/docs/agent-configuration/subagents)
- The earlier official docs (developers.openai.com/codex/subagents, now redirecting) advised keeping `max_depth = 1`, because raising it "can turn broad delegation instructions into repeated fan-out". They also described `spawn_agents_on_csv` behind the experimental `features.enable_fanout` flag, with a per-worker timeout of 1,800 s (`agents.job_max_runtime_seconds`) **[via search snippet of the official page]**. — [developers.openai.com/codex/subagents](https://developers.openai.com/codex/subagents)
- **Custom agents** are one TOML file per agent in `~/.codex/agents/` or `.codex/agents/`. Required fields are `name`, `description` and `developer_instructions`. Optional fields are `model`, `model_reasoning_effort`, `model_reasoning_summary` and `model_verbosity`. Capability reduction uses `features.shell_tool`, `features.apps`, `features.plugins`, `features.memories`, `features.request_permissions_tool` and skills toggles. — [Codex docs: Subagents](https://learn.chatgpt.com/docs/agent-configuration/subagents)
- **(b)** The source-code study describes `SpawnAgentForkMode` = `FullHistory | LastNTurns(N)`. A filter keeps system, developer and user messages plus final assistant answers, and drops intermediate tool calls and reasoning. Typed `InterAgentCommunication` records (Spawn, Message, Followup, Result) flow through an InputQueue. There are two tool generations, `multi_agents` and `multi_agents_v2`. Delegation can be disabled, explicit-request-only, or proactive. A CSV fan-out returns results via `report_agent_job_result` under a shared JSON schema. — [arXiv 2609.00006](https://arxiv.org/html/2609.00006v1)
- **(c)** "Subagents inherit the parent's sandbox and approval settings. A custom agent file can't replace them." In non-interactive runs, an action needing a new approval fails and the error returns to the parent. With no explicit setting, a subagent inherits the parent's model and reasoning effort. — [Codex docs: Subagents](https://learn.chatgpt.com/docs/agent-configuration/subagents)
- **(e)** "Codex waits until all requested results are available, then returns a consolidated response". Subagents should return summaries, not raw intermediate output. — [Codex docs: Subagents](https://learn.chatgpt.com/docs/agent-configuration/subagents)
- **(h)** Subagent workflows "consume more tokens than comparable single-agent runs". — [Codex docs](https://learn.chatgpt.com/docs/agent-configuration/subagents). gpt-5.4-mini uses about 30% of a GPT-5.4 turn's quota (54.4% vs 57% on SWE-Bench Pro), so it is recommended for parallel scans and reviews **[secondary, Mar 2026]**. — [D. Vaughan, Codex gotchas](https://codex.danielvaughan.com/2026/03/29/subagent-gotchas-known-issues/)
- **(i)** Known issues as of March 2026 **[OLD-ish: Mar 2026; may be fixed]**:
  - #15250: custom TOML agents could not be invoked by name.
  - #14866: subagents stuck in "awaiting instruction", with 62 `spawn_agent` calls against 15 `close_agent` calls (a leak), caused partly by hitting the 6-thread cap and Linux inotify exhaustion.
  - #15177: the UI showed the wrong model for mini children.
  - #16051: the child finished but the parent got "never returned the requested audit payload". The suggested mitigation was to have the child write its output to a file.
  - Approval requests surfaced from background threads.
  — [Vaughan, Mar 29 2026](https://codex.danielvaughan.com/2026/03/29/subagent-gotchas-known-issues/)
- **Codex cloud (f):** "Each task has its own workspace and can keep working while your computer is asleep." Multiple tasks start from one environment with separate working state. Results are reviewed as changed files and test results, then committed or opened as a PR. — [Codex docs: Cloud](https://learn.chatgpt.com/docs/cloud)
- **Best-of-N:** `codex cloud exec --attempts N` takes N from 1 to 4 (default 1), and each attempt gets its own container and agent loop **[secondary, Apr 2026]**. OpenAI's original rollout notice said Codex "generates multiple responses simultaneously for a single task" **[OLD: ~mid-2025]**. — [Vaughan, Apr 1 2026](https://codex.danielvaughan.com/2026/04/01/codex-cloud-exec-best-of-n-attempts/); [OpenAI community](https://community.openai.com/t/codex-generates-multiple-responses-simultaneously/1287639)
- **OpenAI Agents SDK:** it has two primitives. With **agents-as-tools** (`Agent.as_tool()`) the manager keeps control. With **handoffs** a specialist becomes the active agent. Orchestration can be "via LLM" or "via code" (structured outputs, chaining, evaluator loops, `asyncio.gather` for parallel work). — [OpenAI Agents SDK: Multi-agent](https://openai.github.io/openai-agents-python/multi_agent/)

### Inferences
- Codex is the most conservative default among the CLIs: depth 1 and 6 threads. Its fork filter (keep only final answers, drop tool chatter) is a useful middle ground between fresh and full-fork context.
- The "Solve the task yourself" refusal string is a simple, robust recursion guard that INIT could copy verbatim at its broker boundary.
- No evidence was found that Codex CLI isolates local subagents in worktrees. They share the parent sandbox and workspace. Isolation is a cloud-tier feature.

### Gaps
- No official current documentation of the best-of-N attempt range or of how a "best" attempt is chosen (selection appears to be human).
- The concurrency limits and pricing for Codex cloud tasks per plan are not officially sourced. A figure of 200–1,200 cloud tasks per 5 h on Pro 20x is **[secondary, unverified]** ([SSOJet](https://securityboulevard.com/2026/06/6-coding-agents-that-actually-run-sub-agents-in-parallel/)).
- Whether issues #16051 and #14866 are fixed in current releases was not checked.

---

## Q3. Cursor: subagents, parallel agents (best-of-N in worktrees), Cursor 2.x/3.x multi-agent

### Takeaway
Cursor has two layers. Task-tool subagents run in a fresh context, can be read-only or background, nest 2 levels at most since 2.5, and **read `.claude/agents` and `.codex/agents` files too**. Parallel agents run in separate worktrees or cloud VMs: up to 8 on one prompt since 2.0 (Oct 2025), plus `/multitask` async subagent fleets since 3.2 (Apr 2026). Subagents share the parent checkout by default, and isolation is opt-in.

### Cited Findings
- **(a)** Built-in subagents are **Explore** (faster model, many parallel searches) and **Browser** (MCP browser control that filters noisy DOM snapshots and screenshots). Custom subagents live in `.cursor/agents/`, **`.claude/agents/`, `.codex/agents/`**, or the user-level equivalents. `.cursor/` wins on name conflicts. Frontmatter fields: `name`, `description`, `model` (default `inherit`, accepts `[effort=high,context=300k]` brackets), `readonly` (default false; blocks edits and state-changing shell commands), `is_background` (default false). — [Cursor docs: Subagents](https://cursor.com/docs/subagents)
- **(b)** Each subagent "starts clean" and does not see prior conversation, so the parent must put the context in the prompt. — [Cursor docs](https://cursor.com/docs/subagents)
- **(c)** Subagents inherit all parent tools including MCP. Cloud subagents use the team's MCP configuration. Admin and plan policies can override a configured model. — [Cursor docs](https://cursor.com/docs/subagents)
- **(d/e)** The parent issues several Task calls in one message to run them in parallel. Foreground runs block, and background runs write output to `~/.cursor/subagents/`. Every run returns an agent ID that can resume it with context preserved. A failed subagent returns an error status to the parent. — [Cursor docs](https://cursor.com/docs/subagents)
- **(f)** "By default, subagents share the parent's checkout." On request, each gets its own branch in a **separate git worktree or a cloud VM with a repo clone**, and the changes stay on that branch until the parent merges them. `/in-cloud` runs the next task on its own VM and branch, and `/autopilot` hands a PR to a cloud subagent. — [Cursor docs](https://cursor.com/docs/subagents)
- **(g)** **Nesting since Cursor 2.5: one level.** The main agent and its direct subagents can launch subagents, but a grandchild cannot. The docs give no maximum subagent count and suggest starting with "2-3 focused subagents". — [Cursor docs](https://cursor.com/docs/subagents)
- **(h)** "five parallel subagents use roughly five times the tokens of one agent". On legacy request-based plans without Max Mode, subagents run on Composer regardless of `model`. Named third-party models bill as third-party usage, and Teams/Enterprise plans add a "Cursor Token Rate". — [Cursor docs](https://cursor.com/docs/subagents)
- **Cursor 2.0 [OLD: Oct 29, 2025]:** "Run up to **eight agents in parallel** on a single prompt", each in a git worktree or on a remote machine. The changelog page does not describe automatic best-of-N selection. — [Cursor changelog 2.0](https://cursor.com/changelog/2-0). Changes are brought in with "Apply All" to the local branch (a forum staffer reply). — [Cursor forum](https://forum.cursor.com/t/cursor-2-0-x-what-are-parallel-agents-doing/138911)
- **Cursor 3.2 (Apr 24, 2026):** `/multitask` runs **async subagents** instead of queuing, and splits larger tasks across "a fleet of async subagents". Worktrees appear in the Agents Window and a branch can be moved to the local foreground with one click. Multi-root workspaces were added. **3.3 (May 7, 2026):** "Run plans in parallel", where independent plan steps run on async subagents and dependent steps stay ordered. — [Cursor 3.2 changelog](https://www.cursor.com/changelog/04-24-26); [3.3 changelog](https://cursor.com/de/changelog/05-07-26)
- An AI Engineer Europe 2026 talk ("replacing 12k LOC with a 200 LOC skill") reports that Cursor moved its worktree orchestration into prompts and subagents, at the cost of enforced checkout boundaries **[secondary]**. — [ai.engineer talk](https://www.ai.engineer/talks/WE_Gnowy3uw-replacing-12k-loc-200-loc-skill)
- Cloud agents run on Firecracker microVMs orchestrated by a Rust service, "Anyrun" **[secondary, citation unclear]**. — [SSOJet](https://securityboulevard.com/2026/06/6-coding-agents-that-actually-run-sub-agents-in-parallel/)
- **(i)** Practitioner complaint: dependent tasks can't be parallelized — "you can't parallelize, and that's most real work". — [Liran Baba, 2026](https://liranbaba.dev/blog/cursor-3-parallel-agents/). The docs note startup overhead, and that subagents may be slower than the main agent on simple tasks. — [Cursor docs](https://cursor.com/docs/subagents)

### Inferences
- Reading `.claude/agents` and `.codex/agents` shows that agent-definition files are becoming a de facto cross-harness format.
- Cursor's shift from a hard-coded worktree orchestrator to a prompt-plus-subagent "skill" is evidence that harnesses are moving orchestration into model-editable artifacts. That is the same direction as INIT's T0 `agents/<name>/AGENT.md`.

### Gaps
- No official maximum for `/multitask` async subagents or for concurrent cloud agents. No official pricing per parallel agent.
- Whether the 8-agent limit from 2.0 still holds in 3.x is unconfirmed.

---

## Q4. Devin (Cognition): managed Devins, and the evolution of the "Don't build multi-agents" stance

### Takeaway
Cognition moved from "Don't Build Multi-Agents" (about June 2025) to shipping **managed Devins** (Mar 19, 2026): a coordinator Devin spawns full child Devins, each in its own VM, messages them, puts them to sleep, tracks their ACU spend and reads their trajectories. Its April 2026 position is that **parallel writers still fail**. What works is (1) one writer with contributing agents ("contribute intelligence rather than actions"), (2) a clean-context reviewer, (3) a "Smart Friend" stronger model consulted with forked context, and (4) "map-reduce-and-manage" with managed Devins.

### Cited Findings
- **(a/f)** "Each managed Devin is a full Devin with its own isolated VM, terminal, browser, and dev environment". The coordinator scopes work, assigns pieces, monitors progress, resolves conflicts and compiles results. Children start with clean context, run in parallel, and each has its own session link. "Managed Devins are available now for all users." — [Cognition blog, Mar 19 2026](https://cognition.com/blog/devin-can-now-manage-devins)
- **(d)** Coordinator controls: message children mid-task, put them to sleep or terminate them, schedule self-messages for follow-ups, monitor **ACU consumption per child**, and read children's full trajectories to improve later decomposition. — [Cognition blog](https://cognition.com/blog/devin-can-now-manage-devins)
- The docs say Devin "automatically breaks down large tasks and delegates to managed Devins when it makes sense", and users can also request it explicitly **[via search snippet]**. — [Devin docs: advanced capabilities](https://docs.devin.ai/work-with-devin/advanced-capabilities)
- **(g)** No documented maximum number of children. A "10 at a time" example appears in a user prompt, not as a limit. — [Cognition blog](https://cognition.com/blog/devin-can-now-manage-devins)
- **(e)** The merge mechanism is not specified. The examples mention Markdown summaries, test reports and "a set of PRs". — [Cognition blog](https://cognition.com/blog/devin-can-now-manage-devins)
- **Stance (Apr 22, 2026), from Walden Yan's "Multi-Agents: What's Actually Working":**
  - Parallel-writer swarms still fail, because agents make conflicting implicit choices.
  - Working patterns are the **single writer + contributors**, the **reviewer loop** with a clean context that sees only the diff (Devin Review averages about 2 bugs per Devin PR, about 58% of them severe), **"Smart Friend"**, and **managed Devins**.
  - In Smart Friend, the primary model passes "a forked copy of its full context" to a stronger model. Small→large pairings (SWE-1.5 with Sonnet 4.5) fell short because the weak model didn't know when to escalate. Frontier pairings (Claude + GPT) helped on the hardest tasks.
  - Managed-Devin failure modes: managers are over-prescriptive without codebase context, agents assume shared state that doesn't exist, and "children don't report back by default".
  - Principles: share as much context as possible, keep writes coordinated, use clean context for review, and treat communication as the main open problem. Unstructured swarms are "a distraction".
  — [Cognition blog, Apr 22 2026](https://cognition.com/blog/multi-agents-working)
- Windsurf was rebranded as **Devin Desktop** (June 2, 2026), with an "Agent Command Center" Kanban for many agents and ACP support for third-party agents (Codex, Claude Agent, OpenCode). Devin Local was rewritten in Rust, is up to 30% more token-efficient, and supports subagents **[secondary]**. — [apidog, 2026](https://apidog.com/blog/whats-new-in-devin-2026/); [SSOJet](https://securityboulevard.com/2026/06/6-coding-agents-that-actually-run-sub-agents-in-parallel/)

### Inferences
- Cognition's shift is narrower than a reversal. It endorses read-only or advisory multiplicity and VM-isolated managed children with one coordinator, and still rejects concurrent writers on shared state. That supports INIT's "members read, never write the workspace without a grant" rule.
- The Smart Friend result implies that a weak primary model needs explicit escalation triggers. A strong primary model can decide on its own.

### Gaps
- ACU price per child session and any concurrency cap were not found.
- The original "Don't Build Multi-Agents" post (about June 2025, inferred from "10 months ago" in the Apr 2026 post) was not re-fetched.

---

## Q5. Amp (Sourcegraph / Amp Inc.): Oracle, Task subagents, Librarian, Puck, Orbs

### Takeaway
Amp treats subagents as **specialized tools with their own model routing**: Search, Oracle, Librarian, Task and Read Thread. Each has its own context and returns only a final summary, with no mid-task steering and no peer messaging. The Oracle is a stronger reasoning model consulted as a tool, now GPT-5.4. In July 2026 Amp added **Puck**, a meta-agent that launches and coordinates agents running in **Orbs**, which are persistent per-thread remote machines.

### Cited Findings
- **(a)** The subagents are **Search** (fast code retrieval), **Oracle** (hard reasoning and planning), **Librarian** (external codebases and large source material), **Task** (delegated coding and verification end to end) and **Read Thread** (summarizes other threads). There are 4 modes: `low`, `medium`, `high`, `ultra`. Amp picks subagents automatically, mostly in `medium` mode. Plugins can define custom modes and subagents (`amp.createAgent(...)`). — [Amp docs: Models & subagents](https://ampcode.com/docs/models-and-subagents)
- **(b/d/e)** Subagents "start with the instructions and context the main agent gives them, not the full conversation"; they "can't communicate with each other or be guided mid-task"; the main agent receives "only the final summary". — [Amp docs](https://ampcode.com/docs/models-and-subagents)
- **(h)** "Each subagent runs its own model, chosen independently of the main agent's". For example, a main agent on a ChatGPT subscription can spawn subagents billed on Amp credits. Pinning one raw model affects only the main agent. — [Amp docs](https://ampcode.com/docs/models-and-subagents)
- **Oracle history:**
  - Originally "a read-only subagent powered by … OpenAI's o3", slower and pricier than the Sonnet 4 main agent, and deliberately not pushed hard in the system prompt **[OLD: 2025]**. — [Amp news: Oracle](https://ampcode.com/news/oracle)
  - Then GPT-5 **[OLD: 2025]**. — [Amp news: GPT-5 Oracle](https://ampcode.com/news/gpt-5-oracle)
  - Then **GPT-5.4 (Mar 5, 2026)**: internal response quality rose from 60.8% to 68.2%, and mean latency fell from about 6.7 to about 4.9 min. It is used with Opus 4.6 in `smart` mode and was added to `deep` mode (main model GPT-5.3-Codex). — [Amp news, Mar 5 2026](https://ampcode.com/news/gpt-5.4-the-new-oracle)
- **Puck (about July 20, 2026):** "a quick assistant and a home base for launching and coordinating other agents … an experiment". It can "split a request across several agents and gather their reports", track and message the agents it started, and create projects. — [Amp news: Meet Puck](https://ampcode.com/news/meet-puck); [Amp docs: Puck](https://ampcode.com/docs/puck)
- **Orbs** are per-thread machines where "the agent keeps working" after the laptop closes. Event-driven, self-scheduling Orbs were reported in July 2026 **[secondary]**. — [Amp manual](https://ampcode.com/manual); [digitalapplied, Jul 26 2026](https://www.digitalapplied.com/blog/amp-event-driven-orbs-self-scheduling-agents-2026)

### Inferences
- Amp's design treats a sub-agent as a capability router: the value lies in mixing models per role (cheap search, frontier reasoning), not in parallelism. The Oracle pattern matches Cognition's "Smart Friend".
- Puck plus Orbs converges with Devin's managed Devins and Claude Code's agent view: a meta-agent over persistent remote sandboxes.

### Gaps
- No documented concurrency cap, nesting depth or per-subagent cost for Amp. Not documented whether the current Oracle is still read-only or receives a forked context.

---

## Q6. OpenHands: delegation, micro-agents, SDK

### Takeaway
OpenHands retired its V0 sequential `AgentDelegateAction`-style delegation. The V1 SDK offers `TaskTool` (one separate conversation per task, resumable) and `DelegateTool` (`spawn` named sub-agents, then `delegate` tasks to them concurrently in threads). Sub-agents share the parent's workspace and LLM config, and definitions are Markdown with YAML. OpenHands also hosts rival harnesses (Claude Code, Codex, Gemini CLI) as ACP backends. Sources conflict on which tool is wired by default.

### Cited Findings
- **(a/g)** The official guide: the agent first **spawns** sub-agents with identifiers, then sends work with **`delegate`**. Each sub-agent "inherits the same LLM configuration as the parent agent and operates in the same workspace". Tasks run in parallel using threads (`register_tool("DelegateTool", DelegateTool)` from `openhands.tools.delegate`) **[via search snippet of official page; direct fetch 404'd]**. — [OpenHands SDK: Agent delegation](https://docs.openhands.dev/sdk/guides/agent-delegation)
- Source-code study: "The V0 sequential delegate is retired." **TaskTool** (params `prompt`, `subagent_type`, `description`, `resume`) creates a separate conversation per task with its own event log and metrics. **DelegateTool** runs tasks concurrently in threads. Definitions are Markdown+YAML at project, user, builtin and plugin levels. Built-ins are code-explorer, bash-runner, web-researcher and a general-purpose default. **Nesting is supported.** Parallel tool calls serialize on shared resources (files, terminal, browser) through a resource-lock manager, and `tool_concurrency_limit` defaults to 1. A `/goal` endpoint runs an LLM judge after each run. — [arXiv 2609.00006](https://arxiv.org/html/2609.00006v1)
- **Conflict:** an NVIDIA paper says `TaskToolSet` sits behind `enable_sub_agents` (default off) and that a spawn/delegate executor exists but is unwired **[third-party]**. The official docs show `DelegateTool` registered. — [NVIDIA OO Agents, arXiv 2607.20709](https://arxiv.org/pdf/2607.20709); [OpenHands docs](https://docs.openhands.dev/sdk/guides/agent-delegation)
- OpenHands hosts Claude Code, Codex or Gemini CLI as interchangeable ACP backends ("harness hosting"), and supports Docker, Apptainer or remote sandboxes. — [arXiv 2609.00006](https://arxiv.org/html/2609.00006v1)
- SDK design paper **[OLD: Nov 2025]**: "blocking parallel execution" in which the parent spawns and monitors sub-agents until all tasks complete **[via search snippet]**. — [OpenHands SDK paper, arXiv 2511.03690](https://arxiv.org/abs/2511.03690)

### Inferences
- OpenHands' resource-lock manager for parallel tool calls is a concrete, transferable answer to the shared-workspace conflict problem that Copilot /fleet and Claude agent teams leave to the user.

### Gaps
- No current numbers for maximum sub-agents or depth. The current status of "micro-agents" (renamed or merged into skills?) was not verified. The legacy `AgentDelegateAction` details were not re-fetched.

---

## Q7. Google (Gemini CLI, Antigravity, Jules) and Amazon Kiro

### Takeaway
Google consolidated onto **Antigravity**, which according to the Gemini CLI docs replaced Gemini CLI on June 18, 2026. Antigravity has the most permissive model-driven topology found: the **model can define transient subagents at runtime** (`define_subagent`) and invoke them (`invoke_subagent`). Nesting goes up to **10 levels**. Subagents message each other by conversation ID and can read each other's transcripts. Workspace modes are inherit, branch (worktree) or share. Gemini CLI's subagents were flat (no nesting) and supported remote A2A agents. Jules is an async cloud agent with plan-tiered concurrency of 3, 15 or 60. Kiro has the most structured approach: the main agent plans an immutable dependency graph, bounded review loops, and spawn allowlists.

### Cited Findings

**Gemini CLI [superseded June 18, 2026]**
- Subagents are on by default (`experimental.enableAgents`) and exposed to the main agent "as a tool with the same name". Built-ins: `codebase_investigator`, `cli_help`, `generalist` (inherits the main agent's tools), and `browser_agent` (disabled by default; needs Chrome 144+). Files go in `.gemini/agents/*.md` and `~/.gemini/agents/*.md`. Fields: `name`, `description`, `kind` (`local`|`remote`), `tools` (supports `*` and `mcp_*`; inherits all if omitted), `mcpServers`, `model` (default inherit), `temperature` (default 1), **`max_turns` (default 30)**, **`timeout_mins` (default 10)**. **"Subagents cannot call other subagents"**, as recursion protection, even with the `*` wildcard. Policy rules can be scoped per subagent with `subagent = "name"`. The page states that Gemini CLI was replaced by Antigravity CLI on June 18, 2026. — [Gemini CLI docs: Subagents](https://geminicli.com/docs/core/subagents/)
- Remote subagents use **A2A** (`kind: remote`), with auth types apiKey, http, google-credentials and oauth (PKCE). Subagents launched Apr 15, 2026. — [Gemini CLI: Remote subagents](https://geminicli.com/docs/core/remote-agents/); [Google Developers Blog](https://developers.googleblog.com/subagents-have-arrived-in-gemini-cli/)
- Source study: `invoke_agent` tool (since v0.39). The generalist has a 10-minute and 20-turn run budget. Local and remote agents use a "symmetric" protocol. It is the corpus's only A2A server. — [arXiv 2609.00006](https://arxiv.org/html/2609.00006v1)

**Antigravity (2.0 launched at Google I/O, May 19, 2026)**
- **(a)** Built-in subagents: `research`, `browser` (only via `/browser`), and **`self` (a clone of the calling agent)**. Configured agents are Markdown+YAML in `.agents/agents/<name>.md`, `~/.gemini/config/agents/`, or plugins. **Dynamic spawning:** the parent calls **`invoke_subagent`** with a role and an initial prompt, and **`define_subagent` creates transient subagents during a session**. Fields: `tools` (default `[]`), `mainAgent`, `subagent`, `model` (`inherit|flash|pro`), **`commandExecutionPolicy` (default `sandbox`; `off|auto|eager|sandbox`)**, `mcpServers`, `skills`, `plugins`. — [Antigravity docs: Custom subagents](https://antigravity.google/docs/subagents)
- **(b/f)** Subagents get a clean context and do not inherit the parent's history. **Workspace options:** `inherit` (same workspace), **`branch` (isolated git worktree)**, `share` (shared directory). — [Antigravity docs](https://antigravity.google/docs/subagents)
- **(c)** Subagents inherit the parent's allowed command prefixes, file read/write scopes and sandbox settings. Parents keep access to child worktrees. Authorization requests "bubble up". — [Antigravity docs](https://antigravity.google/docs/subagents)
- **(d/e)** Subagents run asynchronously. Their lifecycle is Running → Idle (has sent its result to the parent; wakes on a new message with prior context) → Killed (temporary worktree removed, JSONL transcript kept). **"Agents message each other by conversation ID (parent, subagents, or known peers)"** and "can read each other's transcripts". — [Antigravity docs](https://antigravity.google/docs/subagents)
- **(g)** **"Maximum nesting depth is 10 levels below the primary agent."** No numeric concurrency limit is given. `/boost` gives a three-tier hierarchy (Orchestrator → coordinators → workers) on Google One AI Premium Pro/Ultra and Enterprise. `/teamwork-preview` gives coordinated agent teams. — [Antigravity docs](https://antigravity.google/docs/subagents)
- **(h)** In a launch demo, 93 parallel sub-agents built the core of an OS in 12 h using 2.6B tokens for under $1,000 **[secondary: MarkTechPost via SSOJet; MarkTechPost returned 403]**. — [SSOJet/Security Boulevard](https://securityboulevard.com/2026/06/6-coding-agents-that-actually-run-sub-agents-in-parallel/)
- **(i)** An unmapped or misspelled tool name may hang the subagent process (a documented known issue). Antigravity breaks on worktree repos that use `extensions.worktreeConfig` **[forum, secondary]**. Practitioners advise per-subagent ports and DBs, because worktrees do not isolate ports, databases or caches **[secondary]**. — [Antigravity docs](https://antigravity.google/docs/subagents); [Knightli, Jul 27 2026](https://knightli.com/en/2026/07/27/google-antigravity-2-subagents-hooks-scheduled-tasks-workflow/)

**Jules**
- Concurrent tasks: **3 / 15 / 60** (Jules / Pro / Ultra). Daily tasks (rolling 24 h): **15 / 100 / 300**. — [Jules docs: Limits and plans (undated)](https://jules.google/docs/usage-limits/)
- `jules remote new --parallel` runs the same prompt multiple times, with a max of 5 per a Nov 10, 2025 CLI update **[secondary, unverified]**. The API is `v1alpha`. — [kie.ai / tooldirectory (secondary)](https://tooldirectory.ai/tools/jules)

**Amazon Kiro (page updated Oct 2, 2026)**
- **(a)** There are two built-ins (context gathering, and general purpose for parallel work). The main agent picks custom agents by `description`. An unknown agent name fails rather than falling back. — [Kiro docs: Subagents](https://kiro.dev/docs/custom-agents/subagents/)
- **(g)** **"The main agent plans a full dependency graph before execution starts"**: independent tasks run in parallel, dependents wait, and **the graph cannot be changed during execution**. Review loops are configured with `target`, `trigger` (at least 4 chars) and **`max_iterations` (1–10)**; self-loops and mutual loops (A→B→A) are rejected. No maximum number of concurrent sub-agents is stated. — [Kiro docs](https://kiro.dev/docs/custom-agents/subagents/)
- **(c)** `toolsSettings.subagent.availableAgents` (glob allowlist of spawnable agents) and **`trustedAgents`** (run without permission prompts). A custom sub-agent has only its own `tools` and `permissions`. An orchestrator needs `subagent` in its `tools` to delegate. — [Kiro docs](https://kiro.dev/docs/custom-agents/subagents/)
- **(b)** Isolated per sub-agent: history and context window. Shared with the main agent: steering files, MCP servers, workspace files, spec state, permission config and hook triggers. With workflows enabled, delegation runs as a background workflow. **Conflict:** an older IDE page said "subagents do not have access to Specs, and Hooks will not trigger in subagents" **[older page, via search snippet]**. — [Kiro docs](https://kiro.dev/docs/custom-agents/subagents/); [Kiro IDE subagents](https://kiro.dev/docs/chat/subagents/)
- An experimental CLI **Delegate** feature runs separate chat sessions with specific agents in parallel to the main conversation (`kiro-cli settings chat.enableDelegate true`). — [Kiro docs: Delegate](https://kiro.dev/docs/cli/experimental/delegate/)

### Inferences
- Antigravity is the closest production analogue to an "empower the model maximally" harness: model-authored transient agent types, a `self` clone, 10-level depth, and peer messaging. Kiro is the opposite pole, with a pre-planned immutable DAG and bounded loops. Together they bracket the design space for INIT.

### Gaps
- Antigravity's concurrency caps, pricing, and whether `define_subagent` definitions persist. No official Google source for the 93-agent demo was retrieved.
- Kiro's maximum parallelism and nesting depth.

---

## Q8. Other notable entrants: GitHub Copilot (/fleet, Agent HQ), Factory, Roo Code, Goose, OpenCode, Hermes, Pi, Mistral Vibe

### Takeaway
Most other entrants copy the Claude Code shape: Markdown agents, a Task tool, a fresh context, a single-message return, and usually **no nesting**. Distinctive mechanisms: Copilot **/fleet** runs dependency-ordered waves in a shared filesystem with no locking (last writer wins). **Agent HQ** assigns one issue to several vendors' agents (best-of-N across vendors, results as draft PRs). Factory caps the autonomy of read-only droids. Roo uses an orchestrator mode that cannot itself read or write. Hermes intersects tool sets and runs a SQLite-blackboard Kanban swarm.

### Cited Findings
- **Copilot CLI /fleet (Apr 1, 2026):** a background orchestrator splits the objective into work items with dependencies, dispatches independent items as background sub-agents, polls, dispatches the next wave, and verifies. **Sub-agents cannot message each other.** **"All sub-agents share one filesystem with no file locking … the last one to finish overwrites the other silently."** Custom agents live in `.github/agents/` (name, description, model, tools). The model is the current default unless one is specified. Failure modes: overwrites, sequential execution when the prompt is vague, missing context, and overhead on linear work. — [GitHub blog, Apr 1 2026](https://github.blog/ai-and-ml/github-copilot/run-multiple-agents-at-once-with-fleet-in-copilot-cli/); [GitHub docs: /fleet](https://docs.github.com/en/copilot/concepts/agents/copilot-cli/fleet)
- /fleet "may consume more GitHub AI Credits" because each agent calls the model independently. — [GitHub docs](https://docs.github.com/en/copilot/concepts/agents/copilot-cli/fleet)
- **Agent HQ [OLD: announced Oct 2025]:** Mission Control lets users "assign multiple agents to a task" (Copilot, Claude, Codex). Results return as draft PRs and comments inside GitHub's permission and audit model. — [i-programmer](https://www.i-programmer.info/news/105-artificial-intelligence/18651-github-adds-claude-and-codex-to-agent-hq.html); [AlternativeTo, Oct 2025](https://alternativeto.net/news/2025/10/github-unveils-agent-hq-a-central-hub-for-ai-coding-agents-from-top-providers)
- **Factory droids:**
  - Definitions are `.factory/droids/*.md`. Fields: `model` (default inherit), `reasoningEffort`, `tools` (a category such as `read-only|edit|execute|web|mcp`, or an ID list), `mcpServers`.
  - Task tool parameters are `subagent_type`, `description` and `prompt`, plus `run_in_background`, `resume` and `complexity`. `TaskOutput`/`TaskStop` handle background runs.
  - There is a fresh context per call and **no nesting** (no Task tool inside a droid). `AskUser` is disabled, and `tools: all` is rejected.
  - **Read-only droids "never run above Low autonomy"**, and autonomy is capped by an org maximum. In Spec Mode, subagents are read-only.
  — [Factory docs: Custom droids (undated)](https://docs.factory.com/harness/subagents)
- **Roo Code [OLD: 2025]:** the Orchestrator ("Boomerang") is a built-in mode that delegates via `new_task`, and each subtask has an isolated context. Children return a summary to the parent (v3.8.0). From v3.15.2 the Orchestrator "no longer reads, writes, executes commands, or utilizes MCP servers". — [Roo docs: Boomerang tasks](https://docs.roocode.com/features/boomerang-tasks); [v3.8.0 notes](https://docs.roocode.com/update-notes/v3.8.0); [v3.15.2 notes](https://docs.roocode.com/update-notes/v3.15.2)
- **Goose:** it "can autonomously decide to use subagents". Execution is sequential by default and parallel when the prompt says "parallel" or "concurrently". Subagents are one-off and created from scratch, while **subrecipes** are the reusable form. Goose can delegate to external agents (Claude Code, Codex) via ACP **[via search snippets]**. — [Goose docs: Subagents](https://goose-docs.ai/docs/guides/subagents); [Goose blog, Sep 26 2025](https://goose-docs.ai/blog/2025/09/26/subagents-vs-subrecipes); [Goose as conductor, May 5 2026](https://goose-docs.ai/blog/2026/05/05/goose-as-conductor/)
- **From the source-code study of 11 harnesses (Sept 2026)** — [arXiv 2609.00006](https://arxiv.org/html/2609.00006v1):
  - **Hermes:** `delegate_task` with in-process children on a thread pool, fresh conversation, summary-only return, and **toolsets intersected with the parent's**. A 6-tool blocklist covers recursion, user interaction, memory writes and scheduling. Defaults: **3 concurrent children, 50-iteration budgets, depth 1**. `role="orchestrator"` plus a spawn-depth setting unlocks nested trees. A Kanban swarm (planner → workers → verifier → synthesizer) runs as separate `hermes -p <profile>` processes over a **SQLite blackboard**. `/moa` queries up to 8 advisory models at once.
  - **Pi:** core Pi has no spawn tool. A roughly 1,000-line extension spawns separate `pi --mode json` processes, and the parent parses the child JSONL for progress and **cost**. Parallel mode uses a pool of 4 with a maximum of 8 tasks. There is **no depth counter** (nesting unbounded).
  - **OpenCode:** a child Session with a `parentID` is resumable via `task_id` and returns `<task_result>`. **Children inherit only the parent's deny rules.** Recursion is off by default and opt-in per agent, with no numeric cap.
  - **Mistral Vibe:** sequential, one hop. Only profiles tagged "subagent" are delegable, which blocks escalation to write-capable profiles.
  - **OpenClaw:** child processes over ACP/RPC, with `MAX_PROMPT_BYTES = 2MB`.

### Inferences
- Copilot /fleet's documented silent last-writer-wins behavior is the clearest evidence that parallel writers in a shared tree need either worktrees or locks. Prompt discipline alone does not prevent it.
- Factory's autonomy cap for read-only droids and Hermes's toolset intersection are cheap, harness-enforced least-privilege rules that INIT's broker could implement directly.

### Gaps
- Cline's sub-agent or parallel features were not researched (no distinct mechanism found in the time available).
- Concurrency caps for /fleet and Factory are undocumented.
- The current Agent HQ GA and plan status (late-2025 sources only).

---

## Q9. Convergence vs divergence across harnesses

### Takeaway
**Converged:** (1) agent definitions as Markdown/TOML files with name, description, tools, model and effort, increasingly cross-readable; (2) a Task/Agent tool chosen by the model from the description; (3) **fresh context by default** with a **single condensed summary returned**; (4) tool allowlists, with read-only explorer/research built-ins in nearly every harness; (5) children inherit the parent's sandbox and permissions and cannot widen them; (6) worktree or VM isolation as an **opt-in for writers**; (7) explicit depth and concurrency caps. **Still diverging:** nesting depth (0 to 10, or unbounded), communication (none vs mailbox vs peer messaging vs a shared task list), fork vs fresh context, LLM turn-by-turn orchestration vs scripted orchestration, shared checkout vs per-child sandbox, single-writer doctrine vs parallel writers in worktrees, and the protocol used (in-process vs A2A/ACP).

### Cited Findings
- 9 of the 11 studied harnesses implement coordinator-worker as an **overlay on the iterative loop** (Aider and Mini-SWE-Agent are single-agent, and Pi uses extensions). The study calls this "convergent evolution". Optimization targets differ: Claude Code optimizes cost (shared cache), Codex isolation and scale (depth-tracked tree, CSV fan-out), Gemini CLI portability (A2A), Mistral Vibe simplicity, and Hermes containment. 8 of 9 use in-process or non-standard channels for their own sub-agents. ACP appears in 6 systems, but for editor and host roles, not for sub-agent coordination. Feature diffusion is fast: "the half-life of a distinctive feature is measured in weeks". — [arXiv 2609.00006](https://arxiv.org/html/2609.00006v1)
- Cross-readable definitions: Cursor loads `.claude/agents/` and `.codex/agents/` alongside `.cursor/agents/`. — [Cursor docs](https://cursor.com/docs/subagents)
- Read-only built-ins: Claude Explore/Plan ([CC](https://code.claude.com/docs/en/sub-agents)), Codex `explorer` ([Codex](https://learn.chatgpt.com/docs/agent-configuration/subagents)), Cursor Explore ([Cursor](https://cursor.com/docs/subagents)), Gemini `codebase_investigator` ([Gemini CLI](https://geminicli.com/docs/core/subagents/)), Antigravity `research` ([Antigravity](https://antigravity.google/docs/subagents)), Kiro context gathering ([Kiro](https://kiro.dev/docs/custom-agents/subagents/)), Amp Search/Oracle/Librarian ([Amp](https://ampcode.com/docs/models-and-subagents)), OpenHands code-explorer ([arXiv](https://arxiv.org/html/2609.00006v1)).
- Nesting spectrum:
  - 0 (no nesting): Gemini CLI ([docs](https://geminicli.com/docs/core/subagents/)), Factory ([docs](https://docs.factory.com/harness/subagents)), Claude agent teams ([docs](https://code.claude.com/docs/en/agent-teams)).
  - Depth 1: Codex default ([source](https://github.com/openai/codex/blob/main/codex-rs/core/src/config/mod.rs)), Hermes default ([arXiv](https://arxiv.org/html/2609.00006v1)).
  - 2 levels: Cursor ([docs](https://cursor.com/docs/subagents)).
  - 3 layers: Claude Code subagents ([docs](https://code.claude.com/docs/en/sub-agents)).
  - 10: Antigravity ([docs](https://antigravity.google/docs/subagents)).
  - Unbounded: Pi, and opt-in OpenCode ([arXiv](https://arxiv.org/html/2609.00006v1)).
- Concurrency spectrum: Hermes 3 ([arXiv](https://arxiv.org/html/2609.00006v1)), Pi pool 4 ([arXiv](https://arxiv.org/html/2609.00006v1)), Codex 6 ([source](https://github.com/openai/codex/blob/main/codex-rs/core/src/config/mod.rs)), Cursor 2.0 parallel agents 8 ([changelog](https://cursor.com/changelog/2-0)), Claude workflows 16 (up to 256) ([docs](https://code.claude.com/docs/en/workflows)), Claude subagents 20 ([docs](https://code.claude.com/docs/en/sub-agents)), Jules 3/15/60 cloud tasks ([docs](https://jules.google/docs/usage-limits/)), Claude workflow total 1,000 agents per run ([docs](https://code.claude.com/docs/en/workflows)).
- Communication spectrum:
  - None (summary only): Amp ([docs](https://ampcode.com/docs/models-and-subagents)), Copilot /fleet ([blog](https://github.blog/ai-and-ml/github-copilot/run-multiple-agents-at-once-with-fleet-in-copilot-cli/)), Factory ([docs](https://docs.factory.com/harness/subagents)).
  - Parent→child messaging and resume: Codex `send_input` ([source](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/handlers/multi_agents/spawn.rs)), Devin coordinator ([blog](https://cognition.com/blog/devin-can-now-manage-devins)), Cursor resume by ID ([docs](https://cursor.com/docs/subagents)).
  - Peer messaging: Claude named subagents and teams with file mailboxes ([docs](https://code.claude.com/docs/en/agent-teams)), Antigravity by conversation ID plus transcript reading ([docs](https://antigravity.google/docs/subagents)).
  - Shared task list or blackboard: Claude teams ([docs](https://code.claude.com/docs/en/agent-teams)), Hermes SQLite ([arXiv](https://arxiv.org/html/2609.00006v1)).
- Isolation spectrum:
  - Shared checkout by default: Cursor subagents ([docs](https://cursor.com/docs/subagents)), Copilot /fleet with no locks ([blog](https://github.blog/ai-and-ml/github-copilot/run-multiple-agents-at-once-with-fleet-in-copilot-cli/)), Claude teams ([docs](https://code.claude.com/docs/en/agents)), OpenHands same workspace with resource locks ([arXiv](https://arxiv.org/html/2609.00006v1)).
  - Opt-in worktree: Claude `isolation: worktree` ([docs](https://code.claude.com/docs/en/sub-agents)), Antigravity `branch` ([docs](https://antigravity.google/docs/subagents)), Cursor ([docs](https://cursor.com/docs/subagents)).
  - VM or container per child: Devin ([blog](https://cognition.com/blog/devin-can-now-manage-devins)), Codex cloud attempts ([Vaughan](https://codex.danielvaughan.com/2026/04/01/codex-cloud-exec-best-of-n-attempts/)), Cursor cloud subagents ([docs](https://cursor.com/docs/subagents)).
- Context-passing spectrum:
  - Fresh by default: nearly all harnesses.
  - Optional full fork: Claude forks with cache reuse ([docs](https://code.claude.com/docs/en/sub-agents)), Codex `fork_context` ([source](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/handlers/multi_agents/spawn.rs)).
  - Filtered fork: Codex `LastNTurns` keeps only final answers ([arXiv](https://arxiv.org/html/2609.00006v1)).
  - Forked full context to an advisor: Cognition Smart Friend ([Cognition](https://cognition.com/blog/multi-agents-working)).
- Orchestration-locus divergence: in LLM-turn-by-turn harnesses (most of them) the plan lives in context. In Claude **dynamic workflows** "the script holds the loop, the branching, and the intermediate results", and the docs list "Who decides what runs next: The script". — [CC Workflows](https://code.claude.com/docs/en/workflows). Kiro uses a pre-planned immutable DAG ([Kiro](https://kiro.dev/docs/custom-agents/subagents/)). The OpenAI Agents SDK distinguishes "via LLM" from "via code" orchestration ([OpenAI](https://openai.github.io/openai-agents-python/multi_agent/)).
- Write doctrine divergence: Cognition says parallel writers fail and advocates a single writer ([Cognition](https://cognition.com/blog/multi-agents-working)). Claude `/batch` (5–30 worktree subagents) ([CC](https://code.claude.com/docs/en/agents)), Cursor's 8 parallel worktree agents ([Cursor](https://cursor.com/changelog/2-0)) and Claude workflows' per-file isolated-copy migrations ([CC Workflows](https://code.claude.com/docs/en/workflows)) use parallel writers whose outputs are merged afterward.

### Comparison matrix (compact; see the per-harness sections for sources)

| Harness | Spawn API | Context | Tool scoping | Comms | Return | FS isolation | Concurrency / depth | Cost note |
|---|---|---|---|---|---|---|---|---|
| Claude Code subagents | `Agent` tool + `.claude/agents/*.md` | fresh; fork optional (shares cache) | tools/disallowedTools; background tool subset; inherits bypass/auto | SendMessage, resume by ID | summary; async notification | opt-in worktree (from default branch) | 20 concurrent; depth 3 (env) | own requests count; SDK `maxBudgetUsd` |
| Claude agent teams | named Agent call (experimental flag) | fresh + CLAUDE.md/skills | per-definition tools; lead's permission mode | file mailboxes, peer DMs, shared task list + file locks | idle notif. with final answer | none (partition files) | no hard cap (3–5 advised); no nested teams | ~7× tokens (plan mode) |
| Claude workflows | model-written JS (`agent/parallel/pipeline`) | fresh per agent | session permission rules | none (script variables) | script return; JSON schema | per-agent isolated copy optional | 16 concurrent (≤256); 1,000/run; 4,096 items/call | warning >25 agents or >1.5M tokens |
| Codex CLI | `spawn_agent`/`send_input`/`resume_agent`/`wait`/`close_agent`; TOML agents | fresh; `fork_context` full or last-N filtered | inherits sandbox + approvals (cannot widen) | send_input; mailbox records | consolidated after all done | shared (cloud: container/task) | 6 threads; depth 1 | "more tokens"; mini ≈30% quota |
| Cursor | Task tool; `.cursor/.claude/.codex` agents | fresh | inherits all tools; `readonly` flag | resume by ID | result/error status | shared by default; opt-in worktree or cloud VM | 2 levels; 8 parallel agents (2.0) | ~N× tokens |
| Devin | coordinator auto-delegates (internal MCP) | clean per child | full Devin per child | coordinator messages, sleep/kill | compiled by coordinator (PRs/summaries) | VM per child | undocumented | ACU tracked per child |
| Amp | auto subagent tools (Search/Oracle/Librarian/Task) | given instructions only | per-role model routing | none mid-task | final summary | Orbs (remote machines) | undocumented | subagents billed independently |
| OpenHands SDK | TaskTool / DelegateTool (spawn, delegate) | separate conversation | inherits LLM config | parent-driven | event log + metrics synced | same workspace; resource locks | threads; nesting supported | — |
| Gemini CLI (superseded) | subagent exposed as tool; `.gemini/agents` | own loop | `tools` list; per-subagent policy | none | to main agent | shared | no nesting; max_turns 30, 10 min | — |
| Antigravity | `invoke_subagent`, **`define_subagent` (runtime)** | clean | inherits prefixes/scopes/sandbox; approvals bubble up | message by conversation ID; read transcripts | result msg then Idle | inherit / branch (worktree) / share | depth 10; `/boost` 3-tier | 93-agent demo <$1k (secondary) |
| Kiro | description-matched; custom agents | isolated history | availableAgents/trustedAgents; own tools | — | aggregated by main | shared workspace | pre-planned immutable DAG; loops ≤10 | — |
| Copilot /fleet | orchestrator waves; `.github/agents` | own window | per-agent tools/model | none between subagents | orchestrator merges | shared FS, **no locking** | undocumented | more AI credits |
| Factory | Task tool; `.factory/droids` | fresh | tool categories; read-only ⇒ Low autonomy | none | one final message; TaskOutput | shared | no nesting | — |
| Hermes | `delegate_task`; Kanban swarm | fresh | toolset ∩ parent; 6-tool blocklist | SQLite blackboard (swarm) | summary only | 6 sandbox backends | 3 concurrent; depth 1; 50 iterations | — |

### Inferences
- The convergent "safe core" is fresh-context, read-mostly workers returning condensed summaries under hard depth and concurrency caps. Write-capable parallelism is used only with physical isolation (a worktree or VM) and an explicit merge step.
- The open frontier is where the plan lives (in context, in a script, or in a DAG) and how agents talk to each other (Cognition calls communication "the main open problem").

### Gaps
- No cross-harness controlled benchmark comparing these topologies on the same coding tasks was found. Most performance claims are vendor-reported.

---

## Q10. Who decides topology: the model or the human?

### Takeaway
In most production harnesses **the model decides when to spawn, how many, and often which model and tools to use**. Humans configure the roster (agent files), and the harness enforces caps. The most model-driven are Antigravity (runtime `define_subagent`, depth 10) and Claude Code with ultracode (the model writes orchestration scripts for every task). Kiro (pre-planned immutable DAG), Codex (depth 1 by default, and a delegation mode setting) and Gemini CLI/Factory (no nesting) are the most human-bounded. Best-of-N features (Codex cloud `--attempts`, Cursor 8 parallel agents, Agent HQ multi-vendor assignment) are human-chosen topologies with human selection of the winner.

### Cited Findings
- Claude Code: "Claude decides on its own when to spawn a subagent and how many to spawn … a subagent can spawn subagents of its own, so one prompt can grow into a tree". The per-invocation `model` parameter is set by Claude and overrides frontmatter. — [Agent SDK: Subagents](https://code.claude.com/docs/en/agent-sdk/subagents); [Subagents](https://code.claude.com/docs/en/sub-agents). With ultracode on, "Claude decides when a task warrants a workflow", and it writes the script. The size guideline is advice to the model, "not a cap". — [Workflows](https://code.claude.com/docs/en/workflows). In teams, "Claude decides the number of teammates to spawn based on your task", and teams can form without being asked, because named subagents become teammates. — [Agent teams](https://code.claude.com/docs/en/agent-teams)
- Antigravity: `define_subagent` "creates transient subagents during a session", and `self` clones the caller. — [Antigravity docs](https://antigravity.google/docs/subagents)
- Codex: `spawn_agent` takes a per-call `model` and `reasoning_effort`. — [spawn.rs](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/handlers/multi_agents/spawn.rs). Delegation modes are disabled, explicit-request-only, or proactive. — [arXiv 2609.00006](https://arxiv.org/html/2609.00006v1)
- Devin "automatically breaks down large tasks and delegates to managed Devins when it makes sense" **[via search snippet]**. — [Devin docs](https://docs.devin.ai/work-with-devin/advanced-capabilities)
- Copilot /fleet: "the main agent analyzes your prompt and decides whether it can be split into subtasks". — [GitHub docs](https://docs.github.com/en/copilot/concepts/agents/copilot-cli/fleet)
- Amp selects subagents automatically (mostly in `medium` mode), and Puck splits requests across agents. — [Amp docs](https://ampcode.com/docs/models-and-subagents); [Meet Puck](https://ampcode.com/news/meet-puck)
- Goose "can autonomously decide to use subagents" **[via search snippet]**. — [Goose docs](https://goose-docs.ai/docs/guides/subagents)
- Kiro: the main agent plans the graph, but "the graph cannot be changed during execution", and loops are bounded at 1–10. — [Kiro docs](https://kiro.dev/docs/custom-agents/subagents/)
- Human-chosen N: Codex `--attempts 1–4` **[secondary]** ([Vaughan](https://codex.danielvaughan.com/2026/04/01/codex-cloud-exec-best-of-n-attempts/)); Cursor "up to eight agents in parallel on a single prompt" ([Cursor 2.0](https://cursor.com/changelog/2-0)); Agent HQ "assign multiple agents to a task" ([AlternativeTo](https://alternativeto.net/news/2025/10/github-unveils-agent-hq-a-central-hub-for-ai-coding-agents-from-top-providers)).
- Model propensity matters. "Claude Opus 5 delegates to subagents more readily than earlier models", so Anthropic adds a no-delegation-unless-asked line on Opus 5 and recommends harness limits, because "Either instruction only steers Claude … Claude Code enforces [the limits] however Claude decides to delegate." — [Agent SDK: Subagents](https://code.claude.com/docs/en/agent-sdk/subagents)

### Inferences
- The industry pattern is "model chooses topology, harness enforces envelope". The envelope moved from prompts to hard caps after runaway-recursion incidents (Claude #68619; Anthropic's 2025 report of 50 subagents spawned for simple queries).

### Gaps
- No harness was found that lets the agent persistently edit its own sub-agent roster and caps under a formal gate. Antigravity's `define_subagent` is session-transient as far as the docs say.

---

## Implications for INIT

These are relevance notes for the recommender, mapped to ADR-011. ADR-011 already fixes: T0 `agents/<name>/AGENT.md` (role prompt, tool subset, model route, budget share), `fleet spawn{agent}` from L0, `quotas.parallel_bodies = 4`, members that read the home and never write the workspace without a grant, members that cannot create members, and "sub-agents unavailable in replays".

1. **Adopt the converged definition format, and keep it cross-readable.** Markdown+YAML with `name`, `description`, `tools` (allowlist) plus `disallowedTools`, `model`, `effort`, `maxTurns`, `background`, `isolation`, and a budget share. This matches Claude Code, Cursor, Gemini, Antigravity, Factory and Copilot, and the `agents/<name>/AGENT.md` row in ADR-011. Cursor's reading of `.claude/agents` and `.codex/agents` suggests INIT could import those files read-only as seeds.
2. **Default to fresh context with an explicit prompt, and offer a cheap fork option.** Claude forks reuse the parent's prompt cache. Codex `fork_context` and `LastNTurns` keep only final answers. For INIT's money ceilings, cache-sharing forks plus sibling prefix staggering (Claude workflows hold siblings up to 5 s) are the biggest cost lever.
3. **Read-only members at L0 are the industry's safest consensus.** It matches Cognition's "contribute intelligence rather than actions" and single-writer doctrine, Factory's rule that read-only droids never run above Low autonomy, Mistral Vibe's "subagent"-tagged-profiles-only guard, and the read-only explorers that every harness ships. When a workspace-write grant is given, require physical isolation: a per-member git worktree (Claude `isolation: worktree`, Antigravity `branch`, Cursor) or a separate Seatbelt instance, plus an explicit merge by the single writer. Copilot /fleet's silent last-writer-wins shows that prompt discipline is not enough.
4. **Enforce the envelope in Root and the broker, not in prompts.** Production defaults: depth 1 (Codex, Hermes) up to 3 (Claude) or 10 (Antigravity); concurrency 3 to 20; spend caps that refuse new spawns and **stop running background children** (Claude SDK `maxBudgetUsd`). Copy Codex's refusal string ("Agent depth limit reached. Solve the task yourself."). INIT's "members cannot create members" equals depth 1, the most conservative production setting. Runaway evidence such as Claude #68619 (more than 50 levels, 1.2M tokens in 30 minutes, children spawned to route around permission denials) argues for Root-side counters and H6-style spawn-churn signals, which ADR-011 already has (more than 120 zone spawns per 60 minutes).
5. **Treat member outputs as untrusted data.** Claude Code scans subagent reports for injected control tags and turn markers, labels inter-agent messages as non-user, forbids relayed consent, and has its auto-mode classifier review messages. INIT should apply the same to member→body returns, and never let a member's text count as an owner turn or grant. This fits the Root's owner-socket-only grants.
6. **Scripted orchestration is a natural self-extension surface.** Claude dynamic workflows (a model-written JS script; `agent`, `parallel`, `pipeline`; deterministic replay because `Date.now` and `Math.random` throw; resumable by replaying cached agent results; stall restarts at most 5) are an orchestration artifact the agent can write, save and rerun. For INIT this could be a T0 or T1 "fleet script", which would also address ADR-011's replay-fidelity gap, because deterministic scripts plus cached child results are replayable.
7. **Model-defined roles at runtime map onto reward-free self-extension.** Antigravity's `define_subagent` and `self` clone are the production precedent for letting the model invent sub-agent types. INIT can go further by persisting them as T0 `AGENT.md` under change groups, which is something no surveyed harness does under a formal gate.
8. **Use the "Smart Friend" or Oracle as a first-class member kind.** A stronger model is consulted read-only, with a forked or curated context (Amp Oracle on GPT-5.4; Cognition's Smart Friend). It is cheap, read-only, and it raises quality on hard steps. It maps to ADR-011's per-subtask model routing at L0. Caveat from Cognition: a weak primary model does not know when to escalate, so give it escalation triggers.
9. **Use a clean-context reviewer as a member pattern.** Devin Review sees only the diff (about 2 bugs per PR, about 58% severe), which works best with no shared prior context. This pairs with INIT's blind judge and gates.
10. **Account cost per child.** Devin tracks ACU per child. Pi parses child JSONL for cost. Claude's `/usage` attributes usage to subagents. INIT's `x-init-caps` header plus the Root usage ledger should tag every member request with its member ID and spec, so harm and cost attribution covers sub-agent patterns.
11. **Coordination primitives worth keeping small:** a shared task list with file-lock claims and dependency unblocking (Claude teams), an immutable pre-planned DAG with bounded review loops of at most 10 (Kiro), and resource locks for parallel tool calls (OpenHands). Peer messaging (Claude, Antigravity) is useful but is where the documented failures cluster: task status lag, early stops, and children not reporting back (Cognition).
12. **Size expectations.** Expect a multi-agent run to cost several times a single session: about 15× chat (Anthropic, 2025), about 7× a session for plan-mode teams (Claude), about N× for N parallel workers (Cursor). Anthropic found multi-agent a poor fit for "most coding tasks". With `parallel_bodies = 4` and read-only members, INIT sits in the efficient zone of this curve.
