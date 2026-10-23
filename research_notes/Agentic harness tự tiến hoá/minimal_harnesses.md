# Minimal Agent Harnesses: Implementation Dossier and Evidence That Minimal Matches Heavy (as of 2026-10-19)

Scope note: sources were fetched on 2026-10-19. Where a figure comes from a WebFetch summary of source code (line counts, for example), it is marked "approx." because the fetch tool summarized the file rather than counting lines exactly. Numbers read off screenshots of images are marked as such.

## Q1. Pi (pi-mono, now earendil-works/pi): architecture, tools, prompt, extensions, sessions, self-extension, benchmarks, products

### Takeaway
Pi is a four-package TypeScript harness (pi-ai for the LLM API, pi-agent-core for the loop, pi-tui for the terminal UI, pi-coding-agent for the CLI). Its core is four tools (read, write, edit, bash) and a system prompt that, together with the tool definitions, was under 1,000 tokens at launch. Everything else comes from hot-reloadable TypeScript extensions, skills and packages, which the agent is encouraged to write for itself. Session files are JSONL trees. Three things have changed since the November 2025 design post. Earendil acquired Pi in April 2026 and moved the repo to `earendil-works/pi`. Pi 1.0 shipped on 2026-10-01 with MCP built in, delivered through a sandboxed "codemode" tool, which reverses the original no-MCP stance. And OpenClaw, the best-known product built on Pi, has since absorbed the agent runtime into its own code, keeping only pi-tui.

### Cited Findings

