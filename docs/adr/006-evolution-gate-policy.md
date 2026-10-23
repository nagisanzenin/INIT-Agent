# ADR-006: Evolution gate policy

```
Status: draft v0.3 (reconciled R2; owner gating removed per v03-decisions D4, 2026-10-20) · 2026-10-19 · Area: evolution · Decision: proposed (statistical parts provisional until Phase 1 measures δ)
Depends on: spec v0.2 §1, §2.1, §6, §7.1, §11; evolution.md; kernel/gate.md (internals); testing.md
Defines:    the policy a self-edit must satisfy to run and to become LKG; what the gate deliberately does
            not judge; the conditions that would reopen the policy
```

## Context

INIT gives the model full freedom over *what* to change in its body. The research says that freedom does not compound without an honest mirror, and that the mirror must be outside the model's reach:

| Finding | Source |
|---|---|
| GEPA's optimized agent fell below its baseline on unseen tasks; ACE's playbook collapsed from 18,282 to 122 tokens in one rewrite | spec §1 |
| Self-generated skills averaged ≈ 0 gain; curated skills +16.6 | SkillsBench, spec §1 |
| An LLM editing a harness invented guardrails for failures that never happened in 15 of 60 runs | Phantom Guardrails |
| DGM faked test logs and deleted its hallucination markers; o3 kept reward hacking in 70–95% of runs after being told not to; PROCTOR's loops reached 100% by reading answer keys at ≈ 68% true capability | DGM, METR, PROCTOR |
| Most proposals fail: Ouroboros's review blocked 63.5% of recent attempts; DGM costs ≈ $22k per run; Ouroboros spent $110.6k over 161 days | Ouroboros, DGM |
| AHE's evolved harness gained +7.3 on TB2 when tuned and evaluated on the same 89 tasks; an independent re-evaluation on a train/validation/test split found an average held-out gain of 0.0 | AHE; arXiv 2607.12227 |
| Harness differences of 11–19 points exist with the model fixed, and self-evolution can find them | spec §1 |
| A gate that only ever adds rules cannot detect phantom ones; ties must favour removal | Phantom Guardrails, Anthropic's scaffold deletions |
| With 50–60-task subsets one task is worth ≈ 2 points | HGM, spec §11 |

The design question is therefore not *whether* to gate, but what the gate decides, what it costs, and how it avoids becoming the thing the agent optimizes.

## Decision

**1. The kernel gates; the agent never grades itself.** All stages run in `init-kernel gate`, in a fresh sandbox outside the agent's, on a kernel-built binary (`cache/target/`, not the agent's build cache). Holdout tasks, canaries, gate code and raw traces live in `vault/`, which the agent can neither read nor write. Gate results are written by the kernel as `refs/notes/gate`, which the agent cannot write.

**2. Staged, cheapest first, early exit.** G0 compiler (check, clippy `-D warnings`, test, `forbid(unsafe_code)` on every target root, build-config policy) → G1 self-map drift → G2 boot probe of the candidate **and** the running parent, comparing proxy-measured head hashes, tool lists, conversation continuity and boot honesty → G3 evolvability (a scripted self-edit through G0–G2) → G4 regression: paired candidate-vs-LKG runs with repeats on the frozen task pool, the hidden holdout and canaries, recording score, reference cost, wall-clock and cache hit. Internals belong to kernel/gate.md.

**3. Scope by what changed, decided by the kernel from paths, not by the agent's claim** (gate.md §3.1). T0 data (`skills/**`, `SELF.md`, `GENESIS.md`) → G0–G1, or G0–G1 plus a synchronous G4 for a wholesale rewrite. A dependency change (`Cargo.lock`, `vendor/`) → G0–G4 with G4 **before** the handoff: no provisional lane for new third-party code. T1 extensions → G0–G3 to be kept provisionally, G4 to become LKG. Everything else (T2) → G0–G3 to run provisionally, G4 to become LKG.

**4. Provisional fast lane.** For T1 and T2: G0–G3 to run, G4 asynchronously to become LKG. A provisional version may not take external actions. Provisional chains are capped at depth 3; G4 runs on the newest head against LKG and accepts or rolls back the whole chain. A G4 reject rolls back at the next idle point (or after at most 60 minutes, at a step boundary). An **inconclusive** G4 (underpowered, over budget, timed out) gets exactly one escalated re-run; if it is still inconclusive, the incumbent wins (`inconclusive_final`) and the version is rolled back. An underpowered gate is neither pass nor fail, and no owner step is required (v03-decisions D4). The owner's `/bless`, `/reject` and `/accept` are optional signals that never gate anything (gate.md §3.12). A failed swap is a non-event for the live conversation.

