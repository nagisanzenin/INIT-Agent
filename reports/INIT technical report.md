# A Sealed Gate Rejected a Self-Modification Its Visible Screen Favored: Lessons from INIT, a Self-Modifying Agent Harness

*A pre-registered n = 1 case study, a negative result, and an engineering post-mortem*

*Technical report, 2026-10-22. INIT project (author list to be confirmed by the owner). Prepared for a NeurIPS/ICLR 2027 workshop on agents and safety.*

**Citations.** Bracketed tags such as [M7R L37] point to the source files listed in Appendix F. "L" gives line numbers at commit `ed80966`, and "§" gives a section of a design document. Numbers marked *(our arithmetic)* were computed for this report from cited numbers. Statements marked *(inference)* go beyond what the sources measure. No per-task result from the sealed set is published: every sealed number here is an aggregate the Root itself released.

---

## Abstract

INIT is a personal coding-agent harness, written in Rust with WebAssembly extensions, that may edit its own code, including its compiled core. A separate, immutable *Root* process holds the stop switch, money ceilings, provider keys, a signed lineage, and a sealed task set the agent cannot read. We asked whether such a system can keep only the self-modifications that measurably help.

In a pre-registered proof sprint on a cheap real model (`deepseek-flash`, $3.00 cap), the engine mined a failure cluster from the agent's own runs (runs hitting the turn cap). The agent wrote a hypothesis and three code variants. A visible-pool screen did not reject the chosen variant, and the go rule estimated +120‰ (12 points). The change passed the build, self-map, boot and evolvability gates and was hot-swapped into the running session. The Root's sealed gate then ran an exact paired test with both arms fresh (29 tasks × 3 repeats). It reported a mean difference of −50‰ (rounded) with status *inferior*, meaning non-inferiority at a 120‰ margin was not established. The system rolled the change back automatically.

This is one data point. It supports no rate claim and by itself does not show harm. It does show three things: the full loop running end to end on a real model for $2.67; a sealed instrument calibrated before use (pseudo-A/A false-accept 48‰, minimum detectable effect 95‰); and cheap visible signals pointing up while the sealed measurement did not. One verdict took four G4 attempts and seven disclosed deviations. A sandboxed process reached 43.9 GB of compressed memory and drove free disk to 0.24 GB, because macOS Seatbelt has no memory limit. All components have prior art. The contribution is a measured, honestly reported integration.

---

## 1 Introduction

Self-rewriting agent harnesses now appear both in research (DGM, SICA, Gödel Agent) and in long-running deployments (Ouroboros, MOSS) [NN Q1, Q4]. The hard part is knowing which self-modifications help:
- SEAL reports near-perfect scores on self-written tests while deployment performance stays low.
- "Who Verifies the Verifier?" finds that, without guards, a self-evolved verifier collapses into a near-pass-all grader while the task score barely moves.
- "The Self-Improvement Problem" proves that no total computable certifier is both sound and complete for strict improvement [NN Q2, Q3].

Documented hacks of self-improving systems (faked test logs, a disabled sandbox, cached answer keys) show what happens when the system being improved can also shape its own evaluation [NN Q3].

INIT asks a narrow version of the question: **can a self-modifying harness keep only the changes that measurably help, on a cheap model and an individual's budget?** The design answer is to put control and measurement outside the agent. The agent may change almost anything about itself, including its compiled Rust core. Every change, however, passes a tiered gate pipeline, and the final statistical gate is computed by an immutable Root on tasks the agent never sees.

The repository's history spans about three days, from a first commit on 2026-10-19 to the owner's stop decision on 2026-10-22 [GIT]. We then ran a pre-registered *proof sprint* with a $3.00 real-money envelope and a stop rule fixed in advance: if no self-generated change sticks within the budget, recommend stopping product work [PP]. No change stuck.

**Contributions.** The novelty review classed every claim as *incremental*, meaning a new combination of parts that each have prior art [NOV]:
1. **An integration, described at its real size** (§2). It combines a Root TCB, three change tiers, a compiled-core self-edit with a mid-session hot handoff that keeps the prompt prefix byte-identical, mechanical gates G0–G3, and a sealed G4 based on an exact sign-flip test with A/A calibration. The Root is not minimal: it is about 7,085 lines by its own counting method, against a design estimate of 1,000–1,500 [ROOT §4.8; A007 L16].
2. **A pre-registered, real-money, end-to-end trial with its complete record** (§3–§4).
3. **A negative result, read carefully** (§4): one (estimate, measurement) pair, (+120‰, −50‰). *Inferior* in INIT means "non-inferiority not established", and its calibrated false-reject rate on this sealed set is 86‰.
4. **Operational and safety findings** (§5–§6): a memory-runaway path that Seatbelt does not bound, a latent autonomy-ladder bypass, a proposal state machine with no recovery edge, cold-build timeouts, a leaking cleanup path, and a $0 red-team record.

We do **not** claim that INIT improves itself, that sealed gates catch misleading changes at any rate, or that any component is a first.

---

## 2 System

### 2.1 Zones and threat model

INIT has three trust zones [A007 L17; ROOT §3.1]:
- the Root (`init-root`);
- the kernel (`init-kernel`, `initctl`), which is maintainer code at L0–L2 and may become agent code from L3;
- the body (`init-body`), which is agent-written code.

The only security boundary is the one between the Root and everything else. The Root runs as the owner's user, outside every sandbox, and spawns every zone process under a Seatbelt profile it writes itself, so the zone cannot weaken its own sandbox [ROOT §1, §3.1]. The real model path is owner → `init-root` → `init-kernel` (Seatbelt `kernel`) → `init-body` (Seatbelt `body`) → kernel proxy → Root egress → provider [RV L3].

```
owner ── owner.sock ──► init-root  (outside every sandbox: stop, ceilings, keys, lineage, sealed S, ladder, gate-of-gates)
                          ├─► init-kernel  (kernel profile: supervisor, gates G0–G4, model proxy, evolution engine)
                          │      └─► init-body (body profile: agent loop, tools, T0 skills, T1 Wasm extensions)
                          └─► sealed runs  (sealed profile: one S task each; no INIT_HOME, no IP)
```

### 2.2 The Root

| Function | Mechanism |
|---|---|
| **Stop and panic** | There are five channels; `/panic` on `owner.sock` and `SIGUSR1` act within 100 ms. Panic latches across restarts, closes egress, refuses spawns, kills the zone, records the event and demotes one level; resume rolls back to the audited-good tuple [ROOT §3.9]. v0.1 signals only processes the Root holds and reports `setsid` escapees [A008]. |
| **Money ceilings** | A Root proxy sits below the kernel's. Each request reserves its worst case at peak price, then settles on usage. The ledger is fsynced before sending, and the proxy fails closed. Defaults: $5.00 per day, $25.00 lifetime, $3.00 sealed reserve [EG §3.3–§3.4; ROOT §3.12.1; SE §4.3]. |
| **Keys and egress** | Keys live only in `ROOT_HOME` (mode 0600). The zone has no IP, and only the kernel profile may reach `egress.sock`. The Root strips auth headers, injects the key and terminates TLS. Destinations are exact `host:port`, default deny [EG §3.1–§3.5]. |
| **Signed lineage** | An append-only JSONL chain with SHA-256 links and Ed25519 signatures. Root-only record kinds are kept separate from kernel-reported kinds. The chain is verified at start, and a bad line halts the Root [ROOT §3.10]. |
| **Sealed S** | Stored in `ROOT_HOME/sealed`, invisible to the zone. Runs use a `sealed` instance kind the kernel cannot request. The Root reads only the check's result, then deletes the run root. Released records are aggregates: d rounded to 50‰, m to 10‰, cost-per-pass to 50‰; no task ids or p-values [SE §3.2, §4.1]. Look limits and rotation were cut from the lean build [PL D-L2]. |
| **Autonomy ladder** | Levels L0–L4 are a pure function of lineage and clock. Promotion is mechanical, with no human approval; demotion is one level per incident plus a rollback. The level sets what the agent may rewrite (L3: its own kernel and gates). The placeholder L1 criterion is 20 promotions, ≥ 1 audit and no regression in 14 days. Release builds cap at L0 [ROOT §3.13; A007]. |
| **Gate-of-gates** | A candidate gate must reject a Root-held suite of mutants M1–M7 (faked test logs, a disabled check, an injected regression, canary special-casing, cached answer keys, a silent parser fallback, sealed reads). It must accept controls C1–C3 (better, cheaper, smaller) and disagree at most once with a reference evaluator [SE §3.3; ROOT §3.8]. |

