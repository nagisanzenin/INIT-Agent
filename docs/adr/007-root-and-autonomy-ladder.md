# ADR-007: An immutable Root and an autonomy ladder instead of human review

Status: accepted by the owner on 2026-10-20 · Area: overview
Depends on: spec v0.3 Part II (§12–§19)   Defines: the trust zones, the Root's seven responsibilities, the L0–L4 ladder

## Context

INIT's purpose is to let future models, eventually more capable than any human, keep scaling themselves. In v0.2 the owner stayed in the loop: accepting tasks, approving crates, kernel changes and external actions, and setting fixed budgets. That design ran into four problems:

- Human review does not scale. The owner and most engineers no longer read code, and review loses its meaning once models surpass people.
- The accept rule (gate.md §3.9) rejects any new capability that the owner's task pool does not exercise (`no_gain`). As a result, the owner's imagination caps the agent's growth.
- Fully self-governing evolution erodes its own constraints through selection. DGM and STOP are examples, and METR found reward hacking in o3. The more capable the optimizer, the subtler the gaps between the measure and the intent that it finds.

## Decision

1. **Separate review from control.** Remove human review everywhere and replace it with mechanical verification. Keep control (stop, rollback, resource ceilings, the boundary on external actions, tamper-proof lineage, the purpose, an honest evaluator) in a small, immutable **Root**: `init-root`, about 1,000–1,500 lines of Rust. That was the design estimate. The build budget is ≈ 7,500 lines as per-module sub-budgets (root/root.md §4.8), a provisional owner decision of 2026-10-20 that the owner may override. It runs as the owner's OS user, outside every sandbox.
2. **Three trust zones:** the Root, then the agent zone (the kernel, which is agent-evolvable from L3), then the body zone. The only security boundary is the one between the Root and everything else.
3. **An autonomy ladder from L0 to L4.** The Root unlocks each level mechanically from the track record it holds in lineage and in the sealed audit, with no human approval. The charter caps the maximum level. Demotion is automatic.
4. **The measuring stick stays honest without humans.** The Root holds three things no gate version can see or change:
   - a sealed task set;
   - a mutation suite that any new gate must reject in full (the gate-of-gates);
   - a periodic sealed audit that rolls the system back on regression.
5. **The agent writes tasks for itself from L1 on.** Its tasks are admitted mechanically and pre-registered, and they never judge their author's own descendants. They can show a new capability is better. They never replace the sealed audit.
6. **The agent can never remove the Root.** Root changes come only from the maintainer or the owner.

## Alternatives considered

| Alternative | Why rejected |
|---|---|
| Keep v0.2's human approval points | Does not scale; the owner's task pool caps the agent's capability; contradicts the owner's vision |
| No Root: the agent may modify everything, including stop, ceilings and the evaluator | With no external anchor, selection erodes constraints. Capability is not the same as a correct objective. An aligned system loses little by keeping a stop, while a misaligned one gains a lot by removing it. The maintainer will not build this path. |
| A large Root that keeps the gate and the kernel immutable forever | The maintainer's evaluator would become the ceiling. A gate the agent may improve, checked by the gate-of-gates and the sealed audit, keeps honesty without capping capability. |
| Human spot-checks as the honesty mechanism | They do not scale and fail as soon as the agent outperforms the reviewer. Mechanical mutants and sealed tasks scale with compute. |

## Consequences

- Phase 0 adds a `root` lane of about 25–35 agent-hours. Keys, money ceilings, panic, whole-system LKG and lineage move from the kernel docs into the Root docs.
- The owner's ongoing work drops to zero required actions. The owner still writes the charter at install and keeps optional `/panic`, ceilings and reports.
- The sealed task set needs sources outside the agent's influence: public benchmark snapshots and maintainer tasks, refreshed by the Root. If those sources dry up, the audit weakens. This is risk R-ROOT-1.
- The ladder's unlock thresholds are placeholders until Phase 1 calibrates them.
- The constitution is rewritten around the Root and the ladder.
