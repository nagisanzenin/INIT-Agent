# Where implementation performance of an LLM agent harness actually matters (state as of 2026-10-19)

Scope: the latency, cost and resource budget of agent loops. The goal is to find which harness components are performance-critical, and where "extreme performance" in the implementation language pays off versus where model and tool latency dominate (Amdahl's law). Every number carries its source date. "Derived" marks my own arithmetic on cited inputs.

---

## 1. Latency breakdown of a typical agent turn and task (model vs tools vs harness overhead)

### Takeaway
Measured traces from 2026 agree that the harness's own CPU work (context building, serialization, orchestration) is a small slice: at most about 9% of end-to-end time in the one study that isolates it. Model generation and tool execution split most of the time, and their ratio depends on the workload. Tool time is heavy-tailed: about 4% of calls make up about 85% of tool time. On real Claude Code and Codex sessions, about 92% of wall-clock time is the human thinking. The harness levers that pay are harness-*design* items on the tool path (process spawn, PTY/shell, sandbox wrapper, search). Raw language speed is not one of them.

### Cited Findings
**Real-user traces of Claude Code and Codex (TraceLab, arXiv 2606.30560v2, 30 Jun 2026; traces Sep 2025–Jun 2026)**
- Dataset: 4,265 sessions (2,676 Claude Code, 1,589 Codex), 357,161 LLM steps, 432,510 tool calls, 43 users. — [TraceLab](https://arxiv.org/html/2606.30560v2)
- Wall-clock per session: human thinking 92.3%, LLM generation 3.3%, tool execution 4.8%. With each idle gap capped at 1 h: human 64.3%, generation 14.5%, tools 21.2%. — [TraceLab](https://arxiv.org/html/2606.30560v2)
- Per request (average 4.3 min, median 38 s, p90 6.4 min): tools take 59.8% of response time and generation 41.0%. Each request makes about 8.8 LLM calls and 10.8 tool calls. — [TraceLab](https://arxiv.org/html/2606.30560v2)
- Average tool-call latency is 16.8 s. Calls over 1 min are 4.9% of Claude Code calls but 92% of its tool time, and 3.1% of Codex calls but 61% of its tool time. Pooled, about 4% of calls make up about 85% of tool time. — [TraceLab](https://arxiv.org/html/2606.30560v2)
- **Harness-side overhead on the tool path:** in Codex, "residual overhead" (end-to-end minus the tool's internal execution) is 18.6% of timed tool time, with a p99 of 10 s. — [TraceLab](https://arxiv.org/html/2606.30560v2)
- About 1.2 tool calls per step (p90: 2), so little parallelism is used in practice. — [TraceLab](https://arxiv.org/html/2606.30560v2)
- Normalized median decode speed: 46.8 tok/s (Claude) and 33.9 tok/s (Codex). Estimated Codex TTFT is about 3.1 s per step. Median output per step is 252 tokens (Claude) and 184 (Codex). — [TraceLab](https://arxiv.org/html/2606.30560v2)

**Controlled harness experiments (Aries, arXiv 2607.29069, 31 Jul 2026; OpenHands, Hermes Agent and OpenClaw on Terminal-Bench 2, SWE-Bench Pro and DeepResearch Bench)**
- Tool execution is 13% to 48% of end-to-end latency, with the top figure for Hermes Agent on Terminal-Bench 2. "Beyond tool execution, the harness spends up to 9% of end-to-end time on orchestration and other host-side processing." — [Aries](https://arxiv.org/abs/2607.29069)
- Tool-call latency has "a severe long tail that spans multiple orders of magnitude." TTFT and TPOT metrics miss these host-side intervals. — [Aries](https://arxiv.org/abs/2607.29069)
- Host-side gaps also lengthen how long trajectory state stays resident in GPU memory between LLM calls. — [Aries](https://arxiv.org/abs/2607.29069)

**Self-hosted open models (Agentic AI Workload Characteristics, arXiv 2605.26297, 25 May 2026; Gemma/Qwen on SWE-bench Pro, Terminal-Bench, GAIA, DABStep, ADE)**
- LLM inference is 71–98% of total runtime and tools 2–29%. SWE-bench Pro reaches 17.7% tool time and Terminal-Bench 17.6%. Framework (harness) overhead was not reported separately. — [Yuan et al.](https://arxiv.org/html/2605.26297v1)
- Within LLM time, decode is 91.0–98.6% and prefill 1.4–9.0%, assuming ideal prefix caching. — [Yuan et al.](https://arxiv.org/html/2605.26297v1)
- Turns per task average 12–62, with outliers up to 786. Mean context on SWE-bench Pro is 68.7K–80.1K tokens. Mean input:output ratio is 53.9×–559.8×. — [Yuan et al.](https://arxiv.org/html/2605.26297v1)

**Ten-application characterization (AgentSysBench, arXiv 2608.15127, 15 Aug 2026; includes Claude Code, Codex, Mini-SWE, OpenClaw)**
- Non-LLM components dominate latency in 5 of 10 applications. Component task latencies diverge by up to 32×. — [AgentSysBench](https://arxiv.org/abs/2608.15127)
- In Claude Code on MCP-Atlas request types, the LLM reaches up to 90% of total time for Movie, BI and DB tasks, while tools reach up to 84% for ETL, Wiki and MAP tasks. — [AgentSysBench](https://arxiv.org/abs/2608.15127)
- The paper names a "control-plane tax": tokens spent on tool schemas, raw observations and auxiliary LLM calls. It grows over multi-step sessions and adds to latency and cost. — [AgentSysBench](https://arxiv.org/abs/2608.15127)

**RL rollout timing (LEGO-RL, arXiv 2608.17393, 18 Aug 2026)**
- Over 3,699 OpenHands SDK training trials, agent execution (LLM, tools and harness combined) is 91.3% of mean wall-clock (840.5 s of 920.4 s). Sandbox setup is 2.3% (mean 21.6 s, p99 275 s), verification 3.9% (p99 928 s), and agent setup 0.5% (4.2 s). — [LEGO-RL](https://arxiv.org/abs/2608.17393)

**Timeouts as a proxy for task length**
- Terminal-Bench 2.0 has 89 tasks, each with its own agent timeout. Hard-task limits are 900 s or 1,800 s, and 8 hard tasks allow 3,600 or 7,200 s. — [Continual-learning eval on TB2](https://arxiv.org/pdf/2607.14004)
- Official TB2 submissions use 5 trials per task and may not override timeouts or CPU, memory and storage limits. — [Harness design survey, 2026](https://arxiv.org/pdf/2606.20683)
- Harbor-Index sets timeouts to about 1.2× the fastest frontier model's observed runtime, or up to 3 h when every model fails. One Claude Code trajectory ran 270 steps over 36 h because of a broken adapter. — [Harbor-Index, arXiv 2609.04298v3, Sep 2026](https://arxiv.org/abs/2609.04298)

**Harness design changes that moved per-action latency**
- In ProRL Agent, replacing a tmux-based bash tool with a direct pseudo-terminal (ptyprocess) lowered average action time from about 0.78 s to 0.42 s. The row-to-ablation mapping is inferred from the paper's Table 3. Load balancing raised GPU utilization from 42% to 78%. — [ProRL Agent, arXiv 2603.18815, Mar 2026](https://arxiv.org/html/2603.18815v1)

### Inferences
- **Amdahl upper bound for "make the harness faster" (Derived):** if the harness's host-side processing is at most about 9% of end-to-end time (Aries, Jul 2026), removing it entirely gives at most about 1.10× task speedup at today's model speeds. A language rewrite would recover only part of that 9%.
- The larger harness-attributable cost sits on the **tool path**, not in "context building." Codex's residual overhead of 18.6% of tool time with a p99 of 10 s (TraceLab, Jun 2026) and the tmux-to-PTY change from 0.78 s to 0.42 s (ProRL, Mar 2026) are design and architecture effects: process spawning, shell wrapper, sandbox and approval layers. Codex is already written in Rust, so language alone did not remove this overhead.
- Typical non-tail tool calls (Derived from TraceLab): if 4% of calls take 85% of the 16.8 s average, the other 96% of calls average about 2.6 s. That is still roughly 100–1,000× the millisecond-scale CPU cost of serializing a 120K-token context. The serialization estimate is mine; no source measured it.
- No study measured per-turn harness CPU in milliseconds (assembly, token counting, JSON). See Gaps.

### Gaps
- No published millisecond-level profile of the harness's own per-turn CPU work (prompt assembly, JSON encode/decode, token counting, diffing, rendering) exists for Claude Code, Codex, OpenHands or SWE-agent. The 2026 searches confirmed this. Aries gives only an aggregate "up to 9%."
- Aries Figure 3 per-harness percentages could not be reliably read from the PDF text extraction. Only the 13–48% tool range and the ≤9% harness figure are quoted.
- No public data on compaction latency (wall-clock) per event. TraceLab reports only counts: 9.7% of sessions have at least one compaction, 4.5% for Claude Code and 18.4% for Codex.

---

## 2. Throughput and time-per-task trends: faster inference, parallel/speculative tool calls, and when harness overhead becomes significant (>1,000 tok/s)

### Takeaway
Output speeds above 1,000 tok/s are shipping in 2026: Codex-Spark on Cerebras and fast modes that are 2.5× faster. When OpenAI reached them, the first new bottleneck was **transport and API-side overhead**, not harness CPU. OpenAI fixed it with persistent WebSockets and incremental input, cutting per-roundtrip overhead by 80% and making long tool chains up to about 40% faster. As generation shrinks toward sub-second steps, tool execution and fixed per-action costs dominate. Per-action costs in the hundreds of milliseconds (process spawn, shell, network round-trips) become significant. Per-turn CPU costs in single-digit milliseconds still do not.

### Cited Findings
**Fast inference offerings (dated)**
- GPT-5.3-Codex-Spark (Feb 12, 2026) on Cerebras WSE-3 delivers ">1000 tokens per second." It has a 128K context window and is text-only. — [OpenAI](https://openai.com/index/introducing-gpt-5-3-codex-spark/)
- For Spark, OpenAI made "end-to-end latency improvements in our harness that will benefit all models": streamlined response streaming, reworked session initialization, and a persistent WebSocket with Responses API changes. OpenAI reports these cut overhead per client/server roundtrip by 80%, per-token overhead by 30%, and time-to-first-token by 50%. These are vendor-reported figures. — [OpenAI](https://openai.com/index/introducing-gpt-5-3-codex-spark/)
- Feb 20, 2026 update: Spark became "about 30% faster" after the WebSocket and Responses optimizations, "serving at over 1200 tokens per second." — [OpenAI community thread](https://community.openai.com/t/gpt-5-3-codex-spark-research-preview-with-1000-tokens-per-second/1374091)
- Claude fast mode (Feb 7, 2026, Opus 4.6) is up to about 2.5× faster output. Current docs (Oct 2026) price it at $10/$50 per MTok on Opus 4.8 and $30/$150 on Opus 4.6 and 4.7, versus a $5/$25 standard Opus price. That is a 2× to 6× premium. — [Claude Code fast mode docs](https://code.claude.com/docs/en/fast-mode); [Simon Willison, Feb 7 2026](https://simonwillison.net/2026/Feb/7/claude-fast-mode/); [Vercel changelog (Opus 4.7)](https://vercel.com/changelog/fast-mode-for-opus-4-7-available-on-ai-gateway)
- Cerebras advertised Qwen3-Coder at up to 2,000 tok/s in Aug 2025. InfoWorld (Sep 2025) disputed the sustained figure. Cerebras reported GLM-4.7 at about 1,000 TPS and Kimi K2.6 trials approaching 1,000 tok/s (May 2026). — [Cerebras Code](https://www.cerebras.ai/blog/introducing-cerebras-code); [InfoWorld](https://www.infoworld.com/article/4055909/); [Cerebras Kimi K2](https://cerebras.ai/blog/cerebras-kimi-k2-Enterprise)
- Baseline for comparison: median normalized decode on real Claude Code and Codex sessions was 46.8 and 33.9 tok/s, with Codex TTFT about 3.1 s per step (traces to Jun 2026). — [TraceLab](https://arxiv.org/html/2606.30560v2)

**Transport is the first harness-adjacent bottleneck at high tok/s**
- Responses API WebSocket mode (Apr 2026) keeps a persistent connection to `/v1/responses` and sends only new input items plus `previous_response_id`. OpenAI says it is "up to roughly 40% faster end-to-end" for rollouts with 20+ tool calls. — [OpenAI WebSocket mode docs](https://developers.openai.com/api/docs/guides/websocket-mode); [OpenAI engineering post](https://openai.com/index/speeding-up-agentic-workflows-with-websockets)
- Partners reported Cline 39% faster multi-file workflows and Cursor up to 30% faster (May 2026). — [InfoQ, May 2026](https://infoq.com/news/2026/05/openai-websocket-responses-api)
- A third-party summary of OpenAI's post says model inference reached nearly 1,000 tok/s while users experienced about 65 tok/s before the transport fixes. It reports gains of about 5–10% for 2–5 turns and about 20–30% for 10–20 tool calls, a 60-minute maximum connection, one in-flight response per connection, and Codex CLI using WebSocket by default from about v0.128. These figures are secondary; verify them against OpenAI. — [Daniel Vaughan, May 3 2026](https://codex.danielvaughan.com/2026/05/03/codex-cli-websocket-mode-responses-api-persistent-connections-agent-loop-performance/)
- OpenAI's engineering post frames it this way: "as model inference gets faster, the services and systems that surround inference also need to speed up." — [OpenAI engineering post (via search excerpt)](https://openai.com/index/speeding-up-agentic-workflows-with-websockets)

**Fewer model round-trips (structural, not language)**
- Anthropic Programmatic Tool Calling (Nov 24, 2025): orchestrating 20+ tool calls in one code block "eliminates 19+ inference passes." Average tokens fell from 43,588 to 27,297 (−37%). No measured latency figure was given. — [Anthropic engineering](https://www.anthropic.com/engineering/advanced-tool-use)
- Tool Search Tool (same post): 50+ MCP tools cost about 72K tokens upfront, versus about 8.7K total with search, an 85% reduction. — [Anthropic engineering](https://www.anthropic.com/engineering/advanced-tool-use)

**Speculative and parallel tool execution (research, 2025–2026)**
- PASTE ("Act While Thinking," arXiv 2603.18897, 2026) speculatively runs predicted tool calls while the LLM is still generating. It reports an average 48.5% task-completion-time reduction and 1.8× tool throughput, though an AlphaXiv summary says 43.5%. — [arXiv 2603.18897](https://arxiv.org/html/2603.18897)
- Client-side speculative tool execution without engine changes (arXiv 2512.15834) works best for stateless, cheap tools. Speculative Actions (ICLR 2026) presents a lossless "as-if-sequential" framework. — [arXiv 2512.15834](https://www.arxiv.org/pdf/2512.15834); [Speculative Actions](https://www.cs.columbia.edu/~kkaffes/papers/speculative-actions-iclr26.pdf)
- One vendor blog profiles a tool-call decision on a fast backend at about 300 ms TTFT plus about 700 ms to emit the JSON, and 3–8 tool calls before a code edit. This is a vendor estimate, not a measurement. — [General Compute blog](https://www.generalcompute.com/blog/building-a-code-agent-why-each-step-needs-sub-second-inference)

### Inferences
- **Amdahl at 1,000+ tok/s (Derived from TraceLab medians plus Spark speeds):**
  - Today a step costs about 3 s TTFT plus about 214 tokens at about 40 tok/s (about 5.3 s), so about 8–9 s of LLM time.
  - At 1,000 tok/s with about 0.3–0.5 s TTFT, the LLM part falls to about 0.5–0.7 s.
  - A typical non-tail tool call (about 2.6 s, derived in Q1) then dominates the step.
  - A fixed per-step harness cost of 50 ms becomes about 1.5% of a roughly 3.1 s step. A 300–500 ms cost becomes about 10–15%. Examples of costs that size: an extra process spawn, a tmux round-trip, an HTTP reconnect, or a full re-upload of a 100K+ token context.
  - So once models run above 1,000 tok/s, the harness must eliminate costs in the **hundreds of milliseconds per action**. Microseconds of interpreter speed do not matter.
- The 2026 evidence shows the payoff came from **protocol and architecture**: persistent connections, incremental context deltas, and server-side state. OpenAI's Codex harness is already Rust and still needed these fixes.
- Removing whole inference passes is the biggest single lever as models get faster. Programmatic tool calling, parallel tool calls and speculation each save one TTFT plus decode per avoided round-trip. In practice parallelism is underused (1.2 tool calls per step, TraceLab), so INIT Agent should make the scheduler parallel- and speculation-ready.
- Streaming render cost scales with tokens per second. At 1,000+ tok/s a TUI that re-parses or re-renders the full message on each token becomes a real CPU sink (see Q6).

### Gaps
- No public, independent per-component profile of a harness running against a 1,000+ tok/s model (e.g., Claude Code or OpenCode on Cerebras) was found.
- The official OpenAI WebSocket engineering post returned HTTP 403. Its detailed breakdown of server-side per-request work is known only via secondary summaries.
- Current Groq and Gemini Flash tok/s numbers for coding models were not found in this pass.

---

## 3. Scale scenarios: hundreds to thousands of parallel agents (RL rollouts, eval harnesses, background/cloud agents) — memory/CPU per agent and runtimes

### Takeaway
At scale, the bottlenecks are sandboxes, environment setup, tail latency and KV-cache memory, not harness CPU. Harbor, the de facto eval orchestrator, is a Python library that ran about 0.3M trajectories. Black-box production harnesses such as Claude Code, Codex and Pi now run *inside* RL rollouts by the thousands. That makes per-instance harness **memory footprint, startup time and stability** matter, because they multiply by N. A small, fast-starting, leak-free harness binary is a real advantage here, and it is the one scenario where implementation-language efficiency clearly pays.

### Cited Findings
**Eval harness scale**
- Harbor is "a Python library originally developed for Terminal-Bench." Harbor Adapters support 22 agents and 80+ benchmarks (May 2026). Harbor-Index ran 16 model–harness configurations (Codex, Claude Code, Gemini CLI and others) across 54 benchmarks and 6,627 tasks with 3 trials each: about 0.3M trajectories, 226B tokens, and more than $300K of compute. — [Harbor-Index, arXiv 2609.04298v3, Sep 10 2026](https://arxiv.org/abs/2609.04298)
- Harbor backends are docker, daytona, modal, e2b, runloop and gke. `--n-concurrent` sets parallel trials, with examples of 100 concurrent trials on Daytona. — [Harbor quickstart](https://mintlify.com/harbor-framework/harbor/quickstart)
- One practitioner's TB2 runs used 48 concurrent cloud sandboxes against a Daytona tier-3 pool of 250 vCPU / 500 GB RAM. Most tasks needed 1 vCPU / 2 GB and a few needed up to 4 vCPU / 8 GB. This is a community figure. — [coder/mux tbench skill](https://lobehub.com/skills/coder-mux-tbench)

**RL rollouts: harness inside the loop**
- Polar (arXiv 2605.24220, May 22 2026) runs Codex, Claude Code, Qwen Code and Pi as black-box harnesses in RL. Its offline data generation runs the pi-coding-agent v0.67.68 harness in a per-task Apptainer SIF image with Node.js 22 layered on top, `max_concurrent`=5–8 per job, and a 3,600 s per-task timeout. — [Polar](https://arxiv.org/abs/2605.24220)
- LEGO-RL (Aug 2026) trains through the OpenHands SDK, Claude Code and OpenCode harnesses. The trainer waits for rollouts 40.8% of training time with OpenHands SDK and 66.1% with Claude Code, so "rollout generation rather than optimization is the primary throughput bottleneck." — [LEGO-RL](https://arxiv.org/abs/2608.17393)
- Kimi K3's RL environment can instantiate Kimi Code, Claude Code, Codex, OpenClaw and Hermes. Liquid AI proxies harnesses as black boxes. Cursor runs "hundreds of thousands of concurrent sandboxed coding environments" for Composer RL. DeepSeek runs "hundreds of thousands of sandboxes per cluster" (survey, Sep 11 2026). — [HF blog "One sandbox per rollout"](https://huggingface.co/blog/sergiopaniego/rl-environments-2026)
- ProRL Agent (Mar 2026) decouples rollout as an HTTP service from the trainer. It uses Singularity rather than Docker for rootless HPC and runs hundreds of concurrent instances. Throughput is 0.37 instances/s on its full system versus 0.25–0.30 with a component removed. — [ProRL Agent](https://arxiv.org/html/2603.18815v1)
- RollArt (Dec 2025): environment initialization is about 15% of SWE-bench rollout time normally. When environment failures occur, env.reset reaches 78%. — [RollArt](https://arxiv.org/html/2512.22560v1)
- SkyRL-Agent (Nov 2025): an asynchronous pipeline dispatcher gives a 1.55× speedup over naive async batching. Rollouts mix CPU-bound runtime init, GPU inference and CPU-bound reward. — [SkyRL-Agent](https://www.alphaxiv.org/overview/2511.16108v1)
- SWE-MiniSandbox (2026): hundreds of containers starting at once on one machine create a "thundering herd" on disk and memory. — [SWE-MiniSandbox](https://www.alphaxiv.org/abs/2602.11210)

**Sandbox fleet numbers (vendor-reported, 2026)**
- Tencent runs millions of gVisor sandboxes daily for agentic RL (Apr 23 2026). — [gVisor blog](https://gvisor.dev/blog/2026/04/23/scaling-agentic-rl-sandboxes-to-the-millions-with-gvisor-at-tencent/)
- A GKE Agent Sandbox customer handles spikes of more than 30,000 sandboxes on one cluster. — [Google Cloud blog](https://cloud.google.com/blog/products/containers-kubernetes/accelerate-agentic-rl-with-gke-agent-sandbox)
- Modal supports up to 50,000 concurrent sandboxes per customer. — [Modal](https://modal.com/resources/best-sandboxes-training-coding-agents-rl)
- Prime Intellect's default is 512 active sandboxes. — [Prime Intellect](https://www.primeintellect.ai/blog/sandboxes)
- Measured create/resume times (2026, time to first executed command): E2B 717/662 ms, Daytona 742/1,254 ms, Modal 2,437/2,347 ms, Vercel 1,852/3,333 ms. Daytona markets "sub 90ms" creation. — [LogRocket comparison](https://blog.logrocket.com/comparing-ai-agent-sandbox-platforms-e2b-modal-daytona-and-more/)

**Resource profiles**
- Across over 100 production sandboxes, each co-hosting an OpenClaw-style harness, CPU is idle more than 80% of the time and memory more than 50%. During tool calls, idle time drops below 20%. Tearing sandboxes down after each tool call cuts instance cost by 65%, but snapshot capture/restore costs offset this. — [Aries, Jul 2026](https://arxiv.org/abs/2607.29069)
- Sandbox working-set memory peaks at 28 GB per session during compilation and tests. A single Claude Code session served by DeepSeek-V4 can hold up to 11 GB of KV cache on GPU. — [AgentSysBench, Aug 2026](https://arxiv.org/abs/2608.15127)
- Agent contexts cut an LLM backend's resident capacity from over 176 to 40 concurrent requests (4.4×) under a fixed 25 GB KV budget with Qwen3.6. With 32 sessions, the median engine concurrency was only 22 and p95 queueing delay was 7.9 s. — [Aries](https://arxiv.org/abs/2607.29069)
- A self-published ACP adapter benchmark shows a native Rust adapter with about 4 ms cold start and about 3 MiB idle RSS, versus about 100 ms and 76 MiB for the Node-based adapter. — [pi-acpinator](https://libraries.io/cargo/pi-acpinator)

### Inferences
- Harness RSS × N is a real budget once harnesses run inside rollouts. At 76 MiB vs 3 MiB per idle process (pi-acpinator), 1,000 concurrent agents is about 74 GiB vs 3 GiB of RAM (Derived). Leaky Node harnesses that grow to multiple GB (see Q6) are disqualifying at that scale. This is the clearest case for a compiled, GC-free or low-footprint harness core.
- Even so, sandbox memory (GBs per session, up to 28 GB peak) and GPU KV (up to 11 GB per Claude Code session) are 1–3 orders of magnitude larger than any harness process. Harness footprint is a secondary but real cost.
- Rollout throughput depends on tail management: stage-specific timeouts, async scheduling, fast environment reset and stale-job cleanup (LEGO-RL, ProRL, RollArt). Harness language does not set it. Harbor in Python orchestrating 0.3M trajectories shows that the orchestrator's language does not matter.
- INIT Agent design implication: provide a "headless, embeddable" mode with fast startup (tens of ms), small RSS, no leaks over 1,000+ step sessions, deterministic token-level logging, and a proxy-friendly LLM client. That makes INIT Agent usable as a black-box RL or eval harness, which is where 2026 labs actually spend.

### Gaps
- No published per-instance RSS or CPU numbers for the real Claude Code or Codex CLI running headless inside RL rollouts were found.
- LEGO-RL's higher trainer wait with Claude Code (66.1% vs 40.8%) is not attributed to harness speed versus trajectory length. Do not read it as "Claude Code is slow."

---

## 4. Local inference integration: performance-critical components and their languages

### Takeaway
If the harness hosts or calls local models, the hot paths are inside the inference engine: kernels, KV-cache management, scheduling, tokenization and detokenization. These are already written in C/C++/CUDA/Rust (llama.cpp, MLX, mistral.rs, candle, HF tokenizers, tiktoken's core) or are Python-orchestrated with native kernels (vLLM, SGLang). The harness's job is to **feed the prefix cache**: append-only prompts, stable prefixes, session/slot affinity. It should not re-implement those components. Tokenizer speed matters for budget counting and batch work, but CPU-side tokenization or detokenization inefficiencies inside servers have caused measurable slowdowns.

### Cited Findings
**Engines and languages (repos)**
- llama.cpp: C/C++ (ggml). — [llama.cpp](https://github.com/ggml-org/llama.cpp)
- MLX: C++ core with Python/Swift APIs. — [MLX](https://github.com/ml-explore/mlx)
- mistral.rs: Rust. — [mistral.rs](https://github.com/EricLBuehler/mistral.rs)
- candle: Rust. — [candle](https://github.com/huggingface/candle)
- vLLM: Python orchestration with CUDA/C++ kernels. — [vLLM](https://github.com/vllm-project/vllm)
- SGLang: Python runtime with native kernels. — [SGLang](https://github.com/sgl-project/sglang)
- HF tokenizers: Rust core. — [HF tokenizers](https://github.com/huggingface/tokenizers)
- tiktoken: Rust core with a Python API. — [tiktoken](https://github.com/openai/tiktoken)

**Prefix/KV caching behavior**
- llama-server keeps the previous request's KV in its slot and prefills only the new suffix (`cache_prompt` defaults to true). `--cache-reuse N` can reuse chunks after an earlier change. — [llama.cpp discussion #13606](https://github.com/ggml-org/llama.cpp/discussions/13606)
- Multi-slot selection uses an LCP-similarity "sufficient" match rather than the optimal one, which causes unnecessary recomputation on very long prompts. — [llama.cpp discussion #18244](https://github.com/ggml-org/llama.cpp/discussions/18244)
- SGLang RadixAttention shares prefixes across all requests. A secondary source reports about 29% higher throughput than vLLM on ShareGPT-like prompts and up to 6.4× on prefix-heavy workloads, dependent on prompt ordering. — [traeai SGLang lesson](https://learn.traeai.com/t/ai-engineering/phases/17-infrastructure-and-production/06-sglang-radixattention)
- Agent workloads see an empirical prefix hit rate of 84.6–99.5% (ideal cache). Decode dominates LLM time (91–98.6%) once prefix caching works. — [Yuan et al., May 2026](https://arxiv.org/html/2605.26297v1)
- Applications with static system prompts and append-only histories (e.g., Claude Code) reach cache hit rates up to 99%. Apps that restructure context each turn (e.g., DeepResearch) fall to 1% or lower. — [AgentSysBench, Aug 2026](https://arxiv.org/abs/2608.15127)

**CPU-side bottlenecks in servers**
- vLLM detokenizer PR #24174: CPU time between steps grew from 4.6 ms at about 4K generated tokens to 8.9 ms at about 60K, because the full history was copied each step. The fix brought it to 2.9 ms, flat. — [vLLM PR #24174](https://github.com/vllm-project/vllm/pull/24174)
- Multi-GPU serving: going from 5 to 32 CPU cores cut victim-request TTFT under load by more than 5×. — [arXiv 2603.22774, Mar 2026](https://arxiv.org/html/2603.22774v1)
- Snowflake found 90% of vLLM embedding time was CPU work (tokenization and serialization). — [Snowflake engineering](https://www.snowflake.com/en/blog/engineering/embedding-inference-arctic-16x-faster/)
- SGLang outputs wait in ZMQ queues when the tokenizer or detokenizer stage saturates, and the bottleneck is hard to localize (open issue). — [SGLang issue #43304](https://github.com/sgl-project/sglang/issues/43304)

**Tokenizer speed (2026)**
- Gigatoken (Rust, Jul 2026) claims 24.53 GB/s encode on a 144-core EPYC: 681× tiktoken and 989× HF tokenizers on GPT-2. On an M4 Max it does 8.79 GB/s (140× tiktoken). Its compatibility mode is slower. These are self-reported. — [MarkTechPost, Jul 23 2026](https://www.marktechpost.com/2026/07/23/meet-gigatoken-a-rust-bpe-tokenizer-that-encodes-text-at-24-53-gb-s-up-to-989x-faster-than-huggingface-tokenizers/)
- The `tiktoken` Rust crate claims 4–15× over Python tiktoken, but only about 1.5× on Unicode-heavy input. — [crates.io tiktoken](https://crates.io/crates/tiktoken/2.1.1)

### Inferences
- Tokenizer arithmetic for a harness (Derived): a 120K-token context is about 0.5 MB of text. Even the 2025-era Python tiktoken (Rust core) tokenizes that in well under 100 ms. Incremental counting (tokenize only the append, about 875 tokens median per TraceLab) makes it negligible. Exact counting only matters for compaction triggers; most harnesses use estimates.
- For INIT Agent with local models, the performance-critical *harness* decisions are three. (1) Byte-stable, append-only prompt layout so llama.cpp, MLX or SGLang reuse KV; one changed early byte forces a full re-prefill. (2) Session-to-slot affinity, or a radix-cache engine, for concurrent agents. (3) Avoid frequent context restructuring, such as per-turn retrieval injected near the top.
- Writing kernels, tokenizers or schedulers in the harness itself would duplicate mature C++/Rust engines. Bind to them via FFI or HTTP instead.

### Gaps
- No measurement found of llama.cpp or MLX prefix-reuse hit rates under real coding-agent traffic.
- The SGLang 6.4× and 5× multipliers are from secondary sources, not the SGLang paper or blog.
- MLX and mistral.rs prompt-cache specifics were not investigated in this pass.

---

## 5. Prompt-cache economics: harness design vs language speed

### Takeaway
Prompt-cache hit rate is the single largest cost and TTFT lever a harness controls. Cache reads are billed at 0.1×, 0.05× or 0.025× of input price, depending on model, as of Oct 2026. Coding-agent contexts are about 100–300× more input than output. A cache-breaking harness bug (timestamp in the system prompt, non-deterministic JSON key order, reordering or editing tools, mid-session mode switches) multiplies per-step input cost about 10–40× and adds seconds of TTFT. No language-level optimization comes close to that magnitude. For a **self-modifying** harness this is the central constraint: every edit to tool definitions invalidates the entire cache.

### Cited Findings
**Pricing and rules (current docs, Oct 2026)**
- Anthropic cache write (5-min TTL) is 1.25×, write (1-h TTL) 2×, and read 0.1× of base input. Exceptions: Opus 5.5 and Sonnet 5.5 read at 0.05×, and Fable 5.1 and Mythos 5.1 at 0.025×. Example: Opus 5.5 input is $4/MTok, a 5-min write $5/MTok, and a cache hit $0.20/MTok. — [Anthropic prompt caching docs](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)
- Anthropic cache prefix order is tools → system → messages. Changing tool definitions invalidates tools, system and messages caches. Toggling the speed setting (fast vs standard) or `tool_choice` invalidates system and messages. Thinking and effort changes invalidate messages. The TTL clock starts at request start, so a 4-minute streamed response leaves about 1 minute of the 5-minute TTL. Minimum cacheable length is 512–4,096 tokens depending on model. Cache hits do not count against rate limits on the 1-h cache. — [Anthropic prompt caching docs](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)
- OpenAI (GPT-5.6+) cache reads at 0.1× uncached input (0.05× for GPT-6.1 Sol) and writes at 1.25×, with a 1,024-token minimum. TTL is `30m` for GPT-5.6+; earlier models use in-memory (about 5–10 min) or 24h. Best practices: static content first, append rather than rewrite, keep tool definitions and order stable, and use compaction sparingly because it resets reuse. — [OpenAI prompt caching guide](https://developers.openai.com/api/docs/guides/prompt-caching)

**Latency effect (measured)**
- Anthropic launch table (Aug 14; originally the 2024 launch, the fetched page displays 2025):
  - 100K-token cached prompt: TTFT 11.5 s → 2.4 s (−79%), cost −90%.
  - 10K-token many-shot prompt: 1.6 s → 1.1 s (−31%), cost −86%.
  - 10-turn conversation: about 10 s → about 2.5 s (−75%), cost −53%.
  - Headline: "up to 90%" cost and "up to 85%" latency reduction for long prompts.
  - [Anthropic prompt caching post](https://claude.com/blog/prompt-caching)

**Observed hit rates and cost structure in real coding agents**
- Token-weighted prefix hit rate on Claude Code and Codex traces is 95.7%: 97.5% on tool-result steps and 84.4% on user-initiated steps. Hit rate falls when idle gaps exceed about 5 min and almost always misses after 1 h. — [TraceLab, Jun 2026](https://arxiv.org/html/2606.30560v2)
- Even with those hit rates, prefix tokens are 59.5% of cost, append 29.2% and output 11.2%. The average session costs $9.70 (median $0.61). The median per-step prefix is about 119K tokens, the append about 875 and the output about 214. Total input:output is about 290:1. Retaining cache across human think time would cut priced cost by at most about 12.8%. — [TraceLab](https://arxiv.org/html/2606.30560v2)
- Median human wait between requests is 1.4 min (p90 20.6 min). Tool gaps rarely exceed 5 min. — [TraceLab](https://arxiv.org/html/2606.30560v2)
- Manus (Jul 2025) calls KV-cache hit rate "the single most important metric for a production-stage AI agent." Its average input:output ratio is about 100:1. Its rules: stable prefix (no second-precise timestamps), append-only context, deterministic serialization ("some languages don't guarantee stable key ordering"), explicit breakpoints. It masks tool logits instead of adding or removing tools mid-task. — [Manus blog](https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus)
- Applications with static prompts and append-only histories (Claude Code) reach up to 99% hits, while context-restructuring apps fall to ≤1%. — [AgentSysBench](https://arxiv.org/abs/2608.15127)

### Inferences
- **Cost of a cache miss per step (Derived, Opus 5.5 prices from the Oct 2026 docs, TraceLab medians):**
  - Cached step: 119K × $0.20/M plus 875 × $5/M ≈ $0.028 input.
  - Full miss: 120K × $4/M ≈ $0.48 input, about 17× more, plus several seconds of extra TTFT (cf. 11.5 s → 2.4 s at 100K tokens).
  - Over about 84 steps per session (TraceLab-derived), that is about $2.4 vs about $40 of input.
  - A harness that drops from 96% to 80% hit rate roughly doubles to triples input spend.
- Language choice affects cache hits only through **determinism**: stable key order, stable float and whitespace formatting, stable tool ordering, no time-varying text in prefixes. Any language can do this. It must be enforced with byte-level tests, e.g., hashing the serialized prefix each turn and alerting on unexpected diffs.
- **For a self-modifying harness (INIT Agent):**
  - Tool definitions sit at the head of the cache prefix, so every self-edit of tools or the system prompt is a full miss: about 10–40× input cost for that turn plus a TTFT spike.
  - Batch self-modifications at session or compaction boundaries.
  - Prefer deferred or searchable tools (Tool Search: 85% fewer upfront tokens) and logit masking over schema churn.
  - Put mutable "self" content after the stable prefix.
  - Avoid mid-session speed or effort toggles.
- The 1-h TTL (2× write) pays when human gaps are 5–60 min (TraceLab p90 is 20.6 min). The 5-min TTL suffices for autonomous loops, because tool gaps rarely exceed 5 min.

### Gaps
- No published TTFT-vs-cached-length curve for 2026 models (Opus 5.x, GPT-5.6+) was found. The 2024 Anthropic table remains the cleanest public latency measurement.
- OpenAI's guide gives no numeric latency reduction, only "reduce the time spent processing input."

---

## 6. Case studies where harness language/runtime performance visibly mattered

### Takeaway
Visible performance wins in 2026 harnesses came from four things: (a) **rendering architecture**, with differential or virtualized TUIs replacing full redraws; (b) **runtime and footprint**, through startup, memory leaks, GC and single-binary install; (c) **search/indexing algorithms**, with indexed regex replacing linear ripgrep on huge monorepos; and (d) **transport**, through WebSockets and incremental context. Codex's Rust rewrite was justified by install, sandboxing, memory and no-GC predictability, with no published latency benchmarks. Claude Code stayed on TypeScript and fixed its problems with a renderer rewrite, Bun, and a Rust port of Bun that gave 10% faster startup. Codex in Rust still had idle-CPU and helper-process issues. Language mattered at the margins of footprint and stability. Architecture mattered more.

### Cited Findings
**Codex CLI → Rust (May 30, 2025)**
- Stated reasons: "Zero-dependency Install" (Node v22+ was "frustrating or a blocker"), "Native Security Bindings" (Rust already used for Linux sandboxing), "Optimized Performance — no runtime garbage collection, resulting in lower memory consumption," and an "Extensible Protocol." No benchmarks were published. — [openai/codex discussion #1174](https://github.com/openai/codex/discussions/1174)
- On Dec 2, 2025, a maintainer said `npm i -g @openai/codex` had installed the native build "for months." — [openai/codex discussion #1174](https://github.com/openai/codex/discussions/1174)
- Even in Rust: one blog reports the Codex CLI at about 30% CPU while idle, because a status animation redraws about every 32 ms. Desktop and IDE reports describe sub-agent tasks holding CPU at 90–100%, accumulating helper processes, and an Electron renderer growing past 5 GB. These are user reports. — [aident.ai](https://aident.ai/blog/fix-codex-cli-high-cpu-background-terminal); [openai/codex discussion 36406](https://github.com/openai/codex/discussions/36406)

**Claude Code (TypeScript): rendering, memory, runtime**
- Flicker: Claude Code 2.0.72 (Dec 2025) "rewrote the renderer from scratch" while keeping React, because Ink lacked fine-grained incremental updates. Pi's TUI is cited as the "gold standard for differential rendering." — [steipete](https://steipete.me/posts/2025/signature-flicker)
- Secondary reports say the rewrite (Thariq Shihipar, Jan 22 2026) targeted cross-platform bugs and "excessive garbage collection." It diffs terminal cells, emits minimal ANSI, reduces flicker about 85%, and works within a ~16 ms frame budget. Anthropic also contributed DEC mode 2026 synchronized-output support to VS Code's terminal and tmux. These are unconfirmed by a primary post. — [letsdatascience](https://letsdatascience.com/news/anthropic-rewrites-claude-code-terminal-rendering-76034b52); [thuliobittencourt](https://thuliobittencourt.com/claude-code-fullscreen-rendering-sem-flicker-software-house-2/)
- Fullscreen (alt-screen) rendering is opt-in from v2.1.89 (`/tui fullscreen`, or `CLAUDE_CODE_NO_FLICKER=1` before v2.1.110). "Only visible messages are kept in the render tree, so memory stays constant regardless of conversation length." — [Claude Code fullscreen docs](https://code.claude.com/docs/en/fullscreen)
- Memory leak reports:
  - v2.0.36: 129 GB virtual and a system freeze (RSS about 415 MB at measurement). — [GitHub #11315](https://github.com/anthropics/claude-code/issues/11315)
  - Jan 12 2026, v2.1.3: 304 GB. — [claudeissues #17615](https://claudeissues.com/issue/17615-memory-leak-claude-process-using-304gb-memory)
  - Jan 29 2026, v2.1.19: about 2 GB/min growth while idle, with 90% CPU. — [claudeissues #21665](https://claudeissues.com/issue/21665-bug-memory-leak-loop-2gb-minute-ram-growth-even-when-idle)
  - Jan 31 2026, v2.1.27: 93 GB heap, 16+ GB RSS, and `/resume` running more than 10 min at 100% CPU. — [claudeissues #22188](https://claudeissues.com/issue/22188-memory-leak-claude-code-process-grows-to-93-gb-heap-allocation)
- Runtime: Anthropic acquired Bun on Dec 3, 2025. Bun "directly drove" Claude Code's native installer. — [Anthropic](https://anthropic.com/news/anthropic-acquires-bun-as-claude-code-reaches-usd1b-milestone)
- Bun was ported from Zig to Rust in about 11 days with up to about 64 parallel Claude instances, roughly 1M lines and about $165K of API cost (Jul 8, 2026). Stated reasons were memory safety: use-after-free, double-free and leaks. — [Bun blog "Rewriting Bun in Rust"](https://bun.com/blog/bun-in-rust)
- Results of the Rust port:
  - Claude Code v2.1.181 (Jun 17 2026) and later run on it. Linux p50 startup went from 517 ms to 464 ms (−10%); "otherwise, barely anyone noticed."
  - Bun.serve throughput rose 4.8%.
  - After 2,000 in-process `Bun.build()` calls, memory was 609 MB vs 6,745 MB before.
  - Binary size fell about 20% (88 → 70 MB on Linux).
  - [Bun blog](https://bun.com/blog/bun-in-rust); [Simon Willison, Jul 19 2026](https://simonwillison.net/2026/Jul/19/claude-code-in-bun-in-rust/)
- A third-party test reported that harness choice (Claude Code, Codex CLI, Pi) changed solve rates only a few points but changed token usage a lot. Claude Code used about 2× Pi's tokens on SWE-bench Lite because of prompt and tool-definition overhead. — [General Compute / search summary](https://www.generalcompute.com/blog/how-coding-agents-depend-on-inference-speed)

**Harness implementation languages (source study pinned to Jul 2026 releases)**
- Claude Code: TypeScript. Codex CLI: Rust. Gemini CLI: TypeScript. Mistral Vibe: Python. Goose: Rust (donated to the Linux Foundation AAIF, Apr 2026). Grok Build: Rust, closed binary (May 2026). Antigravity CLI: closed Go binary. No harness imports a general agent framework, and none uses vector-embedding code retrieval. — [Harness Engineering source study, arXiv 2609.00006](https://arxiv.org/abs/2609.00006)
- Harbor-Index (Sep 2026): "The model underlying an agent is more influential for benchmark performance than the harness." — [Harbor-Index](https://arxiv.org/abs/2609.04298)

**Search/indexing (ripgrep limits)**
- Cursor (Mar 23, 2026): in enterprise monorepos, "we routinely see `rg` invocations that take more than 15 seconds." Agents "just love to use grep," often in parallel. Cursor built a **client-side** sparse n-gram inverted index ("Instant Grep"), with the postings file on disk, an mmapped sorted hash table, and state keyed to a Git commit plus a layer for local edits. No numeric speedups or implementation language were stated. — [Cursor blog](https://cursor.com/blog/fast-regex-search)
- syntext, an open-source Rust hybrid index based on the Cursor post and GitHub code search, claims about 20× over ripgrep (self-reported). — [syntext](https://docs.rs/crate/syntext/1.0.0)

**Editors and terminals (Zed, Warp)**
- Zed 1.0 (2026, Rust and GPUI): reported cold start of about 0.12 s, idle memory of about 222 MB and input latency of about 2 ms. These are secondary figures that vary by source. External agents run as separate processes over ACP. — [noqta](https://noqta.tn/en/news/zed-editor-1-0-stable-rust-ai-coding-2026)
- A Zed fork for agent fleets had to fix ACP thread streaming "from O(N²) down to O(delta)." — [Helix blog](https://blog.helix.ml/p/how-we-forked-zed-to-run-a-fleet)
- Warp: Rust, GPU-accelerated UI, "smooth at 60fps on multi-megabyte build logs" (vendor/listing). Its agent's Terminal-Bench pass rate of about 65% was set by model and planning choices. — [Warp Terminal-Bench blog](https://www.warp.dev/blog/terminal-bench); [agentconn](https://agentconn.com/agents/warp/)

### Inferences
- **Overall ranking: where INIT Agent's extreme-performance effort pays (cross-question synthesis)**
  1. **Prompt-cache determinism and append-only context** (Q5). Worth about 10–40× on input cost per avoided miss and seconds of TTFT. For a self-modifying harness, gate and batch tool/system edits. Language-independent but must be engineered and tested byte-exactly.
  2. **Fewer and overlapped round-trips** (Q2). Persistent WebSocket or incremental context (up to about 40% faster on long tool chains), parallel and speculative tool calls (about 43–48% task time cut in PASTE), and programmatic tool calling (removes N−1 inference passes). This architecture becomes *more* important as models speed up.
  3. **Tool-execution path** (Q1). Persistent PTY instead of tmux or spawn-per-command (0.78 → 0.42 s/action), cheap sandbox/approval wrappers (Codex residual overhead is 18.6% of tool time), indexed code search for big repos (rg above 15 s), tail-aware timeouts, and background long tools. This is where systems-language performance genuinely helps: process management, PTY, file index, file watching.
  4. **Footprint and stability at N-way fan-out** (Q3). Fast startup (tens of ms), small and flat RSS, no leaks, and virtualized history in UI and memory. Compiled/no-GC cores are a clear win here (3 MiB vs 76 MiB adapter, and Claude Code's leak history). Bun's Rust port bought only about 10% startup.
  5. **Streaming render path** (Q6). Differential or cell-diff rendering, incremental markdown parsing (O(delta), not O(N²)), and 16 ms frame batching. At 1,000+ tok/s, naive full re-render is the main harness CPU sink.
  6. **Per-turn context assembly, JSON and token counting.** Lowest priority. Millisecond-scale against seconds of model and tool time. The ≤9% upper bound in Aries includes all host-side orchestration. Make it incremental (no full-history copies per step, cf. vLLM's O(n) detokenizer copy bug) and stop there.
- Local inference (Q4): delegate to llama.cpp, MLX, SGLang or vLLM (C++/Rust/CUDA). The harness's contribution is KV-friendly prompt layout and slot affinity.
- The Amdahl model shows that model plus tool time is at least about 90% of task time today, and generation shrinks with 1,000+ tok/s models. Optimization effort should therefore follow this order: (round-trips avoided) > (cache hits) > (tool path latency) > (footprint × N) > (render) > (per-turn CPU).

### Gaps
- No primary Anthropic engineering post with measured renderer numbers (frame times, CPU) was found. The 85% and 16 ms figures are secondary.
- No published before/after latency or memory benchmark exists for Codex TypeScript vs Rust.
- No data was found on file-watching costs (FSEvents/inotify) in agent harnesses, or on how Claude Code or Codex handle large-repo watching. This needs a dedicated search.
- Cursor's Instant Grep post gives no numeric speedup or implementation language.
- Zed and Warp performance figures come from secondary or vendor sources, not controlled measurements.