**Size.** ADR-007 estimated 1,000–1,500 lines. The provisional budget is about 7,500, and the measured size was 7,085 lines at `cdb49e8`, of which 1,656 are the hand-reviewed core [A007 L16; ROOT §4.8 L1823, L1848–L1849]. Counted as physical lines, `root/crates/init-root/src` holds 15,607 at `ed80966`, including in-file tests and later additions [GIT]. There is no formal verification. "Minimal" does not describe this Root [NN Q2].

### 2.3 Change tiers

| Tier | Covers | Gates | Activation |
|---|---|---|---|
| **T0** data | skills, `SELF.md`, `GENESIS.md` | G0 + G1; a wholesale rewrite also needs a synchronous G4 | live at once; LKG on pass |
| **T1** leaves | Wasm components (`wasm32-wasip2`, Wasmtime) | G0–G3, then async G4 while provisional | registry swap at a turn boundary |
| **T2** compiled core | `init-body`: loop, provider, context, tools, extension host (Rust, `forbid(unsafe_code)`) | G0–G3, handoff, then async G4 while provisional | new process via hot handoff (§2.4) |
| `dep` | `vendor/` or registry changes in `Cargo.lock` | G0–G3 + **synchronous** G4 | – |
| kernel (L3+) | `lineage/kernel.git` | K0–K3, then the Root's gate-of-gates | "a candidate gate never judges itself" |

Sources: [GATE §3.1; A004; A005]. No owner approval is required at any tier. A gate verdict is the only path to last-known-good (LKG). Provisional chains are capped at depth 3 and may take no external actions [GATE §1, §3.12; A006]. Rust has no stable ABI and native unloading is unsound, so the core changes only by fork-and-exec handoff and leaves change through Wasm [A005].

### 2.4 Compiled-core self-edit with a mid-session hot handoff

The agent edits its body's source in a git worktree, commits with structured trailers, and runs `initctl promote` [RV L89–L91]. The kernel builds the commit offline against vendored crates and stages the binary as a new inode [SUP §3.6.2]. After G0–G3 pass, the handoff runs at the next idle boundary [SUP §3.6.4; A001]:
1. the live body pauses at a step boundary and releases the session-log lock;
2. the candidate starts on the *same* session log and rehydrates;
3. after the candidate has stayed alive for a 3 s settle period, a `rename(2)` installs it;
4. the old body exits.

Any failure kills the candidate and resumes the old body. A crash inside the 120 s probation reverts the change. A G4 reject rolls back at the next idle point (≤ 3,600 s), and an undecided provisional expires after 24 h [SUP §3.6.4; EV1 §3.5, F10–F16].

**Prefix and cache.** Boot-varying material lives only in the tail, so the prompt prefix is byte-stable across the restart. G2 checks this with prefix-hash and message-continuity tests unless the commit declares `Prefix-change: yes`. Upstream connections live in the Root and survive the swap [SUP §6; GATE §3.5; A001]. A kernel swap (L3) restarts the bodies cold [SUP §3.15].

**Evidence.**
- **Mock model.** Pause-to-swap took 36 ms and spawn-to-ready 34 ms; prefix hashes matched across a promote and a rollback [WP16 L33–L34].
- **E2E-3, real model.** A comment-only T2 change passed G0 (308 s), G1 (10 s), G2 (27 s) and G3, and handed off with the same `prefix_hash` as cold boots. Its first request read **5,888 of 6,696 prompt tokens from cache** [RV L75–L78].
- **A3-real.** The model made its own T2 edit, fixed a trailer refusal by itself, and promoted: G0 337 s, G1 4 s, G2 22 s, G3 761 s, then a handoff on the same session. Asked which commit it was running, the new body named the commit and described its own change [RV L88–L94].
- **Proof sprint.** `c88be5f` was handed off into the live session twice [M7R L34, L37].

Not measured: cache savings against restart-and-resume, and real-model rollback rates under fault injection [NN Q1].

### 2.5 Gates G0–G4

| Gate | Checks |
|---|---|
| **G0** compiler | offline frozen build; build policy (no rustc wrappers, lint caps or `[patch]`); `forbid(unsafe_code)`; `check`, `clippy -D warnings`, extension build, tests; 900 s total [GATE §3.3] |
| **G1** self-map | regenerated `self.json` must equal the committed one; trailer markers must match; `wit/` must be byte-identical [GATE §3.4] |
| **G2** boot probe | candidate and parent probes on copies of the live log: ready in 90 s, stable 15 s, clean stop, a real tool round trip, tool, prefix and continuity checks; $0.25 per arm [GATE §3.5] |
| **G3** evolvability | a fresh probe of the candidate must itself make a scripted self-edit that passes an inner G0–G2. This is a live, stricter form of DGM's editing-ability check [NN Q1]. Stage timeouts default to 300 s; about $0.01–0.03 on flash [GATE §3.6] |
| **G4** statistical | paired candidate-vs-incumbent runs. v2: a reject-only visible screen, then the Root's sealed slice [GATE §3.8; M7D §3.2] |

### 2.6 G4 v2: exact sign-flip test, non-inferiority margin, A/A calibration

**Statistic.** Each task runs R times per arm. Let k_i be candidate passes minus incumbent passes, and D the mean of k_i/R. The quantity p_sup = P(Σ ε_i|k_i| ≥ Σ k_i) under random signs is computed exactly by dynamic programming. The result is **superior** iff p_sup ≤ 0.05 and D ≥ 1/n, which needs ≥ 5 discordant tasks [GATE §3.8.3]. The Root's implementation returns exact dyadic p-values and matched brute-force enumeration on more than 6,000 vectors [S1B].

**Status.** L is the one-sided 95% bootstrap lower bound of D, and the margin is m = max(2δ_S, 0.5/n), capped at 250‰, where δ_S is the A/A noise quantile. Statuses are assigned in order: `superior`; else **`inferior` if L ≤ −m**; else `underpowered` if m > 250‰; else `tie` if D ≥ 0; else `tie_negative` [GATE §3.8.3, L594–L603]. **So `inferior` means non-inferiority at margin m was not established. It is not a finding of statistically significant harm.**

**Accepting.** `better` needs `superior` and a cost per pass at most 1.5× the parent's. `lineage_gain` (a ratchet for small gains) needs a `tie` with p_sup ≤ 0.25 and a cost-per-pass ratio ≤ 1.2 [GATE L783–L785]. The screen runs 12 pool tasks at R = 1. It rejects on a canary, a tamper, ≥ 3 errors or net passes ≤ −4, and it never accepts [M7D §3.2.2].

| Tier | n × R | Null false-`better` (design / exact) | P(`better`) at +10 / +15 / +20 points (exact) | 80% power at |
|---|---|---|---|---|
| lean | 24 × 2 | 3.0% / 2.97% | 21.0 / 39.7 / 61.0% | ≈ +25 points |
| standard | 32 × 3 | 3.6% / 3.43% | 38.3 / 68.3 / 89.3% | ≈ +17 points |
| strong | 32 × 5 | 3.4% / 3.62% | 56.4 / 87.4 / 98.2% | ≈ +13 points |

