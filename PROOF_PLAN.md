# Proof sprint: does INIT's self-evolution actually work? (owner decision 2026-10-21)

The owner asked whether INIT makes a novel contribution and said that without one the project should stop.

Assessment:
- Not novel: the agent loop, tools, TUI, providers, sandboxing, self-editing code (DGM/SICA), harness optimization (Harness²), and paired-significance gates (Gated-QD, 2026).
- Possibly novel, as architecture:
  1. live hot-swap of the agent's own compiled core mid-conversation, keeping the session and prompt cache, with automatic rollback;
  2. a minimal immutable Root (stop, ceilings, keys, signed lineage, sealed evaluator) around a self-modifying agent, with an autonomy ladder unlocked by measured record;
  3. a gate-of-gates: mutation testing of the evaluator itself, so the agent may evolve its own gate.
- Unproven: that self-evolution yields real improvement. No self-generated change has stuck yet.

The owner chose to run a proof sprint before the v1 breadth work (providers, UX, platforms: about 800 AH). That breadth work is ON HOLD.

## Goals and stop rule
1. **Stick:** at least one change proposed by the agent itself (from real failure evidence, evolution-v2 reflect) passes G4 v2 on sealed S v1 and becomes LKG, on real deepseek-flash.
2. **Improve:** a measurable gain over the LKG chain on the untouched reserve tasks (M7's milestone check), or an honest null result.
3. **Hold:** a red team (adversarial agent behaviour against the real Root in test mode) cannot raise ceilings, read keys, forge lineage, game G4 (canaries, test-file tamper), or escape the sandbox; the gate-of-gates rejects every mutant.
4. **Novel:** a serious prior-art review confirms or refutes claims 1–3 above.

Stop rule: if no self-generated change sticks within the sprint (budget about $3 real), or the review finds the claims already done, recommend stopping product work, possibly publishing the Root and gate as a technical report or library.

## Scope (minimum to prove; the rest of M7 and evolution-v2 waits)
- **M7 core** (docs/m7-design.md):
  - cost probe (10 runs, about $0.03);
  - S v1 generators (32 + 24 reserve + 4 canaries, maintainer-written, Root-only);
  - G4 v2 (exact sign-flip test plus margin, screening on the visible pool, an incumbent run cache, a per-run token cap, off-peak scheduling);
  - the milestone check on the reserve; A/A calibration.
- **Evolution-v2 core** (docs/evolution-v2.md, when it lands): the reflect stage (failure mining with Repro evidence, INV/DOM), propose → screen → gate, and the lessons library (minimal).
- **Red team:** a $0 scripted-adversary suite plus a small real-model adversarial session.
- **Novelty review:** a deep-research report.
- **Budget:** at most $3.00 real of the owner's $5 (balance-checked); the Root meter is the safeguard.