#### Status, ownership and versions (October 2026)
- The repo now lives at `github.com/earendil-works/pi`. Its About text reads "AI agent toolkit: unified LLM API, agent loop, TUI, coding agent CLI." It is MIT-licensed, with about 116.3k stars, 14.8k forks and 6,996 commits on main as of 2026-10-19. — [GitHub earendil-works/pi](https://github.com/earendil-works/pi)
- The packages in the monorepo are `@earendil-works/pi-ai`, `pi-agent-core`, `pi-coding-agent` and `pi-tui`, plus three newer ones: `chord` (a typed runtime for hot-reloading plugins), `pi-telemetry` and `pi-durable` (a runtime for persistent conversations). — [GitHub earendil-works/pi](https://github.com/earendil-works/pi)
- Mario Zechner announced on 2026-04-08 that he had joined Earendil and that "pi is owned by Earendil, the company." The repo moved from `badlogic/pi-mono` to `earendil-works/pi`. The core stays MIT ("MIT, forever. Non-negotiable."), with no CLA or DCO. Commercial tiers are planned: Fair Source/DOSP features and proprietary enterprise/cloud features. Mario, Armin and Colin make the technical decisions. — [Mario Zechner, "I've sold out"](https://mariozechner.at/posts/2026-04-08-ive-sold-out/)
  - Conflicting detail on the npm scope: that post planned `@earendil/pi`, but the current README installs `@earendil-works/pi-coding-agent`. — [I've sold out](https://mariozechner.at/posts/2026-04-08-ive-sold-out/); [coding-agent README](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/README.md)
  - Conflicting detail on the Earendil co-founder: one account names Colin Sidoti, while The Register names "Colin Daymond Hanna". — [WebSearch summary of letsdatascience/eesel coverage](https://www.eesel.ai/blog/pi-coding-agent); [The Register](https://www.theregister.com/ai-and-ml/2026/10/02/pi-coding-agent-pulls-a-180-and-adds-mcp-support/5300678)
- Pi 1.0 shipped on 2026-10-01. It includes native MCP via "Codemode", virtual-model extensions, deferred tool loading, Anthropic cache warming, mid-conversation system messages, a new TUI theme and full-screen mode by default. The experimental Pi Durable is a separate package that orchestrates multiple conversations with SQLite + JSONL storage, and Earendil kept it separate so Pi itself stays minimal. — [The Register, 2026-10-02](https://www.theregister.com/ai-and-ml/2026/10/02/pi-coding-agent-pulls-a-180-and-adds-mcp-support/5300678); [Techzine, 2026-10-02](https://www.techzine.eu/news/devops/144715/pi-1-0-will-include-mcp-and-a-dedicated-layer-for-long-running-ai-agents/)
- Earendil's rationale for the MCP reversal: "Ultimately what Pi needs is quite similar to what MCP needs: a sandbox to play with in the form of an interpreter." — [The Register](https://www.theregister.com/ai-and-ml/2026/10/02/pi-coding-agent-pulls-a-180-and-adds-mcp-support/5300678)
- The pi.dev "What we didn't build" list now reads: MCP is built in via Codemode (MCP extensions are still supported); sub-agents mean tmux or extensions; permission popups mean a container or an extension; plan mode means files or an extension; to-dos mean TODO.md; background bash means tmux. — [pi.dev](https://pi.dev/)
- A review checked on 2026-09-15 (before 1.0) listed the stable release as v0.85.1 (2026-09-05) and said there was "no built-in MCP client", which is consistent with MCP arriving at 1.0. A GitHub release v1.1.0 also exists, date unconfirmed. — [rywalker.com research on Pi](https://rywalker.com/research/pi); [newreleases.io v1.1.0](https://newreleases.io/project/github/earendil-works/pi/release/v1.1.0)

#### Design rationale (Mario Zechner, 2025-11-30)
- Principle: "if I don't need it, it won't be built." The goals are full control over context with no hidden injections, inspectability of every model interaction, a cleanly documented session format, and working with self-hosted models. That last goal is why pi calls provider APIs directly instead of going through the Vercel AI SDK. — [What I learned building an opinionated and minimal coding agent](https://mariozechner.at/posts/2025-11-30-pi-coding-agent/)
- The system prompt plus tool definitions come to "below 1000 tokens", and only AGENTS.md files are appended. — [same post](https://mariozechner.at/posts/2025-11-30-pi-coding-agent/)
- Reasons for leaving features out (original stance):
  - **No MCP:** "MCP servers are overkill for most use cases." Playwright MCP is 21 tools (13.7k tokens) and Chrome DevTools MCP is 26 tools (18k tokens). The alternative is CLI tools with READMEs that the agent reads on demand.
  - **No sub-agents:** they are a "black box within a black box" with poor context transfer, so spawn `pi` via bash instead.
  - **No plan mode:** write PLAN.md, or restrict tools with `pi --tools read,grep,find,ls`.
  - **No to-dos:** they "generally confuse models more than they help", so use TODO.md.
  - **YOLO by default:** "As soon as your agent can write code and run code, it's pretty much game over"; use a container.
  - **No background bash:** use tmux.
  — [same post](https://mariozechner.at/posts/2025-11-30-pi-coding-agent/)
- Earlier Zechner posts behind the no-MCP stance: "MCP vs CLI: Benchmarking Tools for Coding Agents" (2025-08-15) and "What if you don't need MCP at all?" (2025-11-02). — [mariozechner.at index](https://mariozechner.at/)
- Mario's motivation, per the Pragmatic Engineer interview: Claude Code had become unpredictable as features piled up, so he wanted a harness that behaves consistently, with as few features as possible. — [Pragmatic Engineer, "Building Pi, and what makes self-modifying software so fascinating"](https://newsletter.pragmaticengineer.com/p/building-pi-and-what-makes-self-modifying)
- Missing compaction at launch: "Missing compaction hasn't been a problem for me personally." Compaction was added later (see below). — [blog, read via browser](https://mariozechner.at/posts/2025-11-30-pi-coding-agent/)

#### Package architecture
- **pi-ai** is a unified LLM API.
  - Calls: `models.stream(model, context, options)` (an event stream you consume with `for await`, then `s.result()`), `models.complete(...)`, the `streamSimple`/`completeSimple` variants that take a unified `reasoning` option, and `models.getModel(provider, id)`.
  - `Context = { systemPrompt, messages, tools }` is plain JSON-serializable data. Message roles are `user`, `assistant` and `toolResult` (a toolResult references `toolCallId`). Content blocks are `text`, `thinking`, `image` and `toolCall`.
  - API types: `anthropic-messages`, `openai-completions`, `openai-responses`, `openai-codex-responses`, `azure-openai-responses`, `google-generative-ai`, `google-vertex`, `mistral-conversations`, `bedrock-converse-stream`.
  - Tools are TypeBox schemas, checked with `validateToolCall`. On a validation failure you return an `isError: true` tool result so the model can retry.
  - Every assistant message carries `usage` and `cost.total`.
  - An `AbortSignal` produces `stopReason: 'aborted'` and keeps partial content.
  — [pi-ai README](https://github.com/earendil-works/pi/blob/main/packages/ai/README.md)
- **Cross-provider handoff (current README):** user and toolResult messages pass through unchanged. Assistant messages from the same provider, API and model are kept as-is, including thinking. Assistant messages from a different model have their thinking blocks dropped. Text and tool calls carry over everywhere. — [pi-ai README](https://github.com/earendil-works/pi/blob/main/packages/ai/README.md)
  - This differs from the November 2025 blog, which said Anthropic thinking traces were converted to `<thinking>`-tagged text blocks on handoff and described the whole thing as "best-effort". Treat it as a behavior change or simplification. — [blog](https://mariozechner.at/posts/2025-11-30-pi-coding-agent/)
- **pi-agent-core** is the loop.
  - `new Agent({ initialState: {systemPrompt, model, thinkingLevel, tools, messages}, streamFn, convertToLlm, transformContext, getApiKey, steeringMode, followUpMode, toolExecution: "parallel"|"sequential", beforeToolCall, afterToolCall, prepareRequest, finishTurn, ... })`.
  - Methods: `prompt`, `continue`, `steer`, `followUp`, `subscribe`, `abort`, `waitForIdle`, `reset`.
  - `AgentTool = { name, label, description, parameters (TypeBox), executionMode?, execute(toolCallId, params, signal, onUpdate) → {content, details} }`. Throwing makes the result `isError: true`.
  - Custom message roles are added via declaration merging on `CustomAgentMessages` and filtered out in `convertToLlm`.
  — [pi-agent-core README](https://github.com/earendil-works/pi/blob/main/packages/agent/README.md)
- **Loop source** (`packages/agent/src/agent-loop.ts`, approx. 600–650 lines by summary):
  - Exports `agentLoop(prompts, context, config, signal, streamFn)` and `agentLoopContinue(...)`, which return `EventStream<AgentEvent, AgentMessage[]>`, plus `runToolCall` for tools that call tools.
  - The outer loop is `while (true)`; it continues only when follow-up messages arrive or a continuation is requested. The inner loop is `while (hasMoreToolCalls || pendingMessages.length > 0)`.
  - Tool calls run in parallel by default. Calls are prepared in order, run with `Promise.all`, and their results are emitted in the original order. A call runs sequentially if `config.toolExecution === "sequential"` or if any call is marked sequential.
  - If `stopReason === "length"`, no tools run, and every call gets an error asking the model to re-issue it.
  - An unknown tool returns "Tool X not found".
  - Events: `agent_start`, `turn_start`, `message_start/update/end`, `tool_execution_start/update/end`, `turn_end`, `agent_end`.
  — [agent-loop.ts](https://github.com/earendil-works/pi/blob/main/packages/agent/src/agent-loop.ts)
- **pi-tui** is a minimal terminal UI framework. It uses differential rendering, synchronized output to avoid flicker, and editor and markdown components. — [blog](https://mariozechner.at/posts/2025-11-30-pi-coding-agent/)
- **pi-coding-agent** is the CLI. It has four modes (interactive TUI, print/JSON, RPC, TypeScript SDK) and 15+ providers via API key or OAuth (`/login`, `/model`, Ctrl+L). Steering works with Enter (steer the running agent) and Alt+Enter (queue a follow-up). — [pi.dev](https://pi.dev/); [coding-agent README](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/README.md)
- The request lifecycle: a message is appended to the active branch. The request is built from the system prompt, the active branch, the tools and the model settings. The response streams and is recorded, tools execute and their results are recorded (one turn), and another turn starts if tool results or queued messages need one. — [how-pi-works.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/how-pi-works.md)

#### The four core tools (current schemas)
- **read** `{ path: string ("Path to the file to read (relative or absolute)"), offset?: number ("Line number to start reading from (1-indexed)"), limit?: number ("Maximum number of lines to read") }`.
  - It handles text and images (jpg/png/gif/webp/bmp; images are auto-resized and attached as image blocks).
  - Output is truncated at `DEFAULT_MAX_LINES` / `DEFAULT_MAX_BYTES`. The blog gives 2,000 lines; the byte limit wasn't visible in the fetched file.
  - Continuation hint: `[Showing lines S-E of T. Use offset=N to continue.]`.
  - read.ts is approx. 190 lines.
  — [read.ts](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/tools/read.ts); [blog](https://mariozechner.at/posts/2025-11-30-pi-coding-agent/)
- **write** `{ path, content }`. It creates or overwrites the file and creates parent directories. — [blog](https://mariozechner.at/posts/2025-11-30-pi-coding-agent/)
- **edit** now takes `{ path: string, edits: [{ oldText: string, newText: string }, ...] }` with at least one edit.
  - Every `oldText` must be exact, unique in the original file, and non-overlapping. Edits are matched against the original file, not applied incrementally.
  - The BOM is stripped, and line endings are normalized to LF for matching and then restored.
  - The legacy top-level `{path, oldText, newText}` form, the November 2025 schema, is still accepted by `prepareEditArguments`.
  - Result: "Successfully replaced N block(s) in {path}.", with `details.diff`, `details.patch` and `details.firstChangedLine`.
  - edit.ts is approx. 230 lines.
  — [edit.ts](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/tools/edit.ts)
- **bash** `{ command: string ("Shell command to execute"), timeout?: number ("Timeout in seconds (optional, no default timeout)") }`.
  - Output is tail-truncated by lines/bytes, and the full output goes to a `pi-bash` temp file whose path is appended.
  - Structured output: `{output, truncated, full_output_path?, exit_code, wall_time_seconds}`.
  - Processes are spawned detached and the whole process tree is killed on timeout or abort. A non-zero exit returns `isError: true` and "Command exited with code N."
  - It exports `PI_SESSION_ID`, `PI_SESSION_FILE`, `PI_PROVIDER`, `PI_MODEL` and `PI_REASONING_LEVEL` to the child.
  - A pluggable `BashOperations.exec(command, cwd, {onData, signal, timeout, env}) → {exitCode}` lets you redirect execution (e.g. SSH).
  - bash.ts is approx. 370 lines.
  — [bash.ts](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/tools/bash.ts)
- Optional built-ins: read-only `grep`, `find` and `ls` (off by default), and `powershell` on Windows. — [blog](https://mariozechner.at/posts/2025-11-30-pi-coding-agent/); [rywalker](https://rywalker.com/research/pi)

#### System prompt (current source, `system-prompt.ts`, approx. 226 lines)
- Default preamble: "You are an expert coding assistant operating inside pi, a coding agent harness." — [system-prompt.ts](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/system-prompt.ts)
- Section order: `preamble` (untagged), `tools`, `rules`, `docs`, `addendum` (appendSystemPrompt), `project_context` (context files rendered as `<project_instructions path="...">`), `skills` (only when a read-capable tool is selected), `cwd`, then any custom sections. Each section is wrapped in a tag named after it. — [system-prompt.ts](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/system-prompt.ts)
- Tool lines are rendered as `- ${name}: ${toolSnippets[name]}`, followed by "In addition to the tools above, you may have access to other custom tools depending on the project." The rules include "Use bash for file operations like ls, rg, find" (when grep/find/ls are off), "Be concise in your responses" and "Show file paths clearly when working with files". — [system-prompt.ts](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/system-prompt.ts)
- The **docs section**, which is the self-extension hook, reads: "Pi documentation (read only when the user asks about pi itself, its SDK, extensions, themes, skills, or TUI):" It is followed by the README, docs and examples paths and a topic-to-file map (extensions, themes, skills, prompt templates, TUI, keybindings, SDK, custom providers, models, packages, env vars, MCP servers, codemode). — [system-prompt.ts](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/system-prompt.ts)
- The prompt is now section-diffable. `diffSystemPromptSections(previous, current)` produces patches, and these are persisted as `role: "system"` message entries carrying `sections` and `toolsAdded`/`toolsRemoved`. This is the Pi 1.0 "mid-conversation system messages" feature. — [system-prompt.ts](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/system-prompt.ts); [session-format.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/session-format.md)
- Users can customize via AGENTS.md and SYSTEM.md files. — [pi.dev](https://pi.dev/)

#### Extensions (the main extension mechanism)
- Location: `~/.pi/agent/extensions/` (global) or the project extensions dir. A `.ts`/`.js` file or a directory with `index.ts`. Loaded with **jiti**, so there is no build step. For development: `pi --extension ./hello.ts`. — [extensions.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md)
- Minimal extension (verbatim from the docs):
  ```ts
  import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
  export default function (pi: ExtensionAPI) {
    pi.registerCommand("hello", {
      description: "Show a greeting",
      handler: async (name, ctx) => { ctx.ui.notify(`Hello, ${name || "world"}!`, "info"); },
    });
  }
  ```
  — [extensions.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md)
- API methods: `pi.on()`, `registerTool()`, `registerCommand()`, `registerShortcut()`, `registerFlag()`, `registerProvider()`, `registerMcpServer()`/`unregisterMcpServer()`/`getMcpServers()`, `registerVirtualModel()`, `registerToolRenderer()`, `sendUserMessage()`, `sendMessage()`, `appendEntry()` (persists session data excluded from the model context), `getActiveTools()`/`setActiveTools()`/`getAllTools()`, and `events` (an inter-extension bus). UI: `ctx.ui.notify/confirm/custom(...)`, plus widgets, status and editor access. — [extensions.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md)
- A custom tool has `name`, `description`, a TypeBox `parameters` schema and `execute()`, which returns `{content, details}`. `terminate: true` ends the auto-continuation if every tool in the batch agrees, and `outputSchema`/`structuredContent` are optional. Tools can't be unregistered, but re-registering with `exposure: "hidden"` withdraws them. — [extensions.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md)
- Interceptable events:
  - **Input:** `input` (transform or handle).
  - **Agent run:** `before_agent_start` (modify or replace the prompt), `agent_start`/`agent_end`, `agent_before_settle`, `agent_settled`.
  - **Turns and context:** `turn_start`, `turn_end` (can return `continue: true`), `context` (transform the messages for one request), `context_with_system`.
  - **Provider:** `before_provider_headers`, `before_provider_request` (rewrite the payload), `after_provider_response`, `provider_stream_event`, `cache_warming_decision`.
  - **Messages:** `message_start/update/end` (`message_end` can replace the message).
  - **Tools:** `tool_call` (mutate the input or block), `tool_result` (redact or replace), `tool_execution_*`.
  - **Sessions:** `session_start/shutdown`, `session_before_switch/fork`, `session_before_compact`, `session_compact_failed`, `session_before_tree`.
  - **Resources:** `project_trust`, `resources_discover`, `mcp_servers_change`.
  - **Shell:** `user_bash`.
  — [extensions.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md); [compaction.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/compaction.md)
- Hot reload: `/reload` re-applies changes ("Pi can change itself in place"). Programmatically, `await ctx.reload()` replaces the extension runtime. — [pi.dev](https://pi.dev/); [extensions.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md)
- Example extensions ship for sub-agents, plan mode, permission gates, sandboxing, SSH and MCP. Bundles of extensions, skills, prompt templates and themes are distributed as "Pi packages" via npm or git. — [pi.dev](https://pi.dev/)
- Security caveat: extensions run in-process with the OS permissions of the pi process, and there is no sandbox by default. — [how-pi-works.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/how-pi-works.md); [rywalker](https://rywalker.com/research/pi)

#### Skills and prompt templates
- Skills follow the Agent Skills spec: a directory with `SKILL.md` whose frontmatter has `name` (lowercase/digits/hyphens, ≤64 chars) and `description` (≤1024 chars). Optional fields are `allowed-tools`, `disable-model-invocation` and others.
  - Discovery covers the user and project skills dirs plus `~/.agents/skills/` and `.agents/skills/`, walking up to the repo root.
  - Progressive disclosure: only the name, description and path go into the system prompt, and the model reads SKILL.md on demand.
  - `/skill:name args` forces a load.
  — [skills.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/skills.md)
- Prompt templates are `/name` templates that expand editor input before it becomes a user message. — [pi.dev](https://pi.dev/); [how-pi-works.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/how-pi-works.md)

#### Session storage (JSONL tree)
- Path: `~/.pi/agent/sessions/--<path>--/<timestamp>_<session-id>.jsonl`, where the cwd is encoded with `/ \ :` turned into `-`. — [session-format.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/session-format.md)
- Format versions: v1 was linear (auto-migrated), v2 added the tree via `id`/`parentId`, and v3 (current) renamed the `hookMessage` role to `custom`. The header line is `{"type":"session","version":3,"id":"uuid","timestamp":"...","cwd":"/path/to/project"}`. — [session-format.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/session-format.md)
- Entry types: `message` (user/assistant/toolResult/system), `model_change`, `thinking_level_change`, `compaction` (`summary`, `firstKeptEntryId`, `tokensBefore`), `context_edit` (`targetId`, `replacement`), `branch_summary` (`fromId`, `summary`), `custom` (extension state, not in the LLM context), `custom_message` (in the context), `label`, `session_info`, `usage`. Every non-header entry has `type`, `id` (8-hex), `parentId` and `timestamp`. — [session-format.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/session-format.md)
- Tree semantics: the leaf is the current position, and the context is built by walking from the leaf to the root. Branching just adds a child to an earlier entry in the same file. The latest `compaction` on the path sets where retained entries begin. — [session-format.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/session-format.md)
- UI: `/tree`, bookmarks/labels, `/export` (HTML) and `/share` (gist). — [pi.dev](https://pi.dev/)

#### Compaction (added after launch)
- Auto-compaction triggers when `contextTokens > contextWindow - reserveTokens`, with defaults `reserveTokens` 16384 and `keepRecentTokens` 20000.
- The cut point is chosen by walking back until the keep budget is filled, never cutting at a tool result.
- The summary uses the sections Goal, Constraints & Preferences, Progress (Done/In Progress/Blocked), Key Decisions, Next Steps and Critical Context, plus `<read-files>`/`<modified-files>` lists.
- `/compact [instructions]` compacts manually. When you navigate in `/tree`, the abandoned branch is summarized into a `branch_summary` entry.
- Extensions can override via `session_before_compact`.

— [compaction.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/compaction.md)

#### Codemode / MCP (Pi 1.0)
- `codemode` is a tool whose argument is a JavaScript async-function body. It runs in a **QuickJS sandbox** with no Node, filesystem, network or timers, and calls pi tools and non-LLM `models` from there. Only the script output reaches the model.
- `codemode.mode`: `on` (default once enabled) keeps the declared tools visible, while `only` hides them behind codemode.
- Deferred-exposure tools, including MCP tools by default, are discovered via `searchTools()`, `describeTool()` and `ALL_TOOLS`.
- MCP tools appear as `tools.mcp__server__tool`. The inline tool-declaration budget is about 3,000 tokens, and script output is capped at 10,000 tokens.
- Codemode must be enabled; the default enablement isn't stated in the doc.

— [codemode.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/codemode.md)

#### Self-extension
- Armin Ronacher (2026-01-31): instead of downloading extensions, you "ask the agent to extend itself" or have it remix an existing one. Hot reload lets the agent write an extension, reload it, test it and iterate. Extensions can persist state into sessions and render custom TUI.
  - His agent-written extensions: `/answer`, `/todos` (markdown in `.pi/todos`), `/review` (runs in a separate branch), `/control` (one pi prompting another), `/files`, a CDP browser skill, and a pip→uv redirect.
  - The session tree is used for side quests and reviews without polluting the main context.
  — [Armin Ronacher, "Pi: The Minimal Agent Within OpenClaw"](https://lucumr.pocoo.org/2026/1/31/pi/)
- Pragmatic Engineer: Pi is a "minimalist, self-modifying" agent. Users build a bespoke harness for each task, and Mario sees this as a preview of self-modifiable software. — [Pragmatic Engineer](https://newsletter.pragmaticengineer.com/p/building-pi-and-what-makes-self-modifying)
- The mechanism in code: the system-prompt docs section points the model at pi's own docs and examples, extensions are loaded with no build step (jiti), and `/reload` applies them. — [system-prompt.ts](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/system-prompt.ts); [extensions.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md); [pi.dev](https://pi.dev/)

#### Terminal-Bench results for Pi
- Mario ran Terminal-Bench 2.0 with Claude Opus 4.5 in pi, 5 trials per task (leaderboard-eligible), and compared pi against other harnesses with their native models. He said Terminus 2 was "holding its own". — [blog](https://mariozechner.at/posts/2025-11-30-pi-coding-agent/)
- Run 1 (screenshot of the result image): 428 trials, 71 errors (57 AgentTimeoutError), mean 0.479, 213 passes and 215 fails. On his rendered leaderboard: **pi + Opus 4.5 = 49.8% ± 2.4, rank 7** as of 2025-12-02. — [terminal-bench.png](https://mariozechner.at/posts/2025-11-30-pi-coding-agent/media/terminal-bench.png); [results.jpeg](https://mariozechner.at/posts/2025-11-30-pi-coding-agent/media/results.jpeg)
- Run 2 (CET hours, screenshot): **51.2% ± 2.9, 152/297 passed, 69 errors, rank #6 of 61**. — [results2.png](https://mariozechner.at/posts/2025-11-30-pi-coding-agent/media/results2.png)
- Pi does **not** appear on the official tbench.ai Terminal-Bench 2.0 leaderboard (137 rows checked on 2026-10-19). Same-model official rows for Opus 4.5: Droid 63.1, Letta Code 59.1, Mux 58.4, Terminus 2 57.8, Goose 54.3, Claude Code 52.1, OpenHands 51.9, OpenCode 51.7. — [tbench.ai TB2 leaderboard](https://www.tbench.ai/leaderboard/terminal-bench/2.0)

#### Products built on Pi
- OpenClaw (Peter Steinberger) was published as Clawdbot in November 2025, renamed Moltbot on 2026-01-27 after an Anthropic trademark request, and renamed OpenClaw around 2026-01-30. Sources disagree on the exact interval. — [Wikipedia: OpenClaw](https://en.wikipedia.org/wiki/OpenClaw); [alternativeto](https://alternativeto.net/news/2026/1/the-viral-ai-agent-clawdbot-rebrands-a-third-time-and-now-has-a-social-network-for-bots/)
- OpenClaw today is a local "Gateway" control plane for sessions, tools, events and channels (WhatsApp, Telegram, Slack, Discord, Signal, iMessage, Teams and "20+ more"), plus companion apps/nodes (voice, canvas, camera), swappable model/harness plugins, skills via ClawHub, and optional sandboxing. It has about 392.4k stars and thanks Mario "for pi". — [GitHub openclaw/openclaw](https://github.com/openclaw/openclaw)
- Historically OpenClaw embedded Pi in-process. It imported `createAgentSession()`/`SessionManager` from pi-coding-agent and pi-agent-core, with entry `runEmbeddedPiAgent()`; one docs copy pins pi-agent-core 0.49.3. It reached MCP through **mcporter** (MCP as CLI/TS bindings) rather than native MCP. — [docs.openclaw.ai/pi (search summary)](https://docs.openclaw.ai/pi); [Armin Ronacher](https://lucumr.pocoo.org/2026/1/31/pi/)
- **Current state:** OpenClaw's runtime is "OpenClaw-owned", and "No external agent framework packages remain". Only `@earendil-works/pi-tui` is still a dependency. Its own `packages/agent-core` (`@openclaw/agent-core`) exists, and the legacy runtime id `pi` normalizes to `openclaw`. OpenClaw adds its own tool policy, before/after tool adapters, compaction safeguards and context pruning, and a harness registry (built-in vs plugin harnesses such as `codex`). — [docs.openclaw.ai/agent-runtime-architecture](https://docs.openclaw.ai/agent-runtime-architecture); [docs.openclaw.ai/pi](https://docs.openclaw.ai/pi)
- Other Pi-derived projects seen in search results but not verified from primary sources: "Oh My Pi (omp)", and "Pu.sh", a "full coding-agent harness in 400 lines of shell" inspired by Pi (same 7-tool surface, exact-text edits). HN commenters said the shell was minified to hit the line count. — [yuv.ai on Oh My Pi](https://yuv.ai/blog/oh-my-pi-omp-explained); [Show HN: Pu.sh](https://hn.svelte.dev/item/47968112)

### Inferences
- **Reimplementation sketch of Pi's core, reconstructed from the sources above:**
  ```
  state = {systemPrompt(sections), model, tools=[read,write,edit,bash], messages=[]}   # tree in JSONL
  on user input: append message entry (parentId = leaf); leaf = new id
  loop:                                   # outer: re-enter if follow-ups queued
    pending = steering_queue.drain()
    while has_tool_calls or pending:
      emit turn_start; append pending msgs
      ctx = walk(leaf→root) with latest compaction cut + context_edits  →  extension 'context' hooks
      if tokens(ctx) > window - 16384: compact(keep last ~20k tokens; structured summary)
      resp = provider.stream(ctx)        # pi-ai normalizes providers, usage, cost
      append assistant entry
      if resp.stopReason=="length": error results for all calls; continue
      calls = resp.toolCalls → 'tool_call' hooks may block/mutate → validate(TypeBox)
      results = parallel(execute) (sequential if any tool requires) → 'tool_result' hooks
      append toolResult entries; has_tool_calls = calls nonempty and not all terminate
      pending = steering_queue.drain()
    if follow_up_queue nonempty: pending = it; continue
    break
  ```
- A faithful minimal Pi clone needs about four modules: provider adapter, loop, four tools and a JSONL tree store. The extension system (event hooks plus registerTool plus hot reload) is what makes Pi a substrate for self-evolution. The agent edits a TypeScript file under `~/.pi/agent/extensions/`, calls `/reload`, and its own capabilities change without a rebuild. The session tree adds a natural sandbox for trying an extension on a branch.
- The 1.0 changes show the minimalism is about the *core*, not the product. MCP came back only through a sandboxed interpreter (codemode) with deferred tool loading, which preserves the "small, stable prompt" property. Long-running needs went into a separate package (Pi Durable) rather than the core.
- OpenClaw internalizing the runtime suggests Pi's core is small enough to fork and own outright. That is a strength for a self-evolving harness: the whole core can live in your repo and be modified by the agent itself.
- Pi's self-reported TB2 score with Opus 4.5 (about 50–51%) sits roughly level with Claude Code (52.1) and OpenCode (51.7) and below Terminus 2 (57.8). The unofficial run had many timeouts (57 of 428), so harness timeouts and API error rates probably depressed it. This is an inference from the error distribution.

### Gaps
- No exact lines-of-code count for any Pi package was found. The file sizes above are approximate summaries of the fetched files, not measured counts; a `cloc` on a clone would settle it.
- The exact current read-tool byte limit (`DEFAULT_MAX_BYTES`) and the bash truncation defaults live in `truncate.ts`, which wasn't fetched. The blog's "2000 lines" is for read; "50KB" is unconfirmed.
- Whether codemode/MCP is enabled by default in 1.0 or 1.1 is unclear. The codemode doc says you must enable it, and pi.dev says it's "built in".
- The project-level extension directory path (likely `.pi/extensions/`) and the AGENTS.md/CLAUDE.md lookup order weren't confirmed from fetched docs.
- No official Earendil blog post for Pi 1.0 was fetched directly. Dates come from The Register and Techzine.
- No independent benchmark of Pi 1.0 was found.

## Q2. mini-swe-agent: a ~100-line, bash-only agent

### Takeaway
mini-swe-agent (Princeton/Stanford SWE-agent team) is the official SWE-bench "bash-only" harness. It has one bash action run as an independent `subprocess.run` per step, a strictly linear message history, and no compaction. With Claude 4.5 Opus it reaches 76.8% on SWE-bench Verified versus Anthropic's self-reported 80.9% with its own harness. v2 (around February 2026) switched the default to native tool calling, with a single `bash` tool, while keeping the text-regex mode as an option.

### Cited Findings
- README claims: "The 100 line AI agent that solves GitHub issues or helps you in your command line." It "Scores >74% on the SWE-bench verified benchmark." It has "no tools other than bash" and "doesn't even need to use the tool-calling interface of the LMs". The history is linear, so the trajectory equals the LM messages. Actions run via `subprocess.run`, so sandboxing amounts to swapping in `docker exec`. It has about 8.7k stars. Users include Meta, NVIDIA, Essential AI, IBM, Nebius, Anyscale, Princeton and Stanford. This is "mini-swe-agent v2". — [GitHub SWE-agent/mini-swe-agent](https://github.com/SWE-agent/mini-swe-agent)
- v2 migration notes: "Tool calling is the default", and text-based parsing moves to the `litellm_textbased` model class. Messages use the model's native output format plus an `extra` field. The submit marker changed from `echo MINI_SWE_AGENT_FINAL_OUTPUT` to `echo COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT`. `Environment.execute(command)` became `execute(action: dict, cwd="")`, and a `swerex_modal` environment was added. — [v2 migration guide](https://mini-swe-agent.com/latest/advanced/v2_migration/)
- The bash tool schema (`models/utils/actions_toolcall.py`, 113 lines):
  `{"type":"function","function":{"name":"bash","description":"Execute a bash command","parameters":{"type":"object","properties":{"command":{"type":"string","description":"The bash command to execute"}},"required":["command"]}}}`.
  A `FormatError` is raised on no tool calls, an unknown tool, invalid JSON, or a missing `command`. — [actions_toolcall.py](https://github.com/SWE-agent/mini-swe-agent/blob/main/src/minisweagent/models/utils/actions_toolcall.py)
- The loop (`agents/default.py`, approx. 230 lines including config and serialization) has `run()` render the system and instance templates and then do `while True: self.step()` until the last message has role `"exit"`. `step()` is `return self.execute_actions(self.query())`. Actions come from `message["extra"]["actions"]`. Config defaults: `step_limit` 0 (none), `cost_limit` 3.0, `wall_time_limit_seconds` 0, `max_consecutive_format_errors` 3. — [default.py](https://github.com/SWE-agent/mini-swe-agent/blob/main/src/minisweagent/agents/default.py)
- Model classes: `litellm_model.py` (approx. 164 lines; cost via `litellm.completion_cost`), `litellm_response_model.py`, `litellm_textbased_model.py`, plus openrouter, portkey and requesty variants. — [models dir](https://github.com/SWE-agent/mini-swe-agent/tree/main/src/minisweagent/models); [litellm_model.py](https://github.com/SWE-agent/mini-swe-agent/blob/main/src/minisweagent/models/litellm_model.py)
- SWE-bench config (`config/benchmarks/swebench.yaml`, about 950 words of templates):
  - Each response needs a THOUGHT section and at least one bash call; several are allowed if independent.
  - Each action runs in a new subshell, so `cd`/`export` must be prefixed.
  - Workflow: find and read files, write a reproduction script, edit, re-run, test edge cases.
  - Submit via `git diff > patch.txt`, then `echo COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT && cat patch.txt`.
  - Observations over 10,000 characters are cut to the first and last 5,000 characters, with a warning to use narrower commands.
  - Limits: `step_limit` 250, `cost_limit` 3, command timeout 60, `bash -c`, Docker env. `PAGER=cat` and similar env vars are set.
  - Model: claude-sonnet-4-5 with `parallel_tool_calls: true`.
  — [swebench.yaml](https://github.com/SWE-agent/mini-swe-agent/blob/main/src/minisweagent/config/benchmarks/swebench.yaml)
- The FAQ argument for no persistent shell: it's unclear when a command has finished (the heuristics were "flaky"), bad commands kill the session, and interrupts corrupt output extraction. The cost is that the agent "cannot even change directories or export environment variables" persistently. — [mini-swe-agent FAQ](https://mini-swe-agent.com/latest/faq/)
- The minimal-agent.com tutorial builds a "first prototype in 50 lines" (about 60 lines):
  - Messages list, then query the LM, then parse the first ` ```bash-action ` block with the regex ``r"```bash-action\s*\n(.*?)\n```"`` and `re.DOTALL`, or `<bash_action>` tags for weaker models.
  - Run `subprocess.run(shell=True, timeout=30)` with stderr merged into stdout, and append the output as a `user` message.
  - The `exit` action ends the run. A `FormatError` reminds the model to "include exactly 1 action".
  - It claims mini-swe-agent is "built exactly the same" and gets "up to 74%", "only a few percent below highly optimized agents".
  — [minimal-agent.com](https://minimal-agent.com/)
- Official SWE-bench Verified bash-only leaderboard (mini-SWE-agent; model, % resolved, avg $/instance, date, mini version):
  - Claude 4.5 Opus (high): 76.80, $0.75, 2026-02-17, v2.0.0
  - Gemini 3 Flash (high): 75.80, $0.36
  - MiniMax M2.5 (high): 75.80, $0.07
  - Claude 4.6 Opus: 75.60, $0.55
  - Claude 4.5 Opus (medium): 74.40, $0.72 (v1.16.0)
  - Gemini 3 Pro Preview: 74.20
  - GLM 5 / GPT 5.2 (high) / GPT 5.2 Codex: 72.80
  - Claude 4.5 Sonnet (high): 71.40
  - Kimi K2.5: 70.80
  - DeepSeek V3.2 (high): 70.00
  - Claude 4 Opus: 67.60
  - GPT-5 (medium): 65.00
  - Devstral Small (2512): 56.40
  - GPT-4o: 21.62
  - Qwen2.5-Coder 32B: 9.00
  
  Verified "Defaults to bash-only setting (run with mini-SWE-agent)". News items: "JUL 2025 mini-SWE-agent scores 65% on SWE-bench Verified in 100 lines of Python", and ProgramBench released May 2026. — [swebench.com](https://www.swebench.com/)
- Comparison point: Anthropic reported Claude Opus 4.5 at 80.9% on SWE-bench Verified with its own scaffolding. Smol.ai noted that the official leaderboard uses fixed mini-SWE-agent scaffolding, which put Opus 4.5 at 74.4%. — [Trilogy AI news brief](https://trilogyai.substack.com/p/news-brief-anthropic-releases-claude); [smol.ai](https://news.smol.ai/issues/25-11-24-opus-45)
- Recent releases: v2.4.0 (10 Jun) dropped the T=0 default in the SWE-bench configs and capped consecutive format errors; v2.4.1 kills the process group on timeout; v2.4.6 (23 Jul) counts billed-but-unparseable calls against `cost_limit`. — [mini-swe-agent releases](https://github.com/SWE-agent/mini-swe-agent/releases)
- On Terminal-Bench 2.0 (official): Mini-SWE-Agent scores 42.5 with Sonnet 4.5 (Claude Code 40.1), 35.1 with Opus 4.1 (Claude Code 34.8), 33.9 with GPT-5 (Codex CLI 49.6), and 29.8 with Haiku 4.5 (Claude Code 27.5). — [tbench.ai TB2](https://www.tbench.ai/leaderboard/terminal-bench/2.0)

### Inferences
- Reimplementation pseudo-code (v2 tool-calling variant):
  ```
  msgs = [system(tpl), user(instance_tpl(task))]
  while True:
    if cost >= 3.0 or steps >= 250: exit(LimitsExceeded)
    resp = llm(msgs, tools=[BASH_TOOL])           # FormatError → append error msg; max 3 consecutive
    msgs.append(resp)
    for call in resp.tool_calls:
      out = subprocess.run(["bash","-c",call.command], cwd, timeout=60, env)  # or docker exec
      if out startswith "COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT": exit(submission=rest)
      msgs.append(tool(call.id, truncate_head_tail(out, 10_000)))
  ```
- The "minimal matches heavy" claim holds up: the gap between bash-only mini-SWE-agent and vendor-optimized scaffolds on SWE-bench Verified is about 4–6 points for frontier models (Opus 4.5: 76.8 vs 80.9). The cost is low ($0.07–$0.75 per instance).
- The "~100 lines" README claim describes the agent class's core logic. The v2 `default.py` file, with config, templates, serialization and error handling, is larger (approx. 230 lines by summary).

### Gaps
- The exact v2.0.0 release date wasn't found. The leaderboard shows v2.0.0 runs dated 2026-02-17, so the release was probably early to mid February 2026.
- No published per-model comparison of mini-SWE-agent against the full SWE-agent on the same models in 2026 was found.
- No source explains how mini-swe-agent handles context-window overflow. There is no compaction in the FAQ or config; it relies on the step and cost limits and on observation truncation.

## Q3. Terminus / Terminus 2: the Terminal-Bench reference harness

### Takeaway
Terminus 2 is a deliberately neutral, model-agnostic harness with "a single tool, a headless terminal" (tmux). The model emits JSON with keystrokes and wait durations and reads back incremental screen output. It is long-context safe through proactive summarization with a three-call handoff. On Terminal-Bench 2.0 it beats vendor CLIs for Claude models (Opus 4.5: 57.8 vs Claude Code 52.1) but trails vendor or tuned harnesses for OpenAI models (GPT-5.2: 54.0 vs Codex CLI 62.9). Tuned descendants of Terminus (Terminus-KIRA, Meta-Harness) add 12–14 points on Opus 4.6.

### Cited Findings
- Code location: `laude-institute/harbor/src/harbor/agents/terminus_2/`, containing `terminus_2.py` (approx. 1,300 lines by summary), `tmux_session.py`, `terminus_json_plain_parser.py`, `terminus_xml_plain_parser.py`, `asciinema_handler.py` and `templates/` (`terminus-json-plain.txt`, `terminus-xml-plain.txt`, `timeout.txt`). — [harbor terminus_2 dir](https://github.com/laude-institute/harbor/tree/main/src/harbor/agents/terminus_2); [templates](https://github.com/laude-institute/harbor/tree/main/src/harbor/agents/terminus_2/templates)
- Loop: `for episode in range(self._max_episodes)` (unbounded by default, 1,000,000). Each episode:
  1. Check the tmux session is alive.
  2. Optionally summarize proactively.
  3. Call the LLM and parse the response.
  4. For each command, run `session.send_keys(keystrokes, block=False, min_timeout_sec=min(duration, 60))`.
  5. Read `get_incremental_output()`, limited to 10,000 bytes (first and last 5,000 bytes, with the omitted count reported).
  6. Record the trajectory step.
  
  — [terminus_2.py](https://github.com/laude-institute/harbor/blob/main/src/harbor/agents/terminus_2/terminus_2.py)
- Response schema (JSON template, about 650 words): `{"analysis": str, "plan": str, "commands": [{"keystrokes": str, "duration": float (default 1.0)}], "task_complete": bool (optional)}`.
  - Keystrokes must end in a newline to execute, and use tmux escapes such as `C-c`/`C-d`.
  - Durations: about 0.1 s for `cd`/`ls`, about 1.0 s for compilers, under 60 s, poll rather than wait long. Empty keystrokes mean wait.
  - An XML variant exists, and only the XML parser can salvage truncated output.
  
  — [terminus-json-plain.txt](https://github.com/laude-institute/harbor/blob/main/src/harbor/agents/terminus_2/templates/terminus-json-plain.txt); [terminus_2.py](https://github.com/laude-institute/harbor/blob/main/src/harbor/agents/terminus_2/terminus_2.py)
- Task completion needs **double confirmation**. The first `task_complete: true` sets `_pending_completion` and asks for confirmation; a second consecutive one ends the run. — [terminus_2.py](https://github.com/laude-institute/harbor/blob/main/src/harbor/agents/terminus_2/terminus_2.py)
- Context management:
  - **Proactive:** summarize when free tokens drop below `proactive_summarization_threshold`, default 8,000.
  - **Reactive:** on `ContextLengthExceededError`, unwind to about 4,000 free tokens and then summarize.
  - **Full summarization** is three sub-calls: (1) a summary from the unwound history; (2) a fresh-context call that asks "at least five" questions given the summary and current screen; (3) answers from the full history. The chat is then rebuilt from the system prompt, the questions and the answers.
  - **Fallbacks:** a short summary, then a no-LLM summary built from the instruction and the last 1,000 screen characters.
  - There is a docstring/code discrepancy on the threshold-0 behavior.
  
  — [terminus_2.py](https://github.com/laude-institute/harbor/blob/main/src/harbor/agents/terminus_2/terminus_2.py)
- Paper (Terminal-Bench 2.0, arXiv 2601.11868, 2026-01-17, Merrill et al., 85 authors; 89 tasks): Terminus 2 has "a single tool, a headless terminal" and works "using only Bash commands". It was built as "a neutral testbed for comparing model performance" because scaffolds are often tuned to particular models.
  - Results (Table 2):
    - Opus 4.5: Terminus 2 57.8, Claude Code 52.1, OpenHands 51.9.
    - Sonnet 4.5: Terminus 2 42.8, Mini-SWE-Agent 42.5, OpenHands 40.3, Claude Code 40.1.
    - Gemini 2.5 Pro: Terminus 2 32.6, Mini-SWE-Agent 26.1, Gemini CLI 19.6, OpenHands 15.7.
    - GPT-5: Codex CLI 49.6, OpenHands 41.5, Terminus 2 35.2, Mini-SWE-Agent 33.9.
  - Conclusion: "model selection is usually more important than agent scaffold".
  
  — [arXiv 2601.11868 (HTML)](https://arxiv.org/html/2601.11868v1); [abstract](https://arxiv.org/abs/2601.11868v1)
  - Note: the paper's OpenHands numbers (GPT-5 41.5, Gemini 2.5 Pro 15.7) differ slightly from the live leaderboard (43.8, 16.4). — [tbench.ai](https://www.tbench.ai/leaderboard/terminal-bench/2.0)
- Live Terminal-Bench 2.0 leaderboard, same-model comparisons (checked 2026-10-19):
  - **Claude Opus 4.6:** Meta-Harness 76.4, Capy 75.3, Terminus-KIRA 74.7, TongAgents 71.9, Droid 69.9, Mux 66.5, Terminus 2 62.9, Claude Code 58.0.
  - **GPT-5.3-Codex:** SageAgent 78.4, Droid 77.3, Simple Codex 75.1, Terminus 2 64.7.
  - **GPT-5.2:** Droid 64.9, Codex CLI 62.9, Terminus 2 54.0.
  - **Gemini 3 Pro:** Ante 69.4, Droid 61.1, Terminus 2 56.9.
  - **Claude Haiku 4.5:** Goose 35.5, Mini-SWE-Agent 29.8, Terminus 2 28.3, Claude Code 27.5, OpenHands 13.9.
  - **Top overall:** GPT-5.5 + NexAU-AHE 84.7, LemonHarness 84.5, Capy 83.1, Codex CLI 82.2.
  - The tbench.ai home page now captions results as "Terminal-Bench 4.0".
  
  — [tbench.ai TB2 leaderboard](https://www.tbench.ai/leaderboard/terminal-bench/2.0)
- Meta-Harness (Lee et al., arXiv 2603.28052; Stanford/MIT/KRAFTON per summaries) searches over harness *code*, starting from Terminus 2 and Terminus-KIRA. An agentic proposer reads earlier candidates' code, scores and traces from a filesystem. On TB2 with Opus 4.6 it reaches 76.4% versus the hand-engineered Terminus-KIRA's 74.7%. The search and evaluation used the same 89 tasks (no held-out split), with manual and regex leakage audits. — [arXiv 2603.28052](https://arxiv.org/abs/2603.28052); [HF papers](https://huggingface.co/papers/2603.28052); [TB2 artifact repo](https://github.com/stanford-iris-lab/meta-harness-tbench2-artifact)

### Inferences
- Reimplementation pseudo-code:
  ```
  tmux new-session; screen = capture()
  msgs=[sys(template), user(instruction + screen)]
  for ep in range(max_eps):
    if free_tokens(msgs) < 8000: msgs = handoff(summary(), questions(), answers())
    j = parse_json(llm(msgs))       # analysis, plan, commands[], task_complete
    for c in j.commands: tmux send-keys c.keystrokes; sleep(min(c.duration,60))
    obs = head_tail(incremental_output(), 10_000)
    if j.task_complete: if pending: return; pending=True; obs += confirm_prompt
    else: pending=False
    msgs += [assistant(j), user(obs)]
  ```
- The interface is even more minimal than Pi's: no file tools at all, and the terminal *is* the tool, which also covers interactive programs (REPLs, vim, servers). This generality is why it serves as a neutral baseline.
- Terminus 2 has a **summarization/handoff mechanism that Pi originally lacked and mini-swe-agent still lacks**. That suggests long-horizon context management is the first "heavy" feature a minimal harness needs.
- The Meta-Harness result is directly relevant to a self-evolving harness. Starting from a minimal open harness, an automated proposer that edits harness code found improvements of about 13.5 points over Terminus 2 on Opus 4.6. It used the same tasks for search and evaluation, so expect overfitting risk without a held-out split.

### Gaps
- The differences between Terminus 1 and Terminus 2 weren't researched.
- Appendix F of the TB2 paper (Terminus 2 details) wasn't retrieved.
- No details were found on what Terminus-KIRA changes relative to Terminus 2.
- "Simple Codex" (75.1% with GPT-5.3-Codex) may be another minimal harness, but its design wasn't checked.

## Q4. Other minimal designs: Amp tutorial, smolagents/CodeAct, Aider, OpenCode, nanocode, Simon Willison's llm

### Takeaway
The same pattern recurs: a loop, a handful of tools (often just a shell), and append-only history. Amp's tutorial agent is under 400 lines of Go with three tools. nanocode is about 250 lines of Python with six tools. smolagents keeps its agent logic under 1,000 lines and uses Python code as the action space, which CodeAct showed gives up to 20% higher success. Aider is the main counterexample to "bash only": it invests in context selection (a repo map) and edit formats instead of tools. OpenCode is a heavier client/server design with plan/build agents and a permission engine, and it scores about the same as Claude Code on TB2.

### Cited Findings
- **Amp, "How to Build an Agent"** (Thorsten Ball, 2025-04-15, Go): "less than 400 lines of code", about 300 lines by the end. Tools: `read_file {path}`, `list_files {path?}`, `edit_file {path, old_str, new_str}` (creates the file if missing). The loop reads user input, runs inference with tools, executes any tool_use blocks, appends their results as a user message, and repeats. Thesis: "It's an LLM, a loop, and enough tokens." — [ampcode.com/how-to-build-an-agent](https://ampcode.com/how-to-build-an-agent)
- **nanocode** (1rgs): a single Python file of about 250 lines with no dependencies and six tools (read, write, edit, glob, grep, bash). It supports Anthropic and OpenRouter, is MIT-licensed, was listed as released 2026-01-11, and was built with Claude Code and then used to build itself. Comparisons list no MCP, multi-agent, sandbox, permissions or hooks. — [sourcepulse nanocode](https://www.sourcepulse.org/projects/22511849); [terminaltrove](https://terminaltrove.com/ai-coding-agents/nanocode/); [deepwiki](https://deepwiki.com/1rgs/nanocode) (primary repo github.com/1rgs/nanocode not fetched directly)
- **smolagents** (Hugging Face): "the logic for agents fits in ~1,000 lines of code" (`agents.py`). `CodeAgent` writes actions as Python snippets in which tool calls are function calls; `ToolCallingAgent` uses JSON. Code actions "use 30% fewer steps". Sandboxes: E2B, Blaxel, Modal, Docker; `LocalPythonExecutor` "not a security sandbox". About 29.9k stars. — [GitHub huggingface/smolagents](https://github.com/huggingface/smolagents); [HF blog](https://huggingface.co/blog/smolagents)
- **CodeAct** (Wang et al., ICML 2024): executable Python code as the unified action space gives "up to 20% higher success rate" than JSON/text actions across 17 LLMs on API-Bank and a new benchmark. CodeActInstruct has 7k trajectories, and CodeActAgent is fine-tuned from Llama2/Mistral. — [arXiv 2402.01030](https://arxiv.org/abs/2402.01030)
  - Pi 1.0's codemode is a sandboxed CodeAct for tool composition.
- **Aider:**
  - **Repo map:** files plus key symbols and signatures, ranked by a graph-ranking algorithm over the file dependency graph. It is fitted into `--map-tokens` (default 1k) and grows dynamically when no files are in the chat. — [aider repomap](https://aider.chat/docs/repomap.html)
  - **Edit formats:**
    - `whole` (the full file);
    - `diff` (search/replace blocks: `<<<<<<< SEARCH` … `=======` … `>>>>>>> REPLACE` after the file path);
    - `diff-fenced` (path inside the fence, for Gemini);
    - `udiff` (simplified unified diff, for GPT-4 Turbo laziness);
    - `editor-diff`/`editor-whole` for architect mode, where an architect model plans and an editor model applies.
    
    Aider uses no tool-calling API for edits. — [aider edit formats](https://aider.chat/docs/more/edit-formats.html)
- **Simon Willison's LLM 0.26** (2025-05-27): any Python function is a tool (`-T/--tool`, inline `--functions`, plugin toolboxes). `model.chain()` detects tool requests, executes them and re-prompts, and runs async tools concurrently via `asyncio.gather()`. He describes the field as converging on "tools in a loop". — [simonwillison.net](https://simonwillison.net/2025/May/27/llm-tools/)
- **OpenCode** (anomalyco/opencode, about 217.2k stars, MIT): agents `build` (default, full access), `plan` (read-only, denies edits, asks before bash) and a `general` subagent. It uses a client/server design (TUI over HTTP+SSE), models via models.dev, and a permission engine. On TB2 with Opus 4.5 it scores 51.7%. — [GitHub anomalyco/opencode](https://github.com/anomalyco/opencode); [yuv.ai OpenCode overview](https://yuv.ai/blog/opencode); [tbench.ai](https://www.tbench.ai/leaderboard/terminal-bench/2.0)

### Inferences
- A feature matrix of the minimal end of the design space:

| Harness | LOC (claimed) | Action space | Tools | History | Context mgmt |
|---|---|---|---|---|---|
| minimal-agent.com | ~50–60 Py | regex bash block | 1 | linear | none |
| mini-swe-agent v2 | ~100 core | tool-call bash | 1 | linear | truncation only |
| Amp tutorial | <400 Go | tool calls | 3 | linear | none |
| nanocode | ~250 Py | tool calls | 6 | linear | not found |
| Pi core | not measured | tool calls | 4 (+3 opt) | JSONL tree | compaction + branch summaries |
| Terminus 2 | ~1,300 (approx.) | JSON keystrokes | 1 (tmux) | linear + handoff | proactive 3-call summarization |
| smolagents | <1,000 agents.py | Python code | N | ReAct memory | not checked |

- Aider shows a different axis of "minimal": a tiny tool surface (none) but a heavy context strategy (repo map). For self-evolution, the context-selection policy is a natural target for the evolver alongside the tool set.

### Gaps
- No current benchmark numbers were collected for Aider (the polyglot leaderboard wasn't fetched), smolagents (GAIA), nanocode or the Amp tutorial agent.
- The exact current LOC of smolagents' `agents.py` wasn't verified.
- OpenCode's built-in tool list, LSP integration and MCP were not verified from primary docs.

## Q5. What minimal harnesses lose versus heavy ones, and how they compensate

### Takeaway
The evidence says minimal general-purpose harnesses roughly match vendor CLIs and beat generic heavy frameworks for many models (especially Claude). They lose 5–15 points to (a) model-co-trained vendor harnesses for OpenAI models and (b) heavily tuned harnesses at the top of Terminal-Bench. The concrete gaps are long-horizon context exhaustion, timeouts and persistent state, safety and permissioning, and integrations (MCP). Each minimal design compensates with a specific cheap mechanism: compaction or summarized handoff, tmux, containers, CLI-plus-README instead of MCP, and files instead of todo or plan tools. Pi 1.0 shows that under mass usage, some of these come back, but outside the core prompt (sandboxed codemode, a separate durable package).

### Cited Findings
- **Model beats scaffold on average:** "model selection is usually more important than agent scaffold when optimizing for performance", though scaffold matters for some models (Gemini 2.5 Pro: Terminus 2 32.6 vs OpenHands 15.7). — [Terminal-Bench 2.0 paper](https://arxiv.org/html/2601.11868v1)
- **Where minimal wins or ties:**
  - On TB2, Terminus 2 ≥ Claude Code for Opus 4.5 (57.8 vs 52.1), Opus 4.6 (62.9 vs 58.0), Sonnet 4.5 (42.8 vs 40.1) and Haiku 4.5 (28.3 vs 27.5).
  - Mini-SWE-Agent ≥ Claude Code for Sonnet 4.5 (42.5 vs 40.1).
  
  — [tbench.ai TB2](https://www.tbench.ai/leaderboard/terminal-bench/2.0)
- **Where minimal loses:**
  - With OpenAI models, the vendor or tuned harness wins: GPT-5 Codex CLI 49.6 vs Terminus 2 35.2; GPT-5.2 Codex CLI 62.9 vs 54.0; GPT-5.3-Codex Droid 77.3 vs Terminus 2 64.7.
  - At the top, tuned harnesses win: Opus 4.6 Meta-Harness 76.4 and Terminus-KIRA 74.7 vs Terminus 2 62.9.
  
  — [tbench.ai TB2](https://www.tbench.ai/leaderboard/terminal-bench/2.0); [Meta-Harness](https://arxiv.org/abs/2603.28052)
- **On SWE-bench Verified the gap is small:** mini-SWE-agent with Opus 4.5 scores 74.4–76.8 versus Anthropic's self-reported 80.9 with its own scaffold. — [swebench.com](https://www.swebench.com/); [Trilogy AI](https://trilogyai.substack.com/p/news-brief-anthropic-releases-claude)
- **Long-horizon and context exhaustion:**
  - Pi launched without compaction ("hasn't been a problem for me personally") and later added threshold-based structured compaction plus branch summaries.
  - Terminus 2 does proactive summarization at 8k free tokens with a question-answer handoff.
  - mini-swe-agent relies on a 250-step and $3 cap and 10k-character head/tail truncation.
  
  — [Pi blog](https://mariozechner.at/posts/2025-11-30-pi-coding-agent/); [Pi compaction.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/compaction.md); [terminus_2.py](https://github.com/laude-institute/harbor/blob/main/src/harbor/agents/terminus_2/terminus_2.py); [swebench.yaml](https://github.com/SWE-agent/mini-swe-agent/blob/main/src/minisweagent/config/benchmarks/swebench.yaml)
- **Timeouts and persistent processes:**
  - Pi's TB2 run had 57 AgentTimeoutError in 428 trials. Pi has no background bash and recommends tmux.
  - mini-swe-agent's stateless subprocess per action loses `cd`/`export` persistence.
  - Terminus 2's tmux gives persistence and interactivity, but the model must manage waits itself (the `duration` field).
  
  — [Pi TB image](https://mariozechner.at/posts/2025-11-30-pi-coding-agent/media/terminal-bench.png); [Pi blog](https://mariozechner.at/posts/2025-11-30-pi-coding-agent/); [mini FAQ](https://mini-swe-agent.com/latest/faq/); [Terminus template](https://github.com/laude-institute/harbor/blob/main/src/harbor/agents/terminus_2/templates/terminus-json-plain.txt)
- **Safety:**
  - Pi is YOLO by default ("pretty much game over" once code can run) and recommends containers. Extensions execute arbitrary code in-process, and there is no sandbox by default. Permission gates and sandboxing are offered as example extensions.
  - OpenCode and Claude Code ship permission engines and plan modes.
  - OpenClaw runs tools on the host for the main session unless sandboxing is configured.
  
  — [Pi blog](https://mariozechner.at/posts/2025-11-30-pi-coding-agent/); [rywalker](https://rywalker.com/research/pi); [pi.dev](https://pi.dev/); [OpenCode](https://github.com/anomalyco/opencode); [OpenClaw](https://github.com/openclaw/openclaw)
- **Integrations:**
  - Pi originally replaced MCP with CLI tools plus READMEs, because MCP tool definitions cost 13.7k–18k tokens per server and reloading them mid-session invalidates the cache.
  - Pi 1.0 re-added MCP but behind a QuickJS codemode sandbox with deferred tool discovery, to keep the prompt stable.
  
  — [Pi blog](https://mariozechner.at/posts/2025-11-30-pi-coding-agent/); [Armin Ronacher](https://lucumr.pocoo.org/2026/1/31/pi/); [codemode.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/codemode.md)
- **Human-side risks** (Pragmatic Engineer): automation bias, agents not feeling the "pain" of bad code and so rarely refactoring, and complexity as the enemy of both humans and agents. — [Pragmatic Engineer](https://newsletter.pragmaticengineer.com/p/building-pi-and-what-makes-self-modifying)
- **Feature creep under adoption:** Pi 1.0 added MCP, virtual models, deferred tools, cache warming and mid-conversation system messages. Earendil kept long-running orchestration in a separate Pi Durable package "to keep the original Pi agent from growing too complex". — [Techzine](https://www.techzine.eu/news/devops/144715/pi-1-0-will-include-mcp-and-a-dedicated-layer-for-long-running-ai-agents/); [The Register](https://www.theregister.com/ai-and-ml/2026/10/02/pi-coding-agent-pulls-a-180-and-adds-mcp-support/5300678)

### Inferences
- For a self-evolving harness seeded from Pi, the evidence points to this minimal-but-sufficient core:
  1. four tools plus tmux guidance;
  2. a sub-1k-token prompt with a "docs about yourself" section;
  3. a JSONL tree session (branches give cheap rollback and A/B of self-modifications);
  4. threshold compaction (Pi) or a Terminus-style Q&A handoff;
  5. hooks on `context`, `tool_call` and `tool_result`, so the evolver can add guardrails and context policies without touching the loop.
- The room left for evolution is large and measurable. On TB2 with the same model, tuned harnesses beat minimal ones by 12–14 points (Opus 4.6), and Meta-Harness got there automatically from a Terminus 2 seed. It needs a held-out task split to avoid the same-set overfitting the Meta-Harness authors acknowledge.
- The harness-model fit effect (Codex CLI beating Terminus 2 by 9–14 points on GPT-5.x, but Claude Code losing to Terminus 2 on Claude) suggests per-model specialization is a real axis. A self-evolving harness should key its evolved variants by model.

### Gaps
- No controlled ablation was found that isolates which heavy features (subagents, todo tools, permission prompts, MCP) cause score differences. The leaderboard deltas confound harness, prompt tuning, timeouts and API reliability.
- No public data was found on Pi's performance on long-horizon tasks after compaction was added, or for Pi 1.0 on any benchmark.
- Terminal-Bench now advertises a 4.0 version. TB 4.0 results for minimal harnesses weren't collected because the leaderboard data loads dynamically and only the 2.0 table was read.