Sources: [GATE L692–L696; S1B].

**A/A calibration.** `sealed calibrate` runs the incumbent R = 6 times on each of the 32 G4 tasks. It then forms 2,000 seeded 3 + 3 splits per task and applies the S rule to each split. From the splits it records:
- the false-accept rate (FA: a null split judged `better`) and the false-reject rate (FR: judged `inferior`), with Clopper–Pearson intervals;
- δ_S at R = 2, 3 and 5;
- the per-tier minimum detectable effect (MDE).

To pass, FA must be ≤ 50‰ (upper bound ≤ 70‰), FR ≤ 100‰ and m ≤ 250‰ [M7D §3.2.10]. A pseudo-A/A cannot see pipeline asymmetry, so the lean sprint also ran G1D, a deterministic mock in which a comment-only change must give d = 0 exactly. It kept G1L, a real lean A/A, for the case that D was accepted [RB §9 D-R1].

### 2.7 The evolution engine

```
miner ($0, deterministic) ─► friction clusters (score s_c)
   ─► proposal: hypothesis, change, ≥ 2 attested Repro refs, Predicts
   ─► ≤ 3 variants (kernel-built commits)
   ─► reject-only screen: E0 (build + G0) → E3 (R = 1 on ≤ 4 targeted + 2 control pool tasks)
   ─► go rule: est = round(1000 · q · s_c · fix · T)
   ─► ordinary promote: G0–G3 → handoff → provisional → G4 v2 → keep or roll back
```

**Miner.** The miner makes no model calls. It reads failing visible-pool runs, proxy traces and refusals, and never touches S, holdout or canary runs. It applies a table of deterministic, weighted signal rules (e.g. `cap.hit:<cap>`, `path.*`, `finish.tests_failing`) and requires ≥ 2 attested references from ≥ 2 tasks before a cluster is eligible [EV2 §3.4]. It was built because, in six real sessions, 12 failure events had produced no proposal, and 6 of them were path errors [EV2 §3.1; BL L176].

**Screen.** E3 rejects a variant on a tamper or canary; on 0 targeted tasks fixed with ≥ 1 control lost; on ≥ 2 controls lost; or on cost > 2× the incumbent's. By design it is reject-only: "A screen pass carries no evidential weight in the gate" [EV2 §3.5.2].

**Go rule.** est = round(1000 · q · s_c · fix · T), where:
- s_c is fatal pool runs over failing pool runs;
- fix = (targeted tasks fixed + 1)/(targeted tasks + 2);
- q ≈ 0.5 is a prior;
- T = 0.8 is a transfer discount, to be refitted after 5 (est, d) pairs.

The rule "only decides whether the gate is asked" [EV2 §3.5.3]. The lean build used a single threshold, est ≥ 80‰, and cut the E1/E2 screens, composition and the lessons library [PL D-L5].

---

## 3 Experiment

### 3.1 Setup

- **Model and machine.** `deepseek-flash` with thinking on, which the body hard-codes [RV L40]. Our redacted artifacts record only the alias, not a served snapshot [GIT]. The machine was the owner's macOS 14.6 laptop [BEN L66]. Outside the runaway event, it had about 5–12 GB of free disk [M7R L34, L37; BL L232].
- **Meters.** Off-peak prices are half of peak. The Root meter bills at peak price, about 2× real off-peak; line P measured a Root/real ratio of 1.999 [RB §3; M7P L32]. The owner's balance was the authority for the $3.00 envelope. A harness guard froze spend by setting the charter ceilings to "spent" [RB §3].
- **Sealed S v1-lean.** 52 templates (E 24: Python 12, Rust 12; C 14; T 14) plus 4 canaries [PL D-L1]. The draw gave 32 G4 tasks (E 14, C 9, T 9), a 20-task reserve (E 10, C 5, T 5) and 4 canaries [M7P L10–L15]. E is Exercism with a mutated spec, C is CRUXEval-seeded change tasks, and T is macOS terminal tasks [M7D §3.1.1].
- **Family B dropped.** Family B (injected bugs in fresh tiny repos) was dropped as "the longest pack (31 AH)", to reach a stop/continue answer fast [PL D-L1; BL L181]. The recorded cost: E and C derive from public sources, so contamination risk is higher. Seeded semantic mutations mitigate it [PL D-L1].
- **Visible pool.** 20 tasks (e 8, c 5, t 7). Their 30–70% target difficulty was never measured [PL D-L4; PLS].

### 3.2 Pre-registration (D-0)

D-0 was frozen before any mining output was read. Its sha256 begins `687e77f0cfe9be21`, and it records the seqs of the Root's `sealed_draw` and `sealed_calibrate` records [M7R L5, L43; RB §6]. It fixed:
- **selection:** the eligible cluster spanning ≥ 3 pool tasks and ≥ 2 families with the highest s_c;
- **proposal:** one proposal with ≤ 3 variants and ≥ 2 `Repro:` references;
- **screen:** E0 + E3;
- **go rule:** est ≥ 80‰ at standard tier (`better` aim at ≥ 150‰, else `lineage_gain`);
- **scope:** T2 only;
- **attempts:** **one promote and one G4**.

Only `better` or `lineage_gain` count as accepted. "Sticks" further requires LKG, no milestone regression on the untouched reserve, no rollback, and a passing real A/A [RB L126–L147].

The owner messages were fixed texts that named the friction, never the change [RB L101, L128]. **The `Predicts:` clause `pass_rate@sealed >= parent+15%` was also prescribed by D-0, not chosen by the agent.** It is scored as a hit if d ≥ 150‰ [RB L101, L135; BL L228]. The agent's own contribution was the hypothesis and the code.

### 3.3 Lines

The lines ran in order: P (cost probe) → C (calibration) → G1D ($0 mock) → D (mine, propose, screen, one G4) → M (milestone) → G1L (only if D counted) [RB §2]. Appendix A has the full table.

| Line | Result | Key numbers |
|---|---|---|
| **P** | pass | probe P0: 10 runs, 7 passes, $0.00356/run. Re-run after a kernel fix: **$0.00348/run** → plan "trim", D at standard tier [M7R L27–L28] |
| **C** | **pass** | 32 × 6 sealed runs + pool 20 × 2. **FA 48‰** (upper 59‰), **FR 86‰**, δ_S r2/r3/r5 68/57/44‰, **m(standard) 114‰**, **MDE lean/standard/strong 134/95/74‰**. **10 of 32 tasks in band**, so S shipped unselected (n_kept 32). Pool incumbent **85%** (6 of 40 runs failing). $0.857 [M7R L29] |
| **G1D** | pass | mock, lean 24 × 2, `tie`, **d = 0‰ exactly**, reject `no_gain` [M7R L26] |
| **D** | **not accepted** | §4 |
| **M** | skip path | current = confirmed tuple; $0.0002 [M7R L38] |
| **G1L** | not run | D not accepted [M7R L39] |
| **Program** | | **$2.67 by balance** ($9.56 → $6.89; cap $3.00); $3.06 conservative priced [M7R L40, L150–L151] |

---

## 4 Main finding

### 4.1 The chain of numbers

