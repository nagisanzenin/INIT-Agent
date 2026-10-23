# Systems-engineering concepts for LLM sub-agent concurrency and delegation (as of 2026-10-22)

Scope: which classic concurrency and delegation concepts are proven and future-proof foundations for LLM sub-agents, how 2025–2026 harnesses use them, and what a trusted, immutable base (INIT's Root) must enforce while the model designs its own delegation topology. Classic sources are cited as origins. Agent-era sources carry their dates; anything older than about 9 months is flagged where the field moves fast. "Inference" and "speculation" are marked.

---

## 1. Actor model and Erlang/OTP supervision: isolation, mailboxes, "let it crash", supervision trees

### Takeaway
Actors plus OTP supervision are the most proven reliability pattern for long-lived concurrent systems. The parts that carry over to LLM agents are isolation (no shared memory), asynchronous mailboxes, links and monitors, restart-intensity limits and escalation. 2025–2026 agent frameworks have adopted them: AutoGen v0.4 (since folded into Microsoft Agent Framework), Jido 2.0 in Elixir, and several 2026 essays. Taken literally, "let it crash" does not fit LLM agents, because a clean restart throws away expensive context. Agents need supervision combined with durable state, so a restart replays from a log instead of starting from zero.

### Cited Findings
**Origins**
- Hewitt, Bishop and Steiger (IJCAI 1973) introduced actors: computation is done by actors that exchange messages, and that is the only way they interact. — [Hewitt et al. 1973](https://www.ijcai.org/Proceedings/73/Papers/027B.pdf)
- Joe Armstrong's 2003 thesis argues for building reliable systems "in the presence of software errors": isolated processes, fail fast, and separate supervisors that recover. Its case study is Ericsson's AXD301 ATM switch. — [Armstrong thesis 2003](https://erlang.org/download/armstrong_thesis_2003.pdf)

**OTP mechanics**
- Restart strategies are `one_for_one`, `one_for_all`, `rest_for_one` and `simple_one_for_one`.
- Restart intensity: if more than `intensity` restarts happen within `period` seconds, the supervisor terminates all its children and then itself, escalating to its own supervisor. The defaults are intensity 1 and period 5.
- Child restart types are `permanent`, `transient` and `temporary`. Shutdown is `brutal_kill`, a timeout, or `infinity`.
- Source: [Erlang/OTP supervisor principles](https://www.erlang.org/doc/system/sup_princ.html)
- Links are bidirectional: an exit signal propagates unless the process traps exits. Monitors are unidirectional and deliver a `'DOWN'` message. Supervision is built on links. — [Erlang reference manual: processes](https://www.erlang.org/doc/system/ref_man_processes.html)

**The AXD301 "nine nines" claim and its caveats**
- Ulf Wiger (Ericsson, 2008) wrote that the "9 nines" figure came from a British Telecom quote about one network during an initial trial period. That quote was the only number approved for external communication when Armstrong needed one for a talk. The design target was "5 nines or better", and Wiger asked people to say "better than 5 nines" instead.
- Telecom availability usually ignores disturbances shorter than 30 s. Wiger's post gives the nine-nines budget as about 3.1 ms a year, and says one 30 s outage would use up the budget for about 10,000 years. Note that 10⁻⁹ × 1 year ≈ 31.5 ms, so his figures look off by about 10×, but the point stands: nine nines is not a meaningful target.
- Field data over several years supported "better than 5 nines", though Wiger did not publish the exact figure.
- Source: [Ulf Wiger, erlang-questions, June 2008](https://erlang.org/pipermail/erlang-questions/2008-June/035824.html)

**Supervision mapped onto LLM agents (2025–2026)**
- **AutoGen v0.4 (Microsoft Research, Jan 2025)** rebuilt AutoGen on an asynchronous, event-driven actor model: "actors" exchange messages and do work. AutoGen has since gone into maintenance mode. Its successor is Microsoft Agent Framework, which secondary sources date to public preview in Oct 2025 and to GA on 2026-04-02; those dates are not verified against a primary source. — [MSR blog, AutoGen v0.4](https://www.microsoft.com/en-us/research/?p=1116288); [AutoGen 0.4 launch, devblogs Jan 17 2025](https://devblogs.microsoft.com/autogen/autogen-reimagined-launching-autogen-0-4/); [atlan.com summary](https://atlan.com/know/ai-agent/what-is-autogen/)
- **Jido (Elixir)** runs agents as ordinary OTP children: you add a Jido instance to the application's supervision tree.
  - Agents are immutable data updated through `cmd/2`. Actions transform state, signals route events, and directives describe effects for the runtime to perform: a pure-decision / effectful-runtime split.
  - The LLM is optional and lives in a separate package (`jido_ai`).
  - Jido 2.0 was released on 2026-02-22 (per ByteIota, secondary).
  - Sources: [Jido hexdocs](https://hexdocs.pm/jido/readme.html); [Jido 2.0.0-rc.1 docs](https://hexdocs.pm/jido/2.0.0-rc.1/Jido.html); [Chris Lema, opinion piece](https://chrislema.com/markdown/ai-agent-systems)
- **"Your Agent Needs a Supervisor, Not a Retry Loop" (tianpan.co, 2026-07-02)** argues that the point of supervision trees "was never 'restart things when they crash'". Recovery is a separate concern, owned by a separate process in a hierarchy. It maps OTP choices onto agents:
  - restart with clean state = a fresh context;
  - restart with saved state = durable execution (Temporal, LangGraph checkpointers);
  - escalate.
  - Source: [tianpan.co, Jul 2026](https://tianpan.co/blog/2026-07-02-your-agent-needs-a-supervisor-not-a-retry-loop)
- **Zylos Research (2026-03-16)** treats LLM sessions, executors, memory services and tool subprocesses as supervised children, and calls state recovery "the hardest problem" for long-running agent sessions. — [Zylos, Mar 2026](https://zylos.ai/research/2026-03-16-supervisor-trees-fault-tolerance-ai-agent-systems)
- **agents.erl** combines Erlang supervision trees with OpenAI's API: a top-level supervisor with one worker per API family (announcement only; maturity unknown). — [agents.erl announcement](https://idsc2025.substack.com/p/welcome-agentserl-new-open-source)

**Anthropic's production multi-agent research system (2025-06-13; 16 months old, check for staleness)**
- It resumes agents from their last state after errors instead of restarting, because restarts are expensive.
- It combines the model's own adaptation (telling the agent a tool is failing) with deterministic retries and checkpoints.
- It uses "rainbow deployments" because agents are stateful mid-task.
- Source: [Anthropic Engineering, Jun 13 2025](https://www.anthropic.com/engineering/multi-agent-research-system)

### Inferences
- What transfers unchanged:
  - share-nothing isolation (one process and one sandbox per agent);
  - asynchronous mailboxes;
  - monitors, so a parent learns of a child's death;
  - restart intensity (MaxR in MaxT, then escalate), which caps a crash loop's cost and turns a local fault into a signal at the right level of the tree.
- What changes:
  - the unit of restart has to be "restart from a checkpoint or log" (durable execution, §6), not "restart from init";
  - the supervisor of an LLM agent may itself be an LLM that decides *how* to recover (re-prompt, re-route, give up), while the mechanical backstops stay non-LLM: kill, restart intensity, escalation to Root.
- **Future-proofness verdict: high.**
  - Isolation and supervision do not depend on how smart the supervised component is. A stronger model fails less often but still fails: provider errors, OOM, rate limits, adversarial inputs.
  - A stronger model also makes a stronger supervisor policy, which is plastic.
  - The hand-coded part to avoid is a *fixed* restart strategy per agent. Let the model choose one_for_one or rest_for_one for the trees it builds, and let the base enforce only the intensity cap and escalation.
- Mark Miller's thesis (§5) shows that the actor model and the object-capability model are nearly the same model: an actor can only message actors it holds references to. Actor isolation and capability security therefore compose naturally, which matters for the synthesis (§9).

### Gaps
- No independent production case study of Jido or another OTP-based LLM-agent framework at scale was found; the sources are project docs and opinion posts.
- No primary Ericsson document on AXD301 field availability was found beyond Wiger's 2008 post.
- Microsoft Agent Framework's GA date (2026-04-02) comes only from secondary sources.

---

## 2. Structured concurrency: children cannot outlive the parent's scope

### Takeaway
Structured concurrency (Sústrik 2016, N. J. Smith 2018) makes "a task's children finish, or are cancelled, before the task's scope exits" a language-level invariant, with errors and cancellation propagating along the tree. It is now standard in Kotlin, Swift and Python. Java ships it as final only in JDK 28 (March 2027), after nine incubator and preview rounds. For sub-agents it gives the cleanest rule for lifetimes, cancellation and **budget reclamation**: when a scope closes, every descendant is dead and its unspent budget returns to the parent. The one thing it does not give for free is preemptive cancellation. Cancellation is cooperative in every mainstream implementation, so an agent harness needs a hard kill underneath.

### Cited Findings
**Origins**
- Martin Sústrik coined "structured concurrency" for libdill (2016): coroutines bound to lexical scopes. — [250bpm: Structured Concurrency](http://250bpm.com/blog:71)
- Nathaniel J. Smith, "Notes on structured concurrency, or: Go statement considered harmful" (2018-04-25):
  - an unscoped `go`/spawn is the concurrency equivalent of `goto`, because it breaks function abstraction;
  - Trio's *nursery* restores the rule that when a function returns, everything it started has finished;
  - an exception in a child cancels its siblings and propagates to the parent;
  - cancel scopes and timeouts nest.
  - Source: [vorpus.org, 2018](https://vorpus.org/blog/notes-on-structured-concurrency-or-go-statement-considered-harmful/)

**Mainstream adoption**
- **Kotlin:** a scope cannot complete until all its child coroutines complete, and cancellation propagates to children. Kotlin also has `SupervisorJob` and `supervisorScope`, in which a failing child does not cancel its siblings or the parent. Structured lifetimes and OTP-style supervision are combined in one API. — [Kotlin coroutines basics](https://kotlinlang.org/docs/coroutines-basics.html); [Kotlin exception handling: supervision](https://kotlinlang.org/docs/exception-handling.html#supervision)
- **Swift:** SE-0304 introduced task groups and child tasks bound to their parent, with cooperative cancellation (Swift 5.5, 2021). — [SE-0304](https://github.com/apple/swift-evolution/blob/main/proposals/0304-structured-concurrency.md)
- **Python:** `asyncio.TaskGroup` (3.11), modelled on Trio nurseries, cancels the remaining tasks when one fails. — [Python asyncio TaskGroup](https://docs.python.org/3/library/asyncio-task.html#task-groups)

**Java's timeline**
| JEP | JDK | Stage | Note |
|---|---|---|---|
| 428, 437 | 19, 20 | incubator | |
| 453 | 21 | preview | |
| 505 | 25 | fifth preview | |
| 525 | 26 | sixth preview | adds `Joiner.onTimeout()`; `allSuccessfulOrThrow()` returns a `List` |
| 533 | 27 (GA 2026-09-15) | seventh preview | |
| 543 | 28 (GA March 2027) | **final**, integrated Oct 2026 | no API changes from JDK 27 |
- Sources: [JEP 525](https://openjdk.org/jeps/525); [InfoQ: JEP 533, May 2026](https://www.infoq.com/news/2026/05/jep-533-jdk-27/); [InfoQ: JEP 543 integrated into JDK 28, Oct 2026](https://www.infoq.com/news/2026/10/jep-543-jdk-28/)
- The JEP 543 status rests on one InfoQ report; confirm at openjdk.org/jeps/543.

**How 2025–2026 agent harnesses handle lifetimes**
- **Claude Code agent teams** (docs fetched 2026-10-22):
  - an in-process teammate's subagents must run in the foreground "because a teammate's background work can't outlive the lead's process". Spawning a background subagent from a teammate errors;
  - teammates cannot spawn teammates ("no nested teams");
  - shutdown is cooperative: the lead sends a request, and the teammate "can approve… or reject with an explanation";
  - "Shutdown can be slow: teammates finish their current request or tool call before shutting down."
  - Source: [Claude Code docs: agent teams](https://code.claude.com/docs/en/agent-teams)
- **Claude Code subagents** (docs fetched 2026-10-22): "The spawning subagent doesn't wait for a background nested subagent, which reports to your main conversation if it outlasts the spawning subagent." So background nested subagents are **not** structurally scoped to their spawner; they re-parent to the root conversation. — [Claude Code docs: sub-agents](https://code.claude.com/docs/en/sub-agents)
- **Anthropic's research system (Jun 2025)** ran subagents synchronously: the lead waits for each batch. That was simpler, but the lead could not steer and one slow search could stall the system. Async execution was noted as adding problems with "result coordination, state consistency, and error propagation". — [Anthropic Engineering, Jun 2025](https://www.anthropic.com/engineering/multi-agent-research-system)
- **ReDel** offers a blocking `DelegateOne` and a non-blocking scheme. The non-blocking scheme can leave orphaned "zombie" agents if their results are never collected (per a secondary review). — [ReDel docs: delegation](https://redel.readthedocs.io/en/stable/_sources/delegation.rst.txt); [Moonlight review](https://www.themoonlight.io/en/review/redel-a-toolkit-for-llm-powered-recursive-multi-agent-systems)

### Inferences
- **Lifetime invariant for sub-agents:** every sub-agent belongs to exactly one scope owned by its parent. When the parent ends, the scope cancels all descendants, cooperatively first and then by kill, and returns their unspent reservations. That turns "orphaned agents still spending money" (ReDel zombies, Claude Code background re-parenting) into something that cannot happen.
- **Budget reclamation is the agent-specific payoff.** In threads, structured concurrency mainly prevents leaks and lost errors. In agents, a leaked child is a leaked *spend stream*. Scope exit is the natural point to settle the account (§7).
- **Cancellation must be two-tier.** LLM tool calls are not preemptible at arbitrary points, and every mainstream implementation cancels cooperatively.
  - Tier 1 is a cooperative stop with a grace period at a step boundary.
  - Tier 2 is a process-group kill by the trusted base.
  - Claude Code's "teammate may reject shutdown" is acceptable only because a hard kill (the user closing the session) exists above it.
- **Long-lived workers still fit.** Kotlin's `supervisorScope` and Erlang's supervisors show that a scope can contain long-lived, independently failing children. A "daemon" sub-agent is simply a child of a long-lived scope, ultimately the Root's, never an unscoped detach.
- **Future-proofness verdict: very high.** The invariant concerns accountability and resource conservation, not intelligence. A stronger model benefits because it can reason about its own tree. Java's nine-round road to final shows the API shape is hard to get right (joiners, timeouts), but the invariant itself has not changed since 2016–2018.

### Gaps
- No 2025–2026 agent harness was found that documents a full structured-concurrency guarantee, where a parent's exit provably kills all descendants and refunds their budget. Claude Code comes closest for in-process teammates.
- Whether Claude Code's background nested subagents are killed when the *session* ends is not stated in the visible docs. Agent teams state that team directories are cleaned up at session end.

---

## 3. Fork-join and map-reduce with verification

### Takeaway
Fork-join and map-reduce are thoroughly proven: MapReduce since 2004, work-stealing since Cilk. For LLM agents the decisive variable is the **generation–verification gap**. Repeated sampling raises coverage log-linearly, but final accuracy is capped by the selector.
- With an executable oracle (tests, compilers, proof checkers), "generate many, verify cheaply" works dramatically: SWE-bench Lite rose from 15.9% to 56% with 250 samples.
- With majority vote or reward models, gains plateau after a few hundred samples.
- LLM judges remain clearly below oracle verifiers.
Verification is cheaper than generation when the check is executable, structural or source-anchored. It is not cheaper when checking requires redoing the reasoning or exercising taste.

### Cited Findings
**MapReduce (Dean and Ghemawat, OSDI 2004)**
- Map tasks are deterministic and side-effect-free, so failed or straggling tasks are simply re-executed.
- "Backup tasks" (speculative duplicate execution near the end of a job) cut completion time: the paper reports that sort took 44% longer with backup tasks disabled.
- Source: [MapReduce, OSDI 2004](https://research.google/pubs/mapreduce-simplified-data-processing-on-large-clusters/)

**Repeated sampling and verifiers**
- **Large Language Monkeys** (Brown et al.; v1 2024-07-31, v3 2024-12-30):
  - coverage grows "often log-linear[ly]" with sample count over four orders of magnitude;
  - DeepSeek-Coder-V2 on SWE-bench Lite goes from 15.9% with one sample to 56% with 250, beating the then single-sample state of the art of 43%;
  - without automatic verifiers, "majority voting and reward models plateau beyond several hundred samples".
  - Source: [arXiv 2407.21787](https://arxiv.org/abs/2407.21787)
- **Cobbe et al. (2021, GSM8K):** trained verifiers re-ranking sampled solutions gave a boost the authors describe as roughly equivalent to a 30× increase in model size. — [arXiv 2110.14168](https://arxiv.org/abs/2110.14168)
- **Self-consistency** (Wang et al. 2022): majority vote over sampled reasoning paths improves chain-of-thought accuracy. — [arXiv 2203.11171](https://arxiv.org/abs/2203.11171)
- **Weaver (Stanford):** "a LM can generate a correct response, but we fail to identify it". Weak-verifier ensembles shrink the generation–verification gap by 14.5% on average over majority voting, and a distilled 400M cross-encoder keeps 98.2% of the gain at 99.97% less verification compute. — [Weaver paper](https://scalingintelligence.stanford.edu/pubs/weaver.pdf)
- **Measuring separately:** a 2026 critical review (arXiv 2609.14500) recommends tracking coverage (pass@k), conditional selection accuracy and returned-answer accuracy as separate quantities. — [arXiv 2609.14500](https://arxiv.org/pdf/2609.14500)

**Fork-join in agent harnesses**
- **Anthropic research system (Jun 2025):**
  - an Opus 4 lead with Sonnet 4 subagents beat single-agent Opus 4 by 90.2% on an internal eval;
  - token usage alone explained 80% of BrowseComp variance;
  - multi-agent runs used about 15× the tokens of chat;
  - an LLM-as-judge with a single prompt and a 0–1 score plus pass/fail matched humans best;
  - human testing still caught failures (SEO content farms) that automation missed.
  - Source: [Anthropic Engineering](https://www.anthropic.com/engineering/multi-agent-research-system)
- **Claude Code dynamic workflows** (docs fetched 2026-10-22):
  - `parallel()` and `pipeline()` are fork-join primitives;
  - the documented quality pattern is "independent agents adversarially review each other's findings";
  - `/deep-research` "votes on each claim" and filters out claims that fail cross-checking;
  - claims the verifiers could not check (rate limit, API error) are reported as unverified, not refuted.
  - Source: [Claude Code docs: workflows](https://code.claude.com/docs/en/workflows)
- **MAST** (Cemri et al., NeurIPS 2025; first posted March 2025):
  - 14 failure modes in 3 categories: system design, inter-agent misalignment, and **task verification**;
  - failures stem mostly from system design and inter-agent misalignment "rather than solely from underlying LLM limitations".
  - Source: [arXiv 2503.13657](https://arxiv.org/html/2503.13657v3)

### Inferences
**When verification is cheaper than generation for agent outputs:**
- **Executable oracles:**
  - unit and property tests, type-checks and builds;
  - a sandboxed run that reaches a known state;
  - proof checkers;
  - schema validation of structured output.
- **Source-anchored claims:** a verifier fetches the cited URL and checks the quote. That is cheaper than finding it.
- **Constraint satisfaction:** checking a plan respects budgets or paths is easier than producing one.
- **Mechanical harm signals:** crash, cost blow-up, owner correction. INIT's H1–H12 are verifiers of this kind, cheap and model-free.

**When it is not cheaper:**
- open-ended research synthesis or design quality, where an LLM judge correlates with the generator and plateaus;
- tasks with irreversible side effects, which cannot be verified by trying;
- long-horizon value (ADR-011's point that usefulness only shows longitudinally).

**Verdict: medium-high future-proofness as a pattern, but it belongs to the model, not the base.**
- Stronger models need fewer samples, but the asymmetry remains wherever a cheap oracle exists.
- The base should not hard-code best-of-N. It should provide cheap, trustworthy oracles: sandboxed test runs, Root-run probes, a provenance-checking fetch. Then *any* topology the model invents can use them.
- MapReduce's lesson that re-execution is safe only for side-effect-free tasks translates directly. Read-only sub-agents can be speculatively duplicated or retried freely; writing sub-agents cannot.

### Gaps
- No 2025–2026 study was found that measures the generation–verification gap for *agentic* (multi-step, tool-using) outputs specifically, as opposed to math and code answers.
- No quantitative comparison was found of LLM-judge cross-checking against single-pass research in Claude Code's `/deep-research`.

---

## 4. Coordination substrate: mailboxes vs blackboards vs shared task lists

### Takeaway
- **Point-to-point mailboxes** (actors) are the best-proven substrate and the only one that is also a *security* primitive: the sender is authenticated, and the recipient is reachable only if you hold its address.
- **Blackboards** (Hearsay-II, 1970s) have come back in 2025 LLM papers with modest gains, but every agent reads a growing shared context.
- **Shared task lists** with atomic claim (Claude Code agent teams) work for bags of independent tasks and degrade with dependencies and stale status.
For LLM agents the cost that grows with agent count is *context tokens per agent*. Hierarchical, tree-structured mailboxes keep each agent's inbound context at roughly its parent plus its children, which is why they likely degrade least (inference). Task lists and blackboards are better treated as data structures the model builds on top.

### Cited Findings
**Hearsay-II (CMU speech understanding, 1971–76)**
- Independent "knowledge sources" read and write a shared blackboard of hypotheses.
- A scheduler (focus of control) decides which knowledge source runs next.
- Source: [Erman et al., ACM Computing Surveys 1980](https://dl.acm.org/doi/10.1145/356810.356816)

**2025 LLM blackboard systems**
- Han and Zhang (arXiv 2507.01701, July 2025): agents act according to blackboard contents, round after round until consensus. They report competitive accuracy with "less tokens" than static and dynamic multi-agent baselines, but a main agent still picks who acts next. — [arXiv 2507.01701](https://arxiv.org/html/2507.01701v1)
- Salemi et al. (arXiv 2510.01285; v1 2025-09-30, v2 2026-01-31): requesters post to the blackboard, and agents decide on their own whether to contribute, with no central task assignment. Up to 9% relative F1 gain in data-lake discovery. They argue master–slave systems need "precise knowledge of each sub-agent's capabilities". — [arXiv 2510.01285](https://arxiv.org/abs/2510.01285v2)

**Claude Code agent teams** (docs fetched 2026-10-22)
- **Architecture:** a lead, teammates, a shared task list and a mailbox. Each mailbox is a JSON file per agent. A message counts as sent only when the write to the recipient's file succeeds.
- **Task list:**
  - states are pending, in progress and completed;
  - a dependent task cannot be claimed until its dependencies complete;
  - claiming uses file locks to prevent races;
  - teammates self-claim the next unblocked task.
- **Limits:**
  - "Task status can lag: teammates sometimes fail to mark tasks as completed, which blocks dependent tasks";
  - "no nested teams";
  - one team per session;
  - recommended size is 3–5 teammates;
  - "Two teammates editing the same file leads to overwrites."
- **Message trust:** a message from another agent is marked as coming from another Claude session, not the user. A teammate "can't approve a permission prompt or supply consent on your behalf" and cannot relay a denied action to bypass a check. This is a mailbox provenance rule.
- Source: [Claude Code docs: agent teams](https://code.claude.com/docs/en/agent-teams)

**Failure evidence**
- **MAST:** "inter-agent misalignment" is one of three failure categories. — [arXiv 2503.13657](https://arxiv.org/html/2503.13657v3)
- **Cognition, "Don't Build Multi-Agents"** (Walden Yan, 2025-06-12; 16 months old, check for staleness):
  - "Share context, and share full agent traces, not just individual messages";
  - "actions carry implicit decisions";
  - in their Flappy Bird example, parallel subagents made conflicting stylistic assumptions;
  - they recommend a single-threaded agent, plus a compression model for long tasks;
  - they expect multi-agent parallelism to unlock later as context passing improves.
  - Source: [Cognition blog](https://cognition.com/blog/dont-build-multi-agents)
- **Anthropic (Jun 2025):** early versions had agents "distracting each other with excessive updates", and vague delegation duplicated work. — [Anthropic Engineering](https://www.anthropic.com/engineering/multi-agent-research-system)

### Inferences
**How each substrate degrades as agent count n grows (inference):**
| Substrate | What each agent must read | Main way it degrades |
|---|---|---|
| Blackboard | the whole board, O(total contributions) | context bloat, attention dilution, contention on one shared artifact |
| Flat task list | O(tasks) to choose | stale status, dependency deadlocks (Claude Code's own limitation), and claim races that need atomic primitives |
| Tree of mailboxes | O(1 parent + its children) | depth increases latency and loses information at each summarization hop (Cognition's concern) |

So a tree of mailboxes degrades least in *cost*, and the main risk becomes *information loss through summaries*. Two mitigations:
- children return structured artifacts and file references, not prose;
- a parent may hand a child read access to the full trace (Cognition's "share full traces"), which is a capability grant, not a message.

**Read/write asymmetry.** Most observed multi-agent failures (Flappy Bird, file overwrites) come from *parallel writers*. Parallel *readers* with one writer (the parent integrates) avoid most of them. This supports ADR-011's "members read the home, never write the workspace" as the right default.

**Verdict.**
- The trusted base should provide one primitive: authenticated, bounded, point-to-point mailboxes between related agents, with Root-observable metadata.
- Blackboards, task lists, debate and voting are *conventions the model builds* (plastic, `agents/**` or T1 extensions). Under the bitter-lesson lens, hard-coding one substrate is the mistake, because a stronger model will pick the substrate per task.

### Gaps
- No controlled study was found comparing mailbox, blackboard and task-list coordination for LLM agents across agent counts. The degradation analysis above is reasoning, not measurement.
- MAST's 14 individual mode names were not retrieved (see the paper's appendix).

---

## 5. Capability-based security for delegation

### Takeaway
Object capabilities are the right authorization model for sub-agents.
- **Authority is a held, unforgeable reference.** Delegating means passing a reference, and every delegation can only *attenuate*. Ambient authority (a sub-agent inheriting everything its parent can do) is what produces confused deputies.
- **Mature substrates:**
  - OS level: seL4 (formally verified) and Capsicum (shipped in FreeBSD);
  - tokens: macaroons, Biscuit, UCAN;
  - identity, a complement: SPIFFE.
- **2025–2026 agent work:**
  - CaMeL applies capability-style provenance tags to data flow;
  - MCP's 2025-11-25 spec adds incremental, audience-bound OAuth scopes;
  - DeepMind's "Intelligent AI Delegation" (Feb 2026) and CAPMAS (Sep 2026) propose macaroon-style attenuable delegation tokens for agents.
- **Verdict: highest future-proofness.** A stronger model is a more capable confused deputy and a more valuable target for prompt injection, so least authority matters more as models improve, not less.

### Cited Findings
**Theory**
- **Hardy (1988), "The Confused Deputy":** a compiler with authority to write a billing file was tricked into overwriting it through a user-supplied output path. The fix is to make designation and authority travel together. — [Hardy 1988, ACM SIGOPS OSR](https://dl.acm.org/doi/10.1145/54289.871709)
- **Mark S. Miller, "Robust Composition"** (PhD thesis, 2006):
  - unifies access control and concurrency control in the object-capability model (the E language: event-loop actors, promises);
  - "only connectivity begets connectivity": an object gets authority only by creation, by parenthood, or by being handed a reference;
  - attenuation is done by wrapping a reference in a facet that forwards a subset of operations.
  - Source: [Miller thesis](http://www.erights.org/talks/thesis/)

**OS substrates**
- **seL4:** a microkernel with a machine-checked proof of functional correctness (Klein et al., SOSP 2009). All authority is mediated by capabilities, which can be copied with diminished rights (mint). — [seL4 about](https://sel4.systems/About/)
- **Capsicum** (Watson et al., USENIX Security 2010): adds capability mode to POSIX. After `cap_enter()`, a process loses access to global namespaces and acts only through rights-limited file descriptors. It ships in FreeBSD. — [Capsicum project page](https://www.cl.cam.ac.uk/research/security/capsicum/)
- **SPIFFE** gives workloads attested, verifiable identities (SVIDs). It answers *who* a workload is, not *what it may do*, so it pairs with capability tokens rather than replacing them. — [SPIFFE overview](https://spiffe.io/docs/latest/spiffe-about/overview/)

**Token substrates**
- **Macaroons** (Birgisson et al., NDSS 2014):
  - bearer tokens whose holder can append *caveats* via chained HMACs, so anyone can attenuate offline but nobody can widen;
  - third-party caveats require "discharge" macaroons.
  - Source: [Google Research: Macaroons](https://research.google/pubs/macaroons-cookies-with-contextual-caveats-for-decentralized-authorization-in-the-cloud/)
- **UCAN:** delegation chains in which each link must be equal to or narrower than its proof. — [UCAN spec](https://github.com/ucan-wg/spec)
- **Biscuit:** Ed25519 public-key verification, Datalog policies, offline attenuation blocks. — [biscuitsec.org](https://www.biscuitsec.org/)
- **Critiques for agent use** (AIP paper, arXiv 2603.24775, March 2026):
  - macaroons are symmetric (every verifier holds the minting key) and bearer (not bound to a holder key);
  - their key–value caveats are "insufficient for expressing conditional policies (temporal bounds, budget limits, tool-parameter constraints)";
  - UCAN's nested-JWT encoding grows token size with delegation depth;
  - Biscuit meets their requirements.
  - Source: [arXiv 2603.24775](https://arxiv.org/html/2603.24775v1)

**2025–2026 agent work**
- **CaMeL** (Debenedetti et al., Google, Google DeepMind and ETH; arXiv 2503.18813, March 2025):
  - a privileged LLM turns the trusted user request into a program, and a custom interpreter runs it;
  - a quarantined LLM handles untrusted data without tool access;
  - each value carries a "capability" (provenance plus allowed readers), and policies are checked before tool calls;
  - it solved 77% of AgentDojo tasks with provable security, against 84% undefended (an earlier version reported 67%).
  - Source: [arXiv 2503.18813](https://arxiv.org/abs/2503.18813); [code](https://github.com/google-research/camel-prompt-injection)
  - Enterprise follow-up: [Operationalizing CaMeL, arXiv 2505.22852](https://arxiv.org/html/2505.22852v1)
- **MCP authorization, 2025-11-25 revision** (secondary sources; verify at modelcontextprotocol.io):
  - OAuth 2.1 with PKCE, RFC 8707 resource indicators (audience binding against token passthrough), and RFC 9728 protected-resource metadata;
  - *incremental scope consent*: a 401 or 403 with `insufficient_scope` and the needed `scopes`, so clients start minimal and step up;
  - a later revision, 2026-07-28, reportedly adds issuer validation and deprecates dynamic client registration (unverified).
  - Downstream delegation (RFC 8693 token exchange) is outside the spec, per one vendor reading.
  - Sources: [MCP 2025-11-25 summary (glama mirror)](https://glama.ai/mcp/servers/@chrishayuk/chuk-mcp-server-reference/blob/9ddab97f9b578b38cce2687dadabf9c33da69271/specs/2025-11-25/README.md); [Keycloak MCP integration](https://www.keycloak.org/securing-apps/mcp-authz-server); [wavect enterprise MCP authz](https://wavect.io/de/blog/enterprise-mcp-authorization-architecture.md)
- **"Intelligent AI Delegation"** (Tomašev, Franklin, Osindero, Google DeepMind; arXiv 2602.11865, 2026-02-12): treats delegation as a sequence of decisions about task allocation, transfer of authority and responsibility, role boundaries, clarity of intent, and trust and monitoring. Per the AIP paper it proposes macaroon-based "Delegation Capability Tokens", but it is a position paper with no shipped protocol. — [arXiv 2602.11865](https://www.arxiv.org/abs/2602.11865); [AIP paper's characterisation](https://arxiv.org/html/2603.24775v1)
- **CAPMAS** (arXiv 2609.06500, Sep 2026): macaroon-based privilege delegation among agents, with each token pre-scoped from the user's natural-language request. The authors claim no prior practical architecture combined offline delegation with semantic request bounding (their claim, unverified). — [arXiv 2609.06500](https://arxiv.org/pdf/2609.06500)
- **IETF draft-asor-wimse-agent-delegation-chain:** proposes JOSE/OAuth-based chained delegation with a subsumption-checking verifier, meaning offline attenuation without a shared secret. — [IETF draft -01](https://apache.otenet.gr/doc/internet-drafts/draft-asor-wimse-agent-delegation-chain-01.xml)
- **Claude Code** (docs fetched 2026-10-22):
  - a subagent's `tools` is an allowlist and `disallowedTools` a denylist; with no `tools` field, a subagent "inherits every tool available to subagents";
  - permission mode is inherited, and in bypass or auto mode the parent's mode overrides the child's setting;
  - `isolation: worktree` refuses commands that resolve into the main checkout;
  - "No message from any agent counts as your approval".
  - The default is therefore ambient inheritance with optional attenuation, not attenuation by construction.
  - Source: [Claude Code docs: sub-agents](https://code.claude.com/docs/en/sub-agents)

### Inferences
- **The four delegated dimensions** (tools, paths, network, money) map onto capabilities as follows:
  - **Tools:** a set; the child's set is a subset of the parent's.
  - **Paths:** read and write sets, enforced by the OS sandbox (Capsicum- or Seatbelt-style), never only by the harness. The child's set is a subset of the parent's.
  - **Network:** egress classes or hosts; the child's is a subset of the parent's.
  - **Money:** *not* a set but a conserved quantity, so it needs carving (§7), not just a subset check. The AIP critique is right that plain macaroon caveats cannot express it well. It needs a stateful issuer that tracks outstanding reservations, which is exactly what a Root egress ledger is.
- **Two different "capability" ideas:**
  - CaMeL's capabilities are *information-flow labels on data*;
  - ocap and macaroon capabilities are *authority on actions*.
  - A sub-agent system needs authority capabilities for delegation. Information-flow labels matter for the prompt-injection case: a sub-agent that read untrusted web content should not drive a write.
  - Speculation: a "tainted child cannot command a clean parent" rule is the sub-agent analogue of CaMeL's quarantined LLM.
- **Where tokens are needed.** In a single-machine harness with a trusted spawn broker, tokens are unnecessary. The broker holds a capability table per instance, as in seL4's CSpace, and checks every spawn and every egress against it. Macaroon, Biscuit or UCAN tokens matter only when delegation crosses trust domains (remote agents, MCP servers).
- **Verdict: very high future-proofness.** Least authority is independent of model capability. A more capable delegatee can do more harm with ambient authority. The bitter-lesson lens favours it, because it constrains *reach*, not *strategy*.

### Gaps
- No production deployment of attenuable capability tokens for LLM sub-agents was found. DeepMind's work is a position paper, and CAPMAS and AIP are preprints.
- The exact MCP 2025-11-25 and 2026-07-28 wording on delegation was not retrieved from modelcontextprotocol.io.

---

## 6. Workflow-as-code and deterministic orchestration (Temporal, CodeAct, model-written orchestration)

### Takeaway
"The model writes a deterministic orchestration program and a trusted runtime executes it, with event-sourced replay" is a durable pattern, and in 2026 it is already shipped. Claude Code's dynamic workflows:
- make `Date.now()` and `Math.random()` throw so a relaunch replays the same `agent()` calls;
- cache completed agents' results;
- cap a run at 16 concurrent and 1,000 total agents.
It joins two proven lines: Temporal-style durable execution, where deterministic workflow code is replayed from a history and side effects such as LLM calls run as activities, and code as the action language (CodeAct, smolagents, "code execution with MCP"). It is safe because the runtime, not the script, holds authority. The script is a plan, and every agent it spawns goes through the same permission and sandbox checks.

### Cited Findings
**Temporal**
- Workflow code must be deterministic: on recovery the runtime replays the event history and re-executes the code to rebuild state.
- Non-deterministic work (network calls, LLM calls) goes in activities, whose results are recorded in the history.
- Child workflows have a *Parent Close Policy* (terminate, request-cancel or abandon), which is an explicit choice about structured lifetimes.
- Sources: [Temporal docs: workflows and determinism](https://docs.temporal.io/workflows); [Temporal docs: child workflows](https://docs.temporal.io/child-workflows)
- **Temporal and OpenAI Agents SDK integration** (public preview announced 2025-07-30; 15 months old): agent runs become durable workflows, model calls run as activities, and `activity_as_tool` wraps activities as tools, so agents survive crashes, rate limits and network failures. — [Temporal blog](https://temporal.io/blog/announcing-openai-agents-sdk-integration); [BusinessWire, 2025-07-30](https://www.businesswire.com/news/home/20250730783559/en/Temporal-and-OpenAI-Launch-Integration-for-Enterprises-Developing-Production-Agents); [InfoQ, Sep 2025](https://www.infoq.com/news/2025/09/temporal-aiagent)

**Code as the action and orchestration language**
- **CodeAct** (Wang et al., ICML 2024): executable Python actions instead of JSON tool calls gave up to 20% higher success rates. — [arXiv 2402.01030](https://arxiv.org/abs/2402.01030)
- **smolagents CodeAgent** (Hugging Face, Dec 2024): writes actions as code. The launch post reports about 30% fewer steps (LLM calls) than JSON tool calling. — [HF blog: smolagents](https://huggingface.co/blog/smolagents)
- **DSPy** (Khattab et al. 2023): LM pipelines as declarative programs that a compiler optimizes. — [arXiv 2310.03714](https://arxiv.org/abs/2310.03714)
- **Anthropic, "Code execution with MCP"** (2025-11-04): agents write code that calls MCP tools exposed as files in a sandbox, so intermediate results stay out of context.
  - A cited example drops from 150,000 to 2,000 tokens (98.7%). That number is secondhand.
  - An independent AIMultiple benchmark found input tokens fell 78.5%, output tokens rose 120% and latency rose 7%, at equal success.
  - Anthropic's Programmatic Tool Calling measured a 37% average token cut.
  - Sources: [Anthropic Engineering](https://www.anthropic.com/engineering/code-execution-with-mcp); [AIMultiple benchmark](https://research.aimultiple.com/code-execution-with-mcp)

**Claude Code dynamic workflows** (docs fetched 2026-10-22)
- **What it is:** "A workflow moves the plan into code." Claude writes a JavaScript script with `agent()`, `parallel()`, `pipeline()` and `phase()`, and a runtime executes it in an isolated environment. Intermediate results stay in script variables, not in Claude's context.
- **Determinism and replay:**
  - `Date.now()`, `Math.random()` and a no-argument `new Date()` throw "so that a relaunched run repeats the same `agent()` calls";
  - on relaunch, completed agents return saved results. The first agent whose prompt differs, and every agent after it, re-runs, as does every agent after a failed one.
- **Limits:**
  - no shell or filesystem access and no module loading in the script itself;
  - up to 16 concurrent agents, configurable from 1 to 256;
  - 4,096 items per `parallel()` or `pipeline()` call;
  - 1,000 agents per run ("Prevents runaway loops");
  - stalled agents restart up to 5 times.
- **Budget signals:**
  - an advisory `Large workflow` warning above 25 agents or 1.5M projected tokens ("doesn't pause or limit the run");
  - a usage-limit pause and resume;
  - a size guideline that is "advice, not a cap".
- **Authority stays with the runtime:**
  - spawned agents get "the same permission checks and sandboxing as any other tool call";
  - in auto mode, the prompt a script passes to `agent()` "doesn't count as a request from you", because it is script-computed text.
- **Model view:** the table contrasts "Who decides what runs next": Claude turn by turn (subagents, teams) against "The script" (workflows), which scales to "dozens to hundreds of agents per run".
- Source: [Claude Code docs: workflows](https://code.claude.com/docs/en/workflows)

### Inferences
- **The pattern separates three things:**
  - *plan* (model-written code; plastic, inspectable, diffable, rerunnable);
  - *execution* (trusted runtime: replay, caps, scheduling);
  - *authority* (permission and sandbox checks on every spawned agent).
  
  That is the Temporal split (workflow, worker, server) with the LLM writing the workflow. It is future-proof under the bitter-lesson lens: the stronger the model, the better the programs it writes, and the base never fixes a topology.
- **Determinism belongs to the orchestrator, not the agents.** Agents are activities with recorded results. The replay rule ("first divergent prompt and everything after re-runs") is a conservative, sound cache. It wastes work after a mid-fan-out failure, which Claude Code documents.
- **Caveat: workflow-as-code is a *layer*, not a foundation.** Its safety comes entirely from the primitives it calls (spawn, mailboxes, budgets, sandboxes). A model can write a workflow runtime as a T1 or T2 capability in INIT's plastic surface, provided the spawn primitive below it enforces the invariants. Making the workflow runtime part of the trusted base would hard-code one orchestration style.
- **Verdict: proven (Temporal in production for years, Claude Code shipping in 2026) and future-proof as a model-side tool.** Its weak point is cost: replay after a failure re-runs completed downstream work, and Claude Code's 1.5M-token warning is advisory. The hard bound must still come from below.

### Gaps
- The Temporal and OpenAI integration's GA status in 2026 was not verified (it was a public preview in mid-2025).
- No benchmark was found comparing model-written workflow scripts with turn-by-turn orchestration on cost and quality.

---

## 7. Budget propagation: hierarchical quotas, deadline and cancellation propagation

### Takeaway
Infrastructure has long settled on hierarchical carving: Linux cgroups v2 distributes resources top-down, Go and gRPC propagate the earliest deadline and cancellation, and Kubernetes caps aggregates per namespace. "A child's budget is carved out of its parent's remaining budget, the sum of outstanding child reservations never exceeds the parent's remainder, and unspent reservations return at scope exit" is the right invariant for money, tokens, wall clock and spawn count. Agent harnesses mostly do not enforce it today:
- the Claude Agent SDK has one session-wide `max_budget_usd` that reportedly includes subagent spend;
- Claude Code subagents have no per-subagent dollar budget, only `maxTurns`, model and effort;
- workflow cost warnings are advisory;
- INIT's per-member shares are soft in the kernel proxy, and only the zone total is hard at the Root.

### Cited Findings
**Infrastructure precedents**
- **cgroups v2:** resources are distributed top-down. A cgroup can further distribute a resource only if its parent distributed it to it, and limits nest. `pids.max` bounds the number of processes in a subtree (a fork-bomb guard); `memory.max` and `cpu.max` bound the rest. — [Linux cgroup v2 docs](https://docs.kernel.org/admin-guide/cgroup-v2.html)
- **Go context:**
  - "When a Context is canceled, all Contexts derived from it are also canceled";
  - "If the parent's deadline is already earlier than d, WithDeadline(parent, d) is semantically equivalent to parent", so a child deadline is the minimum of its own and its parent's.
  - Source: [pkg.go.dev/context](https://pkg.go.dev/context)
- **gRPC:** recommends propagating deadlines to downstream calls, so a server that calls other services passes its remaining deadline down; cancellation propagates the same way. — [gRPC deadlines guide](https://grpc.io/docs/guides/deadlines/)
- **Kubernetes:** a ResourceQuota caps aggregate consumption per namespace, and a LimitRange sets per-pod or per-container defaults and min/max. — [ResourceQuota](https://kubernetes.io/docs/concepts/policy/resource-quotas/); [LimitRange](https://kubernetes.io/docs/concepts/policy/limit-range/)

**Agent harnesses today**
- **Claude Agent SDK:**
  - `max_budget_usd` (Python SDK since 0.1.6) ends a session with `error_max_budget_usd` when exceeded, and the docs call it "a good default for production agents";
  - a third-party guide says it counts subagent spend, so the budget is shared session-wide, not carved per subagent, and lists separate caps for nesting depth and concurrency;
  - the subagent-spend detail is secondary.
  - Sources: [Agent SDK agent loop docs](https://code.claude.com/docs/en/agent-sdk/agent-loop); [SDK example max_budget_usd.py](https://git.joshthomas.dev/mirrors/claude-code-sdk-python/src/branch/main/examples/max_budget_usd.py); [developersdigest, 2026](https://www.developersdigest.tech/blog/claude-agent-sdk-whats-new-2026)
- **Claude Code subagents** (2026-10-22):
  - no per-subagent dollar budget; a subagent's requests "count toward the same usage limits as your main conversation";
  - controls are `maxTurns`, model, effort and `autoCompactWindow`;
  - structural bounds: nesting up to 3 layers below the main conversation by default (`CLAUDE_CODE_MAX_SUBAGENT_SPAWN_DEPTH`; set it to 1 to disable nesting), 20 concurrent subagents by default (`CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS`), and "no limit on the total number of subagents… over a session".
  - Source: [Claude Code docs: sub-agents](https://code.claude.com/docs/en/sub-agents)
- **Claude Code workflows:** 1,000 agents per run and 16 concurrent (hard); the 25-agent / 1.5M-token warning and size guideline are advisory. — [Claude Code docs: workflows](https://code.claude.com/docs/en/workflows)
- **Anthropic research system (Jun 2025):**
  - effort scaling was *in prompts*: 1 agent with 3–10 tool calls for simple queries, 2–4 subagents for comparisons, more than 10 for complex research;
  - early versions spawned "50 subagents for simple queries";
  - multi-agent runs cost about 15× chat tokens.
  - Source: [Anthropic Engineering](https://www.anthropic.com/engineering/multi-agent-research-system)
- **RLM (Dec 2025):** a single query can fan out into dozens or hundreds of sub-calls, per commentary. The authors claim comparable or lower cost per query. — [arXiv 2512.24601](https://arxiv.org/html/2512.24601v1); [DeepLearning.AI, The Batch](https://www.deeplearning.ai/the-batch/recursive-language-models-offer-path-to-dramatically-expand-beyond-the-context-window)

**INIT today**
- Each fleet member has a `member` proxy scope with a daily share (`budget_share_musd_day`). It is enforced softly by the kernel proxy, clamped below the Root's money ceilings, while "the Root's egress enforces the hard total". — `docs/kernel/supervise.md` §3.14 (local)
- The Root egress treats `x-egress-scope`, `x-egress-body` and `x-egress-reservation` as "labels for the ledger only, never inputs to a ceiling or class decision". — `docs/root/egress.md` line 241 (local)

### Inferences
- **The carving invariant, stated precisely (proposal):** for every node p with children c:
  - `reserve(c) ≤ remaining(p) − Σ outstanding reservations of p's other children`;
  - `spend(c) ≤ reserve(c)`;
  - `deadline(c) = min(deadline(p), requested)`;
  - `depth(c) = depth(p) + 1 ≤ D`;
  - `live_children(p) ≤ F`;
  - `subtree_spawns(p) ≤ S`;
  - at scope exit, `remaining(p) += reserve(c) − spend(c)`.
  
  This is cgroups v2 plus Go context, applied to dollars and model tokens.
- **Why it beats one shared pool** (Claude Agent SDK style): with a shared pool, one runaway child can starve its siblings and its parent's own completion step, leaving the parent no budget to integrate or report. Carving guarantees the parent keeps a reserve. The Anthropic "50 subagents" episode is evidence that prompt-level effort rules are not enough.
- **Who must enforce it.** Only the component that meters the spend can enforce carving: the egress or ledger. If the orchestrator (the kernel) is plastic or untrusted, per-subtree budgets in the kernel are soft by construction. INIT has exactly this property today, since egress attribution headers are labels only.
- **Verdict: very high future-proofness.** Conservation of money is a law the owner wants regardless of model capability. The model chooses how to *split* the budget, and the base only guarantees the split cannot exceed the whole. That is the bitter-lesson-friendly division.

### Gaps
- Official Anthropic documentation stating that `max_budget_usd` includes subagent spend was not retrieved; only a third-party guide says so.
- No 2025–2026 harness was found with hard, per-subtree dollar carving. This looks like an open niche, but absence of evidence is not proof.
- The Kubernetes Hierarchical Namespace Controller (hierarchical quotas) has an unverified 2026 status, so it is not cited as a precedent.

---

## 8. Recursion and self-similarity: agents that run the same harness

### Takeaway
Recursive self-delegation, where an agent spawns agents that run the same harness and may spawn further, has moved from demo to shipped default:
- ReDel (EMNLP 2024 demo);
- Recursive Language Models (Dec 2025): the model decomposes its own context in a REPL and calls itself;
- OpenAI's "agents as tools";
- Claude Code, which since 2026 lets subagents nest 3 levels by default, with 20 concurrent and no session-wide total cap.
It needs hard bounds on depth, fan-out, total spawns and budget, which is exactly what Unix process trees plus cgroups (`pids.max`) provide. Self-similarity is the property that makes the model the topology designer: if every node has the same spawn primitive and the base checks each spawn locally, any tree the model builds is safe by induction.

### Cited Findings
**Research systems**
- **ReDel** (Zhu, Dugan, Callison-Burch, UPenn; EMNLP 2024 demo):
  - a toolkit for recursive multi-agent systems in which "models decide for themselves when to delegate and how to structure delegation";
  - delegation is a tool that spawns a sub-agent and manages its lifecycle;
  - its docs say the root agent tends to *overcommit* (do work itself instead of delegating) and recommend giving the root no tools.
  - Sources: [ACL Anthology](https://aclanthology.org/2024.emnlp-demo.17); [arXiv 2408.02248](https://arxiv.org/html/2408.02248v1); [ReDel docs](https://redel.readthedocs.io/en/stable/_sources/redel.rst.txt)
- **Recursive Language Models** (Zhang, Kraska, Khattab, MIT; arXiv 2512.24601, Dec 2025):
  - the prompt is a variable in a Python REPL, and the root model (depth 0) probes and slices it with code and calls sub-LM instances (depth 1) on pieces;
  - reports handling inputs "up to two orders of magnitude beyond model context windows";
  - on OOLONG-Pairs at 32k tokens, RLM-GPT-5 reached 58% against about 0% for GPT-5;
  - post-trains a first small "natively recursive" model.
  - Source: [arXiv 2512.24601](https://arxiv.org/html/2512.24601v1); [DeepLearning.AI](https://www.deeplearning.ai/the-batch/recursive-language-models-offer-path-to-dramatically-expand-beyond-the-context-window)
- **OpenAI Agents SDK:** an agent can be exposed as a tool to another agent (`as_tool`) or receive a handoff. Recursion is possible when an agent includes itself or a clone among its tools. — [OpenAI Agents SDK: tools](https://openai.github.io/openai-agents-python/tools/)

**Claude Code** (docs fetched 2026-10-22)
- "By default, a subagent can spawn subagents of its own, up to three layers below the main conversation. At the depth limit, Claude Code withholds the `Agent` tool from every subagent except a fork."
- 20 concurrent subagents by default, and no limit on the total per session.
- *Forks* inherit the whole conversation and the parent's exact tool pool, and are "never blocked by the limit".
- Agent teams do not nest.
- Sources: [sub-agents](https://code.claude.com/docs/en/sub-agents); [agent teams](https://code.claude.com/docs/en/agent-teams)

**Classic process trees**
- Unix `fork`/`exec` builds a tree: every process has a parent, the parent learns of a child's death via `wait`, and orphans are re-parented to init.
- cgroups v2 attaches resource limits to subtrees, including `pids.max`, which caps the total process count of a subtree. — [cgroup v2 docs](https://docs.kernel.org/admin-guide/cgroup-v2.html)

**INIT today**
- "Members cannot create members, so the recursion is bounded" (depth 1).
- `parallel_bodies` defaults to 4, and exceeding it is a demoting refusal.
- ADR-011 allows members at L0 and keeps "Members still cannot create members".
- Sources: `docs/kernel/supervise.md` §3.14; `docs/adr/011-reward-free-self-extension.md` §4.5 (local)

### Inferences
**Which bounds are needed:**
- **Depth D:** guards against unbounded recursion and the loss of information through summaries at each hop. Claude Code uses 3 and RLM's published experiments use 1.
- **Live fan-out per node F:** guards against a context and coordination explosion.
- **Global live count:** INIT's `parallel_bodies`, which also caps RAM on a 16 GB machine.
- **Total spawns per subtree or time window:** guards against fork-bomb-like churn. Claude Code's per-session total is *unbounded*; INIT's H6 (more than 120 zone spawns in 60 min) is a sentinel, not a cap.
- **Carved budget:** from §7.

**Why self-similarity matters for "the model designs the topology."** If the spawn check is local and compositional (child ⊆ parent on capabilities, budget conserved, depth+1 ≤ D), then *any* tree the model builds satisfies the global invariants by induction. The base never needs to understand the topology, which is the bitter-lesson property. A non-self-similar design breaks this: special "orchestrator" and "worker" kinds with different rules hard-code a two-level topology, as Claude Code's non-nesting teams and INIT's member-cannot-spawn rule both do today.

**Exceptions break the induction.** Claude Code's forks bypass the concurrency limit and get the parent's *exact* tool pool, which is benign in a single-user CLI. In a self-modifying harness, any spawn path that bypasses the check, or inherits rather than attenuates, is where the invariant leaks.

**Verdict.**
- Proven in classic process trees and only emerging in LLM agents (2024–2026 demos and recent product defaults).
- Highly future-proof, because recursion is the general mechanism and fixed topologies are the special cases.
- Speculation: as models get better at decomposition (RLM's "natively recursive" training), the useful depth and fan-out should grow, so D, F and S should be charter values the owner can raise, not code constants.

### Gaps
- No published measurement was found of failure or cost scaling against recursion depth for LLM agents. RLM reports depth 1, and Claude Code's depth-3 default is not justified in its docs.
- The ReDel paper's overcommitment and undercommitment rates were not retrieved.

---

## 9. Synthesis: ranking and the composite concept

### Takeaway
Rank by proven track record, by future-proofness under the bitter lesson (more freedom under hard invariants), and by fit for a harness whose trusted base enforces money and scope. The winners are the *mechanism-only* concepts:
- capability attenuation;
- budget carving;
- structured lifetimes;
- process trees with bounds;
- supervision with restart intensity.

They compose into one recommendation: **supervised, capability-attenuated, budget-carved recursive actors with structured lifetimes**, more compactly a **capability-carved process tree**. The trusted base enforces a small set of *local, compositional spawn and egress checks*, and the model chooses everything else, including writing orchestration programs (workflow-as-code) that run on the same primitive.

### Cited Findings
- **The bitter lesson** (Sutton, 2019): general methods that leverage computation beat methods that build in human knowledge of how the domain works. — [Sutton, The Bitter Lesson](http://www.incompleteideas.net/IncIdeas/BitterLesson.html)
- **Empirical stance of practitioners:**
  - Anthropic recommends starting simple and adding orchestrator-worker multi-agent patterns only when they pay off. — [Building effective agents, Dec 2024](https://www.anthropic.com/engineering/building-effective-agents)
  - Cognition: single-threaded by default (Jun 2025). — [Cognition](https://cognition.com/blog/dont-build-multi-agents)
  - MAST: most failures are system design and coordination. — [arXiv 2503.13657](https://arxiv.org/html/2503.13657v3)
  - All of these argue against hard-coding complex topologies in the base.

### Inferences
**Ranking** (5 = best; the fit column assumes a Root that owns money, scope, kill and lineage):

| Concept | (1) Proven | (2) Future-proof | (3) Fit for a trusted base | Who owns it |
|---|---|---|---|---|
| Capability attenuation (ocap, seL4, Capsicum; tokens only across domains) | 4: verified kernels, FreeBSD; agent use is preprints | 5: more capable delegatees need less ambient authority | 5: maps onto broker and sandbox parameters | **Base** |
| Budget carving and deadline propagation (cgroups v2, Go context, gRPC) | 5 | 5: conservation is owner policy, not model policy | 5: the Root already meters egress | **Base** (the split is the model's) |
| Process tree with depth, fan-out and total bounds (fork, `pids.max`) | 5 | 5: bounds are charter values that can grow | 5: the Root already brokers every spawn | **Base** |
| Structured lifetimes (nurseries, TaskGroup, StructuredTaskScope) | 4: mainstream since 2018–2022; Java final only in March 2027 | 5 | 4: needs a two-tier stop (cooperative, then kill) | **Base** (containment and refund); model picks blocking or async |
| Supervision trees with restart intensity and escalation (OTP) | 5: decades in telecom | 4: the mechanics hold; the *policy* should be the model's | 4: intensity and escalation in the base, strategy in the model | Split |
| Mailboxes (point-to-point, authenticated sender) | 5: Erlang, actors | 4 | 4: needs Root-visible provenance | Base primitive; protocol plastic |
| Workflow-as-code and durable replay (Temporal, Claude Code workflows) | 4: Temporal in production; Claude Code shipping in 2026 | 5 as a model tool | 3 as base (hard-codes a style); 5 as a plastic layer | **Model** (T1/T2) |
| Fork-join, map-reduce, best-of-N with verifiers | 5 (MapReduce); 4 (verifiers in code and math) | 3–4: depends on oracle availability | 3: the base should offer oracles, not the pattern | **Model**; base supplies cheap oracles |
| Blackboard or shared task list | 2–3: Hearsay-II is historic; 2025 papers show modest gains | 3 | 2 as base | **Model** convention |
| CaMeL-style information-flow tags | 3: one benchmark, 77% against 84% | 4 | 3: a plastic body-level defence; hard to make Root-enforced | Model and body; base can taint-track at egress (speculation) |

**Composite A, the foundation (Root-enforced): the capability-carved process tree.** Every agent instance is a node with:
1. **One parent and a structured lifetime.** When the parent's scope closes (exit, crash past restart intensity, kill, or the Root's `stop`), all descendants are stopped, cooperatively first and then by kill, and their unspent reservations return to the parent. Long-lived "daemon" children exist only as children of a Root-owned scope granted by the charter, never as unscoped detaches.
2. **An attenuated capability set** that is a subset of the parent's:
   - a tool subset;
   - read and write path sets (the sandbox profile is a pure function of them);
   - egress classes;
   - workspace-write (a grant only);
   - spawn rights (D_remaining, F, S).
3. **A carved budget:** USD, wall-clock deadline and spawn count, reserved from the parent's remainder and metered by the Root at egress against the *instance*, not against a label.
4. **Restart intensity and escalation:** more than R crashes in T seconds stops the node and escalates to the parent as a `DOWN` signal. A repeated pattern at the top becomes a Root harm signal.
5. **Authenticated mailboxes** to the parent and children (and peers, if the parent granted the address). The sender's identity is stamped by the base, so no agent can impersonate the owner or the parent. This is Claude Code's "messages from agents are not your approval" rule, made structural.
6. **A signed lineage record per spawn and exit:** parent, child, capability digest, reservation, and spent-versus-refunded.

Every check is local to a spawn or egress event, so arbitrary model-designed trees stay within the global invariants by induction. This is the self-similar property the owner wants.

**Composite B, the model's tool (plastic): plans as programs.**
- The model may write deterministic orchestration scripts (fork-join, map-reduce with verifiers, debates, blackboards, task lists) that a replaying runtime executes, as Temporal and Claude Code workflows do.
- The runtime is itself a capability inside the plastic surface. Every `agent()` it calls is a Composite A spawn, so a script cannot exceed its own node's capability or budget.
- The script gives repeatability, inspectability and context savings. Composite A gives safety.

**What the trusted base enforces:**
- monotone attenuation;
- budget conservation and refund;
- deadline = min;
- bounds on depth, fan-out and total spawns;
- containment of descendants' lifetimes, with a two-tier kill;
- restart-intensity caps;
- sender authentication on messages;
- Root-measured metering per instance;
- signed lineage;
- the global ceilings and stop.

**What the model chooses freely:**
- whether to delegate at all (Cognition's default of not delegating stays available);
- topology: depth up to D, fan-out up to F, recursion, peers or tree;
- roles, prompts, model routing and effort per child;
- how to split the budget among children;
- which capability subset to pass down;
- blocking or asynchronous joins;
- the supervision strategy (one_for_one, rest_for_one, one_for_all) within the intensity cap;
- the coordination convention (mailbox protocol, blackboard, task list);
- the verification strategy (best-of-N, adversarial review, tests);
- writing and saving orchestration scripts.

### Gaps
- The ranking scores are judgments built on the cited evidence, not measurements.
- No system was found that implements all of Composite A for LLM agents. Claude Code has depth and concurrency bounds and tool allowlists, but shared rather than carved budgets, inherited rather than attenuated permission modes, and background re-parenting. Composite A is therefore an assembly of individually proven parts, not a proven whole.

---

## Implications for INIT

### Takeaway
INIT already has the hard parts of Composite A in the right place:
- the Root is the single spawn broker (Seatbelt cannot nest, so every instance goes through the Root's `exec-clean` and profile composition);
- the Root holds the money ceilings, kill and signed lineage;
- the Root's harm sentinel watches spawn churn (H6).

Three things are missing:
1. **Per-instance hard metering.** Today egress attribution headers are "labels… never inputs to a ceiling", so per-member shares are soft in a kernel that ADR-011 makes plastic.
2. **A parent–child relation in the broker, with attenuation and containment checks.**
3. **Self-similar spawning.** "Members cannot create members" hard-codes depth 1.

The recommendation is to move these three into the Root as local, compositional checks, and to leave topology, roles, supervision strategy and orchestration scripts on the plastic surface (the `agents/**` T0 specs, plus a T1/T2 workflow runtime the agent can build itself).

### Cited Findings
- **Root as broker:** the Root's spawn broker launches every zone process "because Seatbelt cannot nest". It renders the composed profile, creates the mark, launches through `exec-clean`, records the instance and reaps it. The kernel keeps every decision about what to run. — `docs/kernel/supervise.md` §2 item 2, §3.4 (local)
- **Fleet today:**
  - each member has its own root `fleet/<bid>/` under the body profile, and no body can see another's root;
  - each member has a `member` proxy scope with `budget_share_musd_day`, enforced softly in the proxy, with the hard total at Root egress;
  - `member_crash_limit` = 3 in 600 s;
  - `member_idle_stop_s` = 3,600;
  - "Members cannot create members";
  - exceeding `parallel_bodies` (default 4) is a demoting refusal.
  - Source: `docs/kernel/supervise.md` §3.14, §3.7 (local)
- **ADR-011:**
  - sub-agent specs `agents/<name>/AGENT.md` (role prompt, tool subset, model route, budget share) are T0 from L0, and members start with `fleet spawn{agent}`;
  - members "read the home, never write the workspace"; workspace-writing members are "grant only";
  - `fleet spawn` drops its L2 requirement, but "Members still cannot create members";
  - `ceiling.background` (30% of `money.day_usd` at L0) bounds spend outside owner turns;
  - the kernel is plastic from L1, and nothing safety-relevant may depend on its honesty;
  - H6 rolls back on more than 120 zone spawns in 60 min.
  - Source: `docs/adr/011-reward-free-self-extension.md` §1.1, §1.2, §3.3, §4.5 (local)
- **Egress labels:** `x-egress-scope`, `x-egress-body` and `x-egress-reservation` "are labels for the ledger only, never inputs to a ceiling or class decision". — `docs/root/egress.md` line 241 (local)

### Inferences
**Mapping Composite A onto INIT.** These are proposals, labelled R- for Root, K- for kernel and B- for body.

**R1. Spawn tree in the broker.**
- Every `spawn{kind:"body"}` names `parent: <instance>`. The Root records a tree in its instance table (`state/root.json`) and in lineage as a `spawn` record carrying parent, child, capability digest and reservation.
- The primary is the root node at depth 0. The Root rejects a spawn whose parent is not a live body instance.
- A hostile plastic kernel could lie about the parent. That is acceptable: misattribution can only move budget *within* the zone total, and every node's capabilities are still checked against the claimed parent's. For this to hold, R2 and R3 must be checked against the parent's recorded set, not the request.

**R2. Monotone attenuation by profile parameters.**
- The child's Seatbelt parameters must be a subset of the parent's: read set, write set, egress classes, tool classes the Root can see, and `workspace_write`.
- Because the Root composes every profile from parameters, attenuation becomes a parameter-subset check at V1–V6 time. This is seL4's mint-with-fewer-rights, done in the broker.
- The `agents/<name>/AGENT.md` tool subset stays a body or kernel convention for tools the Root cannot observe. Whatever the Root *can* observe (paths, network, egress class, spawn rights) becomes Root-enforced.

**R3. Hard budget carving at egress.**
- Promote `x-egress-body` from label to *key*, but only once the Root can verify which instance sent a request:
  - either a per-instance egress socket handed over by the broker, with the Root mapping socket to instance;
  - or the Root-written macOS mark already used for initctl peer attribution.
  
  The verification method is speculation and needs design. Without verification, per-subtree budgets stay soft in a kernel that is plastic from L1.
- Then keep in the Root ledger, per instance: `reserve`, `spent`, and `Σ children's reserves`, with:
  - refusal when `spent + Σ child reserves + request estimate > reserve`;
  - refund at child exit;
  - `deadline = min(parent's, requested)`.
- The zone-wide `money.day_usd` and `ceiling.background` remain the outer ceiling, so carving only ever *tightens*.

**R4. Structured lifetimes.**
- When an instance exits or is killed, the Root stops all its descendants: cooperative `stop` with a grace period first (the kernel's §3.8.3 path), then `kill` sweep after the grace.
- It appends `exited` events bottom-up and settles reservations.
- Members surviving their parent is allowed only for children of the *primary*. The primary plays the Erlang top-level supervisor role, and its children are already restarted from `fleet.json` at cold start.
- This removes the ReDel-zombie and background-re-parenting class of leaks.

**R5. Bounds as charter values, scaled by level like the blast-radius budget.**
- `extend.spawn_depth` D: suggested 2 at L0, 3 at L2+, replacing "members cannot create members".
- `extend.spawn_fanout` F per node.
- `quotas.parallel_bodies` as the global live cap. 4 is RAM-bound on a 16 GB M-series machine and must be co-checked with the memory watchdog.
- A per-subtree spawn total S per hour, making H6's 120/60 min a *cap* per subtree and keeping it as a sentinel zone-wide.
- **Recommendation:** a spawn over D, F or S should be a *non-demoting* refusal (the model learns the bound). Keep demotion only for the global `parallel_bodies` breach that a correct kernel pre-checks, as today.

**R6. Restart intensity per node.**
- Generalize `member_crash_limit` (3 in 600 s) to every node.
- Exceeding it stops that node's subtree and sends a `DOWN`-style notice to the parent's mailbox, as the kernel already notifies via `send_input`.
- Only the primary's crash loop stays a system signal (H1, safe mode), as now.

**R7. Mailbox provenance.**
- Kernel-relayed `fleet send` and `send_input` frames should carry a Root-stamped or kernel-stamped sender instance id. The body treats any non-owner sender as a non-owner, mirroring Claude Code's "no message from any agent counts as your approval".
- Owner authority stays solely on the Root's owner socket, as now.

**K/B. What stays plastic (kernel from L1, body from L0):**
- supervision strategy per subtree;
- who to spawn and when;
- role specs in `agents/**`;
- mailbox protocols;
- blackboards and task lists (for example a task file under the parent's `work/`, readable by the children it grants);
- verification patterns;
- a **workflow runtime** (Composite B) built as a T1 extension or T2 module that calls `fleet spawn`, with replay from a session-local event log. Its caps (16 concurrent, 1,000 per run in Claude Code's case) are the model's choice, nested *inside* R1–R6.

**Read-only by default.** Keep ADR-011's rule that members read the home and never write the workspace. MapReduce re-execution, Cognition's conflicting-writers lesson and Claude Code's file-overwrite warning all say parallel *readers* are cheap and safe, while parallel *writers* need a grant and a single integrator (the parent).

**Lineage and the sentinel for free.**
- With a spawn tree in lineage, ADR-011's attribution and quarantine extend naturally: a sub-agent pattern is a `cap` (`agent/<name>`), its subtree spend is Root-observed, and harm correlation (Fisher test, §6.3) can treat "turn used `agent/<name>`" like any other capability.
- No LLM decides anything safety-relevant, consistent with ADR-006 decision 8 as kept by ADR-011.

**Future-proofness check against "AGI-level model, reward-free self-extension":**
- every Root rule above is a conservation or containment law, with no topology, no reward and no judgment;
- the model can grow depth, fan-out and budget only through owner grants of charter values;
- inside them it may invent any delegation scheme, including ones nobody anticipated;
- this matches ADR-011's principle: gate on generic harm and bounds, never on predefined gain.

### Gaps
- Whether the Root can verify the sending instance on the egress socket in INIT's current design (per-instance socket, peer credentials or mark) was not checked in `root/egress.md` beyond the "labels only" line. This is the critical feasibility question for R3.
- The RAM cost per member body on the owner's 16 GB machine, which sets a realistic `parallel_bodies` and D×F, was not measured here.
- The effect on Root size (ADR-011 open question 2, Root growth) of adding R1–R7 was not estimated. R1, R2, R4 and R5 extend existing broker validation. R3 is the largest addition (a per-instance reservation ledger).
