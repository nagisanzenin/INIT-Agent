# Evidence base: when sub-agents and multi-agent LLM systems help or hurt (as of 2026-10-22)

Legend. **[DATA]** = a measured number, with the conditions as reported. **[GUIDANCE]** = a vendor recommendation or design rule. **[OPINION]** = an argument or forecast. **[UNVERIFIED]** = only seen in a secondary source or a search snippet, not confirmed on the primary page. **[OLD yyyy-mm]** = the source is more than about 6 months old (before about 2026-04-22) and may be superseded. All primary pages were fetched on 2026-10-22.

## 1. Anthropic: the multi-agent research system (June 2025) and 2026 follow-ups

### Takeaway
Anthropic's orchestrator-worker research system beat single-agent Claude Opus 4 by 90.2% on an internal research eval. It cost about 15× chat tokens, against about 4× for a single agent. Token usage alone explained 80% of BrowseComp variance, so most of the gain is spending more tokens across parallel, isolated contexts. Anthropic itself scopes the pattern to breadth-first, read-heavy research. It names shared-context work, heavily dependent work and "most coding tasks" as poor fits. The 2026 follow-ups (a C compiler built by 16 parallel Claudes, a planner-generator-evaluator harness, agent teams) do run parallel writers on code. They make it work only with an external oracle (tests or GCC), mechanical locks, file ownership and a CI gate, and they report cost multipliers of 20× or more.

### Cited Findings
**"How we built our multi-agent research system", 2025-06-13 [OLD 2025-06]**, https://www.anthropic.com/engineering/multi-agent-research-system
- [DATA] A Claude Opus 4 lead with Claude Sonnet 4 subagents "outperformed single-agent Claude Opus 4 by 90.2% on our internal research eval". The eval started at about 20 queries drawn from real usage. Grading was LLM-as-judge: "a single LLM call with a single prompt outputting scores from 0.0-1.0 and a pass-fail grade", later scaled to hundreds of outputs. No CI, no n for the 90.2% figure, and no equal-token single-agent ablation are published.
- [DATA] Single agents use roughly 4× the tokens of a chat interaction (paraphrase); "multi-agent systems use about 15× more tokens than chats." "There is a downside: in practice, these architectures burn through tokens fast."
- [DATA] On BrowseComp, "three factors explained 95% of the performance variance": token usage, number of tool calls and model choice. "token usage by itself explains 80% of the variance". Upgrading to Claude Sonnet 4 gave a larger gain than doubling the token budget on Claude Sonnet 3.7 (no number given).
- [DATA] Parallelism: the lead spins up 3-5 subagents in parallel, and each subagent uses 3+ tools in parallel. Together this "cut research time by up to 90% for complex queries" (wall-clock, not accuracy).
- [DATA] A tool-testing agent that rewrote a flawed MCP tool description gave "a 40% decrease in task completion time for future agents using the new description".
- [GUIDANCE] Effort-scaling rules embedded in the lead prompt: "Simple fact-finding requires just 1 agent with 3-10 tool calls". Direct comparisons get 2-4 subagents with 10-15 calls each. Complex research gets more than 10 subagents with divided responsibilities.
- [GUIDANCE] Each delegation needs an objective, an output format, tool and source guidance, and clear task boundaries. Vague instructions led to duplicated work.
- [DATA, qualitative] Early failure modes: "spawning 50 subagents for simple queries", endless searching, and agents flooding each other with updates.
- [GUIDANCE] Poor fit: "some domains that require all agents to share the same context or involve many dependencies between agents are not a good fit for multi-agent systems today", and "most coding tasks involve fewer truly parallelizable tasks than research". Agents are also not yet good at real-time coordination and delegation.
- [DATA, qualitative] Execution was synchronous: the lead waits for each set of subagents, cannot steer them, the subagents cannot coordinate, and one slow subagent blocks the whole system.
- [OPINION] "small changes to the lead agent can unpredictably change how subagents behave". "Agents are stateful and errors compound." They use rainbow deployments to avoid disrupting running agents.
- [GUIDANCE] Subagents can write outputs to a filesystem and return references, which avoids a "game of telephone" through the coordinator.

**"Effective context engineering for AI agents", 2025-09-29 [OLD 2025-09]**, https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents
- [GUIDANCE] Each subagent "returns only a condensed, distilled summary of its work (often 1,000-2,000 tokens)". This "achieves a clear separation of concerns".
- [GUIDANCE] Use compaction for "tasks requiring extensive back-and-forth" and structured note-taking for "iterative development with clear milestones". Multi-agent architectures "handle complex research and analysis where parallel exploration pays dividends."

**"Effective harnesses for long-running agents", 2025-11-26 [OLD 2025-11]**, https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents
- [DATA, qualitative] The harness is an initializer agent plus a coding agent. They share one system prompt, toolset and harness and differ only in the first user prompt, so this is a role split, not a team.
- [DATA, qualitative] Work goes one feature at a time: "This incremental approach turned out to be critical to addressing the agent's tendency to do too much at once."
- [DATA, qualitative] Failure modes: trying "to attempt to one-shot the app", declaring the job done too early, leaving broken or undocumented state, and marking features done after only unit tests or curl.
- [DATA, qualitative] A JSON feature list was used because the model is less likely to wrongly overwrite it than Markdown.
- [OPINION] "still unclear whether a single, general-purpose coding agent performs best across contexts, or if better performance can be achieved through a multi-agent architecture".

**"Building a C compiler with a team of parallel Claudes" (N. Carlini), 2026-02-05 [OLD 2026-02]**, https://www.anthropic.com/engineering/building-c-compiler
- [DATA] 16 agents on Opus 4.6, "nearly 2,000 Claude Code sessions" over two weeks: 2 billion input tokens, 140 million output tokens, "just under $20,000". The result is a 100,000-line Rust C compiler with "a 99% pass rate on most compiler test suites including the GCC torture test suite". It builds QEMU, FFmpeg, SQLite, Postgres and Redis, and boots Linux 6.9 on x86, ARM and RISC-V.
- [DATA, qualitative] Coordination had no orchestrator: "I don't use an orchestration agent". Agents claim a task by writing a lock file in `current_tasks/`, and git sync forces a second claimant to pick another task. Merge conflicts were "frequent, but Claude is smart enough to figure that out."
- [DATA, qualitative] The failure on one giant task: on the Linux kernel, "Every agent would hit the same bug, fix that bug, and then overwrite each other's changes... Having 16 agents running didn't help because each was stuck solving the same task." The fix used GCC as a known-good oracle: GCC compiled most files and Claude's compiler compiled a random subset, so agents worked on different bugs. Delta debugging found the failing file pairs.
- [DATA, qualitative] "New features and bugfixes frequently broke existing functionality", which led to stricter CI enforcement. Roles were specialized: dedup, performance, codegen quality, a Rust-critic and docs.
- [OPINION] "When there are many distinct failing tests, parallelization is trivial." "it is easy to see tests pass and assume the job is done, when this is rarely the case."

**"Harness design for long-running application development", 2026-03-24 [OLD 2026-03]**, https://www.anthropic.com/engineering/harness-design-long-running-apps
- [DATA] Retro game maker on Opus 4.5: a solo run took 20 min and cost $9. The full planner + generator + evaluator harness took 6 hr and cost $200, "over 20x more expensive". The DAW build with the v2 harness on Opus 4.6 took 3 hr 50 min and cost $124.70 (planner $0.46, first build 2 hr 7 min and $71.08, QA rounds about $3-4 each).
- [DATA, qualitative] Self-evaluation bias: agents praise their own mediocre work. "tuning a standalone evaluator to be skeptical turns out to be far more tractable" than making the generator self-critical. Out of the box, Claude was a weak QA agent that found real issues "then talked itself out of" them.
- [DATA, qualitative] Components were removed as models improved: context resets were dropped with Opus 4.5 and sprints with Opus 4.6. The planner stayed because the generator under-scoped without it. "every component in a harness encodes an assumption about what the model can't do on its own". The evaluator is worth its cost only when the task sits beyond what the model does reliably alone.