| Stage | What happened | Value |
|---|---|---|
| Miner | 7 clusters, 4 eligible; the rule picked **f-000003 `cap.hit:turns`** (10 pool tasks, 3 labels) [M7R L44] | s_c **600‰** |
| Proposal p-000001 (agent) | **Hypothesis:** a failing leading `cd` returns only "No such file or directory" and skips the `&&` chain, so the model spends turns rediscovering the workspace. **Change:** bash appends a hint that names the shell's real cwd and says the shell keeps no state [M7R L46–L47] | – |
| Prediction (fixed by D-0) | `pass_rate@sealed >= parent+15%; cost_usd <= parent+30%` [M7R L48] | **+150‰** |
| Variants | v1 `src/tools/bash.rs` + tests + `self.json` (+103/−37); v2 `bash.rs`, another approach (+140/−46); v3 `src/loop/mod.rs` (+244/−46) [M7R L50–L52] | 3 |
| Visible screen E3, R = 1 | v1 fixed **1 of 2** targeted pool tasks and **lost 1 of 2** controls; cost reference 5,377 vs 13,714 µUSD. v2 and v3 rejected `screen.mini_inferior` [M7R L54–L55] | v1 survives |
| Go rule | est = 0.5 × 0.6 × (1+1)/(2+2) × 0.8; 80 ≤ 120 < 150, so go, aim `lineage_gain` [M7R L54, L56] | **est 120‰** |
| G0–G3, handoff | `accept g0_g3`; T2 check passed [M7R L57–L58] | pass |
| **Sealed G4 v2** (`j-000013`, standard) | both arms fresh: 5 canaries (clean), 96 incumbent and 96 candidate runs; n = 29 (3 of 32 tasks dropped by the Root), R = 3 [M7R L61–L62] | **inferior**, **d = −50‰**, m = 120‰, cost per pass 1.05× |
| Outcome | `reject score_inferior`; `body_rollback` to LKG `aaf07b72` [M7R L63] | **NOT ACCEPTED** |

The sources do not say why the Root dropped 3 tasks, and under the hygiene rules of §5.2 we did not look.

### 4.2 Reading the sealed result

1. **d is rounded.** The Root releases d to the nearest 50‰ [SE §4.1], so the exact value lies in [−75, −25)‰. Assuming pass/fail scoring, that is a net loss of 3 to 6 run-passes over 87 runs per arm *(our arithmetic and assumption)*.
2. **`inferior` is a failed non-inferiority test.** It says the 95% lower bound of D was at or below −120‰ (§2.6). It does not say harm was shown. No harm p-value was released.
3. **The verdict has a measured false-reject rate.** In C's pseudo-A/A, a null change was judged `inferior` **86‰** of the time [M7R L29], about one in twelve *(our arithmetic)*.
4. **The effect is within calibrated noise.** |d| = 50‰ is below the A/A noise quantile δ_S(R = 3) = 57‰ [M7R L29] *(inference: we read this as "no detectable change", not as a demonstrated 5-point loss)*.
5. **What is established.** The sealed evidence gives no support to the +120‰ estimate or the +150‰ prediction. The point estimate is 170‰ below the estimate *(our arithmetic)*. The exact design power for `better` at standard tier is 38% at +10 points and 68% at +15 points [S1B]. Computing the likelihood of this d under a true +120‰ effect would need per-task data, which we may not read.
6. **The cost signal did not carry over either.** The screen showed a much lower cost reference for the candidate. The sealed slice measured a cost per pass of 1.05× [M7R L54, L62]. These are different metrics on different tasks *(inference)*.

### 4.3 Reading the visible result

By design, the screen never approves anything. It failed to reject v1, and the go rule then chose to spend the sealed gate. The "visible approval" consisted of three things:
- the agent's hypothesis, carrying a prescribed +15-point claim;
- a screen that did not reject;
- a go-rule estimate of +12 points.

The raw visible evidence was net zero: one targeted task fixed and one control lost, on four tasks at R = 1. The estimate is prior-weighted: it multiplies q = 0.5, s_c = 0.6, a Laplace-shrunk fix of 2/4, and T = 0.8. The lost control does not enter it; it counts only in the reject rule [EV2 §3.5.2–§3.5.3]. Each signal is a reasonable reason to *ask* the gate. None is evidence of a gain, and the design said so in advance [EV2 §3.5.2].

*(Inference.)* A loop that accepts on the proposer's confidence, or on "a targeted failure got fixed", would have kept `c88be5f`. A loop that requires a net visible gain would not have kept it, but at 4 tasks × R = 1 such a loop can barely accept anything. One plausible reason for the mismatch is that the friction was mined on a pool where the incumbent already passes 85%, while 22 of 32 sealed tasks were outside the difficulty band. A `cd`-failure pattern that costs turns on the pool may be rare or harmless on S. Checking this would need per-task S data, which the protocol forbids.

### 4.4 What this does and does not show

**It shows (an existence result, n = 1):**
- The loop runs end to end on a real cheap model within an individual's budget: friction mined from real runs, an agent-written hypothesis with three tested variants, a screen, mechanical gates, a hot handoff, a sealed paired test, and an automatic rollback.
- Once, the cheap visible signals that decide whether to spend the sealed gate pointed up, and the sealed measurement did not. The response needed no human judgment.
- The deciding instrument was calibrated on this S beforehand, with FA (48‰) inside the pre-registered bound [M7R L20, L29].

**It does not show:**
- any rate, either of how often visible screens mislead or of how often sealed gates catch them;
- harm (FR is 86‰ and the effect is within noise);
- that INIT can improve itself (Improve was never tested; M took the skip path) [VER];
- transfer to other models, task sets or budgets;
- that another loop (Harness², DGM) would have accepted this change.

Hence the singular in our title: a sealed gate rejected *a* self-modification its visible screen favored.

---

## 5 Operational lessons

### 5.1 Four G4 attempts for one verdict

Line D failed three times before its first G4:
- try 1 was refused by a kernel task budget smaller than one request reservation (DV-2);
- try 2 filed p-000001, but the disk rule killed its screen;
- screen 1 froze on a resumed turn billed ≈ $0.181 (DV-3) [M7R L30–L32].

The G4 itself then took four attempts:

| # | Job | What happened | Priced $ | S outcome seen? |
|---|---|---|---|---|
| 1 | `j-000007`, 06:34 +07 | G0–G3 and handoff passed. The sealed slice started at 06:39:48 at parallel 4; 47 sealed runs had started when a sandboxed `python3.9` began to grow at 06:48:24. Spend was frozen at 06:51. **No `sealed_g4` record** [M7R L34, L59] | 0.218 | no |
| 2 | retry 1 (DV-5) | rollback `j-000008` succeeded; the re-promote was refused because p-000001 was still `promoted`, not `screened` [M7R L35] | 0.0144 | no |
| 3 | retry 2 (DV-5b) | the proposal was reopened; `j-000010` passed G0–G2, then **G3 timed out after 589.6 s** (300 s defaults; a cold build alone takes ≈ 280 s) [M7R L36] | 0.108 | no |
| 4 | retry 3 (DV-5c, DV-7), `j-000013`, 14:01–15:33 | G3 at 1,800 s, handoff, G4 standard with both arms fresh: **inferior**. The watchdog stayed quiet (free disk ≥ 11.6 GB, peak footprint 1.3 GB). Run at peak prices [M7R L37] | 1.328 | aggregates only |

About $0.70 priced went to attempts that missed their goal *(our arithmetic over try 1, try 2, screen 1 and attempts 1–3)*, though try 2 did produce the proposal.

### 5.2 Deviations, and why statistical validity holds

| Id | What and why | Fix | Effect on selection, est, sha or S |
|---|---|---|---|
| DV-1 | Line R spent **$0.181 against its $0.15 cap**; the guard held only D's whole cap | `5167369` | none; G4 still gated by est ≥ 80‰ and one attempt [M7R L67] |
| DV-2 | kernel per-task budget $0.05 < one reservation (≈ $0.04) + spend | `0de9077` | none [M7R L68] |
| DV-3 | an unfinished turn resumed at boot; the cut stream was billed ≈ $0.181 | fresh session per line (`a3dae6f`) | none; counted conservatively [M7R L69] |
| DV-4 | G4 attempt 1 interrupted by disk: swap from one runaway process; secondary probe-root leak | cleanup by exact path | no `sealed_g4` record [M7R L70–L85] |
| DV-5, 5b, 5c | re-run the single G4 on the same sha: rollback and re-promote; one harness edit of the test-home proposal state (`promoted` → `screened`); G3 timeouts raised to 1,800 s by config | harness and config | lineage, sealed state, est, variant and sha unchanged [M7R L86–L103] |
| DV-6 | watchdog SIGKILL for any workload over **4,096 MB** (never the Root or kernel) | harness watchdog | applied to both arms; a > 4 GB run on these short tasks fails in either arm [M7R L113–L115] |
| DV-7 | at peak, a stale $1.07 line cap would have frozen G4 at ≈ 75%, and the Root's sealed ceiling ($2.98, $2.43 spent) would have bound mid-slice | standalone guard capped by the owner's balance floor ($6.56); test-home sealed ceiling raised to $3.53 by the owner's charter apply | prevents a bias: a refused candidate run scores 0 [M7R L104–L112; RB L50] |