**5. Accept rule: non-inferiority, then cheaper, then smaller** (exact form: gate.md §3.8–§3.9).
- Non-inferior on score: a one-sided paired cluster bootstrap over tasks with margin m = 2δ (δ measured from the LKG arm's own repeats; m at least half a task, at most 0.15), plus point-estimate guards on the pool **and** on the holdout separately. Same model only.
- Significantly better on score → accept, unless cost × time rose more than 1.5× and the projected task spend at the new cost no longer fits the soft day budget, which is rejected (`costlier_than_budget`, gate.md §3.9).
- Score tie → lower cost × time (beyond its δ) accepts; then the smaller body accepts. A candidate equal on score, cost × time and size is **rejected** (`no_gain`): ties favour the incumbent, which is the pressure toward a small body.
- A pre-registered `Repro: task:` on a frozen pool task (owner or maintainer, or the sealed half of an eligible agent task family; no owner acceptance required) that the candidate passes in every repeat and LKG fails in every repeat (`repro_fixed`) admits a `feat`/`fix`/`guard` whose benefit the sampled pool cannot show.
- A `perf` change (claims only cost/time gains) must show a gain beyond δ on cost or time.
- Comparisons use the proxy's reference-window cost, so time-of-day pricing cannot bias them.
- A perfect score on a canary is treated as cheating: reject, quarantine the commit, notify the owner.

**6. Evidence discipline is mechanical, not prompted.** Every candidate is one agent-authored commit (or one upstream merge) whose trailers pass a deterministic validator (evolution.md §3.9). `Repro:` is mandatory for fixes, guards, and any change that grows the prompt prefix (`Prefix-change: yes`), because "is this a rule?" cannot be checked but "does it grow the prefix?" can.

**7. Predictions are scored, never gated on.** `Predicts:` clauses are kernel-scored offline at G4 and online over later tasks (evolution.md §3.10). The hit rate is a self-signal and a Phase 3 prior. Gating on it would reward predicting only safe things.

**8. Reviewers can only block.** If the agent ever grows an LLM reviewer, its verdict is advisory and can block a candidate but never approve one (PROCTOR).

**9. Budgets are enforced by the kernel.** Per task, per gate, per probe, per day, and an evolution share of the daily budget (kernel/config.md §4.3, gate.md §3.11, evolution.md §3.14); promote-attempt caps per task and per day. G4 never starts what its remaining budget cannot finish. Open-ended search (the L2 fleet) unlocks through the Root's autonomy ladder, within the charter's ceilings; no owner cap is required (spec v0.3 §15).

**10. Model changes reset the baseline.** Comparisons are only ever made within one model. A model change triggers a baseline G4 and the thinning protocol, under the same accept rule.

## Alternatives considered

| Alternative | Why not (as the policy) | What INIT keeps from it |
|---|---|---|
| **No offline gate; online tool creation only** (Live-SWE-agent) | Nothing persists, so nothing compounds; no regression control across tasks | T0/T1 trials inside a task are ungated (spec §6.1); keeping them is gated |
| **Strict improvement required** (must beat the parent) | Rejects neutral simplifications and deletions, which the size tie-break is designed to accept; at n ≈ 20 tasks "better" is mostly noise | — |
| **DGM-style admission only** (compiles and can still edit itself) plus an archive | Too weak for a single live lineage that serves real tasks; DGM relied on its archive and offline budget, which INIT does not have at seed | G3 is exactly DGM's "can it still edit code" check; the archive arrives in Phase 3 |
| **Synchronous G4 before every swap** | Safer, but every T2 change waits for the full regression run (up to 90 minutes) before the agent can use it | Used for `dep` and `t0_escalated` scopes; the fallback for all scopes if the provisional lane misbehaves (roadmap H3) |
| **Multi-model LLM review panel as the gate** (Ouroboros) | Reviewers share the agent's blind spots; review cost; blocks are not measurements | Diff fingerprinting is implicit: the kernel gates the exact fetched commit SHA |
| **Score-only acceptance** | Ignores cost and time; Gödel Agent's gains came largely from calling stronger models; STOP built itself a bigger budget | Cost × time is the first tie-breaker; perf claims must be shown |
| **Single aggregate utility** (SICA: 0.5·score + 0.25·cost term + 0.25·time term) | Lets cost savings buy score losses | A lexicographic rule (score first, then cost × time, then size) |
| **Gate on prediction accuracy** (AHE-style verdicts as accept criteria) | Goodhart: the agent learns to predict the gate | Predictions are scored and shown, not gated |

## Consequences

**Positive**
- Measurements the agent cannot forge (kernel-run, vault-held, notes the agent cannot write), which is the precondition for evolution meaning anything.
- Data changes stay cheap (G0–G1), so the common case of updating skills and the self-map does not burn task budget.
- The body tends to shrink: deletions are accepted on ties, unused growth is listed for retirement, and scaffolds tagged `Compensates-for` are re-tested on every model change.
- The live conversation never stops for a failed change; real-time T2 evolution is possible without risking the running agent.

**Negative and accepted**
- **G4 costs real money per accepted chain.** At gate.md's default sizing (66–98 paired runs) that is ≈ $1.3–2.0 on `deepseek-flash` off-peak, ≈ $3–13 on `deepseek-v4-pro`, and $20–170 on frontier models at TB2.1 per-trial costs. Budgets, the evolution share and the off-peak preference bound it. The money caps are provisional defaults, PENDING OWNER (CONVENTIONS §6 E1 proxy/gate); the model used for G4 is provisionally `deepseek-flash`, PENDING OWNER (evolution-testing ET-E2).
- **Low statistical power.** With 16 tasks (one task ≈ 6 points) the gate reliably catches regressions of about 17–20 points of pass rate, crashes, cost blow-ups and cheating, not 5-point regressions (gate.md §3.8.5). Every gate record carries δ and m, so the resolution is visible. Phase 1 grows the pool to ≥ 30 and tightens `margin_max`; Phase 2 provides external validity.
- **Most proposals will fail**, as in Ouroboros; the attempt caps and the visible reasons in the log are how the agent learns from failures cheaply.
- **The provisional window** runs unproven code for minutes to an hour; it is bounded by depth 3, the no-external-actions rule and immediate rollback on crash.
- **Self-declared change types** can dodge type-specific checks; the prefix rule, the size tie-break and retirement cover the cases that cost the most, and lineage records every claim for later audit.

**What would reopen this policy**
- Phase 1 data showing LKG versions later rolled back by regressions G4 missed (false accepts) above ~10% of accepted chains → raise K/H, repeat runs, or make G4 synchronous for T2.
- δ larger than the effect sizes the agent produces (no change ever distinguishable) → larger pools or paired repeated runs before relying on the tie-breaks.
- Evidence of gate gaming (canary hits, holdout/pool divergence) → rotate holdout, add canaries, tighten hermeticity.
- Phase 2 showing the evolved body below the seed on external tasks while passing its own gate → the gate measures the wrong thing; revisit the pool.

## References

All checked 2026-10-19.

- DGM: https://arxiv.org/abs/2505.22954 ; https://sakana.ai/dgm/
- HGM (one task ≈ 1.7–2 points on 50–60-task subsets; CMP): https://arxiv.org/html/2510.21614v2
- SICA (utility function): https://arxiv.org/abs/2504.15228
- AHE: https://arxiv.org/abs/2604.25850 ; independent re-evaluation: https://arxiv.org/abs/2607.12227
- Ouroboros: https://arxiv.org/abs/2608.08311
- PROCTOR: https://arxiv.org/abs/2609.02246
- Phantom Guardrails: https://arxiv.org/abs/2607.13083
- METR reward hacking: https://metr.org/blog/2025-06-05-recent-reward-hacking/
- Live-SWE-agent: https://arxiv.org/abs/2511.13646
- ACE: https://arxiv.org/html/2510.04618
- Terminal-Bench leaderboard (per-trial costs): https://www.tbench.ai/?version=2.1
- DeepSeek pricing: https://api-docs.deepseek.com/quick_start/pricing/
- evolution.md (flows, validator, predictions, budgets); kernel/gate.md §3.1–§3.13 (scopes, stages, statistics, accept rule, budgets); kernel/config.md §4.3; reconciliation ledgers kernel-proxy-gate (E1, E2) and evolution-testing.