**"Scaling Managed Agents: Decoupling the brain from the hands", 2026-04-08 [OLD 2026-04, borderline]**, https://www.anthropic.com/engineering/managed-agents
- [DATA] Lazy sandbox provisioning cut p50 time-to-first-token by about 60% and p95 by more than 90%.
- [GUIDANCE] Stateless harnesses ("brains") sit over an append-only session log, separate from replaceable sandboxes ("hands"): "brains can pass hands to one another". "the tokens are never reachable from the sandbox where Claude's generated code runs". Not a quality result on sub-agents.

**Claude Code "agent teams" documentation (current page; feature introduced around 2026-02-05 per a secondary source [UNVERIFIED date])**, https://code.claude.com/docs/en/agent-teams
- [GUIDANCE] The feature is experimental and off by default. "Agent teams add coordination overhead and use significantly more tokens than a single session... For sequential tasks, same-file edits, or work with many dependencies, a single session or subagents are more effective."
- [GUIDANCE] "Two teammates editing the same file leads to overwrites. Break the work so each teammate owns a different set of files." Start with 3-5 teammates and 5-6 tasks each. "Three focused teammates often outperform five scattered ones." "Token costs scale linearly." Start with research and review, not parallel implementation.
- [GUIDANCE] Limits: no nested teams, so teammates cannot spawn teammates. Task status can lag, and the lead can stop early. A message from another agent is marked as coming from another Claude session: a teammate "can't approve a permission prompt or supply consent on your behalf". Task claiming uses file locking.
- [UNVERIFIED] Third-party posts say teams use "about 7x" or "3-7x" more tokens than a standard session (https://pub.towardsai.net/claude-code-costs-about-13-a-day-per-developer-and-agent-teams-use-roughly-7x-the-tokens-8fdea1ea40ad). The official page as fetched says only "significantly more tokens".

### Inferences
- The 90.2% is a relative gain on a small, private, LLM-judged eval. It conflates more tokens, parallel context windows and a model mix. Given the 80%-of-variance result, the safest reading is that multi-agent is a way to spend more tokens without overflowing one context. The orchestration itself is a smaller part of the gain.
- 15× chat against 4× for one agent implies the multi-agent system costs roughly 3.75× a single agent. This is a derived ratio, not a reported one. The 2026 coding harness was over 20× a solo run.
- Every Anthropic success with parallel writers in 2026 had an external oracle (test suites, GCC), mechanical mutual exclusion (lock files, git) and a regression gate (CI). Where the work did not split against the oracle (the Linux kernel), 16 agents gave no speedup and overwrote each other's fixes.

### Gaps
- Anthropic has not published an updated research-system eval for Opus 4.5/4.6 or later, or an equal-budget single-agent ablation. None found.
- No official quantitative eval of agent teams (accuracy or token multiplier) was found.

## 2. Cognition: "Don't Build Multi-Agents" (June 2025) and its 2026 revision

### Takeaway
Cognition narrowed its stance; it did not reverse it. On 2026-04-22 Walden Yan wrote that the 2025 observations "still hold today for parallel-writer swarms". What works is "when writes stay single-threaded": read-only subagents, a clean-context generator-verifier review loop, "smart friend" escalation between two strong models, and manager/child Devins on independent pieces in isolated VMs (shipped 2026-03-19). Since mid-2026 the main multi-agent payoff Cognition reports is cost (Devin Fusion, a frontier lead plus a cheaper sidekick, "up to" 60% cheaper at near-frontier score), not accuracy. Their own caveat: "When the judgment is the deliverable, delegating it backfires."

### Cited Findings
**"Don't Build Multi-Agents" (Walden Yan), 2025-06-12 [OLD 2025-06]**, https://cognition.com/blog/dont-build-multi-agents
- [OPINION] Principle 1: "Share context, and share full agent traces, not just individual messages". Principle 2: "Actions carry implicit decisions, and conflicting decisions carry bad results".
- [OPINION] Flappy Bird example: one subagent built a Super Mario-style background and the other a bird that did not look or move like the Flappy Bird bird. "The actions subagent 1 took and the actions subagent 2 took were based on conflicting assumptions not prescribed upfront." Even with shared context, the styles clashed because neither agent saw the other's choices.
- [GUIDANCE] The recommended default is a single-threaded linear agent, plus a separate compression model for long tasks ("This is hard to get right"). Cognition fine-tuned smaller models for compression.
- [OPINION] In June 2025 Claude Code "never does work in parallel with the subtask agent", and the subtask agent is "usually only tasked with answering a question, not writing any code." "running multiple agents in collaboration only results in fragile systems."

**"Devin can now Manage Devins", 2026-03-19 [OLD 2026-03]**, https://cognition.com/blog/devin-can-now-manage-devins
- [GUIDANCE] A coordinator scopes the work, gives each piece to a managed Devin "running in its own isolated virtual machine", monitors progress, "resolve[s] any conflicts" and compiles the results. It can read child trajectories to improve later breakdowns.
- [GUIDANCE] The recommended uses all assume independent pieces: migrations, parallel QA across pages, per-service audits, refactors in independent batches.
- No success rates, speedups or cost figures are given, and the page does not explain how conflicts are detected.

**"Multi-Agents: What's Actually Working" (Walden Yan), 2026-04-22 [borderline, exactly 6 months]**, https://cognition.com/blog/multi-agents-working
- [OPINION] "Our original observations still hold today for parallel-writer swarms." "multi-agent systems work best today when writes stay single-threaded". Because of principle 2, most multi-agent systems are limited to read-only subagents (web search, code search), which behave more like tool calls than collaboration.
- [OPINION/DATA, qualitative] "clean context leads to a notable improvement in capabilities when using a generator-verifier loop" (Devin coding, Devin Review reviewing). No number is given.
- [DATA] Devin Review catches an average of 2 bugs per PR, about 58% of them severe. Enterprise Devin usage grew about 8× in 6 months.
- [OPINION] "smart-friend works today when both models are strong." With SWE-1.5 as the primary it did not work: the quality ceiling is set by the primary. Weak-primary escalation "is still an open problem, and we think it's a training one".
- [DATA, qualitative] Failures: managers trained on small tasks become overly prescriptive and lack deep codebase context; agents assume they share state with their children when they do not; child-to-manager communication does not happen by default; weaker models do not know when to escalate. Unstructured negotiating swarms are "mostly a distraction". The stated goal is "a coordinated system that scales human taste", not "a swarm of autonomous actors".

**"Devin Fusion: Frontier Performance at 60% Lower Cost", 2026-06-29 (benchmark updated 2026-08-07)**, https://cognition.com/blog/devin-fusion
- [DATA, vendor-run benchmark] FrontierCode 1.1 Extended, score and average cost per task: Fable 5 (xhigh) 64.9 at $10.53; Opus 5 (medium) 63.6 at $3.51; **Devin Fusion 63.1 at $1.35**; GPT-5.6 Sol (high) 58.7 at $3.41; Kimi K3 58.2 at $3.12; Grok 4.5 (high) 56.6 at $1.09. The cost reduction is "up to" 60%; an earlier 35% figure was revised. Fusion with Fable 5 was 41% cheaper than a pure Fable 5 harness at equal performance. The page notes Fable 5 access was suspended on 2026-06-12.
- [DATA] 88% of merged PRs from internal Fusion users were driven entirely by the automated router. Delegating a slow test run to the sidekick while the main agent wrote the diff cut cost 62% "with no quality loss".
- [DATA, qualitative] On a hard, judgment-heavy feature, delegating the coding lost the subtle intent: "When the judgment is the deliverable, delegating it backfires." The main agent "should delegate and monitor" and keeps "the significant decisions".
- [UNVERIFIED] "Introducing Fusion in Devin Desktop & CLI", 2026-09-11 (https://cognition.com/blog/local-fusion): a frontier lead plans and reviews while a cheaper execution agent (for example SWE-2) acts, each with its own persistent context. This comes from a search summary; the post was not fetched.

### Inferences
- Cognition and Anthropic converge on one rule: one writer per artifact, with delegation used for reading, verification, mechanical execution and cost routing. Neither publishes a controlled accuracy gain for parallel writers.
- In 2026 the strongest quantified payoff of a two-agent split is cost at equal quality (Fusion), not higher quality. And it fails exactly where the subtask carries implicit judgment, which is principle 2 restated with data.

### Gaps
- No controlled single- versus multi-agent accuracy numbers from Cognition. FrontierCode is Cognition's own benchmark, so there is no independent replication.
- The "Devin manages Devins" page gives no numbers on conflict rates or merge failures.

## 3. MAST: "Why Do Multi-Agent LLM Systems Fail?" (Cemri et al.) and follow-ups

### Takeaway
MAST annotates 1,600+ traces from 7 open-source multi-agent frameworks. It reports failure rates of 41% to 86.7%, sorted into 14 failure modes in 3 categories. By my sum of the per-mode numbers, specification and system design causes about 44%, inter-agent misalignment about 32% and task verification about 24%. Prompt and topology interventions help only modestly: ChatDev on ProgramDev went from 25.0% to 34.4% with a better prompt and to 40.6% with a new topology, and HumanEval moved from 89.6 to 91.5. The authors conclude that fundamental MAS redesign is needed and that better base models will not be enough. A follow-up shows that even locating the failing agent and step is hard: the best method reaches 53.5% at agent level and 14.2% at step level.

### Cited Findings
**MAST, arXiv:2503.13657 (v1 2025-03-17, v2 2025-04-22, v3 2025-10-26) [OLD 2025-10]**, https://arxiv.org/abs/2503.13657 and https://arxiv.org/html/2503.13657v3
- [DATA] 1,600+ annotated traces across 7 MAS frameworks (MetaGPT, ChatDev, HyperAgent, AppWorld, AG2, Magentic-One, OpenManus). 150 traces were used to build the taxonomy. Human inter-annotator κ = 0.88. Models in the traces: GPT-4, Claude 3, Qwen2.5 and CodeLlama, on coding, math and general-agent tasks.
- [DATA] "41% to 86.7% failure rate on 7 state-of-the-art (SOTA) open-source MAS". The paper is internally inconsistent here: the Figure 5 caption says six systems.
- [DATA] Failure-mode prevalence (v3):
  - FC1, specification and system design: FM-1.1 disobey task spec 11.8%; FM-1.2 disobey role spec 1.5%; FM-1.3 step repetition 15.7%; FM-1.4 loss of conversation history 2.80%; FM-1.5 unaware of termination conditions 12.4%.
  - FC2, inter-agent misalignment: FM-2.1 conversation reset 2.20%; FM-2.2 fail to ask for clarification 6.80%; FM-2.3 task derailment 7.40%; FM-2.4 information withholding 0.85%; FM-2.5 ignored other agent's input 1.90%; FM-2.6 reasoning-action mismatch 13.2%.
  - FC3, task verification: FM-3.1 premature termination 6.20%; FM-3.2 no or incomplete verification 8.20%; FM-3.3 incorrect verification 9.10%.
  - Category totals are my sums of the listed numbers, not quoted from the paper: FC1 ≈ 44.2%, FC2 ≈ 32.4%, FC3 ≈ 23.5%.
- [DATA] Interventions (Table 5, accuracy %):
  - AG2 on GSM-Plus, GPT-4: baseline 84.75 ± 1.94; improved prompt 89.75 ± 1.44 (significant); new topology 85.50 ± 1.18 (Wilcoxon p = 0.4, not significant).
  - AG2 on GSM-Plus, GPT-4o: baseline 84.25 ± 1.86; improved prompt 89.00 ± 1.38; new topology 88.83 ± 1.51 (p = 0.03 for both).
  - ChatDev on ProgramDev-v0: 25.0 → 34.4 (prompt) → 40.6 (topology).
  - ChatDev on HumanEval: 89.6 → 90.3 → 91.5.
  - The text also cites +9.4% (CEO given the final say) and +15.6% (verifier step) task success for ChatDev on ProgramDev with GPT-4o.
- [DATA] LLM-as-judge annotator: o1 had accuracy 0.89 and κ 0.58; o1 few-shot had accuracy 0.94, F1 0.80 and κ 0.77. Out-of-domain κ on OpenManus and Magentic-One was 0.79.
- [OPINION] The paper's conclusions: MAS failures require "fundamental MAS redesigns"; "improvements in the base model capabilities will be insufficient to address the full MAST"; "Solutions focused on context or communication protocols are often insufficient for FC2 failures"; and tactical fixes give results that are "not consistent".

**Who&When: "Which Agent Causes Task Failures and When?" (Zhang et al.), arXiv:2505.00212 (v3 2025-06-02) [OLD 2025-06]**, https://arxiv.org/abs/2505.00212
- [DATA] Failure logs from 127 LLM multi-agent systems, annotated with the failing agent and the decisive error step.
- [DATA] The best of three methods reached 53.5% at agent level and 14.2% at step level; some methods fell below random. "Even SOTA reasoning models, such as OpenAI o1 and DeepSeek R1, fail to achieve practical usability."

### Inferences
- About a quarter of MAS failures are verification failures (premature stop, no verification, wrong verification), and about 44% are specification and termination problems. Both are harness properties: explicit termination criteria, an independent verifier, and a spec that travels with each delegation.
- Interventions buy single-digit to about 15-point gains on weak frameworks. Topology changes (for example adding a verifier loop) beat prompt tweaks on ChatDev.
- Attributing a failure to an agent or step with an LLM judge is unreliable (14.2% at step level). Mechanical attribution through IDs, logs and per-change grouping is preferable to LLM-judged blame.

### Gaps
- MAST's frameworks and models are 2023-2024 generation (GPT-4, Claude 3). I found no MAST-style re-annotation of 2026 frontier-model systems such as Claude Code subagents, Devin managers or Codex subagents.
- Per-framework failure rates appear only in figures; the HTML text does not give them.

## 4. Magentic-One and Magentic-UI (Microsoft)

### Takeaway
Magentic-One (Nov 2024) is a ledger-driven orchestrator with four specialist agents. It was statistically competitive with the then state of the art on GAIA and AssistantBench and below it on WebArena. Its own ablation shows the orchestrator's ledgers matter: replacing them with a plain group chat cost 31% on GAIA validation. Its most common errors are looping and insufficient verification. Magentic-UI (Jul 2025) shows that a human pinged on only about 10% of tasks lifts GAIA completion from 30.3% to 51.9%. Both are old by 2026 standards and far below current GAIA leaders.

### Cited Findings
**Magentic-One, arXiv:2411.04468 (2024-11-07) [OLD 2024-11]**, https://arxiv.org/abs/2411.04468 and https://arxiv.org/html/2411.04468v1
- [DATA] Test-set completion %, ± 95% CI:
  - GAIA: Magentic-One (GPT-4o) 32.33 ± 5.3; with o1-preview 38.00 ± 5.5. Best baseline omne v0.1 40.53 ± 5.6; human 92.00.
  - AssistantBench accuracy: 25.3 ± 6.3 (GPT-4o); 27.7 ± 6.5 (with o1). SPA→CB (Claude) 26.4.
  - WebArena (all 812 tasks): 32.8 ± 3.2 (GPT-4o). Jace.AI 57.1 ± 3.4; human 78.2.
- [DATA] Ledgers: the outer loop keeps a Task Ledger (facts, guesses, plan). The inner loop keeps a Progress Ledger that answers five questions each step: done? looping? progressing? who speaks next? what instruction? A stall counter above 2 triggers re-planning, and all agents reset their contexts at each plan update.
- [DATA] Ablation on GAIA validation with GPT-4o: replacing the Orchestrator with a simple GroupChat-style orchestrator (no ledgers, no planning) cost 31%. Removing any one worker cost 21% (Coder, ComputerTerminal) to 39% (FileSurfer).
- [DATA, qualitative] The most common error codes were persistent-inefficient-actions (repeating unproductive actions) and insufficient-verification-steps (marking done without validation).
- [DATA] o1 refused about 26% of GitLab and 12% of Shopping Admin WebArena tasks, so it was not run there.

**Magentic-UI, arXiv:2507.22358 (2025-07) [OLD 2025-07]**, https://arxiv.org/html/2507.22358v1
- [DATA] Autonomous: GAIA 42.52 (o4-mini) against Magentic-One's 38.00; AssistantBench 27.6; WebVoyager 82.2 (o4-mini) or 72.2 (GPT-4o); WebGames 45.5.
- [DATA] Simulated user on GAIA validation (162 tasks):
  - Autonomous: 30.3%.
  - GPT-4o simulated user with side information (a human-written plan): 51.9%, a 71% relative gain. Help was requested in about 10% of tasks, 1.1 times on average.
  - o4-mini simulated user: 42.6%, with help requested in 4.3% of tasks.
- [DATA] Safety: 0 of 24 internal adversarial scenarios succeeded in the default configuration. With mitigations deliberately disabled, prompt injections succeeded, with outcomes including SSH key exfiltration and autonomously approving actions.
- [GUIDANCE] Mechanisms: co-planning, co-tasking, action guards (approval for irreversible actions, by heuristics plus an LLM judge), answer verification, plan memory and multi-tasking.

### Inferences
- A structured orchestrator state (ledger, progress check, stall counter) is measurably better than free-form group chat. This is the Magentic-One analogue of Google's finding that centralized coordination contains error amplification (section 9).
- A rare, targeted external signal (a human on about 10% of tasks) gave more lift (+71% relative) than any architecture change reported in these papers.

### Gaps
- No 2026 Magentic release with new benchmark numbers was found. Microsoft's later agent-framework work was out of scope and not searched.

## 5. OpenAI: Agents SDK (handoffs vs agents-as-tools) and "A practical guide to building agents"

### Takeaway
OpenAI's guidance says to "maximize a single agent's capabilities first". Split into several agents only on evidence: prompts with many if-then-else branches, or tool-selection errors that better tool descriptions do not fix. Some agents handle more than 15 distinct tools while others struggle with fewer than 10 overlapping ones. In the SDK, agents-as-tools (a manager keeps control and the final answer) fits bounded subtasks; handoffs (the specialist owns the rest of the turn) fit routing. This is design guidance with no published numbers.

### Cited Findings
**"A practical guide to building agents" (PDF; date not printed in the extracted text, widely cited as April 2025 [UNVERIFIED date; OLD])**, https://cdn.openai.com/business-guides-and-resources/a-practical-guide-to-building-agents.pdf
- [GUIDANCE] "Our general recommendation is to maximize a single agent's capabilities first. More agents can provide intuitive separation of concepts, but can introduce additional complexity and overhead, so often a single agent with tools is sufficient."
- [GUIDANCE] Before splitting, use a single flexible base prompt with policy variables instead of many prompts.
- [GUIDANCE] When to split, for "Complex logic": prompts with "many conditional statements (multiple if-then-else branches)". For "Tool overload": "The issue isn't solely the number of tools, but their similarity or overlap. Some implementations successfully manage more than 15 well-defined, distinct tools while others struggle with fewer than 10 overlapping tools. Use multiple agents if improving tool clarity... doesn't improve performance."
- [GUIDANCE] Manager pattern: a central LLM orchestrates specialists through tool calls, for when one agent should control execution and talk to the user. Decentralized pattern: handoffs are a "one way transfer" that passes the latest conversation state, for when no single agent needs to keep central control or synthesis.
- [GUIDANCE] "starting with a single agent and evolving to multi-agent systems only when needed."

**OpenAI Agents SDK docs, "Agent orchestration" (current page, no date shown)**, https://openai.github.io/openai-agents-python/multi_agent/
- [GUIDANCE] Two styles: LLM orchestration and code orchestration. Code orchestration is more predictable in speed, cost and performance, and the two can be combined.
- [GUIDANCE] Use agents-as-tools when a specialist "should help with a bounded subtask but should not take over the user-facing conversation", or when one agent should own the final answer and enforce shared guardrails. Use handoffs when routing is part of the workflow and the specialist should own the rest of the turn.
- [GUIDANCE] Listed patterns: manager, chaining, an evaluator loop (a while loop until the evaluator passes), parallel independent agents via `asyncio.gather`, and structured-output routing.

**Codex subagents (2026) [UNVERIFIED]**, https://skill-sprinters.de/blog/tools/chatgpt-codex-multiagent-subagents-mai-2026/ and https://codex.danielvaughan.com/2026/06/08/parallel-subagent-race-codex-cli-multi-agent-v2-claude-dynamic-workflows-architecture-comparison/
- [UNVERIFIED] Secondary sources say Codex spawns parallel subagents and waits for all of them before returning one consolidated answer. One German source says up to 8 parallel subagents since May 2026. Launch dates conflict across sources (March, June and July 2026). The best uses cited are read-heavy work (exploration, PR review), with warnings about parallel writes to one repo. A claimed "5-7x speedup" could not be traced to OpenAI.

### Inferences
- OpenAI's split criteria are about the instruction and tool surface (branchy prompts, overlapping tools), not parallelism. That matches LangChain's distractor-domain result (section 6): splitting helps when one agent's context is cluttered by irrelevant tools and instructions.

### Gaps
- OpenAI has published no quantitative single- versus multi-agent comparison that I found. No official Codex subagent documentation was retrieved.

## 6. LangChain: single agent vs supervisor vs swarm (tau-bench with distractor domains)

### Takeaway
With gpt-4o on a τ-bench retail variant padded with irrelevant "distractor" domains, the single agent was slightly better with 1 domain but degraded sharply with 2 or more. Its token use grew with each domain, while supervisor and swarm stayed roughly flat. Swarm slightly beat supervisor. Fixing the supervisor's context handling (dropping handoff messages from sub-agent state, a message-forwarding tool) gave a "nearly 50% increase in performance". Exact scores appear only in charts.

### Cited Findings
**"Benchmarking Multi-Agent Architectures" (Will Fu-Hinthorn), 2025-06-10 [OLD 2025-06]**, https://www.langchain.com/blog/benchmarking-multi-agent-architectures
- [DATA] Setup: the first 100 examples of the τ-bench retail test split, plus 6 distractor environments (home improvement, tech support, pharmacy, automotive, restaurant, Spotify), each with 19 tools and a wiki, none needed for the task. All runs used `gpt-4o`.
- [DATA, read from charts as described in the text] The single agent is slightly better than the multi-agent systems with 1 domain and drops sharply with 2 or more distractor domains. Swarm slightly outperforms supervisor across the board.
- [DATA, as described] Single-agent tokens rise with each distractor domain; supervisor and swarm stay roughly flat; supervisor uses more tokens than swarm.
- [DATA] Supervisor changes (removing handoff messages from sub-agent state, a message-forwarding tool so the supervisor does not paraphrase, tool-naming variants) together gave a "nearly 50% increase in performance on this benchmark". No per-change breakdown is given.
- [OPINION] The supervisor is the most generic architecture but performs worse when implemented naively.

### Inferences
- In this setup the multi-agent benefit is context isolation, not parallelism: each sub-agent sees only its own domain's tools and wiki. The supervisor's main losses came from information-passing artifacts (paraphrase, routing clutter), which is Cognition's principle 1 measured.

### Gaps
- Exact numeric scores and token counts are not in the text. Single model (gpt-4o) and single n = 100 task set, with no 2026 rerun on stronger models found.

## 7. Google ADK multi-agent patterns and the A2A protocol in 2026

### Takeaway
ADK offers deterministic workflow agents (Sequential, Parallel, Loop), LLM-driven delegation, AgentTool, and in ADK 2.0 graph and collaborative workflows plus experimental agent routing. Parallel sub-agents write to distinct keys in shared session state. ADK publishes patterns, not benchmarks. A2A is an interoperability protocol (v1.0, Linux Foundation, moved to the Agentic AI Foundation in Aug 2026) with no evidence that it improves task outcomes.

### Cited Findings
**ADK docs (adk.dev, current; "Copyright Google 2026", ADK TypeScript 2.0 GA banner)**, https://adk.dev/workflows/ and https://adk.dev/workflows/patterns/
- [GUIDANCE] A single monolithic agent "can become challenging to develop, evaluate, and maintain". Workflow types: template workflows (Sequential, Loop, Parallel), collaborative workflows (ADK 2.0+: "a single agent to act in a dynamic coordinator role" with sub-agents), graph-based and dynamic workflows, and experimental agent routing.
- [GUIDANCE] Patterns:
  - Coordinator/dispatcher (LLM delegation or `AgentTool`; needs clear sub-agent `description`s).
  - Sequential pipeline: "Earlier agents write results (often via `output_key`), later agents read those results".
  - Parallel fan-out/gather: "Execute independent tasks simultaneously to reduce latency, then combine their outputs". Each parallel agent writes "to distinct keys in Shared Session State".
  - Hierarchical decomposition: "recursively breaking them down"; results return "via tool responses or state".
  - Generate-and-review.
  - Iterative refinement: a `LoopAgent` bounded by `max_iterations` or `escalate=True`.
  - Human-in-the-loop: "ADK doesn't have a built-in 'Human Agent' type".
- [GUIDANCE] The patterns page gives no race-condition warning beyond distinct keys, and no decision criteria: "evaluate and test them against your project requirements".

**A2A protocol status**
- [DATA] One year in (April 2026): 150+ supporting organizations, integration across Google, Microsoft and AWS platforms, and production use in supply chain, financial services, insurance and IT operations. https://linuxfoundation.org/press/a2a-protocol-surpasses-150-organizations-lands-in-major-cloud-platforms-and-sees-enterprise-production-use-in-first-year ; https://opensource.googleblog.com/2026/04/a-year-of-open-collaboration-celebrating-the-anniversary-of-a2a.html
- [DATA] Donated to the Linux Foundation in June 2025. The Java SDK 1.0.0.Final is its first GA release, aligned with the A2A 1.0.0 spec. https://quarkus.io/blog/a2a-java-sdk-1-0-0-final-released/
- [UNVERIFIED] IBM's ACP merged into A2A in Aug 2025. In Aug 2026 A2A became a hosted project of the Agentic AI Foundation, which also hosts MCP; sources disagree between Aug 17 and Aug 20. v1.0 adds signed Agent Cards, multi-tenancy, and JSON-RPC plus gRPC bindings. https://blog.pebblous.ai/blog/a2a-mcp-agentic-ai-foundation-authorization/en/ ; https://www.flowhunt.io/blog/agentic-ai-foundation-a2a-mcp-standards/ ; https://you.com/resources/a2a-protocol-explained-what-agent-to-agent-communication-solves

### Inferences
- ADK's "distinct keys per parallel branch, then a gather step" is the same single-writer discipline expressed at the state level. A2A matters only if INIT's members must talk to external agents; it says nothing about whether delegation helps.

### Gaps
- No Google benchmark of ADK patterns. Google's quantitative evidence is the separate scaling paper in section 9. The authoritative current A2A spec revision was not confirmed on a2a-protocol.org.

## 8. Recursive and self-similar agents: RLM, context folding, ReDel, Recursive Agent Harnesses (2026)

### Takeaway
Recursion, meaning an agent that calls copies of itself on slices of a huge input, works well for read-heavy, decomposable, long-context tasks. RLM with GPT-5 lifts OOLONG-Pairs from 0.1 (base) to 58.0 at depth 1 and 76.0 at depth 3, at a median cost comparable to the base model. Deeper recursion helps only the strong model: with Qwen3-Coder-480B, depth 2-3 often hurts because errors propagate through sub-calls. Context folding (branch and return) matches ReAct with a 10× smaller active context. The 2026 "Recursive Agent Harnesses" lifts Oolong-Synthetic from 71.75% (Codex, GPT-5) to 81.36% (same GPT-5). All this evidence is on reading and aggregation tasks. None tests recursive delegation of writes.

### Cited Findings
**Recursive Language Models (Zhang, Kraska, Khattab), arXiv:2512.24601 (v1 2025-12-31, v2 2026-01-28, v3 2026-05-11)**, https://arxiv.org/abs/2512.24601 and https://arxiv.org/html/2512.24601v3
- [DATA] RLMs handle inputs up to two orders of magnitude beyond the context window. Median gains with GPT-5: 26% over compaction, 130% over CodeAct with sub-calls and 13% over Claude Code, "while having comparable cost". RLM-Qwen3-8B (trained) beats Qwen3-8B by 28.3% on average.
- [DATA] Table 1, GPT-5 with sub-calls to GPT-5-mini; columns are CodeQA / BrowseComp+ (1K docs, 6M-11M tokens) / OOLONG (131K) / OOLONG-Pairs (32K):

  | Method | CodeQA | BrowseComp+ | OOLONG | OOLONG-Pairs |
  |---|---|---|---|---|
  | Base | 24.0* | 0.0* | 44.0 | 0.1 |
  | Compaction agent | 58.0 ($1.31) | 70.5 | 46.0 | 0.1 |
  | Claude Code (Opus 4.1) | 12.0* ($2.03) | 0.0* | 40.2 ($3.43) | 0.1 ($6.75) |
  | RLM depth 0 (REPL, no sub-calls) | 58.0 | 88.0 | 36.0 | 43.9 |
  | RLM depth 1 | 62.0 ($0.11) | 91.3 ($0.99) | 56.0 ($0.43) | 58.0 ($0.33) |
  | RLM depth 2 | 66.0 | 92.0 | 56.5 | 65.5 |
  | RLM depth 3 | 58.0 | 92.0 | 58.0 | 76.0 ($0.39) |

  `*` marks runs that hit context limits. Claude Code with context offloading scored 62.0 / 84.0 / 48.0 / 6.5.
- [DATA] Qwen3-Coder-480B-A35B shows the same structure with mixed results. CodeQA went 66.0 (depth 0) → 56.0 (depth 1) → 54.0 (depth 2) → 44.0 (depth 3). OOLONG went 43.5 → 48.0 → 26.0 → 32.0. Only BrowseComp+ improved with depth (46.0 → 68.7).
- [DATA, qualitative] Limitations:
  - All sub-calls are "blocking / sequential".
  - The median RLM run is cheaper than the base model, but the mean cost is higher because of outlier trajectories.
  - Exploding sub-call cost is a flagged risk.
  - Without a prompt warning, Qwen3-Coder launched thousands of sub-calls on basic tasks.
  - Syntax errors propagate through sub-calls.
  - Final-answer detection is brittle.
  - The first decomposition strongly determines the outcome.

**Context-Folding (Sun et al.), arXiv:2510.11967 (2025-10-13) [OLD 2025-10]**, https://arxiv.org/abs/2510.11967
- [DATA] Branch into a sub-trajectory, then fold it into a concise summary on return. The folding agent "matches or outperforms the ReAct baselines" on Deep Research and SWE tasks with an active context "10× smaller", and "significantly outperforms" summarization-based context management. It is trained end to end with FoldGRPO, an RL method with process rewards.
- [UNVERIFIED] Branches cannot spawn branches, so it is effectively depth 1, and the main thread is discouraged from token-heavy tools (from a search summary of the paper). The exact benchmark numbers (BrowseComp-Plus, SWE-Bench Verified) were not retrieved, because the HTML version returned 404.

**ReDel (Zhu, Dugan, Callison-Burch), EMNLP 2024 demo, arXiv:2408.02248 [OLD 2024-08]**, https://arxiv.org/html/2408.02248v2
- [DATA, qualitative] A toolkit for recursive multi-agent systems where "the models themselves flexibly decide when to delegate tasks and how to organize their delegation structure". It provides event logging and replay. The EMNLP abstract claims "significant performance gains on agentic benchmarks", but the specific numbers were not retrieved (gap).

**Recursive Agent Harnesses (Lumer et al.), arXiv:2606.13643 (2026-06-11)**, https://arxiv.org/abs/2606.13643
- [DATA] The recursive unit is a full harness (filesystem, code execution, planning). The parent writes a script that spawns subagent harnesses in parallel, or uses JSON tool calls for 1-5 entries, with a configurable depth limit.
- [DATA] On Oolong-Synthetic (199 samples, 13 context buckets up to 4M tokens): Codex coding-agent baseline (GPT-5) 71.75%; RAH (GPT-5) 81.36%; RAH (Claude Sonnet 4.5) 89.77%. The backbone was held fixed to attribute the gain to the harness. Cost, depth ablation and failure modes were not in the abstract (gap).

**Other 2026 recursive work (abstract level only)**
- [DATA, qualitative] "Recursive Multi-Agent Systems", arXiv:2604.25917 (revised Jul 2026), extends looped or recursive-model scaling to groups of agents. https://arxiv.org/abs/2604.25917
- [OPINION/problem statement] "Self-Evaluating Recursive Agents", arXiv:2610.04902 (Oct 2026): recursive agents delegate to child instances of the same policy, forming a tree. The final outcome is verifiable, but the self-invented intermediate subtasks "carry no ground truth", and scoring each node with a judge is "costly at scale and blind to decomposition quality". https://arxiv.org/abs/2610.04902 [UNVERIFIED beyond the search snippet]

### Inferences
- Recursion wins where one context cannot hold the input and subtasks are reads or aggregations with a clean merge (counting, pairing, retrieval). It is context-window relief, the same mechanism as Anthropic's research subagents.
- Depth helps only when the model at each level is reliable. With a weaker model, errors compound per level (Qwen3-Coder at depth 3 was worse than depth 0 on CodeQA). This is the recursive form of Google's error-amplification result in section 9.
- A cost tail and runaway fan-out (thousands of sub-calls) are a documented failure mode. Any recursive design needs hard per-tree budgets.
- Training signal for intermediate nodes is an open problem as of Oct 2026, which matters for a reward-free system.

### Gaps
- No 2026 result tests recursive delegation on write-heavy or long-horizon coding tasks. RAH's cost and depth data were not retrieved, and no context-folding benchmark numbers were obtained.

## 9. Scaling the number of agents: debate vs voting, best-of-N, compute vs architecture, and model strength

### Takeaway
Across controlled studies, most multi-agent accuracy gains come from extra parallel compute (samples, tokens) and context isolation, not from agents talking to each other. Majority voting explains most of multi-agent debate's gains, and debate alone is a martingale with no expected improvement. The largest controlled study (Google/MIT: 180 configurations, 3 model families) finds multi-agent helps parallelizable tasks: +80.8% to +80.9% with a centralized orchestrator on Finance-Agent. It hurts sequential tasks: every variant lost 39-70% on PlanCraft. Returns vanish or turn negative once the single agent already scores above about 45%. Independent agents amplify errors 17.2×, against 4.4× with a centralized verifier. On model strength: stronger models shrink the advantage of ensembles, debate and MAS over a single agent, but they are also the only models that benefit from deeper recursion or from a "smart friend".

### Cited Findings
**"Towards a Science of Scaling Agent Systems" (Kim, Liu et al., Google Research / MIT), arXiv:2512.08296 (v1 2025-12-09, v2 2025-12-17, v3 2026-04-08); blog 2026-01-28**, https://arxiv.org/abs/2512.08296v2 and https://research.google/blog/towards-a-science-of-scaling-agent-systems-when-and-why-agent-systems-work/
- [DATA] 180 controlled configurations: 5 architectures (single, Independent, Centralized, Decentralized, Hybrid) × 4 benchmarks (Finance-Agent, BrowseComp-Plus, PlanCraft, Workbench) × 3 model families (GPT, Gemini, Claude).
- [DATA] Parallelizable task: centralized coordination +80.8% (abstract) or +80.9% (blog, Finance-Agent). The two figures differ slightly by version.
- [DATA] Sequential reasoning: "every multi-agent variant degraded performance by 39-70%" (the blog names PlanCraft).
- [DATA] Web navigation: decentralized was best, at +9.2% against +0.2%.
- [DATA] Capability saturation: coordination gives "diminishing or negative returns once single-agent baselines exceed ~45%" (abstract).
- [DATA] Tool-coordination trade-off: "under fixed computational budgets, tool-heavy tasks suffer disproportionately from multi-agent overhead" (the example given is a coding agent with 16+ tools).
- [DATA] Error amplification: independent agents 17.2×, centralized 4.4×; the orchestrator acts as a "validation bottleneck". [UNVERIFIED] A trace-level table reportedly gives decentralized 7.8× and hybrid 5.1×.
- [DATA] A predictive model from task properties (tool count, decomposability, sequential dependency) reaches cross-validated R² = 0.513 (blog) or 0.524 (abstract v2/v3). It picks the best architecture for 87% of held-out configurations. Out-of-sample on GPT-5.2: MAE = 0.071, and 4 of 5 scaling principles generalize.

**"More Agents Is All You Need" (Li et al.), TMLR, arXiv:2402.05120 (v2 2024-10-11) [OLD 2024]**, https://arxiv.org/html/2402.05120v2
- [DATA] Sampling-and-voting with up to 40 agents, Table 2:
  - Llama2-13B ×40 vs Llama2-70B ×1: GSM8K 0.59 vs 0.54; MATH 0.09 vs 0.05; Chess 0.18 vs 0.12; MMLU 0.51 vs 0.55; HumanEval 0.18 vs 0.24. The ensemble wins only on the reasoning tasks.
  - GPT-3.5 ×40 vs GPT-4 ×1: GSM8K 0.85 vs 0.88; MATH 0.39 vs 0.40; Chess 0.55 vs 0.65; MMLU 0.70 vs 0.77; HumanEval 0.73 vs 0.88. The ensemble never beats the stronger single model.
- [DATA] Weaker models gain more: on MATH, relative gains were 200% (13B), 120% (70B) and 34% (GPT-3.5). Gains rise and then fall with inherent task difficulty.
- [DATA] Debate on HumanEval scored 0 with Llama2, attributed to noise from referencing other agents' code breaking coherence.

**MacNet, "Scaling LLM-based Multi-Agent Collaboration" (Qian et al.), ICLR 2025, arXiv:2406.07155 [OLD 2024-06/2025-03]**, https://arxiv.org/abs/2406.07155
- [DATA, qualitative] Collaboration among "over a thousand agents". Performance follows logistic growth in agent count (a "collaborative scaling law"), and irregular topologies beat regular ones. No emergence agent count was stated on the abstract page.

**"Debate or Vote" (Choi, Zhu, Li), NeurIPS 2025 Spotlight, arXiv:2508.17536 (v2 2025-10-23) [OLD 2025-10]**, https://arxiv.org/html/2508.17536v2 ; https://neurips.cc/virtual/2025/poster/116557
- [DATA] Across 7 NLP benchmarks, "majority voting accounts for most of the performance gains" in multi-agent debate and is usually on par with it.
- [DATA, theory] Debate "induces a martingale over agents' belief trajectories", so debate alone does not improve expected correctness. Interventions that bias belief updates toward correction help.

**"Stop Overvaluing Multi-Agent Debate" (Zhang et al.), arXiv:2502.08788 (v3 2025-06-21) [OLD 2025-06]**, https://arxiv.org/abs/2502.08788
- [DATA] 5 MAD methods × 9 benchmarks × 4 models: MAD methods "often fail to outperform simple single-agent baselines such as Chain-of-Thought and Self-Consistency", "even when consuming significantly more inference-time computation". Model heterogeneity is "a universal antidote" that improves MAD.

**"Single-agent or Multi-agent Systems? Why Not Both" (Gao et al.), arXiv:2505.18286 (2025-05-23) [OLD 2025-05]**, https://arxiv.org/abs/2505.18286
- [DATA] "the benefits of MAS over SAS diminish as LLM capabilities improve." Cascading requests between a single-agent and a multi-agent system improved accuracy by 1.1-12% and cut deployment cost by up to 20%.

**"Large Language Monkeys" (Brown et al.), arXiv:2407.21787 (v3 2024-12-30) [OLD 2024]**, https://arxiv.org/abs/2407.21787
- [DATA] Coverage scales log-linearly with samples. On SWE-bench Lite, DeepSeek-Coder-V2-Instruct goes from 15.9% (1 sample) to 56% (250 samples), against 43% for the single-sample state of the art.
- [DATA] Without automatic verifiers, "majority voting and reward models plateau beyond several hundred samples", so selection, not generation, is the bottleneck.

**Model-strength evidence from practitioners (2026)**
- [DATA, qualitative] Cognition: smart-friend escalation "works today when both models are strong" and failed with the weaker SWE-1.5 as primary. https://cognition.com/blog/multi-agents-working
- [DATA] RLM: deeper recursion helped GPT-5 (OOLONG-Pairs 58.0 → 76.0) and hurt Qwen3-Coder (CodeQA 56.0 → 44.0). https://arxiv.org/html/2512.24601v3
- [DATA, qualitative] Anthropic dropped harness components (context resets, sprints) as models improved from Sonnet 4.5 to Opus 4.5 and 4.6. https://www.anthropic.com/engineering/harness-design-long-running-apps

### Inferences
- **Compute or architecture?** Mostly compute plus context isolation. Anthropic attributes 80% of variance to tokens, majority voting explains most debate gains, and best-of-N coverage scales log-linearly. Architecture matters for two things: (a) containing errors, where a centralized verifier gives 4.4× instead of 17.2× amplification, and (b) not destroying value on sequential tasks (−39 to −70%).
- **Stronger models?** They shrink the multi-agent advantage on tasks a single agent can already do: the ~45% saturation, Gao et al., and More Agents' smaller gains for GPT-3.5. They grow the ability to use structured delegation that needs reliable children (deeper RLM recursion, smart-friend). The net effect for a frontier-model harness: fewer cases where MAS is needed for accuracy; it stays useful for breadth, context overflow, verification and cost routing.
- Best-of-N without a verifier plateaus. A parallel-candidates design is only as good as its selector.

### Gaps
- The Google paper's equal-budget conditions were not confirmed from the full text: the blog does not say whether budgets were equalized, though the abstract mentions "fixed computational budgets". Its sequential-task penalty is measured on PlanCraft, a crafting planner, not on code editing.
- No 2026 replication of "Debate or Vote" on frontier reasoning models was found.

## 10. Synthesis: a decision rule for when sub-agents help or hurt, backed by the numbers

### Takeaway
Sub-agents help when the work is read-heavy, breadth-first, decomposable into independent pieces with a mechanical merge, verifiable by an oracle, or too large for one context window. They hurt when sub-agents write to shared state, edits are tightly coupled, steps are sequential, the single agent is already competent (above about 45%), or the subtask carries implicit judgment. The 2026 parallel-coding studies quantify the write hazard: two cooperating agents succeed about 30% less often than one agent doing both tasks (CooperBench). Unconstrained parallel pairs pass 23.3% of the time against 50.0% with pre-write admission, which recovers mainly by serializing 96.7% of executions (Claim Plane). A real-world counterweight: on 417 mined Django PR pairs, semantic interference appeared in 1 of 834 runs.

### Cited Findings
**2026 empirical studies of parallel writers**
- [DATA] CooperBench (Khatua et al., arXiv:2601.13295, 2026-01-19): 600+ collaborative coding tasks across 12 libraries in 4 languages, with two agents assigned independent but possibly conflicting features. "Agents achieve on average 30% lower success rates when working together than when performing both tasks individually" (the "curse of coordination"). Failures: channels "jammed with vague, ill-timed, and inaccurate messages"; agents deviating from their commitments; wrong expectations of others' plans. https://arxiv.org/abs/2601.13295
- [DATA] Claim Plane (Nikolaev, arXiv:2608.00947, 2026-08-02): 30 CooperBench pairs (15 conflict, 15 clean) × 3 seeds × 4 arms = 360 runs, with DeepSeek V4 Flash coding.
  - Unconstrained parallel: pair pass 23.3%, integration success 65.6%.
  - Static pre-write admission: 50.0% and 96.7% (+26.7 pp, 95% CI 9.6-60.0). But it serialized 96.7% of executions, including 93.3% of clean cases.
  - On conflict pairs, pair pass went from 6.7% to 60.0%.
  - Dynamic admission: 22.2%, with 46 of 90 runs failing closed.
  - "does not establish useful wall-clock speedup". https://arxiv.org/abs/2608.00947
- [DATA] "Passes Alone, Fails Together" (Xia, Wu, Park, arXiv:2609.25396, 2026-09-21, EXPRESS 2026 workshop): mined Django PR pairs showed interference in 1 of 834 runs; constructed tasks on 12 Django helpers showed it in 97% of runs. A message describing the completed concurrent change recovered 82% of runs. The authors warn that constructed rates do not estimate real-world frequency. https://arxiv.org/abs/2609.25396
- [DATA] AgenticFlict (arXiv:2604.03551, 2026-04): 142K+ agent-authored PRs from 59K+ repos, 107K+ merge-simulated; 29K+ had textual conflicts, a 27.67% rate that varies by agent. This measures conflicts against the base branch, not necessarily between parallel agents. https://arxiv.org/abs/2604.03551
- [DATA] "AI Agent Pull Requests on GitHub" (arXiv:2607.04697 v2, 2026-07): 84.4% of conflicted files were source code, and about 42% of conflicts were structural (modify/delete, add/add). https://arxiv.org/html/2607.04697v2
- [DATA, qualitative] The C compiler kernel stall: 16 agents fixed the same bug and overwrote each other until a GCC oracle split the work (section 1). Claude Code docs: "Two teammates editing the same file leads to overwrites."

**Decision rule.** Each row cites the evidence behind it.

| Condition | Use sub-agents? | Evidence |
|---|---|---|
| Breadth-first reading or research with independent directions | Yes. Use 3-5 parallel workers and scale effort to query complexity (1 agent and 3-10 calls for a simple query). | Anthropic +90.2%, up to −90% wall time; Google +80.8% on parallelizable work |
| Input exceeds one context window and the merge is an aggregation | Yes, by recursion or fold, depth 1 by default. Go deeper only with a strong model and a sub-call budget. | RLM OOLONG-Pairs 0.1 → 58.0 (depth 1); context folding with 10× smaller context; RAH 71.75 → 81.36 |
| One agent's context is cluttered by many unrelated tools or domains | Yes, for isolation (supervisor or swarm), but forward outputs verbatim and strip routing chatter. | LangChain: single agent drops sharply at 2 or more distractor domains; supervisor fixes ≈ +50%; OpenAI tool-overload rule |
| Verification or review of produced work | Yes. Use a separate verifier with a clean context, tuned to be skeptical. | Cognition generator-verifier; Anthropic self-evaluation bias; MAST FC3 ≈ 23.5% verification failures; ChatDev +15.6% with a verifier; Magentic-One ablation −31% without ledgers |
| Many independent failing tests or items against an oracle | Yes, with parallel writers, but only with task claim locks, disjoint ownership and a CI regression gate. | C compiler at 99% torture-suite pass; Claude Code "each teammate owns a different set of files" |
| Sequential, dependent steps (planning, one giant task) | No. | Google −39 to −70% on PlanCraft; C compiler kernel stall; Anthropic "many dependencies" poor fit |
| Shared-state or same-file writes, tightly coupled edits | No. Keep a single writer, or pre-write admission, which effectively serializes. | CooperBench −30%; Claim Plane 23.3% vs 50.0% (serialized 96.7%); Cognition "writes stay single-threaded" |
| Single-agent baseline already ≥ about 45% on the task | No for accuracy, though cost routing may still pay. | Google capability saturation; Gao et al.; More Agents gains smaller for stronger models |
| The subtask carries the key judgment | No. Keep it in the lead. | Cognition Fusion: "When the judgment is the deliverable, delegating it backfires." |
| Debate between peers to reach an answer | No. Use independent sampling plus a vote or verifier instead. | Debate or Vote (martingale); Stop Overvaluing MAD |
| Budget | Plan for about 4× chat tokens (one agent), about 15× chat (multi-agent research) and over 20× a solo run (full long-running harness). Token use scales linearly with teammates. | Anthropic 2025 and 2026; Claude Code docs |

### Inferences
- Under all the rules sits one mechanism: sub-agents add value by spending more compute in parallel, isolated contexts and returning compressed results. They lose value whenever the pieces share implicit decisions or mutable state, because coordination (messages, negotiation) is unreliable. CooperBench's message jamming and MAST FC2 at about 32% show this. Mechanical exclusion fixes it mostly by serializing.
- The real-world frequency of semantic interference may be low (1/834 on mined pairs). The cost of one interference in a self-modifying system (a corrupted harness) is far higher than in a PR queue, so the low base rate does not justify parallel writers there.

### Gaps
- No controlled study compares single-writer plus read-only scouts against parallel writers at an equal token budget on 2026 frontier models. That is the exact comparison INIT needs.

## 11. Implications for INIT

### Takeaway
The evidence supports INIT's current direction in ADR-011 §4.5: up to 4 parallel members that read the home and never write the workspace, writing members only by owner grant, and no members spawning members. INIT should adopt a sub-agent concept of **read-only scouts and clean-context verifiers at depth 1, returning condensed artifacts, with one writer (the live body) per workspace and per plastic surface.** Parallel members may generate candidate self-changes, but admission must stay serial and Root-mechanical. Fan-out should be bounded by budget, with no auto-fan-out on sequential tasks.

### Cited Findings
These are the constraints the evidence supports, each tied to the sources above.
- **Forbid parallel writers to the same workspace or plastic surface. Keep "members never write the workspace" and "grant only".** Sources: Cognition 2026 ("writes stay single-threaded", https://cognition.com/blog/multi-agents-working); CooperBench −30% (https://arxiv.org/abs/2601.13295); Claim Plane, where reliability came from serializing 96.7% of runs (https://arxiv.org/abs/2608.00947); the C compiler kernel overwrites (https://www.anthropic.com/engineering/building-c-compiler); Claude Code same-file overwrites (https://code.claude.com/docs/en/agent-teams). If an owner grant ever allows writing members, require disjoint file ownership, a claim lock checked by the Root or kernel before the write, and a regression oracle. Those are the C compiler's three ingredients.
- **One writer also preserves INIT's reversibility and attribution model.** ADR-011's change groups, per-group snapshots and harm-signal rollback assume a serial history. Interleaved writes from several bodies would make "roll back the newest group" ambiguous. Step-level LLM attribution is only 14.2% accurate (Who&When, https://arxiv.org/abs/2505.00212), which supports INIT's mechanical attribution (`x-init-caps`, change groups) over LLM-judged blame. Every model-created sub-agent pattern should carry a `cap` id so the H-signals can attribute cost and harm to it.
- **Require independent verification and make it clean-context.** A verifier member should get the artifact and the spec, not the generator's trajectory, and should be tuned to be skeptical. Sources: Cognition's clean-context result; Anthropic's self-evaluation bias (https://www.anthropic.com/engineering/harness-design-long-running-apps); MAST FC3 at about 23.5% and the ChatDev verifier at +15.6% (https://arxiv.org/html/2503.13657v3); Magentic-One's top error codes. This does not reintroduce a reward gate. It is optional body-level tooling that the model can build on the plastic surface, while the Root keeps gating only on generic harm.
- **Parallel candidate generation is best-of-N, and best-of-N needs a selector.** Without verifiers, majority voting and reward models "plateau beyond several hundred samples" (https://arxiv.org/abs/2407.21787). In a reward-free design INIT has no selector at creation, so spawning many parallel self-change candidates buys little. Limit parallel self-extension candidates and let the usage ledger and frozen twin do selection over time.
- **Keep depth 1 by default ("members cannot create members").** Allow deeper recursion only as an owner grant for read-only long-context work, with a per-tree sub-call and spend cap. RLM depth helped GPT-5 but hurt Qwen3-Coder, and without a guard Qwen launched "thousands of sub-calls"; mean cost exceeds median because of outlier trajectories (https://arxiv.org/html/2512.24601v3). Claude Code also forbids nested teams.
- **Budget for the multiplier.** Multi-agent research runs about 15× chat and about 3.75× a single agent (derived); a full long-running harness runs over 20× a solo run; team token cost scales linearly with members. The Root's `ceiling.background` share (30-60%) and the H5b cost signal should count member spend per spawning cap, so a sub-agent pattern that multiplies cost without use shows up as cost harm. Anthropic's effort-scaling defaults (1 agent for simple queries, 3-5 parallel for breadth) are a sensible built-in prompt default for the body, not a Root rule.
- **No automatic fan-out on sequential tasks, and do not hard-code a multi-agent architecture in the kernel.** Sequential tasks lose 39-70% under every multi-agent variant, and benefits vanish above about a 45% single-agent baseline (https://arxiv.org/abs/2512.08296v2). Harness components become "dead weight" as models improve (https://www.anthropic.com/engineering/managed-agents). Keeping sub-agent specs on the plastic surface (`agents/<name>/AGENT.md`, ADR-011) lets the model add and remove patterns as its abilities change, which suits the owner's goal of maximal room for AGI-level models.
- **Prefer a centralized lead over peer swarms.** Independent agents amplify errors 17.2× against 4.4× under a centralized validator (https://arxiv.org/abs/2512.08296v2). Cognition calls negotiating swarms "mostly a distraction", and debate adds no expected correctness (https://arxiv.org/html/2508.17536v2). INIT's live body as the single lead, with members reporting back, matches the best-measured topology.
- **Treat member outputs and messages as data, never authority.** Claude Code teammates "can't approve a permission prompt or supply consent" (https://code.claude.com/docs/en/agent-teams). With mitigations disabled, Magentic-UI was prompt-injected into exfiltrating SSH keys (https://arxiv.org/html/2507.22358v1). Because INIT's members read the home, their returned text must not be able to request grants, approve changes or address the Root. ADR-011's H11 egress-volume signal is the right detective control for members that read widely.
- **Hand results back by reference.** Members should return 1,000-2,000-token summaries, or write artifacts to a member-owned scratch area and return a path. This avoids the coordinator "game of telephone" and keeps the lead's context clean (https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents ; https://www.anthropic.com/engineering/multi-agent-research-system).

### Inferences
- The best-supported sub-agent concept for INIT is the Cognition and Anthropic convergence point: **a single-writer lead body plus read-only scout and verifier members at depth 1**. Writing members are grant-gated, with Root-checked claim locks and an oracle. Recursive read-only fan-out over very large inputs is a later, budget-capped grant. This keeps every write on the serial, snapshot-and-rollback path that ADR-011's reward-free safety depends on.
- What remains open for INIT: whether model-built sub-agent patterns help can only be judged longitudinally (usage ledger, frozen twin), as ADR-011 already concedes. The literature offers no up-front benchmark for this case.

### Gaps
- No published study evaluates sub-agents inside a self-modifying or reward-free harness, or measures how parallel members interact with snapshot and rollback semantics. This evidence is transferred from coding and research agents, not measured on INIT-like systems.
- No 2026 controlled comparison at equal token budget of "single writer plus read-only scouts" against "parallel writers with locks" on frontier models was found. A small INIT experiment could close this gap: same tasks, same budget, two topologies, measuring integration success and cost.