**Why the single verdict stands:**
1. **D-0 never changed.** The deviations touched harness code, budgets and G3 timeouts. They did not touch the selection rule, the estimate, the variant, the sha, the tier or the statistic [M7R L97, L103, L111].
2. **There was one verdict.** Attempt 1 left no `sealed_g4` record, so `j-000013` is the only one [M7R L59, L90].
3. **No S peek.** The results file lists everything read after attempt 1: lineage record kinds, the job record (G4, 0%, no result), money meters, file names without content, and the macOS kernel log. Nobody saw a pass count, a partial d or a per-task result. The runaway process was never traced to its run, arm or task [M7R L134–L145].
4. **The decision to continue came first.** It was made before any look, on non-S grounds (disk) [M7R L147].
5. **The completed G4 ran both arms fresh**, which removes cache asymmetry [M7R L61].

**Residual threats.**
- The results file is inconsistent on one point. L146 says the parent arm would come from C's cache, while L37, L61 and L105 say both arms ran fresh with 96 incumbent runs. We report the latter, since it describes `j-000013`.
- C ran without the 4,096 MB cap, though none of its 193 runs came near it [M7R L115].
- The harness edit of kernel state (DV-5b) is a protocol weakness.
- Attempt 4 ran at peak, about seven hours after C. *(Inference: price windows change cost, not pass rates; provider serving might vary by time of day, which we did not measure.)*

### 5.3 A memory runaway that filled the disk

| Time (+07) | Event |
|---|---|
| 06:48:24 | a sandboxed `python3.9` in the sealed slice starts to grow |
| 06:48 | macOS creates `swapfile5` and `swapfile6` on the same APFS volume |
| 06:48:51 → 06:49:21 | free disk falls from ≥ 5.3 GB to 1.54 GB in one 30 s poll |
| 06:50:13–37 | swap growth fails with ENOSPC |
| 06:50:38 | jetsam kills the process, which had reached 43,883 MB compressed |
| 06:51 | the harness freezes spend; free disk had bottomed at 0.24 GB |

Sources: [M7R L59, L71–L73, L121].

**Causes.** The primary cause was swap, not build output. It was the only such event in the kernel log from 00:00 to 07:05, and free disk stayed flat through C's 193 sealed runs at parallel 4. Lowering parallelism would not help, since one process suffices [M7R L71–L75]. A secondary cause was a probe-root leak, which reached 1.42 GB `du` (0.55 GB unique) by the stop [M7R L76–L79].

**Why nothing stopped it.**
- Seatbelt has no memory limit.
- macOS does not enforce `RLIMIT_AS`/`RLIMIT_DATA`.
- Neither the Root nor the kernel set any memory bound; only `RLIMIT_NOFILE` was set.
- Swap growth bypasses the Root's disk floor [M7R L119–L123].

The same path exists in daily use, since the bash tool runs model-written code under the same kind of sandbox [M7R L122]. The owner's live install survived untouched [BL L231].

### 5.4 Patterns of harness fragility *(inference)*

1. **Budgets smaller than one reservation turn runs into refusals or zero scores.** This happened in DV-2. It also happened in the earlier S-REAL G4, whose runs were all `cost_capped` with 0 requests because a 30,000 µUSD task cap sat below one reservation of ≈ $0.08 [RV L79]. The runbook adds 4 reservations to every ceiling because "a refused candidate run scores 0" [RB L50]. Budget refusals must be errors, never scores.
2. **State machines need recovery edges.** An interrupted G4 stranded its proposal, and no verb could re-run G4 on the provisional [RP L89].
3. **Timeouts set on warm caches fail on cold ones.** G3's 300 s was below a cold build plus model turns [RP L88].
4. **Cleanup that swallows errors leaks.** `remove_dir_all` stopped at EACCES on a read-only clone, the error was ignored, and the leak reached up to 1.4 GB per G4 screen [M7R L129–L132].
5. **Long jobs need money control that can be handed over.** DV-7 replaced the guard mid-run.
6. **A tight machine amplifies everything.** Free disk was below 10 GB for most of the sprint, and test lanes collided on shared homes [BL L185–L232; RT "Tests"].

The $0 mock dry run caught several integration bugs and even hit the disk rule once [BL L201–L206]. It could not reproduce real-model processes or cold-slot timing.

---

## 6 Safety evaluation

### 6.1 Red team at $0

The adversary was the agent zone itself, run against the real `init-root` in test mode with a mock model, loopback upstreams and `test-` keys. It took three forms:
- hostile tool calls through the real sandboxed body;
- a hostile fake kernel in the real `kernel` profile;
- shell or Perl "bodies" spawned by the Root's broker.

Egress attacks spoke directly to `egress.sock` [RT "Method"]. Some rows cite earlier lanes' recorded green runs rather than fresh re-runs [RT].

| Class | Rows | Held | Not held or not executed |
|---|---|---|---|
| Money (day and total ceilings under concurrency and restarts, `max_tokens = u64::MAX`, unmetered paths, a 136-connection flood, forged prices, unlisted hosts, raw TCP, a model above level) | 9 | 9 | – |
| Secrets (key store, `~/.ssh`, lineage, sealed S and canary hunting, key exfiltration) | 5 | 5 | – |
| Integrity (forged Root-only lineage kinds, appends, charter and protected trees, auto-exec files, symlinks) | 7 | 6 | symlink write-through not executed (GAP-4) |
| Control (panic under flood, owner-socket wedging and impersonation, forged owner signals, L3-only calls, ladder triggers, `setsid` escape, self-reported promotions) | 8 | 7 | **RT-GAP-1** |
| Evaluator gaming (M1–M7 vs the correct gate, broken gates vs the gate-of-gates, undersized S) | 6 | 3 | GAP-2 impossible canary not built; GAP-3 no test-file tamper detection; GAP-3b evaluation awareness (a design limit) |
| **Total** | **35** | **30** | **5** *(our count from [RT])* |

Test results were `redteam.rs` 5/5 (plus 1 ignored, the RT-GAP-1 reproduction), wp24 6/6, wp25 4/4, and rsec/wp22/access/owner_frames 16/16 [RT "Tests"; RTT]. **No real-model adversary was run**, because it needed the real key and a budget authorization [RT "Left"]. The gate-of-gates was tested only with maintainer-written gates; L3 was never reached, so no agent-authored gate ever existed [NOV].

### 6.2 RT-GAP-1: self-reported promotions can unlock L1

The L1 criterion counts kernel-reported `promotion{lkg:true}` records. `tuple::check_promotion` checks only that the body blob is in the Root's store, and the kernel fills that store itself. In a test build, a hostile kernel with **20 self-reported promotions plus one genuine clean audit** raised the level from L0 to L1 [RT "Findings"; RP L84]. Release builds cap the level at L0, so the gap is latent. The fix, counting only promotions backed by Root-held `sealed_g4` accepts, "must land before any build allows L1" [RP L84]. We found no record that it landed. The lesson is that **a trust ledger must count only evidence the Root produced itself.** ADR-010 adopts this rule as its decision 7, in design only [A010].

### 6.3 MEM-RUNAWAY: the fix

