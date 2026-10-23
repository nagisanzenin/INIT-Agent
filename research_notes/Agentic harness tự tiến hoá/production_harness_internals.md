# Production Agent Harness Internals (state as of 2026-10-19)

Scope: Claude Code / Claude Agent SDK, OpenAI Codex CLI (Rust), Google Gemini CLI, OpenHands (SDK), plus the public parts of Cursor, Amp, Factory Droid, Warp, Devin (Cognition) and Manus.

Evidence labels used throughout:
- **[DOC]**: official docs or engineering blog from the vendor.
- **[SRC]**: read directly from the public source code on GitHub on 2026-10-19 (via `gh api`). Version pins: Codex `rust-v0.163.0` (released 2026-10-15), Gemini CLI `v0.64.0` (2026-10-13), OpenHands `v1.27.1` (2026-10-12), OpenHands `software-agent-sdk` main. Release tags are from `gh api repos/<repo>/releases/latest`.
- **[RE]**: community reverse-engineering, or a third-party write-up of a primary source I could not open.
- **[PRIOR]**: a primary source I know from before this session but did not re-fetch here. The URL is given. Verify before quoting numbers.
- **[INF]**: my inference.

Version anchors: Claude Code v2.1.303 is dated 2026-10-18 per the Piebald prompt tracker ([Piebald-AI/claude-code-system-prompts](https://github.com/Piebald-AI/claude-code-system-prompts)). Claude Opus 5 launched on 2026-07-24 per secondary reports ([36kr](https://www.36kr.com/p/3916538775727746), [bnext](https://www.bnext.com.tw/article/91670/claude-code-prompt-reduction-agent-verification)).

---

## 1. Agent loop: structure, stop condition, streaming, interruption, mid-turn steering

### Takeaway
Every production harness studied is a single-threaded, hand-rolled `while (model returns tool calls) { execute; append results; re-call model }` loop. A turn ends when the model emits an assistant message with no tool calls. None of the eleven systems in a 2026 source-code audit imports a general-purpose agent framework. The engineering effort goes into what surrounds the loop: durable event logs, queued user messages that are injected between tool batches, loop detection, and denial or turn-count circuit breakers. The loop body itself does not get more complex.

### Cited Findings
**Cross-system**
- A source-code study of 11 harnesses (Claude Code, Codex CLI, Gemini CLI, Mistral Vibe, OpenHands, Aider, Mini-SWE-Agent, Hermes, Pi, OpenCode, OpenClaw; about 4M LOC) found the following. "No agent runtime imports a general-purpose agentic framework." None retrieves code with vector embeddings, and the field relies on "hand-rolled async loops and deterministic retrieval." The paper also offers a 90-line minimum-viable-harness scaffold and 18 design recommendations. Its thesis is that in H1 2026 the coding harness moved "from tool to platform." Barbaste et al., arXiv 2609.00006, v1 2026-07-15 (abstract only read) — [arXiv](https://arxiv.org/abs/2609.00006)

**Claude Code [DOC]**
- The loop is described as three blended phases, "gather context, take action, verify results," repeated until the task is done. Claude Code itself is described as "the layer around the model that provides the tools and manages the context... what the term agentic harness refers to." — [How Claude Code works](https://code.claude.com/docs/en/how-claude-code-works)
- Interruption: `Esc` stops immediately and cancels the running tool call. Steering without stopping works by typing a message and pressing Enter. The message is queued, and "if Claude is running tool calls, it reads the message as soon as those calls finish, within the same turn, and adjusts before its next step." — [How Claude Code works](https://code.claude.com/docs/en/how-claude-code-works)
- Persistence: every message, tool use and result is appended to a plaintext JSONL file under `~/.claude/projects/`. This enables resume (`--continue`/`--resume`), fork (`--fork-session`, `/branch`) and rewind. — [How Claude Code works](https://code.claude.com/docs/en/how-claude-code-works)
- Subagents have `maxTurns`. When a subagent reaches the limit, the result is marked partial, and the parent can resume it with `SendMessage`. Subagents now run in the background by default. — [Tools reference](https://code.claude.com/docs/en/tools-reference)
- Circuit breaker in auto mode: a classifier denial comes back to the agent as a tool result with an instruction to find a safer path. After 3 consecutive or 20 total denials the agent stops and escalates to the human. In headless `-p` mode the process terminates instead. (John Hughes, 2026-03-25) — [Anthropic: Claude Code auto mode](https://www.anthropic.com/engineering/claude-code-auto-mode)
- `ScheduleWakeup` tool: in self-paced `/loop`, the model chooses when the next iteration runs (1 min to 1 h) and ends the loop with `stop: true`. This hands the outer stop decision to the model. — [Tools reference](https://code.claude.com/docs/en/tools-reference)
- [RE] Community teardowns describe a single "master loop" with an async message queue that supports mid-turn steering. The vrungta reverse-engineering post is one such teardown, and it was not opened in this session — [vrungta substack](https://vrungta.substack.com/p/claude-code-architecture-reverse)

**Claude Agent SDK / Managed Agents [DOC]**
- Anthropic "Managed Agents" (Lance Martin, Gabe Cemaj, Michael Cohen; 2026-04-08) splits the agent into three virtualized parts:
  - Brain: Claude plus a stateless harness that runs outside the container.
  - Hands: sandboxes and tools exposed uniformly as `execute(name, input) → string`.
  - Session: an append-only event log stored outside the context window.
  The API surface is: `wake(sessionId)` restarts a crashed harness, `getSession(id)` and `getEvents()` read positional slices of history, `emitEvent(id, event)` records each step durably, and `provision({resources})` starts a new container after one dies. Provisioning containers lazily cut p50 time-to-first-token by about 60% and p95 by more than 90%. — [Anthropic: Managed Agents](https://www.anthropic.com/engineering/managed-agents)

**Codex CLI [DOC via RE + SRC]**
- Michael Bolin's "Unrolling the Codex agent loop" (OpenAI, 2026-01-23) is the first post of a series. It says the Codex harness provides the core agent loop behind every Codex surface. — [OpenAI](https://openai.com/index/unrolling-the-codex-agent-loop/) (HTTP 403 to my fetcher, so this is the search-result summary)
- [RE] The loop runs: build prompt (instructions, tools, history) → Responses API inference (streamed) → if a tool call is present, execute it, append the result and loop; otherwise the final assistant message ends the turn. One turn can contain many tool calls, and parallel tool calls in one response are supported. Codex moved from Chat Completions to the Responses API in v0.113.0 (early March 2026). The post is credited with a 40–80% improvement in cache utilisation and a 3% SWE-bench gain from that move; these numbers are unverified against OpenAI's text — [danielvaughan deep dive](https://codex.danielvaughan.com/2026/03/28/codex-agent-loop-deep-dive/)
- [SRC] Codex ships multi-agent control tools as first-class handlers: `spawn`, `send_input`, `wait`, `close_agent`, `resume_agent`. It also has `request_user_input`, `request_permissions`, `sleep`, `current_time`, `get_context_remaining` and `new_context_window`, plus a `code_mode` package with `execute`/`wait` specs. — [codex-rs/core/src/tools/handlers](https://github.com/openai/codex/tree/main/codex-rs/core/src/tools/handlers)

**Gemini CLI [SRC]**
- The loop has a built-in `LoopDetectionService` with these constants: `TOOL_CALL_LOOP_THRESHOLD = 5` identical consecutive tool calls; `CONTENT_LOOP_THRESHOLD = 10` repeats of `CONTENT_CHUNK_SIZE = 50`-char chunks; and an LLM-judge loop check that starts after `LLM_CHECK_AFTER_TURNS = 30`. The judge runs every 10 turns by default, adaptively between 5 and 15, looks at the last 20 history items, and requires `LLM_CONFIDENCE_THRESHOLD = 0.9` confirmed by a separate "loop-detection-double-check" model. — [loopDetectionService.ts](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/services/loopDetectionService.ts)
- There is an explicit `complete_task` tool (`COMPLETE_TASK_TOOL_NAME`), which gives the model a structured way to end a task alongside the "no tool call" stop. — [base-declarations.ts](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/tools/definitions/base-declarations.ts)

**OpenHands [SRC + DOC]**
- The SDK default agent registers a `FinishTool` as the explicit stop signal. — [preset/default.py](https://github.com/OpenHands/software-agent-sdk/blob/main/openhands-tools/openhands/tools/preset/default.py)
- [PRIOR] OpenHands is built on an event-stream architecture: actions and observations are typed events on a stream, and the agent is a function from event history to the next action. CodeAct (Wang et al., ICML 2024) proposed executable Python as the unified action space and reported up to 20% higher success than JSON/text actions — [CodeAct arXiv 2402.01030](https://arxiv.org/abs/2402.01030); [OpenHands SDK paper arXiv 2511.03690](https://arxiv.org/abs/2511.03690)

**Amp [PRIOR]**
- Thorsten Ball's "How to Build an Agent" (Apr 2025) argues an agent is about 400 lines of Go: an LLM, a loop and a few tools (`read_file`, `list_files`, `edit_file`). It is linked from Amp's own news — [ampcode.com/notes/how-to-build-an-agent](https://ampcode.com/notes/how-to-build-an-agent)

### Inferences
- [INF] A concrete loop design that reimplements the shared ideas:
  1. Use an append-only event log as the source of truth. That log is the JSONL in Claude Code and the session in Managed Agents.
  2. Drain a user-message queue between tool batches so the user can steer without aborting.
  3. Stop the turn when the model returns no tool calls, or calls an explicit finish/complete tool.
  4. Add cheap deterministic loop detectors (identical tool calls ×5) before any LLM-judged ones.
  5. Cap denials and turns, and return a partial result rather than failing.
- [INF] The trend in 2026 is to make the harness process disposable and resumable from the log (`wake(sessionId)`). The loop becomes a stateless function over durable state.

### Gaps
- I could not read the full text of OpenAI's "Unrolling the Codex agent loop" (403). Item-ordering details such as where the developer and permissions messages sit relative to AGENTS.md come from secondary sources only.
- Claude Code's exact streaming and queue internals are closed-source. The descriptions above are behavior-level docs.

---

## 2. Tool design: core tool sets, edit formats (with evidence), shell design, descriptions and errors

### Takeaway
The core set has converged on four things: a shell, read, a patch-style or exact-replace editor, and search. Search is increasingly folded into the shell. On top of that sit plan/todo, subagent, web, ask-user and tool-search tools. Edit-format evidence says the format interacts with the model:
- Exact `str_replace` works best for Claude.
- Grammar-constrained `apply_patch` (V4A) works best for OpenAI models trained on it.
- Content-hash line anchors ("hashline") gave large gains for weaker or non-native models.
- Lenient application (fuzzy matching, LLM-based edit repair) matters as much as syntax.

As a result, harnesses now pick tool sets per model family. The shell is moving from one-shot `exec` to PTY sessions with stdin writes, as in Codex `exec_command`/`write_stdin`, OpenHands tmux and Warp Full Terminal Use.

### Cited Findings
**Claude Code tool set and contracts [DOC, v2.1.30x]**

Built-in tools: `Agent`, `AskUserQuestion`, `Bash`, `CronCreate/Delete/List`, `Edit`, `EnterPlanMode`, `ExitPlanMode`, `EnterWorktree`, `ExitWorktree`, `Glob`, `Grep`, `ListAgents`, `LSP`, `Monitor`, `NotebookEdit`, `PowerShell`, `Read`, `ScheduleWakeup`, `SendMessage`, `Skill`, `TaskCreate/Get/List/Update/Stop`, `TodoWrite` (disabled by default), `ToolSearch`, `WebFetch`, `WebSearch`, `Workflow`, `Write`, and others. Skills do not add tools; they run through the single `Skill` tool. — [Tools reference](https://code.claude.com/docs/en/tools-reference)

- **Search moved into the shell.** On macOS, Linux and WSL, `Glob` and `Grep` are now absent by default. "Claude searches with `find` and `grep` through the Bash tool instead. In Claude's shell those two commands run embedded versions of `bfs` and `ugrep`." The dedicated tools return only if Bash is removed or explicitly requested. — [Tools reference](https://code.claude.com/docs/en/tools-reference)
- **Edit contract:**
  - Exact string replacement of `old_string` with `new_string`, with no regex or fuzzy matching.
  - `old_string` must be unique unless `replace_all: true`.
  - Read-before-edit is required for Opus 4.6, Haiku 4.5 and older. Newer models can edit an unread file when no permission prompt would be needed (v2.1.208+).
  - A file changed on disk can still be edited if `old_string` still matches uniquely; the result tells the model to re-read.
  - Viewing a file via `cat`, `head`, `sed -n`, `grep` or `rg` on a single file counts as a read.
  — [Tools reference](https://code.claude.com/docs/en/tools-reference)
- **Read:** returns line-numbered content, and absolute paths are instructed. The default cap is 25,000 tokens per call (`CLAUDE_CODE_FILE_READ_MAX_OUTPUT_TOKENS`), and a whole file over 256 KB is refused. A partial read returns a `PARTIAL view` notice with offset/limit guidance. PDFs over 10 pages are read in ranges of up to 20 pages. `allow_large: true` was added in v2.1.296. — [Tools reference](https://code.claude.com/docs/en/tools-reference)
- **Bash, one process per command:**
  - `cd` persists if it stays inside the project; env vars do not persist (use `CLAUDE_ENV_FILE` or a SessionStart hook); shell aliases and functions from rc files are captured at startup.
  - Default timeout is 2 min (`BASH_DEFAULT_TIMEOUT_MS`) and the max is 10 min (`BASH_MAX_TIMEOUT_MS`). On timeout, a command is moved to the background instead of killed, with the result text "Command did not complete within its 120s timeout and was moved to the background."
  - Output: a successful result is inline up to about 30,000 characters. Beyond that the model gets a file path plus a 2,000-character preview and reads or searches the file. A failure result gets a head-and-tail excerpt of about 10,000 characters. The hard ceiling is 150,000.
  - `run_in_background` jobs get 30 min by default and 2 h max in unattended sessions.
  - Optional memory cgroup cap via `CLAUDE_CODE_TOOL_MEMORY_LIMIT`.
  — [Tools reference](https://code.claude.com/docs/en/tools-reference)
- **Monitor:** runs a command in the background and streams each output line, or each WebSocket message, back to the model as an event. — [Tools reference](https://code.claude.com/docs/en/tools-reference)
- **Tool-description sizes [RE]:** 27 built-in tool descriptions. Subagent prompts are Explore 862 tokens, Plan 1,066, general-purpose 446. Compaction prompt variants are 166–1,536 tokens. The whole system prompt is assembled from 500+ conditional strings — [Piebald-AI tracker](https://github.com/Piebald-AI/claude-code-system-prompts)

**Codex CLI tool set [SRC, rust-v0.163.0]**
- **`apply_patch`** is a FREEFORM (non-JSON) tool constrained by a Lark grammar (`assets/tools/apply_patch.lark`). Its description reads: "This is a FREEFORM tool, so do not wrap the patch in JSON." — [apply_patch_spec.rs](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/handlers/apply_patch_spec.rs)
- **Shell is "unified exec":**
  - `exec_command` "Runs a command in a PTY, returning output or a session ID for ongoing interaction."
  - `write_stdin(session_id, chars, yield_time_ms, max_output_tokens)`: non-empty writes wait 250 ms by default (cap 30,000 ms); empty polls wait 5,000–300,000 ms. Output budget defaults to 10,000 tokens.
  - A legacy `shell` spec still exists.
  — [shell_spec.rs](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/handlers/shell_spec.rs)
- **`update_plan`:** steps are `pending|in_progress|completed`, with "At most one step can be in_progress at a time." — [plan_spec.rs](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/handlers/plan_spec.rs)
- **Other handlers:** `view_image`, `tool_search`, MCP + MCP resources, multi-agent tools, `request_user_input`, `request_permissions`, `get_context_remaining`, `new_context_window`, `code_mode` (execute/wait), `list_available_plugins_to_install`, `request_plugin_install`. — [handlers dir](https://github.com/openai/codex/tree/main/codex-rs/core/src/tools/handlers)
- [RE] `apply_patch` matching falls back in order: exact, then ignore line endings, then ignore all whitespace — [danielvaughan V4A explainer](https://codex.danielvaughan.com/2026/03/31/codex-cli-apply-patch-v4a-diff-format/)

**Gemini CLI tool set [SRC, v0.64.0]**
- Tools: `read_file`, `read_many_files`, `write_file`, `replace` (edit), `glob`, `grep_search`, `list_directory`, `run_shell_command`, `google_web_search`, `web_fetch`, `write_todos`, `ask_user`, `activate_skill`, `enter_plan_mode`, `exit_plan_mode`, `complete_task`, `update_topic`, `get_internal_docs`, MCP resource tools, `tracker_*` task-graph tools (create/update/get/list/add_dependency/visualize), and `invoke_agent`. — [base-declarations.ts](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/tools/definitions/base-declarations.ts), [tool-names.ts](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/tools/tool-names.ts)
- **Per-model tool declarations:** `getToolFamily(modelId)` returns `'gemini-3'` or `'default-legacy'`, and each family has its own declaration set (`model-family-sets/gemini-3.ts`, 33 KB). — [modelFamilyService.ts](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/tools/definitions/modelFamilyService.ts)
- **LLM-based edit repair:** `llm-edit-fixer.ts` uses the system prompt "You are an expert code-editing assistant specializing in debugging and correcting failed search-and-replace operations" with a 40 s timeout. There is also `editCorrector.ts`. A failed `replace` is repaired by a second LLM call instead of being bounced back to the agent. — [llm-edit-fixer.ts](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/utils/llm-edit-fixer.ts)

**OpenHands [SRC]**
- The default preset is `TerminalTool` + `FileEditorTool` (str_replace-style editor) + `TaskTrackerTool`, with optional browser and task toolsets. A GPT-5 preset swaps the file editor for `ApplyPatchTool`, and a Gemini preset uses `ReadFile`/`WriteFile`/`Edit` tools. — [preset/](https://github.com/OpenHands/software-agent-sdk/tree/main/openhands-tools/openhands/tools/preset)
- The terminal is tmux-backed (`tmux_terminal.py`, `tmux_pane_pool.py`), so it is a persistent interactive shell. — [terminal/](https://github.com/OpenHands/software-agent-sdk/tree/main/openhands-tools/openhands/tools/terminal/terminal)
- Other tools in the SDK: `ask_oracle`, `delegate`, `child_conversation`, `tom_consult`, `planning_file_editor`, `glob`, `grep`, `workflow`. — [openhands-tools](https://github.com/OpenHands/software-agent-sdk/tree/main/openhands-tools/openhands/tools)

**Factory Droid [DOC, 2025-09-25]**
- **Hierarchical prompting:** three tiers (tool descriptions, system prompt, injected system notifications). Models showed recency bias, so notifications are "particularly crucial" for error recovery.
- **Minimalist tools:** "complex schemas exponentially increased error rates." Cutting per-call error rates gave "multiplicative gains" in task completion.
- **Per-model edit formats:** "one model prefers FIND_AND_REPLACE edits and another prefers V4A diffs; some models need absolute paths."
- **Environment bootstrap:** languages, repo contents, env vars and processes are presented as shell output, which reduced redundant commands.
- **Search and timeouts:** ripgrep-backed grep. Short default timeouts gave better average performance ("counterintuitively"). Background processes are opt-in and filtered.
- **Results:** Terminal-Bench Core v0.1.1 (80 tasks, 5 runs) scored Opus 4.1 58.8%, GPT-5 52.5%, Sonnet 4 50.5%, versus Claude Code+Opus 43.2% and Codex CLI+GPT-5 42.8%. No per-component ablations were reported.
— [Factory: Terminal-Bench](https://factory.com/news/terminal-bench)

**Warp [DOC]**
- **Terminal-Bench gains** were credited to an optimized model fallback chain, letting the agent control long-running commands, and requiring a todo list for the whole task. The post reports about 65% on Terminal-Bench (v1); an earlier figure is 52%. — [Warp: terminal-bench](https://www.warp.dev/blog/terminal-bench)
- **Full Terminal Use:** the agent reads the live terminal buffer, writes to the PTY, and responds inside running processes (REPLs, interactive prompts) while the user can take over at any time. — [Warp docs](https://docs.warp.dev/agents/full-terminal-use), [Agents 3.0](https://www.warp.dev/blog/agents-3-full-terminal-use-plan-code-review-integration)
- **Agent CLI:** pty connections are multiplexed through a tmux-like layer, and I/O is exposed as "blocks." — [Warp agent CLI](https://warp.dev/blog/introducing-the-warp-agent-cli-coding-agent)
- **Terminal-Bench baseline:** the neutral "Terminus" agent gets only a tmux pane to send keystrokes into — [tbench.ai](https://www.tbench.ai/about)

**Edit-format evidence**
- **Aider:**
  - GPT-4 Turbo (1106) scored 20% with SEARCH/REPLACE and 61% with unified diff on the laziness benchmark. GPT-4-0613 went from 26% to 59%.
  - Design principles: FAMILIAR, SIMPLE (no line numbers), HIGH LEVEL, FLEXIBLE.
  - Disabling flexible patch application raised editing errors 9×.
  — [Aider unified diffs](https://aider.chat/docs/unified-diffs.html)
- **Hashline (Can Bölük, 2026-02-12):**
  - Format: lines are shown as `LINE:HASH|content` with a 2–3 character content hash, and edits reference the anchors (single line, range, insert-after). A stale hash rejects the edit.
  - Setup: 16 models × 3 formats (patch / replace / hashline), 180 tasks × 3 runs, about $300.
  - Results: hashline beat patch in 14 of 16 models, by about +15 points on average. Grok Code Fast 1 went from 6.7% to 68.3%. Claude Sonnet 4.5 gained +14.4 vs patch and +3.3 vs replace. GPT-5.2 Codex gained only +4.6 vs patch and lost 0.4 vs replace, with +26% output tokens.
  - `apply_patch` failure rates on non-OpenAI models: Grok 4 50.7%, GLM-4.7 46.2%.
  - Single-author practitioner benchmark on synthetic React mutations, not peer-reviewed.
  — [Bölük: The Harness Problem](https://stencil.so/blog/the-harness-problem)
- **Cited by Bölük (secondary):** JetBrains Diff-XYZ found "no single edit format dominates across models and use cases." Cursor trained a 70B "apply" model and said "fully rewriting the full file outperforms aider-like diffs for files under 400 lines." — [Bölük](https://stencil.so/blog/the-harness-problem)

**Tool-description and error design [PRIOR]**
- Anthropic, "Writing effective tools for agents" (2025-09-11), recommends:
  - Consolidate tools instead of wrapping every API endpoint.
  - Namespace tool names.
  - Return high-signal fields.
  - Offer a `response_format` (concise/detailed).
  - Truncate with steering hints.
  - Write actionable error messages.
  It notes Claude Code caps tool responses at 25,000 tokens by default; this matches the Read cap documented above. — [anthropic.com/engineering/writing-tools-for-agents](https://www.anthropic.com/engineering/writing-tools-for-agents)
- A tool-testing agent rewrote tool descriptions, which cut task completion time by 40% for later agents. — [Anthropic multi-agent research system](https://www.anthropic.com/engineering/multi-agent-research-system)
- [PRIOR] "Building effective agents" (2024-12-19): for SWE-bench the team "spent more time optimizing our tools than the overall prompt," including switching to absolute file paths after the model made relative-path mistakes — [anthropic.com/engineering/building-effective-agents](https://www.anthropic.com/engineering/building-effective-agents)
- [PRIOR] "Advanced tool use" (2025-11-24): Tool Search, Programmatic Tool Calling and Tool Use Examples. The reported gains include a large token reduction from deferred tool loading and higher MCP-eval accuracy with tool search; re-verify exact figures — [anthropic.com/engineering/advanced-tool-use](https://www.anthropic.com/engineering/advanced-tool-use)
- [PRIOR] "Code execution with MCP" (2025-11-04): presenting MCP servers as code APIs that the agent imports cut one example from about 150,000 to about 2,000 tokens — [anthropic.com/engineering/code-execution-with-mcp](https://www.anthropic.com/engineering/code-execution-with-mcp)

### Inferences
- [INF] What converged:
  - Read with line numbers and paging.
  - An exact-match or grammar-constrained patch editor that fails loudly on ambiguity, plus a lenient applier (whitespace-insensitive fallback) or an LLM repair pass.
  - A shell whose large outputs spill to files the agent can grep, instead of being truncated.
  - Search via ripgrep-class tools, often just inside the shell.
- [INF] Per-model tool sets are now standard practice. Gemini CLI does it with model families, OpenHands with presets, Factory with per-model edit formats, and Claude Code by gating todo/task tools by model. A reimplementation should treat the tool schema as a model-specific parameter, not a constant.
- [INF] Evidence on edit formats is strongest for the claim that the format must match the model's training distribution: Aider's unified diff for GPT-4, V4A for GPT-5-Codex, str_replace for Claude. Hashline's gains concentrate on models without a native format.

### Gaps
- Claude Code's full Edit, Bash and Agent tool description texts were not extracted. They are available in the Piebald repo, which is 333 KB, and only the first 100 KB was read.
- The Diff-XYZ and EDIT-Bench papers were not fetched directly.
- Cursor's current (2026) agent tool list is not public. Its "Instant Apply" model and Composer RL training details were not re-verified this session.

---

## 3. Context management: compaction, condensers, truncation, cache-aware layout, Manus lessons

### Takeaway
The production stack has three layers:
1. **Spill and truncate tool outputs to files**, losslessly, so the agent can re-read them.
2. **Clear or mask old tool observations**, which is cheap, cache-friendly, and per a NeurIPS 2025 study as good as summarization.
3. **LLM summarization/compaction**, only when (1) and (2) are exhausted.

Triggers range from 50% of the window (Gemini CLI) to 90% (Codex). KV-cache stability (append-only, deterministic prefix, no mid-session tool-list changes) is treated as a first-class design constraint. Structured, anchored summaries beat opaque or regenerated ones on recall probes. Artifact or file tracking remains the weakest dimension for every method.

### Cited Findings
**Claude Code [DOC]**
- Order of operations as the window fills: "It clears older tool outputs first, then summarizes the conversation if needed." Users can steer what is preserved with a "Compact Instructions" section in CLAUDE.md or `/compact <focus>`. A thrashing guard stops auto-compaction "after a few attempts" if a single huge output refills context. — [How Claude Code works](https://code.claude.com/docs/en/how-claude-code-works)
- MCP tool definitions are deferred by default and loaded via `ToolSearch`, so only names and server instructions occupy context. Skill bodies load on use, and only descriptions sit in context at start. Subagents have isolated context, and only a summary returns. — [How Claude Code works](https://code.claude.com/docs/en/how-claude-code-works)
- Auto memory: the first 200 lines or 25 KB of `MEMORY.md` load at session start. — [How Claude Code works](https://code.claude.com/docs/en/how-claude-code-works)
- [RE] Compaction prompt variants are base 292 tokens, lean 486, short 400, capped-2,000-words 166, and with-additional-instructions 1,536 — [Piebald-AI](https://github.com/Piebald-AI/claude-code-system-prompts)
- Postmortem (2026-04-23): a cost-saving change that cleared old thinking blocks for sessions idle over 1 h had a bug. It ran on every subsequent turn, so the model lost its reasoning mid-task, leading to "forgetfulness, repetition, and odd tool choices." It ran 2026-03-26 to 2026-04-10 and was fixed in v2.1.101. — [Anthropic April 23 postmortem](https://www.anthropic.com/engineering/april-23-postmortem)
- [PRIOR] "Effective context engineering for AI agents" (2025-09-29) frames context as a finite "attention budget" and recommends:
  - Just-in-time retrieval via identifiers (paths, queries) instead of pre-loading.
  - Compaction, with tool-result clearing as the lightest form.
  - Structured note-taking (e.g., NOTES.md, a memory tool).
  - Sub-agents that return condensed summaries.
  — [anthropic.com/engineering/effective-context-engineering-for-ai-agents](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents)

**Codex CLI [SRC]**
- Auto-compaction triggers at 90% of the context window by default: `auto_compact_token_limit` is derived as `context_window * 9 / 10` and clamped to 90% if configured. `effective_context_window_percent = 95` reserves headroom. — [openai_models.rs](https://github.com/openai/codex/blob/main/codex-rs/protocol/src/openai_models.rs)
- Compaction prompt (verbatim header): "You are performing a CONTEXT CHECKPOINT COMPACTION. Create a handoff summary for another LLM that will resume the task." It asks for progress and decisions, constraints and preferences, next steps, and critical data. — [prompts/templates/compact/prompt.md](https://github.com/openai/codex/blob/main/codex-rs/prompts/templates/compact/prompt.md)
- Summary re-injection prefix: "Another language model started to solve this problem and produced a summary of its thinking process. You also have access to the state of the tools…" — [summary_prefix.md](https://github.com/openai/codex/blob/main/codex-rs/prompts/templates/compact/summary_prefix.md)
- `COMPACT_USER_MESSAGE_MAX_TOKENS = 20_000`. [INF] This appears to cap the user messages retained verbatim across compaction. — [compact.rs](https://github.com/openai/codex/blob/main/codex-rs/core/src/compact.rs)
- Remote (server-side) compaction exists alongside local compaction (`compact_remote_v2*.rs`, `compact_retained_tool_outputs.rs`). There is also a `ModelTokenBudgetConfig` with `reminder_threshold_tokens`, `use_history_notes_extension` and `auto_compact_fallback_prompt`. [INF] Together these suggest token-budget reminders to the model and a history-notes mechanism. — [codex-rs/core/src](https://github.com/openai/codex/tree/main/codex-rs/core/src)
- [RE] Bolin's post describes the `/responses/compact` endpoint returning an encrypted payload that preserves latent state. It also covers prefix-exact caching (≥1,024 tokens, routing by a hash of about the first 256 tokens) and the rule that changing tools, MCP lists or AGENTS.md mid-session breaks the cache — [danielvaughan caching](https://codex.danielvaughan.com/2026/04/21/codex-cli-prompt-caching-maximise-cache-hits-cost-reduction/), [all-ai.de summary](https://www.all-ai.de/news/beitrage2026/codex-agent-loop.md)

**Gemini CLI [SRC]**
- `DEFAULT_COMPRESSION_TOKEN_THRESHOLD = 0.5`: compress when history exceeds 50% of the model limit. `COMPRESSION_PRESERVE_THRESHOLD = 0.3`: keep the last 30% verbatim and summarize the rest. — [chatCompressionService.ts](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/context/chatCompressionService.ts)
- "Reverse token budget": iterating newest to oldest, function responses are kept in full until `COMPRESSION_FUNCTION_RESPONSE_TOKEN_BUDGET = 50_000` is exceeded. After that, "older large tool responses are truncated to their last 30 lines and saved to a temporary file." `RECENT_TURNS_PROTECTED = 3`. `COLLAPSED_FUNCTION_RESPONSE_MAX_BYTES = 2048`. Retrieval tools (`read_file`, `grep`, `glob`, …) are exempt from collapsing. — [chatCompressionService.ts](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/context/chatCompressionService.ts)
- Safety check: if the compressed history has more tokens than the original, the result is `COMPRESSION_FAILED_INFLATED_TOKEN_COUNT` and the old history is kept. — [chatCompressionService.ts](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/context/chatCompressionService.ts)

**OpenHands [SRC + DOC]**
- `LLMSummarizingCondenser` (a rolling condenser) has these defaults:
  - `max_size = 240` events.
  - `keep_first = 2`: the first events are never condensed.
  - Optional `max_tokens`, bounded by the agent LLM's input limit.
  - `minimum_progress = 0.1`.
  It keeps a tail of about `max_size//2 - keep_first - 1` events and summarizes the middle. It can also be triggered by explicit condensation requests. A `pipeline_condenser` composes condensers. — [llm_summarizing_condenser.py](https://github.com/OpenHands/software-agent-sdk/blob/main/openhands-sdk/openhands/sdk/context/condenser/llm_summarizing_condenser.py)
- Blog (Calvin Smith, 2025-04-09): without condensation, cost per turn grows quadratically; with it, linearly. On a SWE-bench Verified subset the solve rate was 54% with the condenser vs 53% without, with up to 2× lower per-turn cost. — [OpenHands condenser blog](https://openhands.dev/blog/openhands-context-condensensation-for-more-efficient-ai-agents)
- [PRIOR] Earlier OpenHands builds shipped many condenser types (observation masking, recent-events, amortized forgetting, LLM attention). The current SDK tree only contains `llm_summarizing`, `no_op` and `pipeline` condensers. — [SDK condenser dir](https://github.com/OpenHands/software-agent-sdk/tree/main/openhands-sdk/openhands/sdk/context/condenser)

**Research evidence: masking vs summarization**
- "The Complexity Trap" (Lindenbauer et al., JetBrains/TUM, NeurIPS 2025) ran SWE-agent on SWE-bench Verified across 5 model configs.
  - Simple observation masking halved cost vs the raw agent and matched or slightly exceeded LLM summarization's solve rate. With Qwen3-Coder 480B, masking was 52% cheaper and +2.6% solve rate.
  - A hybrid cut cost another 7% vs masking alone and 11% vs summarization alone.
  - There is initial evidence that this generalizes to OpenHands.
  — [arXiv 2508.21433](https://arxiv.org/abs/2508.21433)

**Factory compression evaluation (2025-12-16) [DOC]**
- Probe-based evaluation over 36,611 production messages with hundreds of compression points. Probe types are recall, artifact, continuation and decision; GPT-5.2 graded on six dimensions.
- Overall scores: Factory "anchored iterative summarization" (a persistent summary with sections for intent, file changes, decisions and next steps, into which new spans are merged) 3.70, Anthropic SDK compaction (regenerates the full summary each time) 3.44, OpenAI `/responses/compact` (opaque) 3.35.
- Artifact-tracking scores were the weakest for everyone: 2.19–2.45 out of 5.
- Compression ratios were 98.6–99.3%, and the highest-compressing method scored lowest. "The right optimization target is not tokens per request. It is tokens per task."
— [Factory: evaluating compression](https://factory.com/news/evaluating-compression)

**Cursor (2026-01-06) [DOC]**
- "Dynamic context discovery" (Jediah Katz):
  - Long shell and MCP outputs are written to files instead of truncated; the agent `tail`s and reads more as needed.
  - On summarization, the agent gets a reference to the full chat-history file so it can grep lost details.
  - MCP tool descriptions are synced to per-server folders, with only names in static context. In an A/B test this cut total agent tokens by 46.9% in runs that called an MCP tool.
  - Integrated-terminal output is synced to files.
  - The harness is "optimized individually for every new frontier model we support."
  — [Cursor blog](https://cursor.com/blog/dynamic-context-discovery)

**Amp (2025-10-23) [DOC]**
- Removed compaction entirely and replaced it with **Handoff**. Compaction is "lossy" and encourages long, wandering threads with summaries stacked on summaries. With Handoff, the user states a goal, and Amp drafts a new-thread prompt plus a list of relevant files for review. Amp can also read other threads. — [Amp: Handoff](https://ampcode.com/news/handoff)
- [RE] A third-party listing says a rebuilt Amp CLI ("Neo") re-adds automatic compaction. This is unverified on ampcode.com — [hunted.space](https://hunted.space/product/amp-free)

**Manus [DOC, 2025-07-18 + talk notes 2025-10-15]**
- KV-cache hit rate is "the most important metric":
  - Input:output ratio is about 100:1.
  - Cached vs uncached Claude Sonnet input costs $0.30 vs $3/MTok.
  - Keep a stable prefix (no second-precision timestamps), append-only context, deterministic JSON serialization, and explicit cache breakpoints. With vLLM, use prefix caching plus session-ID routing.
  — [Manus blog](https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus)
- Mask, don't remove:
  - Never add or remove tools mid-iteration, because that busts the cache and leaves history referencing undefined tools.
  - Use a context-aware state machine that masks token logits.
  - Hermes-style prefill modes: Auto, Required (prefill to the tool-call token), and Specified (prefill to a function-name prefix).
  - Tool names share prefixes (`browser_`, `shell_`) so groups can be constrained.
  — [Manus blog](https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus)
- File system as context, with "restorable compression": drop page content but keep the URL; drop document content but keep the path. — [Manus blog](https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus)
- Recitation: a typical task is about 50 tool calls. The agent rewrites `todo.md` to push the plan into recent context and counter lost-in-the-middle and goal drift. — [Manus blog](https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus)
- Keep failed actions and stack traces in context. Add controlled variation to avoid few-shot mimicry; in a 20-résumé batch the agent drifted. — [Manus blog](https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus)
- Every tool result has a "full" form stored in the sandbox FS and a "compact" form, a reference such as a path. Stale results are swapped to compact form reversibly. Summarization, with a fixed schema, runs only when compaction hits diminishing returns. — [Lance Martin: Context Engineering in Manus](https://rlancemartin.github.io/2025/10/15/manus/)

**Devin (2025-09-29) [DOC]**
- Sonnet 4.5 showed "context anxiety": it took shortcuts as it believed the window was nearly full. Enabling the 1M beta but capping usage at 200k removed this. The model writes CHANGELOG.md and SUMMARY.md notes unprompted, but those notes were not good enough to replace Devin's own compaction and memory, which Cognition kept. — [Cognition: Rebuilding Devin for Sonnet 4.5](https://cognition.com/blog/devin-sonnet-4-5-lessons-and-challenges)

**Anthropic multi-agent research system [DOC, 2025-06-13]**
- The lead agent saves its plan to Memory because contexts over 200,000 tokens get truncated. — [Anthropic](https://www.anthropic.com/engineering/multi-agent-research-system)

### Inferences
- [INF] A compaction design that combines the best-supported pieces:
  1. Spill tool output beyond about 30k characters or 10k tokens to a file and return path + head/tail. Claude Code, Cursor, Gemini and Manus all converge on this.
  2. Mask old observations (keep the action, replace the observation with a pointer). This is the evidence-backed cheap layer per the Complexity Trap study.
  3. Run an anchored, sectioned summary (intent / files changed / decisions / next steps / open errors) that is merged incrementally, not regenerated. The Factory evaluation supports this.
  4. Keep the raw log durable and searchable after compaction (Cursor history file, Managed Agents session log), so compaction is never irreversible.
  5. Put an explicit artifact or file index in the summary, since artifact tracking is the weakest dimension everywhere.
- [INF] The trigger point is a design choice tied to model behavior. Gemini compresses at 50%, which suits long-context models whose quality degrades early. Codex compresses at 90%, which maximizes cache reuse. Context-anxious models benefited from a hidden headroom cap (Devin 1M/200k).

### Gaps
- I found no official Claude Code statement of the auto-compact trigger percentage. Community claims of about 92–95% were not verified this session.
- I could not read the exact content of Codex's encrypted remote-compaction item (OpenAI post blocked).
- Manus has published nothing new on its harness since the 2025 posts and talks (searches found none).

---

## 4. Sub-agents and multi-agent: designs and evidence for and against

### Takeaway
The field has converged on a middle position:
- Read-only or isolated-context subagents for search, review and research are uncontroversial and widely deployed.
- Parallel *writers* remain unsolved. Cognition still says so in 2026.
- Breadth-first research tasks show large gains from multi-agent setups, at roughly 15× the tokens of chat.

Cognition softened "Don't build multi-agents" in April 2026 to "writes single-threaded, extra agents add intelligence, not actions." Claude Code and Codex now ship background subagents, forks, agent teams and inter-agent messaging as first-class tools.

### Cited Findings
**Evidence for**
- Anthropic research system (2025-06-13):
  - An Opus 4 lead with Sonnet 4 subagents beat single-agent Opus 4 by 90.2% on the internal research eval.
  - On BrowseComp, token usage explains 80% of variance, and tokens + tool calls + model choice explain 95%.
  - Agents use about 4× the tokens of chat, and multi-agent systems about 15×.
  - Parallel tool calling cut research time by up to 90%. The lead spawns 3–5 subagents in parallel, and each subagent uses 3+ tools in parallel. A CitationAgent runs at the end.
  - Poor fit: domains needing shared context or with many dependencies. "Most coding tasks involve fewer truly parallelizable tasks than research."
  — [Anthropic](https://www.anthropic.com/engineering/multi-agent-research-system)
- Cognition (2026-04-22), "Multi-Agents: What's Actually Working":
  - Clean-context reviewer: Devin Review finds an average of 2 bugs per Devin-written PR, about 58% of them severe. It sees only the diff, which avoids context rot.
  - "Smart friend" escalation: a fast primary model (SWE-1.5, about 950 tok/s) calls a stronger model with a forked full context. This underperforms with weak primaries and works as a capability router among frontier models.
  - Manager/child Devins coordinate via an internal MCP. This is live, but the issues are over-prescriptive managers, wrongly assumed shared state, and children not reporting back.
  - Read-only subagents (e.g., DeepWiki) behave like tool calls.
  — [Cognition](https://cognition.com/blog/multi-agents-working)
- Anthropic harness design (Prithvi Rajasekaran, 2026-03-24):
  - Structure: planner → generator → evaluator, GAN-inspired, with the evaluator using Playwright MCP and hard-threshold rubrics. Sprint contracts. File-based handoffs.
  - Retro game maker: solo run 20 min / $9 with a broken core feature; full harness 6 h / $200 with a playable result.
  - DAW on Opus 4.6: 3 h 50 min / $124.70.
  - The evaluator "is not a fixed yes-or-no decision" and is worth it when the task exceeds solo reliability.
  — [Anthropic](https://www.anthropic.com/engineering/harness-design-long-running-apps)
- Manus subagent context sharing comes in two modes:
  - "Communicating": instructions only, and the subagent returns output.
  - "Shared memory": the full context is passed for complex tasks.
  The planner defines an output schema that subagents fill via a `submit results` tool under constrained decoding. — [Lance Martin](https://rlancemartin.github.io/2025/10/15/manus/)
- Anthropic "Building a C compiler with a team of parallel Claudes" (2026-02-05) is listed. It was not opened this session — [Anthropic engineering index](https://www.anthropic.com/engineering)

**Evidence against / limits**
- Cognition, "Don't Build Multi-Agents" (Walden Yan, 2025-06-12):
  - Principles: "Share context, and share full agent traces, not just individual messages" and "Actions carry implicit decisions, and conflicting decisions carry bad results."
  - Flappy Bird example: one subagent builds a Mario-style background, the other a mismatched bird.
  - Recommendation: a single-threaded linear agent, plus a context-compression LLM for long tasks.
  - At the time it noted that Claude Code subagents never ran in parallel with the main agent and mainly answered questions.
  — [Cognition](https://cognition.com/blog/dont-build-multi-agents)
- Cognition 2026 still considers parallel writers "largely unsolved" and unstructured swarms "a distraction." It says the splashy demos (200k-LOC browser, 100k-LOC C compiler) succeeded partly because of simple, verifiable success criteria. — [Cognition](https://cognition.com/blog/multi-agents-working)
- Amp subagents "work in isolation, so they can't communicate with each other, you can't guide them mid-task, and they begin with only the instructions and context the main agent provides." Oracle is a subagent for hard reasoning and planning. — [Amp docs](https://ampcode.com/docs/models-and-subagents) (via search summary)

**Current implementations [DOC/SRC]**
- **Claude Code:**
  - `Agent` tool: separate context window, only the final result returns, background by default, `maxTurns`, resumable.
  - Forks inherit the full parent conversation.
  - Agent teams: a named teammate with a shared task list and `SendMessage`. `ListAgents` and cross-session messaging require v2.1.224+.
  - `Workflow` tool runs a script that orchestrates many subagents.
  - `isolation: worktree` per subagent.
  - The auto-mode classifier checks at delegation (can deny) and at return (warn only).
  — [Tools reference](https://code.claude.com/docs/en/tools-reference), [auto mode](https://www.anthropic.com/engineering/claude-code-auto-mode)
- **Codex:** spawn / send_input / wait / close / resume agent handlers. [RE] Subagents inherit the parent's sandbox and network rules (v0.117.0). — [handlers](https://github.com/openai/codex/tree/main/codex-rs/core/src/tools/handlers), [danielvaughan](https://codex.danielvaughan.com/2026/03/28/codex-agent-loop-deep-dive/)
- **Gemini CLI:** `invoke_agent`. **OpenHands SDK:** `delegate`, `child_conversation`, `ask_oracle`. — [Gemini tool-names](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/tools/tool-names.ts), [OpenHands tools](https://github.com/OpenHands/software-agent-sdk/tree/main/openhands-tools/openhands/tools)
- [PRIOR] Cognition "SWE-grep" (2025-10-16) is an RL-trained fast context-retrieval subagent that issues many parallel search calls per turn over a few turns. Exact figures were not re-verified — [cognition.com/blog/swe-grep](https://cognition.com/blog/swe-grep)

### Inferences
- [INF] Design rule supported by both camps: one writer per working tree. Additional agents should be (a) read-only searchers or researchers returning condensed results, (b) clean-context verifiers or reviewers, or (c) escalation "oracles" with forked full context. Parallel writers need isolation such as git worktrees plus a merge or review step.
- [INF] Multi-agent setups buy breadth and context isolation at roughly 15× tokens. They pay off when the task is parallelizable and verification is cheap.

### Gaps
- I found no controlled ablation of subagents vs single agent on coding benchmarks from any vendor. The multi-agent evidence is for research tasks (Anthropic) or is qualitative (Cognition).
- The Anthropic C-compiler post and Cognition's "Devin can now Manage Devins" were not read.

---

## 5. Memory and instructions: CLAUDE.md / AGENTS.md, Skills, hooks, MCP, plugins, slash commands

### Takeaway
Instruction files have converged on hierarchical markdown loaded at session start: CLAUDE.md, AGENTS.md and GEMINI.md, with AGENTS.md as the cross-vendor standard and Codex capping it at 32 KiB. Best practice has shifted to a short file that acts as an index pointing to deeper docs. Agent Skills (SKILL.md with progressive disclosure) has overtaken MCP in harness adoption (9/11 vs 8/11 systems). MCP tools are now lazily loaded via tool search in Claude Code and Cursor. Behavioral policy is moving from prompt prose into configuration: hooks, permission rules and settings.

### Cited Findings
- Source-code audit: Skills in SKILL.md format appear in 9 of 11 systems vs MCP in 8 of 11. ACP (Agent Client Protocol) appears in six systems and takes on a new role as "harness hosting." In the longitudinal sample "convergence has become imitation, and behavioral policy is moving from prompt prose to configuration." — [arXiv 2609.00006](https://arxiv.org/abs/2609.00006)
- Claude Code loads CLAUDE.md and can read AGENTS.md in its place. Subdirectory CLAUDE.md and path-scoped rules load when a file in that path is read, including via `cat`/`grep`. Auto memory loads MEMORY.md (first 200 lines / 25 KB). System reminders inject CLAUDE.md, output style, file-changed-on-disk notes and attribution lines. — [How Claude Code works](https://code.claude.com/docs/en/how-claude-code-works), [Tools reference](https://code.claude.com/docs/en/tools-reference)
- Skills in Claude Code: descriptions load at session start and full content on use. `disable-model-invocation: true` keeps a skill's description out of context entirely. Skills run through one `Skill` tool. — [How Claude Code works](https://code.claude.com/docs/en/how-claude-code-works)
- Codex AGENTS.md: `DEFAULT_PROJECT_DOC_MAX_BYTES = 32 * 1024`. — [config_toml.rs](https://github.com/openai/codex/blob/main/codex-rs/config/src/config_toml.rs)
- OpenAI "Harness engineering" (2026-02-11): a team of 3 growing to 7 engineers built about 1M LOC and about 1,500 merged PRs in 5 months with zero hand-written code, averaging 3.5 PRs per engineer per day. [RE] Their first, giant AGENTS.md failed. The revised one is a "table of contents" of about 100 lines pointing to a structured docs/ directory, with linters validating cross-links. — [OpenAI](https://openai.com/index/harness-engineering/) (secondary summaries via search)
- Cursor supports the Agent Skills open standard. Skill names and descriptions are static context, and the agent finds skills via grep or semantic search. — [Cursor](https://cursor.com/blog/dynamic-context-discovery)
- Gemini CLI has an `activate_skill` tool and `get_internal_docs`. — [base-declarations.ts](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/tools/definitions/base-declarations.ts)
- [PRIOR] Agent Skills (Anthropic, 2025-10-16) uses three-level progressive disclosure: (1) name and description in the system prompt; (2) the SKILL.md body is read when relevant; (3) bundled files and scripts load or execute on demand. It was later published as an open standard — [anthropic.com/engineering/equipping-agents-for-the-real-world-with-agent-skills](https://www.anthropic.com/engineering/equipping-agents-for-the-real-world-with-agent-skills), [agentskills.io](https://agentskills.io)
- [PRIOR] AGENTS.md was contributed to the Linux Foundation's Agentic AI Foundation alongside MCP in Dec 2025 — [agents.md](https://agents.md)
- Hooks and permission rules share tool names. Hook matchers use bare tool names. Permission rules use `Tool(specifier)` forms such as `Bash(npm run *)`, `Edit(/src/**)`, `WebFetch(domain:…)`, `Agent(Explore)`, `Skill(deploy *)`. — [Tools reference](https://code.claude.com/docs/en/tools-reference)
- The Claude Code April 2026 postmortem lessons: "line-by-line ablations for every system prompt change," and "gate model-specific changes to their target models." — [Anthropic](https://www.anthropic.com/engineering/april-23-postmortem)
- Devin observed that Sonnet 4.5 spontaneously writes CHANGELOG.md/SUMMARY.md "but not CLAUDE.md nor AGENTS.md." — [Cognition](https://cognition.com/blog/devin-sonnet-4-5-lessons-and-challenges)

### Inferences
- [INF] Reimplementation pattern:
  1. A tiny always-loaded instruction index (AGENTS.md, under about 100 lines / 32 KiB).
  2. Path-scoped instruction files loaded lazily when files in those paths are touched.
  3. Skills as description-only stubs with lazily loaded bodies.
  4. MCP and other large tool catalogs behind a tool-search tool.
  5. Deterministic policy (formatting, forbidden commands, notifications) in hooks and config rather than prose.

### Gaps
- I did not fetch the current Claude Code hooks event list or the plugin marketplace spec this session.
- Codex's skills and plugin implementation was not inspected beyond tool names (`list_available_plugins_to_install`, `request_plugin_install`).

---

## 6. Planning: todo/plan tools, plan mode, thinking

### Takeaway
Explicit plan or todo tools were a load-bearing scaffold in 2025: Manus todo.md recitation, Factory's plan tool, Warp's required todo list, and Codex `update_plan` with one in-progress step. In 2026, Claude Code turned its task tools OFF by default for models newer than Opus 4.7 / Sonnet 4.6, because "Claude keeps track of multi-step work without a written checklist." Read-only plan mode survives as a user-facing safety and alignment feature. Reasoning effort defaults are a major quality lever with measured regressions.

### Cited Findings
- Claude Code task tools (`TaskCreate/Get/List/Update`, `TodoWrite`) are available by default only on Opus 4.7, Sonnet 4.6, Haiku 4.5 and earlier. "On newer models, Claude keeps track of multi-step work without a written checklist, and the tools' definitions and reminders take up context." The default applies from v2.1.268. They can be re-enabled with `CLAUDE_CODE_ENABLE_TODO_TOOLS=1`. They stay on in background and cloud sessions. — [Tools reference](https://code.claude.com/docs/en/tools-reference)
- Claude Code plan mode: `EnterPlanMode`/`ExitPlanMode`. Plan mode "explores and proposes a plan without editing your source files." — [How Claude Code works](https://code.claude.com/docs/en/how-claude-code-works), [Tools reference](https://code.claude.com/docs/en/tools-reference)
- Codex `update_plan`: pending, in_progress or completed, with at most one in_progress step. — [plan_spec.rs](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/handlers/plan_spec.rs)
- Gemini CLI has `write_todos`, `enter_plan_mode`/`exit_plan_mode`, and a dependency-graph tracker (`tracker_add_dependency`, `tracker_visualize`). — [base-declarations.ts](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/tools/definitions/base-declarations.ts), [tool-names.ts](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/tools/tool-names.ts)
- OpenHands has a `TaskTrackerTool` in the default preset and a `planning_file_editor`. — [preset/default.py](https://github.com/OpenHands/software-agent-sdk/blob/main/openhands-tools/openhands/tools/preset/default.py)
- Factory's plan tool marks the done step and the next in-progress step, keeping the latest state at the end of context "where LLMs attend most." No metric was given. — [Factory](https://factory.com/news/terminal-bench)
- Warp credits "requiring the agent to keep a todo list for the whole task" for Terminal-Bench gains, and `/plan` for its Terminal-Bench 2.0 position. — [Warp](https://www.warp.dev/blog/terminal-bench), [Agents 3.0](https://www.warp.dev/blog/agents-3-full-terminal-use-plan-code-review-integration)
- Manus `todo.md` recitation counters goal drift over about 50-tool-call tasks. — [Manus](https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus)
- Reasoning effort: Claude Code lowered default effort from high to medium on 2026-03-04 to reduce latency. Users reported it "felt less intelligent," and it was reverted on 2026-04-07. "This was the wrong tradeoff." Opus 4.7 now defaults to `xhigh`, other models to `high`. — [Anthropic postmortem](https://www.anthropic.com/engineering/april-23-postmortem)
- The thinking-clearing bug (dropping old reasoning blocks on every turn) caused forgetfulness and repetition. Interleaved thinking state therefore matters across turns. — [Anthropic postmortem](https://www.anthropic.com/engineering/april-23-postmortem)
- Planning was +18% for Devin on Sonnet 4.5. — [Cognition](https://cognition.com/blog/devin-sonnet-4-5-lessons-and-challenges)
- [PRIOR] Anthropic "think" tool (2025-03-20): a no-op scratchpad tool improved τ-bench results in policy-heavy domains — [anthropic.com/engineering/claude-think-tool](https://www.anthropic.com/engineering/claude-think-tool)

### Inferences
- [INF] Todo tools are model-dependent scaffolding. They help weaker or older models with recitation, and they cost context and reminders for stronger ones. Ship them behind a per-model flag and ablate them on each model upgrade.

### Gaps
- No public ablation quantifies the todo tool's effect for any harness. Manus, Factory and Warp give only qualitative claims, and Anthropic's removal rationale is qualitative.

---

## 7. Safety: permission modes, approval flows, sandboxing, checkpoints/rewind

### Takeaway
There are two complementary layers:
1. **OS-level sandboxing.** Seatbelt on macOS. Bubblewrap + seccomp on Linux, where Codex has dropped Landlock-only for filesystem-restricted policies. Network goes through an egress proxy outside the sandbox, and credentials stay out of the sandbox.
2. **Policy-level approval.** This has moved from per-action human prompts (93% approved) to classifier-gated "auto" modes. Claude Code's two-stage, reasoning-blind transcript classifier is the default as of v2.1.283.

File checkpoints (snapshot before edit) are standard in Claude Code and Gemini CLI. Remote side effects cannot be rolled back.

### Cited Findings
**Claude Code permissions [DOC]**
- Modes are Auto (classifier; the built-in starting mode for interactive terminal and VS Code since v2.1.283), Manual, Accept edits and Plan. `Shift+Tab` cycles through them. — [How Claude Code works](https://code.claude.com/docs/en/how-claude-code-works)
- **Auto mode internals** (2026-03-25):
  - Users approve 93% of prompts.
  - Tiers:
    1. A built-in allowlist of read-only tools plus narrow user rules. Rules granting arbitrary execution (blanket shell, `python`/`node` wildcards, package-manager run) are dropped on entering auto mode.
    2. In-project edits skip the classifier.
    3. A transcript classifier handles everything else.
  - The classifier runs on Sonnet 4.6. Stage 1 is a single-token yes/no biased to block. Stage 2 is chain-of-thought and runs only when stage 1 flags; it reuses stage 1's prompt, so it is mostly cache hits.
  - Inputs are user messages and tool calls only. Assistant prose, tool descriptions and tool outputs are stripped. It is reasoning-blind, so the agent can't talk its way past it, which also limits injection.
  - A separate server-side prompt-injection probe scans tool outputs and adds warnings.
  - Results:

    | Dataset | Stage 1 | Full pipeline |
    |---|---|---|
    | Real internal traffic (n=10,000), FPR | 8.5% | 0.4% |
    | Real overeager actions (n=52), FNR | 6.6% | 17% |
    | Synthetic exfiltration (n=1,000), FNR | 1.8% | 5.7% |

  - Including assistant messages and tool results in the classifier input worsened overeagerness detection.
  — [Anthropic](https://www.anthropic.com/engineering/claude-code-auto-mode)
- **Sandboxing** (David Dworken, Oliver Weller-Davies; 2025-10-20):
  - "Sandboxing safely reduces permission prompts by 84%."
  - Linux uses bubblewrap and macOS uses Seatbelt. The working directory is read-write and nothing else is writable.
  - Network goes over a Unix socket to a proxy outside the sandbox that enforces domains and asks for new ones. Open-sourced as `sandbox-runtime`.
  - In the web version, git credentials stay outside; a proxy validates pushes (e.g., only to the configured branch) and attaches the real token.
  — [Anthropic](https://www.anthropic.com/engineering/claude-code-sandboxing)
- **Managed Agents:** credentials are never in the sandbox. The repo token is used only at init to configure the remote. MCP OAuth tokens sit in a vault behind a proxy that the harness never sees. — [Anthropic](https://www.anthropic.com/engineering/managed-agents)
- **Checkpoints:** before each edit the file is snapshotted. `Esc` `Esc` rewinds. Checkpoints are separate from git and survive resume. They skip symlinked and hard-linked files, and remote actions can't be checkpointed. — [How Claude Code works](https://code.claude.com/docs/en/how-claude-code-works)
- **Resource limits:** an optional memory cgroup across Bash, PowerShell and Monitor. Permission-gating hooks are excluded from the cap so a kernel kill can't let a blocked action through. — [Tools reference](https://code.claude.com/docs/en/tools-reference)

**Codex [SRC]**
- **Linux sandbox:**
  - Bubblewrap is the default filesystem sandbox; Codex uses the system `bwrap` if present, otherwise a bundled one.
  - `--ro-bind / /` makes the filesystem read-only by default, with writable roots layered via `--bind`.
  - `PR_SET_NO_NEW_PRIVS` and seccomp are applied after the filesystem view is set up. Restricted network policies block network syscalls, `AF_VSOCK` is blocked, and `io_uring` is blocked in all modes to prevent bypassing socket-family filters.
  - "Filesystem-restricted execution requires bubblewrap. The legacy Landlock option is rejected for these policies because it cannot isolate app-server Unix sockets."
  - WSL1 is unsupported.
  — [linux-sandbox README](https://github.com/openai/codex/blob/main/codex-rs/linux-sandbox/README.md)
- **macOS and Windows:** Seatbelt implementation files (`cli/src/debug_sandbox/seatbelt.rs`, `exec-server/src/sandbox_integrity/seatbelt.rs`), plus `windows-sandbox-rs` and `windows-sandbox-service` crates. — [codex repo tree](https://github.com/openai/codex/tree/main/codex-rs)
- **Approvals:** the `request_permissions` tool lets the model ask for escalation, and `ApprovalMessages` has `on_request` and `on_request_auto_review` variants. — [shell_spec.rs](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/handlers/shell_spec.rs), [openai_models.rs](https://github.com/openai/codex/blob/main/codex-rs/protocol/src/openai_models.rs)
- [RE] "Smart Approvals" (v0.115.0+): review requests go to a guardian subagent that approves, denies or escalates — [danielvaughan](https://codex.danielvaughan.com/2026/03/28/codex-agent-loop-deep-dive/)
- [PRIOR] Codex's documented modes are sandbox read-only / workspace-write / danger-full-access and approval policies such as untrusted / on-request / never. I did not re-fetch these this session — [developers.openai.com/codex](https://developers.openai.com/codex)

**Gemini CLI [SRC/DOC]**
- macOS Seatbelt profiles: `permissive`, `restrictive` and `strict`, each in `-open` or `-proxied` network variants (six `.sb` files). — [packages/cli/src/utils](https://github.com/google-gemini/gemini-cli/tree/main/packages/cli/src/utils)
- Checkpointing (off by default): before an approved file-modifying tool (`write_file`, `replace`) runs, Gemini commits to a shadow git repo at `~/.gemini/history/<project_hash>` and saves the conversation and the pending tool call. `/restore` reverts files, restores history and re-proposes the tool call. — [checkpointing.md](https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/checkpointing.md)

**Benchmark practice**
- Factory and Warp ran Terminal-Bench with permissions skipped or no confirmation. Leaderboard numbers therefore reflect no-approval operation. — [Factory](https://factory.com/news/terminal-bench), [Warp](https://www.warp.dev/blog/terminal-bench)

### Inferences
- [INF] The best current recipe:
  - Bubblewrap/Seatbelt filesystem jail on the workspace.
  - Egress proxy with a domain allowlist.
  - Credentials only in an out-of-sandbox proxy or vault.
  - A cheap first-stage classifier over (user messages + tool calls) only, escalating to a reasoning stage.
  - Deny-and-continue with consecutive and total denial caps.
  - Shadow-git or snapshot checkpoints for local files.
- [INF] Stripping tool outputs from the safety classifier input is the key anti-injection design choice. The authors tested and reported it.

### Gaps
- I did not verify a published FPR/FNR for Codex's guardian or auto-review.
- Gemini CLI's Linux container sandbox (Docker/Podman) details were not inspected this session.

---

## 8. System-prompt size and deleting scaffolding as models improve ("the harness getting thinner")

### Takeaway
There is strong, repeated evidence that teams delete scaffolding when models improve:
- Anthropic removed sprints and context resets for Opus 4.5 and 4.6.
- Claude Code turned off todo tools for models after Opus 4.7, folded Glob/Grep into Bash, and reportedly cut more than 80% of its system prompt for Opus 5 (July 2026).
- Codex ships a much smaller prompt for Codex-trained models (about 1.1k words vs about 3.9k for GPT-5.1).
- Amp deleted compaction.

The deletion method is line-by-line ablation, and the counter-evidence is just as important: single prompt lines measurably move quality, e.g. a 3% drop from one verbosity line in April 2026.

### Cited Findings
- **Anthropic harness design (2026-03-24):** "every component in a harness encodes an assumption about what the model can't do on its own," and those assumptions should be stress-tested. Sprints and context resets were removed because Opus 4.5 "largely removed that behavior on its own," and "there was good reason to expect 4.6 would need less scaffolding than 4.5 did." — [Anthropic](https://www.anthropic.com/engineering/harness-design-long-running-apps)
- **Managed Agents (2026-04-08):** context resets added for Sonnet 4.5's context anxiety became "dead weight" on Opus 4.5. Harness assumptions "go stale as models improve." — [Anthropic](https://www.anthropic.com/engineering/managed-agents)
- **Claude Code task tools** are off by default for models newer than Opus 4.7 / Sonnet 4.6 (v2.1.268+). The stated reason is that the model tracks work itself and the tools cost context. — [Tools reference](https://code.claude.com/docs/en/tools-reference)
- **Glob/Grep** are removed by default on Unix in favor of Bash with embedded `bfs`/`ugrep`. `TaskOutput` was removed in v2.1.277, and the model reads background output files with `Read`. The read-before-edit requirement was relaxed for newer models. — [Tools reference](https://code.claude.com/docs/en/tools-reference)
- **[RE] Opus 5 cut:** Boris Cherny (Claude Code lead) said at YC Startup School 2026 (around 2026-07-28, 4 days after Opus 5's 2026-07-24 launch) that the team deleted more than 80% of Claude Code's system prompt for Opus 5 and it performed better. Method: "you delete the entire system prompt and then you bring it back line by line to figure out what is the impact of each individual line." Many instructions corrected behaviors the model "should have known… Now, Opus 5 just does it." Prompts should be re-checked on every model release. This comes from secondary reports in Chinese and English tech press; I found no Anthropic primary document. — [36kr](https://www.36kr.com/p/3916538775727746), [bnext](https://www.bnext.com.tw/article/91670/claude-code-prompt-reduction-agent-verification), [woshipm](https://www.woshipm.com/ai/6453025.html)
- [RE] Cherny reportedly builds "for the model six months from now," and frames non-model code as scaffolding: build it now for a 10–20% gain or wait and the model does it. An undocumented "simple" mode that strips system prompts is reported from a single source and is unverified — [charlesjones.dev](https://charlesjones.dev/blog/claude-opus-5-context-engineering-what-to-delete), [BigGo](https://finance.biggo.com/news/7df48019614f68c0)
- **Counter-evidence (2026-04-23):** a single system-prompt line limiting between-tool text to 25 words and final responses to 100 words caused a 3% eval drop on both Opus 4.6 and 4.7, and was reverted after 4 days. Lessons: "line-by-line ablations for every system prompt change," gate model-specific changes, and use soak periods. — [Anthropic](https://www.anthropic.com/engineering/april-23-postmortem)
- **Claude Code prompt size [RE]:** 500+ conditional prompt strings, with the tracked count growing from 350 to 515 by June 2026. Tool descriptions: 27. Subagent prompts are about 0.4–1.1k tokens each. [INF] The always-loaded core is much smaller than the string count suggests, but the tracker gives no single total — [Piebald-AI](https://github.com/Piebald-AI/claude-code-system-prompts)
- **Codex prompt files** (word counts measured from the repo on 2026-10-19):

  | File | Words |
  |---|---|
  | `gpt_5_codex_prompt.md` | 1,088 |
  | `gpt-5.1-codex-max_prompt.md` | 1,221 |
  | `gpt-5.2-codex_prompt.md` | 1,221 |
  | `gpt_5_1_prompt.md` | 3,932 |
  | `gpt_5_2_prompt.md` | 3,509 |
  | `models-manager/prompt.md` | 3,389 |

  Codex-trained models get about 3× shorter instructions than general GPT-5.x. [INF] Newer model prompts appear to be delivered via model metadata (models-manager) rather than per-model files. — [codex-rs/core](https://github.com/openai/codex/tree/main/codex-rs/core)
- [RE] The GPT-5.3-Codex guide says its prompt started from the GPT-5.1-Codex-Max default and was "further optimized against internal evals." A secondary blog claims the GPT-5.6 guide (2026-07-09) reported that leaner prompts improved evals 10–15% while cutting tokens 41–66%; this is unverified on OpenAI's site — [OpenAI GPT-5.3-Codex guide](https://developers.openai.com/api/docs/guides/gpt-5.3-codex), [danielvaughan](https://codex.danielvaughan.com/2026/07/26/gpt56-outcome-first-prompting-lean-system-prompts-codex-cli-agents-md-strategy/)
- **Amp:** deleted compaction in favor of Handoff (2025-10-23). — [Amp](https://ampcode.com/news/handoff)
- **Manus:** rebuilt its framework 4 times by July 2025 ("Stochastic Graduate Descent") and refactored 5 times since its March 2025 launch by October 2025. Peak Ji warned that the harness can limit performance as models advance, and recommends testing across model strengths to check that the harness is "future proof." — [Manus](https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus), [Lance Martin](https://rlancemartin.github.io/2025/10/15/manus/)
- **Devin:** rebuilt for Sonnet 4.5 rather than swapping the model, for 2× speed and +12% end-to-end evals. It kept its own memory and compaction because the model's self-notes were insufficient. Not every scaffold gets deleted. — [Cognition](https://cognition.com/blog/devin-sonnet-4-5-lessons-and-challenges)
- **Cursor:** the harness is tuned per frontier model, and "providing fewer details up front works better as models improve." — [Cursor](https://cursor.com/blog/dynamic-context-discovery)
- **Harness can still dominate:** Factory Droid with the same models (Opus 4.1) scored 58.8% vs Claude Code 43.2% on Terminal-Bench v0.1.1 in Sept 2025. Hashline lifted Grok Code Fast 1 from 6.7% to 68.3%. — [Factory](https://factory.com/news/terminal-bench), [Bölük](https://stencil.so/blog/the-harness-problem)

### Inferences
- [INF] Rule of thumb: keep the stable mechanics, which models don't make obsolete:
  - the loop, sandbox, permissions, durable log, output spilling, and cache-stable layout;
  - tool interfaces matched to the model's training format;
  - verifiers or evaluators for tasks beyond solo reliability.
- [INF] Treat these as per-model, deletable scaffolding, re-ablated at every model release:
  - behavioral prompt prose;
  - todo/plan tools and reminders;
  - context resets and anxiety mitigations;
  - read-before-edit rules;
  - extra search tools.
- [INF] The way to delete safely is the Anthropic recipe: remove everything, add back line by line with a per-model eval suite, then soak and roll out gradually.

### Gaps
- The Opus 5 "80% deletion" claim rests on secondary press coverage of a talk. I found no Anthropic primary document or before/after token counts.
- I found no quantified system-prompt token totals for the current Claude Code release.
- Codex team quotes on harness thinning (beyond prompt-size evidence in the repo) were not found in primary form.
- Cursor, Amp, Factory, Warp and Devin publish no system-prompt sizes.