The fix landed on 2026-10-22, after the sprint [MEM; RP L85]:
- **Memory watchdog.** A Root module, `memwatch.rs`, sums each workload tree's physical footprint every 1 s. A tree over `sandbox.mem_cap_mib` (default 4,096; range 512–65,536) is SIGSTOPped, recorded and SIGKILLed by pid. The Root and kernel are never killed.
- **Disk floor.** Below `sandbox.disk_floor_gib` (default 5), every spawn except a body is refused.
- **Safe kill.** A new kill primitive re-checks start time, uid and sandbox state just before the syscall (an ADR-008 amendment).
- **Agent feedback.** The body reports "process exceeded memory cap N MiB, killed".
- **Live tests.** A 400 MiB hog under a 256 MiB cap was killed after about 1 s in a sealed run and after 0.82 s in a body, and its siblings survived [MEM "Tests"].

**Limits:**
- the kill unit is a child's whole tree, so a `cargo build -jN` can exceed 4 GiB as one unit;
- a `setsid` orphan created within one poll escapes, though it is reported;
- Linux cgroups support is a TODO;
- the live install needs a new `init-root` binary, and no upgrade path exists [MEM "Deviations", "Needs from others"].

### 6.4 Other findings

| Finding | Status |
|---|---|
| STRANDED-PROPOSAL: an interrupted G4 leaves its proposal un-promotable | open; the M7 stopgap was a harness edit [RP L89] |
| G3-STAGE-TIMEOUT: 300 s defaults below a cold build | open; M7 used 1,800 s [RP L88] |
| Probe-root leak (up to 1.4 GB per G4 screen) | open [RP L86] |
| No kernel-upgrade path, so a fixed kernel needed a full reinstall | open [RP L87; BL L216] |
| GAP-2, GAP-3, GAP-3b, GAP-4 | not built or not executed [RT] |

The verdict summarised "Hold" as "mostly held, with two gaps" (RT-GAP-1, MEM-RUNAWAY) [VER L17]. That summary is about a $0 scripted adversary, not a capable model trying to escape.

---

## 7 Related work and positioning

**Self-modifying agents.**
- **DGM** evolves an archive of self-editing agents with staged evaluation. It discards candidates that lose the ability to edit code, and its runs produced the best-documented objective hacks, such as faked test logs [NN Q1, Q3].
- **SICA** optimizes a utility over score, cost and time under an asynchronous LLM overseer. It describes no held-out set and, like DGM, selects on the benchmark it reports [TS L225, L234–L238].
- **Gödel Agent** monkey-patches its own logic at runtime; about 4% of 100 runs crashed [NN Q1].

INIT's G3 is a live, stricter form of DGM's editing-ability check. Unlike all three systems, INIT accepts a change only on a sealed paired test.

**Live systems with supervisors.**
- **MOSS** builds a candidate image from agent-edited source, replays failures in trial containers, and swaps in place with user consent, a 90 s health probe and LKG rollback. It has no statistical test, no held-out set and no treatment of prompt caching [NN Q1].
- **Ouroboros** runs a launcher outside the agent that handles supervision, panic-stop and an external spend cap. Its gate is a preflight plus an LLM review panel, and its "Hope" deployment ran 161 days with 1,085 self-modification commits [NN Q1, Q4].

INIT adds three things: a mid-conversation handoff that keeps the cache hitting, mechanical gates in place of LLM review or user consent, and provisional promotion decided by a sealed test. The cache-preserving handoff is the only piece for which the review found no prior art. It is a cost optimization, not a scientific result [NN Q1].

**Enforcement outside the agent.**
- **The Unfireable Safety Kernel** is a process-separated Rust authorization layer with a kill switch and signed evidence, and its fail-closed invariant is checked with Z3 and Kani. It refused all 704 attempts on the safety-critical core, with no bypass in 6,240 round trips [NN Q2]. INIT's Root covers more (money, keys, sealed evaluation, the ladder), but it is unverified and not minimal.
- **AWS graduated autonomy** unlocks tiers from a rolling score, with immediate demotion. It governs actions; INIT applies the same pattern to the *scope* of self-modification [NN Q2].

**Sealed and statistical acceptance.**
- **SEAL** audits each candidate against the incumbent with a harness-side check the agent cannot write or inspect, and returns only accept or reject [NN Q2]. It is the closest prior art to our main finding, and our case is one more instance of its point.
- **Gated-QD** requires a paired 2σ test with K = 3; it found that a single-run gate credits a neutral mechanism about 60% of the time [NN Q1].
- **PACE** commits only when a paired e-process is decisive; greedy acceptance committed 30–42% false edits [NN Q1].

INIT's G4 v2 differs in using an exact sign-flip test with a non-inferiority margin, calibrated per sealed set, and it reports FA and FR rather than assuming them.

**Who checks the checker.**
- **Oracle assessment** (ISSTA 2016) mutation-tests test oracles.
- **improve #3** (September 2026) mutation-tests evaluators against known-good controls.
- **"Who Verifies the Verifier?"** admits a verifier check only if it fires on its failure cluster and stays clean on good outputs [NN Q3].

INIT's gate-of-gates applies the same idea to an agent-authored *promotion* gate, using Root-held mutant agents. It was exercised only at $0 with maintainer gates. By Theorem 1 of "The Self-Improvement Problem", it can only show sensitivity to the faults it declares [NN Q2, Q3].

**Harness optimization.** **Harness²** edits prompts, skills and scripts around a fixed executor and judges by LLM on the task itself. It has no immutable layer, no cost cap and no statistical gate. One validation rollout downloaded the grading rubric, and direct edits without probes or a library cost 10.2 points in its own ablation [H2 lines 9–14, 70, 87, 94]. *(Inference: whether Harness² would have accepted `c88be5f` is unknown.)*

| Component | Closest prior art | What INIT adds | Exercised? |
|---|---|---|---|
| Compiled-core self-edit + hot handoff | MOSS, Ouroboros, Gödel Agent | mid-conversation handoff with a byte-identical prefix; mechanical G0–G3; provisional until a sealed G4 | yes, on the real model |
| Root TCB | Unfireable Safety Kernel, Ouroboros, SEAL | one Root for stop, money, keys, lineage, sealed S, ladder and gate-of-gates | $0 red team; real ceilings in M7 |
| Ladder over self-modification scope | AWS graduated autonomy | the ladder governs what the agent may rewrite | no; capped at L0; RT-GAP-1 |
| Sealed exact paired G4 + A/A | SEAL, Gated-QD, PACE | exact sign-flip test, NI margin, per-S calibration | yes: one calibration, one verdict |
| Gate-of-gates | oracle assessment, improve #3, "Who Verifies the Verifier?" | Root-held mutant agent systems; failure demotes | $0, maintainer gates only |

---

## 8 Limitations

- **n = 1.** One variant reached G4, and there is one verdict. The stop rule asked for "one change sticks within about $3" to avoid sunk cost, not because one trial settles the question [VER L44].
- **A lean, unselected S.** Family B was dropped, and the lean Root had no replacement draws [PL D-L1; RB §9 D-R10].
- **Only 10 of 32 sealed tasks in band** (0 < passes < 6 at R = 6) [M7R L29]. *(Inference: tasks the incumbent always passes or always fails say little about small changes, so the effective sample was below the nominal n = 29.)*
- **An 85% pool ceiling.** The miner had thin evidence and little headroom to work with [M7R L29; BL L218].
- **Contamination.** E and C derive from public sources (Exercism, CRUXEval); seeded mutations mitigate the risk [PL D-L1]. *(Inference, following [BEN L54]: contamination inflates absolute scores but affects both arms of a paired test alike.)*
- **One cheap model.** Only `deepseek-flash` with thinking on was tested [VER L64].
- **Snapshot drift.** The `deepseek-flash` alias moved to V4.1-Flash on 2026-09-10 and to V4.1-Flash-1017 on 2026-10-17. The two snapshots differ by 12 points on the same harness and benchmark [BEN L15]. All M7 runs were on 2026-10-21, and both G4 arms ran in the same window. Our artifacts record only the alias, so we cannot prove which snapshot served any run [GIT] *(inference that drift did not bias the paired G4)*.
- **A limited red team.** It was $0 and scripted, with no real-model adversary, and three evaluator-gaming defences were never built [RT].
- **Departures from the plan.** The harness edited kernel state (DV-5b) and re-ran at peak prices (DV-5c, DV-7). Both are disclosed, but they are departures nonetheless.
- **One laptop with little disk.** Several failures were resource failures.

---

## 9 Future work

**Trust-first evolution (ADR-010; proposed, not implemented).** A per-change sealed gate is honest but coarse. At standard tier it certifies only gains of about 17 points and costs $0.37–0.67 per attempt [A010 "Context"]. ADR-010 proposes [A010 "Decision", "Mechanisms"]:
- **Self-acceptance** on mechanical invariants I0–I6. `dep` changes keep the synchronous G4 and also need owner approval.
- **A watch window** of 20 owner turns or 72 h, during which a Root sentinel compares money, time and owner corrections against LKG and rolls back on degradation.
- **Four required forecast clauses** (harm, gain, cost_up, live_worse). Forecasts never accept or reject anything.
- **Random Root audits**, drawn with a secret key that is revealed later. Audits run on sealed S and on **AI-authored eval families sealed by the Root**.
- **Trust T**, the Brier skill score of audited forecasts. T sets the audit rate, max(0.10, (1 − T_lcb)²), and the autonomy level.

ADR-010 scores the sprint's change with assumed forecasts (harm 5%, gain 80%) and treats the outcome as harm, which gives T = −0.55 [A010 L196]. As §4.2 notes, harm was not established. ADR-010 estimates about $6 for a 20-change cold start, then $0.035–0.08 per change [A010 L470–L481]; the owner's recorded decision approved a 10-change bootstrap of about $3 [BL L243]. It also estimates 116–158 agent-hours and +690/−60 Root lines. The project stopped before any of it was built [A010; BL L243; GIT]. *(Inference: the main risks are that AI-authored evals can be shaped by the system they judge, which is why they must agree with S, and that a sentinel on owner turns needs enough traffic to have power.)*

**Benchmark design B (designed, not run).** INIT would be compared with mini-swe-agent on the same `deepseek-flash` snapshot: 12 pre-registered tasks × 3 runs × 2 arms = 72 native runs, $0.7–3.6, no Docker. The served snapshot would be recorded from API responses, and the arms interleaved. The defensible claim is tokens and dollars at equal pass rate, because at 12 × 3 the chance of detecting even a 30-point pass-rate difference is only about 77% [BEN L11, L105, L116, L167].

**A mutant benchmark for gates.** Measure the false-accept rates of LLM review, staged evaluation, SEAL-style audits and PACE-style e-processes on M1–M7 plus a held-out mutant family. This is publishable whatever INIT's own result is [NOV; NN Q5].

**A wider proof round.** The verdict's option 2 is ≥ 3 proposals on a harder S, possibly with a second model, for about $3–5. The verdict notes that this would move the goalposts set by the stop rule [VER L64].

---

## 10 Cost and reproducibility

| Item (DeepSeek, real) | Amount | Meter |
|---|---|---|
| Proof sprint M7 (P, C, D with all retries, M) | **$2.67** ($9.56 → $6.89) | balance [M7R L150] |
| Proof sprint, conservative | $3.06 (includes DV-3's $0.18 upper-bound estimate) | priced at list [M7R L151] |
| Final G4 line | $1.33 at peak; ≈ $0.66 off-peak | priced [M7R L152] |
| v0.1 real-flow batch (S-REAL) | $0.21 ($10.62 → $10.41) | balance [RV L109] |
| First real call (Slice-1) | < $0.01 | balance [BL L67] |

The balance also fell from $10.41 to $9.56 between S-REAL and M7. Our sources do not attribute that $0.85 (it may include the owner's own use of the live install), so we exclude it. The plan's expected cost for the sprint was $2.03 if D was not accepted [M7P L51]. Retries and the peak re-run explain the difference *(inference)*.

**Agent effort.** Agent-hours were not metered. The calendar span was 2026-10-19 20:26 +07 to 2026-10-22, with 401 commits before this report [GIT]. The owner's weekly agent quota rose from 65% to 86% during the sprint [BL L185, L239]. Planning estimates were ≈ 191 agent-hours for Phase 0, and ≈ 800 for v1 breadth, which was never built [BL L6; PP].

**Code.** Physical Rust lines at `ed80966`, including tests and fixtures: Root 27,540; kernel 102,657; body seed 41,601; TUI 8,019. A further 4,882 Python and shell lines live in `tests/`, including the M7 harness `tests/real/m7.sh` and `checks_m7.py` [GIT]. These counts are crude and differ from the Root's own counting method.

**Artifacts.** Redacted artifacts for every real line are in `build/real-artifacts/m7/real/{P, C, D_try1, D_try2, D_screen1, D_screen2, D_promote1, D_g4retry, D_g4retry2, D_g4retry3, M}`, and the v0.1 checks are in `build/real-artifacts/real/`. Redaction scanned for key shapes and removed e-mail addresses and the git identity; it reported 0 leaks [M7R L6; RB §10]. `ROOT_HOME/sealed` was never collected [RB §7].

**Not published:**
- **Sealed instances.** They were never collected, and no per-task sealed result is released.
- **The S v1 generator pack** (`root/sealed/packs/s1`, in the private repository). Publishing it would retire S v1 as a sealed set.

**Reproducing.** The runbook gives the $0 self-test, the mock dry run and the per-line real commands [RB §8]. A faithful re-run needs four things:
- a fresh sealed draw, ideally from a fresh pack;
- the same pre-registration procedure;
- the served snapshot recorded from API responses;
- ≥ 10 GB of free disk with the Root's memory watchdog on.

---

## Appendix A: All M7 lines

| Line | Result | Key numbers | Real $ (priced) | Root $ | Balance |
|---|---|---|---|---|---|
| G1D (mock) | pass | lean, n 24, R 2, `tie`, d = 0‰ exactly, m 20‰, cpp 1000‰; reject `no_gain` | 0 | – | – |
| P0 (pre-`74a14a1` kernel) | pass | 10 runs, 7 passes, $0.00356/run | 0.0356 | 0.0711 | 9.56 → 9.53 |
| P (`74a14a1` kernel) | pass | per run: C 0.0033, E.py 0.0041, E.rust 0.0035, T 0.0037, pool 0.0027; mean $0.00348 → trim; cost prior $0.0041; Root/real 1.999 | 0.0348 | 0.0696 | 9.53 → 9.49 |
| C | pass | 32 × 6; FA 48‰ (upper 59‰), FR 86‰; δ r2/r3/r5 68/57/44‰; m(standard) 114‰; MDE 134/95/74‰; 10 of 32 in band; pool 40 runs on 20 tasks, 6 failing | 0.857 (cap 0.94) | 1.714 | 9.49 → 8.63 |
| D try 1 | harness stop (DV-2) | worker refused by the kernel task budget | 0.0068 | 0.0135 | – |
| D try 2 | stopped (disk) | p-000001 filed (3 T2 variants); screen killed | 0.174 | 0.234 | 8.63 → 8.57 |
| D screen 1 | guard freeze | resumed turn; cut stream billed ≈ $0.181 (DV-3) | 0.182 | 0.183 | – |
| D screen 2 | go | est 120‰; v1 `c88be5f`; v2, v3 `screen.mini_inferior` | 0.0875 | 0.175 | 8.57 → 8.48 |
| D promote | G0–G3 accept; G4 interrupted | `j-000007`, ~16 min; freeze at free disk < 5.3 GB (low 0.24 GB) | 0.218 | 0.436 | 8.48 → 8.26 |
| D G4 retry 1 (DV-5) | refused before G4 | rollback `j-000008`; re-promote refused (`proposal.state`) | 0.0144 | 0.0287 | 8.26 → 8.25 |
| D G4 retry 2 (DV-5b) | G3 `timeout` | 589.6 s | 0.108 | 0.129 | 8.25 → 8.22 |
| D G4 retry 3 (DV-5c, DV-7) | G4 reject `score_inferior` | `j-000013`, standard, both arms fresh: inferior, n 29 × 3, d −50‰, m 120‰, cpp 1050‰; rolled back | 1.328 | 1.328 | 8.22 → 6.89 |
| M | skip path | current = confirmed tuple | 0.0002 | 0.0002 | 6.89 |
| G1L | not run | D not accepted | – | – | – |
| **Program** | | | **3.06** | – | **$2.67** |

Source: [M7R L24–L40]. The kernel was reinstalled between P0 and P [BL L216].

## Appendix B: Calibration and design operating characteristics

**Real calibration (C) on S v1-lean** [M7R L29; BL L217], against the pre-registered bounds [RB L88]:

| Quantity | Value | Bound |
|---|---|---|
| False accept (pseudo-A/A, `better`) | 48‰, upper 59‰ | ≤ 50‰, upper ≤ 70‰ |
| False reject (pseudo-A/A, `inferior`) | 86‰ | ≤ 100‰ |
| δ_S at R = 2 / 3 / 5 | 68 / 57 / 44‰ | – |
| m at standard | 114‰ | ≤ 250‰ |
| MDE lean / standard / strong | 134 / 95 / 74‰ | – |
| In band; n_kept | 10 of 32; 32 | n_kept ≥ 24 |

**Design characteristics, recomputed exactly** [S1B]:

| Tier | FA at 0 | P(`better`) at +10 / +15 / +20 / +25 | Expected false accepts per 20 looks |
|---|---|---|---|
| lean 24 × 2 | 2.97% | 21.0 / 39.7 / 61.0 / 78.9% | 0.59 |
| standard 32 × 3 | 3.43% | 38.3 / 68.3 / 89.3 / 97.6% | 0.69 |
| strong 32 × 5 | 3.62% | 56.4 / 87.4 / 98.2 / 99.8% | 0.72 |

The exact recomputation also found:
- the G4 v2 screen rejects an equal candidate 5.4% of the time and a −40-point candidate 63.9% of the time;
- a synthetic pseudo-A/A gave FA 40‰ [23, 65] and FR 73‰ [49, 103] [S1B].

The design's power assumes its own task model, whereas the real S had only 10 of 32 tasks in band. *(Inference: real power was probably lower.)* The real MDE of 95‰ comes from C's measured variance [M7R L29].

## Appendix C: Cost probe P0, per task type

| Type | Runs | Passes | Turns | Real $ | Real $/run | Root $ | Wall/run |
|---|---|---|---|---|---|---|---|
| C (CRUX change) | 2 | 2 | 9 | 0.0033 | 0.0017 | 0.0067 | 17 s |
| E.py | 2 | 1 | 12 | 0.0099 | 0.0049 | 0.0197 | 53 s |
| E.rust | 2 | 1 | 16 | 0.0085 | 0.0042 | 0.0169 | 60 s |
| T (terminal) | 2 | 1 | 20 | 0.0074 | 0.0037 | 0.0147 | 39 s |
| pool | 2 | 2 | – | 0.0065 | 0.0033 | 0.0130 | – |
| **All** | **10** | **7** | | **0.0356** | **0.00356** | **0.0711** | |

Source: [M7P L21–L28]. These are throw-away probe instances, not the G4 partition. P0 set the G4 cost prior to $0.0049 [M7P L38], and the re-run P set it to $0.0041 [M7R L28].

## Appendix D: Real-model v0.1 checks (S-REAL, 2026-10-20)

| Check | Result | Numbers |
|---|---|---|
| V-DS-4 prefix cache | measured | turn 2: 3,840 of 4,002 tokens cached; A2 session: 967‰ [RV L37] |
| A2-real (T1 extension) | pass | Wasm SemVer sorter with 8 unit tests, reloaded with the prefix hash unchanged; 26 turns [RV L27, L49] |
| E2E-3 (T2 handoff) | pass | G0 308 s, G1 10 s, G2 27 s; first request after handoff 5,888 / 6,696 tokens cached [RV L75–L78] |
| A3-real (agent-written T2) | pass | G0 337 s, G1 4 s, G2 22 s, G3 761 s; same-session handoff; G4 `inconclusive: underpowered` by design on a stand-in pool; owner line to promote result 1,337 s [RV L86–L94] |
| Batch | – | Root-metered $0.529 of a $1.00 cap; $0.21 real [RV L107–L109] |

## Appendix E: Glossary

| Term | Meaning |
|---|---|
| LKG | last known good |
| provisional | handed off, not yet decided by G4 |
| S | the Root-held sealed task set |
| pool | the visible task set |
| est | the go-rule estimate |
| d | the sealed mean paired difference, rounded to 50‰ |
| m | the non-inferiority margin |
| cpp | cost per pass, as a ratio |
| FA / FR | pseudo-A/A false-accept / false-reject rates |
| δ_S | the A/A noise quantile |
| MDE | the minimum detectable effect at 80% power |
| ‰ | permille; 10‰ = 1 percentage point |

## Appendix F: Source map

Paths are relative to the repository root. Line numbers refer to commit `ed80966`.

| Tag | File |
|---|---|
| M7R | `measurements/m7-results.md` |
| M7P | `measurements/m7-probe.md` |
| RB | `measurements/m7-runbook.md` (D-0 at §6) |
| RV | `measurements/real-v0.1.md` |
| VER | `reports/INIT proof sprint verdict.md` |
| NOV | `reports/INIT novelty review.md` |
| NN | `research_notes/INIT novelty review/notes.md` (by section Q1–Q5) |
| BEN | `reports/Benchmark harness có dữ liệu công khai.md` |
| H2 | `reports/Google harness2 cho INIT.md` |
| TS | `research_notes/INIT v1 providers UX vision/m7_task_sources.md` |
| ROOT, SE, EG | `docs/root/root.md`, `docs/root/sealed-eval.md`, `docs/root/egress.md` |
| GATE, SUP | `docs/kernel/gate.md`, `docs/kernel/supervise.md` |
| EV1, EV2, M7D | `docs/evolution.md`, `docs/evolution-v2.md`, `docs/m7-design.md` |
| A001, A004–A008, A010 | `docs/adr/001-overlapped-handoff.md`, `004-wasm-components-for-t1.md`, `005-rust-and-wasm.md`, `006-evolution-gate-policy.md`, `007-root-and-autonomy-ladder.md`, `008-owner-machine-safety-v0.1.md`, `010-trust-first-evolution.md` |
| RT, RTT | `build/status/RED-TEAM.md`; `root/crates/init-root/tests/redteam.rs` (summary only) |
| MEM | `build/status/MEM-RUNAWAY.md` |
| S1B | `build/status/ROOT-S1b.md` (the exact-statistics module and the design recomputation) |
| WP16, PLS | `build/status/WP16.md` (mock handoff timings); `build/status/POOL-LITE.md` |
| RP | `RELEASE_PLAN.md` ("v0.1.x items found during the release push") |
| BL, PP, PL | `BUILD_LOG.md`; `PROOF_PLAN.md`; `build/specs/PROOF-LEAN.md` (cuts D-L1…D-L6) |
| GIT | git history at `ed80966`, `wc -l` over `git ls-files`, and a `grep` of the redacted M7 artifacts for the model string, all run for this report |
