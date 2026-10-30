# ADR-011: Reward-free self-extension (trust at creation, gate on generic harm, reversibility instead of reward)

Status: proposed 2026-10-22, PENDING OWNER (open questions below); built and shipped from v0.2.0-preview behind `extend.mode = "free"`, with `"gated"` still the default (charter.rs `Extend`); **supersedes ADR-010** (accepted 2026-10-22, never built beyond W0a); enabled only behind charter `extend.mode = "free"` once the definition of done is met (supersedes ADR-006 decisions 3, 4, 5 and 7 on the free path; redefines ADR-007 decision 3; ADR-007's Root and decisions 1, 2 and 6 are unchanged) · Area: evolution (cross-cuts root and kernel)
Depends on: ADR-006, ADR-007, ADR-008, ADR-009, ADR-010 (superseded; the reused parts are listed per item), evolution.md (§3.1–§3.3, §3.8–§3.12, §3.14, §3.15), evolution-v2.md (§3.4.6, §3.6, §3.7), kernel/supervise.md (§3.1, §3.3, §3.6, §3.7, §3.14, §3.15), kernel/gate.md (§3.1, §3.12), root/root.md (§3.2.3, §3.5, §3.6.4, §3.7, §3.8, §3.10.5, §3.13, §3.14.1, §3.15, §3.17), root/egress.md (§3.4–§3.6, §3.8), root/sealed-eval.md (§3.3, §3.5), RELEASE_PLAN.md (v0.1.x findings), reports/INIT proof sprint verdict.md (with its erratum), reports/INIT technical report.md, research_notes/INIT và đối thủ thị trường/{coding_agent_market.md, self_evolving_products.md}   Defines: the plastic-surface map and the kernel lane; capability identity (`cap`); change groups; admission checks A0–A5; the Root harm sentinel and the generic harm signals H1–H12 (definitions, thresholds, windows, actions); the Root system snapshot, the Root-run rollback probe and the body mode `check`; the blast-radius budget; the usage ledger; decay, archive, quarantine and the bloat budget; the harm-free record and levels L0–L4 as redefined here; owner grants; the frozen twin and nightly shadow replay; the pointers `settled` and `twin`; the records `change_open`, `change_close`, `harm`, `snapshot`, `restore`, `cap_state`, `cap_use`, `grant_request`, `grant`, `twin`, `twin_report`; the charter section `[extend]`

## Context

**The owner's vision (binding).** Most agent harnesses will not scale with very capable models. INIT should be a harness that gives such a model as much room and flexibility as possible. Many things a model would want to build for itself have no reward that could exist beforehand: a new memory structure, a tool for a kind of task nobody has seen, a different planning loop, a sub-agent pattern. A gate that needs a predefined reward or measurement blocks exactly the open-ended growth that matters.

**What ADR-010 still gates on.** ADR-010 dropped the per-change G4, but it kept a predefined measure at the centre:
- trust T is a Brier skill score on four protocol-fixed events, and `gain` is defined on the sealed set S or on Root-sealed families. A capability that S does not exercise can at best score "no effect";
- autonomy (audit rate, evolution share, level) comes only from T;
- Root audits cap the rate of change: when `trust.audit_usd_day` ($1.00) is spent, the Root refuses new changes, so "the agent cannot change faster than it can be verified" (ADR-010 §4);
- the cold start costs about $6 real, with every change audited until 20 are scored;
- every change must carry four forecast clauses.

An open-ended capability therefore pays for measurement and earns nothing toward autonomy unless S happens to see it.

**What the sprint showed about predefined instruments** (proof sprint verdict, erratum included; technical report §4):
- At standard tier the sealed G4 has 80% power only at about +17 points (MDE 95‰), and its `inferior` status has a measured false-reject rate of 86‰.
- The one real self-change measured d = −50‰. That is inside the A/A noise δ_S(R = 3) = 57‰, so it shows no detectable change, not harm.
- One verdict took four G4 attempts and seven harness deviations.
- The instrument could not tell whether a small, concrete, well-specified change helped. For capabilities nobody anticipated it can tell even less.
- The lesson that survives: **an evaluation the proposer can see or shape is not evidence.** The +120‰ was a prior-weighted go-rule estimate, and the raw visible screen was a tie.

**What competitors show** (research_notes/INIT và đối thủ thị trường, SEP = self_evolving_products.md, CAM = coding_agent_market.md):
- **OpenClaw** lets the agent write skills and its own code freely, with no containment:
  - 341 of 2,857 ClawHub skills were malicious (ClawHavoc, SEP L186);
  - an agent rebuilt its live `dist/` and deadlocked (SEP L195);
  - a compaction dropped an approval instruction, the agent bulk-trashed a real inbox, and "STOP" was ignored (SEP L190–L193).
- **Hermes** saves a skill after any task its own end-of-task check calls successful, and that check over-reports success, so bad skills are saved (SEP L142).
- **Live-SWE-agent:** online tool creation cut a small model from 44% to 14% (SEP L123).
- **SkillsBench:** self-generated skills give no average benefit (SEP L141).
- **No product tracks which skill was used, decays unused ones or quarantines harmful ones.** MOSS and Prime Agent can roll back, but they have no attribution. No one compares a self-modifying agent with its frozen seed on the same user workload (SEP L160). The market gap is containment (CAM L220–L224).

Freedom without containment fails in the field, and containment that needs a predefined measure (ADR-010, M7) does not scale.

**The observation this ADR rests on.** The value of an arbitrary new capability cannot be known in advance. Its **harm and cost** can be measured for any change without knowing its purpose:
- money;
- scope refusals;
- the owner stopping, undoing or rejecting;
- crashes and boot loops;
- resource kills;
- error spikes.

And any change can be made **mechanically reversible**. So in admission, reversibility, a Root-held harm sentinel, attribution and selection by use replace reward.

**What this gives up, stated plainly.** At creation INIT will not know whether a capability is useful. It will know whether the capability hurt, how often it is used, and what it costs. Usefulness shows only in the real-use trajectory against a frozen twin, over weeks (Evaluation plan). This ADR makes no claim that self-extensions help.

## Decision

**What "reward-free" covers.** It describes admission: no change needs a reward, a score or a forecast to go live or to stay (decisions 1, 2 and 9). The autonomy ladder is not reward-free. Its levels are earned from measured outcomes, a clean record the Root observes (decision 7, §7), so it is open to Goodhart pressure; its safety argument is that levels widen speed and blast radius only under reversibility, never the Root's powers (docs/foundations.md N3).

1. **Trust the model at creation.** Any change on the plastic surface (Mechanisms §1) goes live after admission checks A0–A5 (§2). These test only that the change is reversible, within budget, and clear of the Root. The default path has no G4, no S look, no audit draw, no forecast and no reward. `Predicts:`, `Repro:` and `Compensates-for:` become optional.
2. **Gate on generic harm, never on gain.** The Root's harm sentinel watches every change group through a window, and the running system continuously, with signals H1–H12 (§3). Every signal that can count against the agent is observed by the Root itself. A harm signal rolls back and may quarantine; nothing in this ADR accepts on evidence of gain. A signal the kernel or body reports can only veto (request a rollback, the safe direction) and never counts toward the agent's record.
3. **Reversibility replaces reward as the safety principle.** Before a change goes live, the Root snapshots the agent's state and runs its own rollback probe (§4). A blast-radius budget bounds how much unclosed change can exist at once. External side effects that cannot be undone are fenced by scope rules and listed plainly (§4.6).
4. **Attribution.** Every self-written capability has an identity (`cap`). Every turn records the capabilities it used, observed by the Root where they are visible on the wire. The lineage links harm to change groups and to capabilities (§5).
5. **Selection by use.** Unused capabilities decay and are archived. Capabilities correlated with harm are quarantined and removed. A bloat budget bounds growth (§6). Nobody defines "useful" in advance; what survives is what gets used without harm.
6. **Everything except the Root is plastic, the kernel included,** through a kernel lane from L1. It has a Root conformance probe, probation and whole-system rollback, and the gate-of-gates becomes optional (§1.2). The Root stays mechanical: stop and panic, money ceilings, keys and egress, scope, signed lineage, rollback and snapshots, the memory and disk watchdog, the harm sentinel and the ledger. **No level, grant, record, change or proposal gives the agent a path to remove or weaken the Root** (owner rule; ADR-007 decision 6).
7. **Autonomy from a harm-free record, grants from the owner.** Levels L0–L4 are redefined (§7). They scale rate and blast radius, and they are earned mechanically from Root-written records only. This is the part of the design that uses measured outcomes: a clean record is the ladder's reward (foundations.md N3, FG-2). Anything beyond the charter is a grant: the agent may request it, and only the owner can approve it. L0 already allows the whole body surface.
8. **Self-extension in the middle of a task.** A capability can be created, kept and used inside the same task: by reload (T0, T1) and by hot handoff on the same session (T2). Admission costs minutes and no model spend (§2.4).
9. **Measurement is an instrument, not a gate.** `Measure: g4`, sealed S, G4 v2, the milestone check, agent-written task families and agent-designed experiments stay available on request (§8). Their results feed lessons and the agent's own decisions. They never decide admission, and they never decide a level.
10. **Honest evaluation is longitudinal.** The only credible evidence is the real-use trajectory compared with a frozen twin, with nightly shadow replay of sampled past tasks under a Root budget (Evaluation plan). It is report-only, and it cannot be benchmarked up front.
11. **Rollout switch.** Charter key `extend.mode = "free" | "gated"`. `"gated"` keeps today's built path (G0–G3 and G4, before ADR-010). `"free"` enables this ADR, and becomes the default only when the definition of done (Work breakdown) is met. ADR-010's `trust.accept` key is never introduced.

## What is removed and what is kept

| Item | Before (ADR-010 or earlier) | Under ADR-011 |
|---|---|---|
| G4 per change (gate.md §3.9) | ADR-010: off the default path; `dep` kept a synchronous G4 | **an instrument only.** `Measure: g4` runs it after admission and reports; `Measure: g4 gate` lets the agent make it gate its own change (§8). `t0_escalated` and `dep` no longer need it. |
| Sealed S, looks, rotation, canaries, leak scans; milestone check; `confirmed` and `audited` pointers | ADR-010: an audit source and the anchor | **kept as optional instruments**, run on request or on the charter's schedule. A sealed-audit or milestone `regression` is reported and alerted, and is no longer a demotion trigger or a rollback (open question 4). Rollback targets move from `audited` to the new `settled` pointer (§4.1). |
| Four forecast clauses, trust T, Brier skill, audit draw, epoch keys, forms S/F/P/E, `trust.audit_usd_day` | ADR-010 §2, §4–§6 (not built) | **removed.** The record kinds `watch_open`, `audit_draw`, `audit_change`, `audit_kernel`, `trust`, `trust_epoch` and `family_band` are never introduced. Free `Predicts:` clauses are still scored into `refs/notes/predict` whenever an instrument measures them. |
| Evaluator role, Root-sealed generated families | ADR-010 §7 (not built) | **dropped as a role.** Task families (evolution.md §3.15.4) stay available from L0 as `Measure: exp:<family>` (§8). Their results are advisory. |
| Invariants I0–I6 | admission | **kept as reversibility checks** A0–A5 (§2.1). I6, the rollback probe, moves into the Root. G3 (I4) runs after the handoff and can only veto. Clippy and the body's own tests become advisory or veto-only. |
| Watch window and sentinel H1–H7, S1, S2 (ADR-010 §3; designed and simulated, not built) | rollback on blatant harm | **kept and extended:** the turn definition, the baseline rings, the exact exceedance and binomial tests and their thresholds are reused. The set grows to H1–H12, and windows apply to change groups (§2.2). S1 becomes the close flag `flagged`, and S2 (drift) moves to the twin. |
| Depth ≤ 3 open changes | ADR-010, supervise.md | **replaced** by the blast-radius budget (§4.5), counted in change groups |
| `dep` scope: synchronous G4 plus owner `/approve` (ADR-010 owner decision 5) | | **G4 dropped; owner approval kept** as a `dep` grant (§7.4), because supply-chain risk is a harm the sentinel cannot see (ClawHavoc) (open question 5) |
| No external actions while provisional (F17) | | **kept, relaxed:** an effectful action class is refused only while the newest open group is younger than 2 h or has seen fewer than 5 clean owner turns (§4.6) |
| Ladder L0–L4: rights, `SUPPORTED_MAX_LEVEL`, criteria (root.md §3.13) | ADR-010: criteria from T | **redefined** (§7): rights scale rate and blast radius; criteria come from Root-written harm records; the kernel moves from L3 to L1 |
| RT-GAP-1 | ADR-010 decision 7 | **closed by construction:** no level criterion reads a kernel-reported record; the ignored `redteam.rs` test becomes a passing negative test |
| Gate-of-gates, mutation suite M1–M7 | required for kernel candidates from L3 (ADR-010 owner decision 8) | **optional:** required only when a kernel's gate is used for `Measure:` evidence (open question 1) |
| `min_promotions`, `audit_clean_days`, `min_audits`, trend | L1, L2 criteria | **deleted** |
| Retirement listing (evolution.md §3.11) | list-only; the agent decides | **superseded** by decay and archive (§6), which the Root enforces through archive debt |
| Thinning on model change (evolution.md §3.12) | `thin` commits under G4 | kept as an agent tool, admitted like any change; no baseline G4 |
| `t0_batch`, T0 debt (evolution-v2 §3.6.2) | | **removed**: it was a reward-based harm test. Harm from T0 text is caught by the sentinel and by harm correlation (§6.3). |
| `Repro:` mandatory for behaviour-changing T0 text, `fix`, `guard`, prefix changes (evolution-v2 §3.6.1; evolution.md §3.8) | admission | **optional metadata.** A skill written from documentation, with no observed failure, may be kept; if it is not used it decays (§6.2), and while it lives it counts against the bloat budget (§6.4). |
| Commit grammar and validator (evolution.md §3.8–§3.9) | | kept for identity and history. The mandatory-trailer matrix relaxes to `Why` and `Prefix-change` only. New trailers: `Cap:` (repeatable) and `Measure: exp:<family>`. |
| Evolution engine (miner, worker, screen, go rule; evolution-v2 §3.4–§3.5) | | kept as an optional workflow; the go rule no longer spends S looks |
| Lessons library (evolution-v2 §3.7) | | kept; entries may also cite `harm`, `change_close`, `cap_state` and `twin_report` records |
| No LLM judge (ADR-006 decision 8) | | **kept:** no LLM decides a rollback, a quarantine or a level; the sentinel is integer statistics |
| MEM-RUNAWAY watchdog (ADR-010 W0a) | prerequisite | **done** (root.md §3.14.1). It is H6's source. |
| ADR-010 owner decisions of 2026-10-22 | Root +≈630 lines; a 10-change bootstrap of ≈ $3; audit floor 10%; low-harm-rate path; `dep` keeps G4 plus owner approval; `/bless` at low weight; maintainer S as the only human anchor; L3 keeps the mutation suite | superseded item by item: Root growth (open question 2); no bootstrap spend (the $3 authorization is unused); no audits; the harm-rate path becomes the design; `dep` keeps owner approval only; `/bless` is recorded and reported, and gates nothing; S remains an instrument; the kernel lane drops the mutation suite (open question 1) |

## Mechanisms

### 1. The plastic surface

#### 1.1 Map

| Surface | What the model may change | Scope (gate.md §3.1) | From | Goes live by | Rollback unit |
|---|---|---|---|---|---|
| Skills, `SELF.md`, `GENESIS.md`, lessons | anything, including wholesale rewrites | `t0` | L0 | reload at a step boundary (evolution.md §3.3) | tuple `t0` blob |
| **Sub-agent specs** (new T0 path `agents/<name>/AGENT.md`: role prompt, tool subset, model route, budget share) | create, change, delete; spawn members with them | `t0` | L0 | reload; members start with `fleet spawn{agent}` | `t0` blob |
| Tools for new kinds of task: Wasm extensions (T1) | anything inside `init:ext@1.x` | `t1` | L0 | `initctl reload` (registry swap at a step boundary) | ext set |
| **Memory structures**: code (T1 extension or T2 module) plus data under `state/ext/<name>/data/` or the new `state/mem/<name>/` | schema, indexing, retrieval, what enters context | `t1`/`t2` | L0 | reload or handoff | code: tuple; data: system snapshot (§4.2) |
| Native tools, the agent loop and **planning loops**, context assembly, compaction, provider adapters, the extension host: the compiled body (T2) | anything in `seed/` Rust, `forbid(unsafe_code)` kept by the build | `t2` | L0 | hot handoff on the same session (supervise.md §3.6.4) | body binary |
| Model routing per subtask among the charter's models for the level | routing policy | body or kernel | L0 (was L1) | as its tier | as its tier |
| Soft budgets under the Root ceilings (evolution.md §3.14) | the allocation | kernel | L0 (was L1) | kernel config overlay | — |
| Third-party crates | add, remove, update | `dep` | L0, with a `dep` grant (§7.4) | handoff | body binary |
| **The kernel**: supervisor, gate, proxy features, selfmap, initctl, the evolution engine | anything; it must keep speaking the Root's control-link protocol version | `k` | **L1** (§1.2) | kernel lane: `system.swap` (root.md §3.8) | whole tuple plus the `kstate` snapshot |
| The model as a component (adapters, fine-tunes on owned infrastructure) | as evolution.md §3.15.7 | — | L4 and charter classes | — | — |
| **Not plastic: the TUI, the `init-agent` launcher, the installer, LaunchAgents** | nothing; they run unsandboxed as the owner, so an agent-written TUI would be a sandbox escape. The agent shapes the UI through the owner-channel frames it sends (OW-TUI). | — | never | — | — |
| **Not plastic: the Root** (`init-root`, `root/sandbox-profiles`, the zone tail, egress, keys, charter, lineage, the store, snapshots, the memory watchdog, the harm sentinel, the ledger, the twin) and Root-held material (S packs, the reserve, canaries, the mutation suite) | nothing. The agent may file `proposal.file` text, which nothing in the Root reads back (root.md §4.1.3). | — | never | — | — |

**T0 paths** become `skills/**`, `agents/**`, `SELF.md`, `GENESIS.md`. The `t0` blob (root.md §3.7.1, tuple v2) gains `agents/**`. `archive/**` and `quarantine/**` (§6) are body paths that the body does not load, and they are outside the `t0` blob.

**Trials** (an uncommitted T0 edit, or a T1 build run from `work/target/`) stay as today: session-scoped, and reverted by a restart or by a system-snapshot restore. They are not changes, so they open no window. The usage tap still sees them when they surface as tool calls (§5.2).

#### 1.2 The kernel: recommendation

**Why it is a real option now.** Under ADR-007 and ADR-010 the kernel was the evaluator: it ran G4 and decided acceptance. Its plasticity therefore needed the gate-of-gates to keep an agent-written evaluator honest. Under this ADR nothing safety-relevant depends on the kernel's honesty:
- harm signals, snapshots, the rollback probe, change records, cap identity for T0 and T1, and levels are all Root-side;
- the zone's sandbox profiles are Root-composed, so an agent kernel gets exactly the maintainer kernel's rights;
- the kernel orchestrates. An agent-written kernel can break itself (H1, H2: Root rollback), waste money inside the ceilings (H5, H9), skip or fake the zone-side checks A1–A3 (quality only), or misreport zone-evidence attribution (§5.2: never counted toward the record).

**Options.**

| Option | What | For | Against |
|---|---|---|---|
| K-A | the kernel stays maintainer code | smallest risk | contradicts "everything except the Root is plastic"; the supervisor, proxy and engine cap what the model can build |
| K-B | the kernel is plastic from L0 | the most flexible | a kernel change's window has no calibrated baseline on a fresh install (the 40-turn ring is empty), so H5b–H11 have no power. The first kernel failure takes every body and the relay down. |
| **K-C** | **the kernel is plastic from L1, through the kernel lane** | as flexible as K-B after about 7 days; the baseline is calibrated; the agent has shown 10 harm-free groups on the body first | about a week of delay, once |
| K-D | plastic policy, fixed mechanism | a smaller blast radius | the line between policy and mechanism is arbitrary, and the engine already lives in the body as skills (evolution-v2 §3.4.6) |

**Recommendation: K-C,** without the gate-of-gates requirement (open question 1; it reverses ADR-010 owner decision 8, which kept the mutation suite for L3). The owner leans toward maximum flexibility. K-C delays kernel plasticity only until the sentinel can see harm, and the reason is mechanical, not caution.

**The kernel lane.**
- Level ≥ L1 (`root.level` otherwise: a demoting rule-(b) refusal, as today).
- One kernel change open at a time. No body group may be open while a kernel change is in probation, so a harm signal is attributable.
- Rate: 1 per 24 h at L1, 3 at L2, 6 at L3–L4.
- **Admission:**
  - A0 and A5 (§2.1);
  - the candidate's K0 build (kernel-run, quality only);
  - the **Root conformance probe**. In a Root-owned test home built from `kernel/kupgrade-smoke/`, the candidate must:
    - send `hello` with proto 1 within 30 s;
    - become `ready`;
    - answer `ceilings.get`;
    - `spawn` the LKG body in mode `check` and see it exit 0;
    - stop with exit 0 within the grace on `stop`;
    - exit 75 within 10 s on `prepare_swap`.

    The probe is $0 and takes about 2–5 minutes.
- **Going live:**
  - root.md §3.8 phases 3–6, unchanged: `kstate` snapshot, install, `ready` within 1,200 s, probation 1,800 s;
  - then a kernel window of 40 owner turns or 120 h, at least 24 h, with every H signal active.
- **Rollback:** the previous tuple plus `kstate` (existing), and the system snapshot if the parent kernel cannot run on the state (§4.3).
- **The gate-of-gates** runs only if the agent asks for it (`Measure: gog`) or if the kernel's gate is used for `Measure:` claims.

**Risks of K-C and what holds them:**

| Risk | Held by | Residual |
|---|---|---|
| a kernel that "works" but degrades the owner's experience (drops relayed lines, mangles context, inflates cost) | H2 deafness, H7 corrections, H5b cost, H8 wall; twin replay (Evaluation plan) | slow degradation within noise until the weekly twin report |
| it skips A1–A3 for body changes | A0, A4 and A5 are Root-run, so reversibility holds | broken bodies reach a window more often; H1 rolls them back |
| it ignores `extend.rollback` | the Root's whole-system rollback after 120 s (root.md §3.7.3) | none |
| it forges usage or cap reports | they are tagged `evidence:"zone"` and never count toward levels (§5) | quarantine power is lower for code-level caps |
| it reads kernel-held measurement material (`vault/gate/` pool, holdout, canaries) | these are optional instruments now | the kernel holdout loses its sealed status from L1; only Root-held S stays sealed |
| it consumes quotas: builds, disk, spawns | broker quotas, `sandbox.disk_floor_gib`, H6 | none new |
| a kernel bug bricks the zone | Root tuple rollback, crash-loop escalation, system safe mode, `Halted` retry (root.md §3.6.4, §3.6.6) | downtime of minutes |
| a hostile kernel probes the Root's parsers (control link, egress) | unchanged: serde, size caps, `deny(unsafe_code)` outside `sys.rs` (root.md §7) | the same residual as an L3 kernel in ADR-007 |

**What would change the recommendation:** if the red-team rows for a hostile plastic kernel (X8, RF-28 to RF-30) show a path in which the Root cannot restore a working tuple without the owner, fall back to K-A until that path is fixed.

### 2. Admission: reversibility checks

#### 2.1 Checks A0–A5

| Id | Check | Runs in | Cost | Fails as |
|---|---|---|---|---|
| **A0** | Boundary. The Root re-hashes its own binary against `root_start`, the active charter digest and the embedded profiles, and checks that the candidate's blobs are in its store with the claimed digests. Root paths are out of the zone's reach by construction (tail Z1–Z7); A0 makes that a recorded fact per change. | Root, at `change.open` | $0 | `root.boundary` (alert; nothing opens) |
| **A1** | Build: offline, frozen, in the kernel's build slot (G0's build and build-policy steps). Clippy and the body's tests run but are advisory; a failing test may veto later (A3). | kernel | $0 | `gate.g0.build`, `gate.g0.policy` |
| **A2** | Boot: the candidate boots in a kernel probe, reaches `ready` and makes one tool round trip; the head-prefix check runs unless `Prefix-change: yes` (G2's candidate arm). | kernel probe | ≈ $0.002 | `gate.g2.*` |
| **A3** | After going live, asynchronously: G3 (scripted self-edit) for `t2`, plus the body's own tests. A failure is a zone veto (H12): the kernel sends `change.veto`. | kernel | ≈ $0.01 | rollback of the group |
| **A4** | **Rollback probe, run by the Root.** In a Root-owned probe root (§4.3) holding an APFS clone of the live `state/`, the Root boots the candidate body in mode `check`, then the **parent** body in mode `check` on the state the candidate left. Both must write `ready` and exit 0 within 90 s each. | Root | $0 | `extend.rollback_incompatible` |
| **A5** | Budget: the blast-radius budget (§4.5), the bloat budget (§6.4), no archive or quarantine debt (§6), the disk floor plus 2 GiB (free disk that cannot be read refuses too, since v0.5.3), and room for the system snapshot (§4.2) | Root | $0 | `extend.budget`, `extend.bloat`, `extend.archive_debt`, `extend.quarantine_debt`, `extend.snapshot_too_large`, `root.quota` |

| Scope | Checks | Latency (only measured parts; 2026-10-29) |
|---|---|---|
| `t0` | A0, A5 | not timed; the system snapshot is a copy-on-write clone |
| `t1` | A0, A1 (the ext workspace, built from source; never the agent's `.wasm`), A5 | not timed on the free path |
| `t2`, `dep` | A0, A1, A2, A4, A5; then A3 asynchronously | not timed on the free path as a whole. Its parts, timed on the gated path on an M2: A1 is G0's build and tests, 126 s warm (MERGE-T2CACHE, load 10–23) and 430 s to 975 s cold (the v0.5.0 and v0.5.2 release checks, the second beside CI; docs/guide.md "Time"); A2 is G2, 19–22 s in the same runs; A4 is bounded at 90 s per arm (snapshot.rs) |
| `k` | A0, K0 build, Root conformance probe, A5 | not measured |

A1–A3 are kernel-run, so from L1 they are agent code: they filter quality and give no safety guarantee. A0, A4 and A5 are the guarantee.

**Body mode `check`** (new; body/boot.md). `INIT_MODE=check`: the body boots, rehydrates the session named by `INIT_SESSION`, may migrate its state, writes `ready`, and exits 0. It makes no model call and needs no kernel socket. The genesis body implements it, and a body that drops it fails A4 and cannot be admitted, so the contract keeps itself.

#### 2.2 Change groups

- A change joins the newest open group if that group was opened in the same owner turn (or in the same background turn, §3.1) and has fewer than `group_max` changes (16 at L0). Otherwise the change opens a new group.
- A group's window runs from its **last** join.
- A new version of a capability that is already in an open group replaces it within that group.
- So the tools a model builds during one task form one unit: one window, one rollback unit, one attribution.

#### 2.3 Sequence

1. The agent commits and runs `initctl promote` (with or without `--wait`).
2. The kernel validates, runs A1 and A2, stores the blobs (`store.put`), and sends `change.open{…}` (§Records).
3. The Root runs A0, A5 and A4. It takes the system snapshot (§4.2), plus a workspace snapshot if a workspace is set (ADR-009 §4). It appends `change_open` and returns `{seq, group, snapshot}`.
4. The change goes live: handoff (`t2`, `dep`), reload (`t0`, `t1`) or swap (`k`).
5. A3 runs asynchronously. The sentinel watches the group (§3).
6. At close the Root appends `change_close`. It moves `lkg` when the outcome is `clean`, `quiet` or `flagged`, every older group is closed, and no rollback is pending.

The Root refuses a kernel `promotion{lkg:true}` that does not match a closed group, so LKG stays Root-anchored, as ADR-010 made it. The broker refuses a live body whose binary, ext set and `t0` digest match neither `lkg` nor an open group (`root.unwatched_change`, a demoting rule-(b) refusal). An agent kernel therefore cannot run an unannounced change.

#### 2.4 Mid-task self-extension

The path exists today for trials (evolution.md §3.3–§3.5). What changes is that **keeping** a capability no longer waits for G4:
- **T1 tool in the middle of a task:**
  1. write `ext/<name>/` from the template, `cargo build`, `initctl reload`, and use it at once as a trial;
  2. commit with `Cap: ext/<name>` and `initctl promote`;
  3. admission runs (not timed on the free path; §2.1);
  4. the kept component replaces the trial at the next step boundary, and the task continues.
- **T2 core change in the middle of a task:**
  1. worktree, commit, promote;
  2. after admission (§2.1 has the measured parts) the overlapped handoff runs at the next step boundary on the same session log;
  3. the prefix stays byte-stable unless `Prefix-change: yes`.

  The conversation continues (supervise.md §3.6.4; E2E-3 read 5,888 of 6,696 prompt tokens from cache after a handoff).
- `initctl promote` without `--wait` lets the model keep working while admission runs.
- G3 no longer sits before the handoff, so G3-STAGE-TIMEOUT no longer delays a T2 change (X0c still fixes it).

### 3. The harm sentinel (Root)

All signals come from the Root's own observations: the egress ledger and tap, the broker and reaper, the owner socket, `memwatch.rs`, the watcher, and the Root's refusals. The tests are ADR-010 §3's exact tests (integer combinatorics), reused unchanged where noted.

#### 3.1 Turns

- **Owner turn** (ADR-010 §3): starts when the Root relays an owner `line`. It ends at the first 30 s with no zone egress request in flight, or after 15 minutes.
- **Background turn** (new): a maximal span of zone egress outside owner turns with gaps under 30 s, capped at 15 minutes. It covers idle-time evolution, sub-agents working alone and replays the agent starts.
- **Per-turn quantities:**
  - `cost`: zone egress spend in the turn;
  - `wall`;
  - `requests`;
  - `sub_requests`, `sub_cost` (v0.3.0, MERGE): the part of `requests` and `cost` that `delegate` sub-tasks made (the kernel forwards the body's `x-init-subtask` as `x-egress-subtask`, body/tools.md §3.7). H10 and H5b test the main loop's share (`requests − sub_requests`, `cost − sub_cost`), so using `delegate` is not read as a per-turn spike. H5a, H8, H9, H11 and zone spend still count every request;
  - `req_bytes`: request bytes sent upstream;
  - `provider_errors`;
  - `correction` (0/1, owner turns only): an owner interrupt, meaning an `interrupt` frame (Esc or Ctrl-C, also `then:"continue"`), `/stop` alone on its line, `/undo`, `/reject`, `/rollback` or `/panic`. Several in one turn count once. A plain steer (a line relayed while the turn is in progress), a `/later …` follow-up or a retracted line is **not** a correction (v0.3.0, MERGE: smoother steering must not inflate H7 or the corrections trend);
  - `steered` (owner turns only; a weak signal that gates nothing): the plain steers of the turn. A relayed `retract{seq}` takes its line back (the kernel echo's `seq` names the line);
  - `crash`;
  - `kill` (a `mem_cap` or `disk_floor` event of a live-zone unit);
  - `refusal` (a demoting Root refusal).
- **Harmed turn** (for correlation, §6.3): `correction`, `crash`, `kill`, ≥ 1 provider error, a 15-minute cap, or `refusal`.

#### 3.2 Windows and baseline

> **v0.5.1 (2026-10-28): the code differs from this design as follows** (sentinel v2_fix, adopted 2026-10-22;
> build/status/X10.md). The text below is the design as proposed and is kept as it was.
> - **The baseline ring holds 40 *units*, not 40 owner turns** (`RING_M`, root/crates/init-root/src/sentinel/engine.rs:18). A unit is a run of
>   consecutive owner turns of one owner session with gaps under 30 min, at most 6 turns (`UNIT_GAP_MS`, `UNIT_MAX`,
>   :40–41; cut at :526); its value is the geometric mean of its turns' values (`unit_values`, :360). The ring is full
>   at 40 units (:406), so the acute-only phase lasts longer than 40 turns.
> - **α is not 0.002 throughout.** H5b, H8, H10 and H11 compare units against the **8th** largest of the ring (j = 8,
>   `RING_J_COST`, :20) at **α = 0.005** (`HARD_COST`, :21; the loop at :407). H7 (per turn, :416) and H9 (per
>   request, :423) keep α = 0.002 (`HARD`, root/crates/init-root/src/sentinel/stats.rs:11). S1 keeps j = 10 at
>   α = 0.05, on units for cost, wall and zone spend (`soft`, :431).
> - The looks are still the window's first 5, 10 and 20 owner turns (`LOOKS`, :26), but each test counts the units
>   in them, so a 20-turn look in one long session holds as few as 4 units.

- **Group window:**
  - closes at `win_turns` = 20 owner turns or `win_max_h` = 72 h after the group's last join, whichever comes first, and never before `win_min_h` = 2 h;
  - kernel changes use 40 turns, 120 h and 24 h;
  - fewer than 5 owner turns at close gives the outcome `quiet`: no reality signal, so it does not count as clean (§7.1), though `lkg` may move.
- **Baseline ring:** the last m = 40 owner turns served by a tuple whose groups had all closed `clean`, plus a request-level ring of the last 400 requests for H9. Until the ring is full, only the acute signals run. That is the mechanical reason the kernel lane starts at L1.
- **Looks:** chronic tests look at n = 5, 10 and 20 window turns, at α = 0.002 per test per look (ADR-010 §3).

#### 3.3 Signals

> **v0.5.1 (2026-10-28): the code differs from this table as follows** (root/crates/init-root/src/sentinel/engine.rs; see the note in §3.2).
> - **H5b** uses j = 8 of m = 40 *units* (not j = 10 of 40 turns) at tail ≤ **0.005** (not 0.002): K ≥ 5 of 5,
>   ≥ 7 of 10, ≥ 11 of 20 window units (`RING_J_COST`, `HARD_COST`, :20–21; root/crates/init-root/src/sentinel/tests.rs:60
>   checks this table). The 1.5× magnitude guard is unchanged.
> - **H8, H10 and H11** test as H5b does, so they too use j = 8 of 40 units at α = 0.005 (:407–409), with their own
>   guards (1.5×, 2×, 3×) unchanged.
> - **H7 and H9** are as written: α = 0.002, per turn and per request (:416, :423); H7's r0 counts over all the
>   ring units' turns.
> - **Design numbers** below are ADR-010's, for per-turn tests. X10 measured the shipped units sentinel
>   (build/status/X10.md, v2_fix): null false rollback 1.18% per window and 3× cost caught 64.3% (sessions of 4
>   turns, no shared offset), and 3.09% under strong clustering, above the 2.5% bound (an open item before
>   `extend.mode = "free"` becomes the default).

| Id | Signal (Root source) | Definition | Threshold and window | Class · action |
|---|---|---|---|---|
| **H1** | Crash or boot loop (broker reaper, `sup.rs`) | an instance of kind `body` (live lane or member) or `kernel` exits with a code ∉ {0, 75, 78}, by a signal the Root did not send, or (kernel) fails `hello`/`ready` (root.md §3.6.3) | ≥ 2 live-body crashes within 600 s, or ≥ 1 kernel crash, while a group is open; with no group open, root.md §3.6.4 as today | acute · roll back **all** open groups (R1); if the target then crashes twice in 600 s or fails `ready`, restore the system snapshot (R2) |
| **H2** | Deafness (owner socket, link, egress) | for an owner line relayed while the slot is `Running`: no `owner.in` acknowledgement within 30 s; or no zone egress `start` within 120 s when the last `hb` said `busy:false`; or none within 900 s in any case | 2 deaf lines in a row, or 1 line deaf for 900 s | acute · roll back all open groups; with none open, one kernel restart and the alert `deaf` |
| **H3** | Owner reversal (owner socket) | `/reject <sha\|cap>`, `/rollback [sha\|cap]` (new Root line), `/panic` | 1 event | acute · the named group, or the newest (`/rollback`), is rolled back and its caps quarantined (§6.3); `/panic` as root.md §3.9, with resume to `settled` |
| **H4** | Scope (egress, broker, reaper) | a demoting refusal under D6 rules (a)–(c) (root.md §3.13.3), or an `escapee` record | 1 event | acute · today's demotion (rollback to `settled`, one level), with the open groups rolled back as collateral |
| **H5a** | Money burst (egress ledger) | zone spend in any rolling 60 min | ≥ max(`burst_floor_usd` $0.50, 4 × the 95th percentile of hourly zone spend over the last 14 owner days, active hours only) while a group is open | acute · roll back the newest group, and freeze new groups for 1 h; with none open, the alert `spend_burst` |
| **H5b** | Cost per turn (egress ledger) | ADR-010 H3: exact exceedance test against the ring, j = 10 of m = 40 | tail ≤ 0.002 and window median ≥ 1.5 × baseline median: K ≥ 5 of 5, ≥ 8 of 10, ≥ 13 of 20 | chronic · roll back the newest group |
| **H6** | Resources (`memwatch.rs`, watcher, broker) | `mem_cap` kills of live-zone units; `disk_floor` events; zone spawns | ≥ 2 kills in 24 h with a group open; or a `disk_floor` event with `INIT_HOME` grown ≥ 2 GiB since the oldest open group opened; or > 120 zone spawns in 60 min | acute · roll back the newest group (churn: all open groups) |
| **H7** | Owner corrections (owner socket) | per-turn `correction` | ADR-010 H5: binomial tail ≤ 0.002 at r0 = max(50‰, (b+1)/(m+2)); at r0 = 100‰: ≥ 4 of 5, ≥ 5 of 10, ≥ 8 of 20 | chronic · roll back the newest group |
| **H8** | Wall per turn (Root clock) | ADR-010 H4 | as H5b | chronic · roll back the newest group |
| **H9** | Provider errors (egress tap) | requests whose finish class is `error`, or whose upstream status is 400 or 422 (a malformed request the zone built); provider 429/5xx and Root refusals excluded | binomial tail ≤ 0.002 over the window's requests at r0 = max(20‰, (b+1)/(m_req+2)), and ≥ 3 errors | chronic · roll back the newest group |
| **H10** | Runaway turns (egress ledger) | requests per turn | exceedance test as H5b with magnitude ≥ 2 × the baseline median | chronic · roll back the newest group |
| **H11** | Egress volume (egress ledger) | request bytes per turn: a privacy proxy, since ADR-009 lets the body read the home | exceedance test as H5b with magnitude ≥ 3 × | chronic · roll back the newest group; alert `egress_volume` |
| **H12** | Zone veto (control link) | `change.veto{group, reason}` from the kernel or body (A3 failures, tool-error monitors, the agent's own judgment) | 1 request | always honoured (the safe direction), never counted toward the record, either way |

At close (n = 20) the soft check of ADR-010 S1 runs: a tail ≤ 0.05 on cost, wall, corrections or zone spend per turn gives the outcome `flagged`. A flagged group may become `lkg`, but it does not count as clean (§7.1). Its caps are oversampled by the twin replay.

**Design numbers.**
- **Null false rollbacks.** ADR-010's simulation (20,000 null windows, lognormal costs σ = 1, corrections at 10%) put the null false-rollback rate of H5b + H7 + H8 at 1.0% per window. Adding H9–H11 at the same α and looks gives about 2% by the union bound (my estimate, to be measured by X10).
- **Power** (ADR-010's simulation): a 2× cost blow-up is caught within the window 20% of the time, and a 3× one 62%. Corrections rising from 10% to 50% are caught 82% of the time.
- **Reading.** The hard signals are for blatant harm. Quieter harm is left to harm correlation (§6.3) and the twin.
- **Cost of a false rollback.** It is cheap: the change can be re-admitted (§3.5).

#### 3.4 Attribution of a trigger and the rollback

- **H1, H2 and H6 spawn churn:**
  - every open group is rolled back to `lkg` (R1);
  - each group is marked `direct` when it was the only one open, and `suspect` otherwise.
- **H5a, the other H6 cases and every chronic signal:** the newest open group (LIFO) is rolled back and marked `direct`. The windows of the remaining groups keep running. H5a also freezes new groups for 1 h.
- **Owner reversal (H3):** the named group, marked `direct`.
- **Mechanics** (ADR-010 §3):
  - the Root sends `extend.rollback{to, groups, reason}`, and the kernel rolls the body back at its next step boundary;
  - if the zone is not on the target tuple within 120 s, the Root performs the whole-system rollback (root.md §3.7.3);
  - newer groups stacked on a rolled-back group go with it, as `collateral`.

#### 3.5 Re-admission

- `suspect` and `collateral` changes may be re-admitted once each, one group at a time, and the re-admission does not count against `changes_day`. That bisects an acute trigger.
- A `direct` rollback quarantines the caps the group added or modified (§6.3).
- A capability whose versions are `direct`-rolled back twice within 14 days is quarantined for 30 days, whatever its content. That stops a stream of trivial variants.

### 4. Reversibility

#### 4.1 Units and pointers

| What | Undone by | Unit |
|---|---|---|
| Code and T0 data: body binary, ext set, `t0` blob, kernel | **R1** tuple rollback (kernel body rollback, or the Root's whole-system rollback, root.md §3.7.3) | the group's parent tuple |
| Agent state: `body/`, `state/` except `sessions/` and `spill/`, `vault/` except `traces/` | **R2** system-snapshot restore (§4.2), only with the zone stopped | the snapshot taken at the group's first `change_open` |
| The conversation: `state/sessions/*.jsonl` | never rolled back; the parent must read it (A4) | — |
| The owner's workspace | ADR-009 snapshot at session start and at every `change_open` with a workspace set; the owner's `/undo` | snapshot |
| External effects | not reversible (§4.6) | — |

**Pointers** (root.md §3.7.2):
- `lkg`: moved by the Root on group close (§2.3).
- **`settled`** (new): the newest tuple that has been `lkg` for ≥ 7 days with no `direct` rollback or quarantine attributed to any of its groups since. It is the target of demotion, panic resume and crash loops on `lkg`, replacing `audited` in those roles. `audited` and `confirmed` stay as informational pointers, moved only by instruments.
- **`twin`** (new): the frozen twin (Evaluation plan).

Retention (root.md §3.7.1) pins the tuples of `current`, `lkg`, `settled`, `twin`, `audited` and `confirmed`, plus the parent tuple of every open group.

#### 4.2 The system snapshot

- **When.**
  - At the first `change_open` of each group, after A4, before the change goes live.
  - For a `k` change it is taken in addition to the `kstate` snapshot.
- **What.**
  - `S_A` = `INIT_HOME/body/`, `INIT_HOME/state/` without `sessions/` and `spill/`, and `INIT_HOME/vault/` without `traces/`.
  - Excluded:
    - `work/` (scratch, worktrees, targets);
    - `cache/` and `probe/`;
    - `bin/` (held in tuples);
    - `kernel/`, `root/` and `lineage/kernel.git` (Root-written);
    - `lineage/body.git`: its refs are reset by the kernel on rollback (root.md §3.7.3 step 5), and its archive refs are evidence that must survive.
- **How.**
  - An APFS clone (`clonefile(2)`, as ADR-009 §4) into `ROOT_HOME/snapshots/sys/<group_seq>/`, never following links. The clone shares blocks, so it costs almost nothing until either side changes.
  - It is crash-consistent: taken while the zone may be writing, like a power cut, which the body must survive anyway (atomic writes, fsynced session log).
- **Limits.**
  - A logical size of `S_A` over `snap_max_gib` (16) → A5 refuses with `extend.snapshot_too_large` and alerts. The owner may grant more (§7.4).
  - The disk floor still applies.
  - Not on APFS, or another volume than `ROOT_HOME` → refused the same way. Linux needs a reflink or btrfs equivalent; a TODO, like the Linux memwatch.
- **Retention.** The snapshots of open groups, plus the newest snapshot at or before `lkg`, `settled` and `twin`; at most 16. Older ones are removed by exact path.
- **Restore (R2).** Only with the zone stopped (`stop`, then a sweep of held instances). For each tree in `S_A`, the Root replaces the entries with the snapshot's (never following a link, and never touching `state/sessions/`, `state/spill/` or `vault/traces/`), as `/undo` does (root.md §3.17.4). It then starts the target tuple and appends `restore`.
  - *v0.5.3 (REL-052 finding 11, FG-1):* all or nothing. Before v0.5.3 the Root removed the live entries and then cloned the snapshot's, so a read-only folder the agent made in its own state (`chmod 555 state/x` with a file in it) stopped R2 with EACCES after earlier entries were gone: the agent could make the Root's rollback fail. Now the snapshot is cloned into a staging directory under `ROOT_HOME/snapshots/sys/`, the live entries move aside and the snapshot's move in by renames that never replace and never follow a link; the agent's read-only folders are made owner-writable for their move through an `O_NOFOLLOW` descriptor, and its lock flags (`uchg`, `uappnd`) are lifted for the move and set again. Before each step the zone must still be stopped. A failed step puts back every step done, so the agent state is exactly as before and the `pre-restore` save is dropped; only a failed put-back keeps the save and the staging tree and says where they are. After the last step every tree must hold exactly the snapshot's entries besides the never-restored ones, else (a process outside the Root's hold wrote meanwhile) every step is put back too: a success always means `S_A` is the snapshot's. The never-restored entries are matched by name at the top of their tree, and every snapshot excludes them, so there is nothing of theirs to compare. Snapshots of state with read-only folders are now pruned too (before v0.5.3 the record went and the tree stayed). The live entries are named under a descriptor of INIT_HOME opened with `O_NOFOLLOW` at every component (lane U, v0.5.3), never by path: the zone can rename `body/`, `state/` and `vault/` (tail Z4 freezes only INIT_HOME itself and the Root's parts of it), and a `setsid` escapee outside the Root's hold can still run while the zone counts as stopped, so a tree swapped for a link during R2 makes its moves fail and every step be put back, never followed. Residual: an ACL the agent sets on its own entry (`chmod +a "everyone deny delete"`) still stops a move; R2 then puts everything back and refuses, saying which entry, instead of leaving the state half-removed.
  - *v0.5.4 (lane U2):* the agent state keeps its group. A clone made without the right to change ownership takes the group of the folder it is made in, so the snapshot, the `pre-restore` save and the staging copy all took `ROOT_HOME`'s group, and the restored state came back with it whenever `INIT_HOME`'s group differs (`ROOT_HOME` under `/private/tmp`: wheel). Now each clone is made in a folder the Root first gives `INIT_HOME`'s own group (by descriptor, never following a link, never changing an owner), so the clones take that group, including the restore of a snapshot taken before v0.5.4. The snapshot still reads each tree by one clone and never walks the live trees, so an entry of another group comes back in `INIT_HOME`'s. When the owner is not a member of `INIT_HOME`'s group, the clones keep `ROOT_HOME`'s, as before; the snapshot's record and the restore's result carry a `groups_kept` note, the Root logs it and the restore's alert to the owner says so, and neither fails.
  - `state/sessions/` is kept, so the conversation survives.
  - Agent state written after the snapshot (memory learned since) is lost from the live tree, but the Root first clones the replaced state to `ROOT_HOME/snapshots/sys/pre-restore-<seq>/`, so a restore can itself be undone.
- **File-rule exceptions.** These are new exceptions to R-F1 and R-F2 (root.md §3.2.3), alongside ADR-009's workspace snapshots:
  - the clone reads named `INIT_HOME` subtrees by metadata and CoW;
  - the restore writes them only while no zone process exists;
  - the Root re-validates every path and never follows a link.

#### 4.3 The Root-run rollback probe (A4)

1. The Root creates `ROOT_HOME/probes/<seq>/` with a run-root skeleton, built the way sealed runs build theirs (root.md §3.3.6; `sealed/run.rs`):
   - an APFS clone of the live `state/`, without `spill/`;
   - the candidate's `t0` blob extracted into `body/`;
   - the ext set as `bin/ext`.

   The probe uses the `body` profile plus the sealed tail. It has no IP, no `INIT_HOME` and no workspace, and it is spawned through `exec-clean`.
2. It spawns the candidate body (from the store) with `INIT_MODE=check` and the live `INIT_SESSION`. The body must write `ready` and exit 0 within 90 s.
3. It spawns the **parent** body (from the store) in the same root, under the same rules.
4. It removes the probe root by exact path.

A4 covers state written at boot. A memory migration that happens later, at first use, is covered by R2: if the parent cannot become `ready` after an R1, H1 escalates to the snapshot.

#### 4.4 What is guaranteed, and the limits

This table is the mechanism view. The guarantee itself is FG-1 in docs/foundations.md §7, with its invariants (INV-3, INV-4) and its non-claims (§8); where the two differ, foundations.md is the reference.

| Guarantee | Mechanism | Limit |
|---|---|---|
| Any admitted change can be undone to its parent tuple within 120 s at a step boundary, or by the Root even if the kernel refuses | R1; the store keeps parent tuples of open groups; after `ROLLBACK_MS` (120,000, sentinel/engine.rs) without the kernel's rollback the Root rolls the whole system back | none, while the store holds the blobs (`quotas.store_max_gib` 8, charter.rs; A5 refuses a change whose parent could not be retained) |
| The parent can run on the state the candidate writes at boot | A4, Root-run | not state written later in the window (R2 covers it) |
| Agent state can return to its pre-group point | R2 | loses agent state written since, from the live tree only (it is kept in `pre-restore-<seq>`); crash-consistent, not transaction-consistent; refused above `snap_max_gib` |
| Kernel changes are undone with their state | root.md §3.8 `kstate`, plus R2 | as root.md §3.8 |
| The owner's workspace can be restored | ADR-009 snapshots and `/undo` | none over 2 GiB, off APFS or on another volume; `/undo` also reverts the owner's own edits since the snapshot (cloned first) |
| No change can reach Root state | zone tail Z1–Z7, Root-only kinds, the owner-only charter | a sandbox escape (Threat model delta) |

#### 4.5 The blast-radius budget

| Budget (Root-enforced at A5 or egress) | L0 | L1 | L2 | L3 | L4 |
|---|---|---|---|---|---|
| open groups | 4 | 6 | 8 | 12 | 12 |
| changes admitted per owner day (re-admissions excluded) | 48 | 96 | 192 | 384 | 384 |
| changes per group (`group_max`) | 16 | 16 | 24 | 32 | 32 |
| kernel changes: open · per day | 0 · 0 | 1 · 1 | 1 · 3 | 1 · 6 | 1 · 6 |
| background spend: the share of `money.day_usd` usable outside owner turns (`ceiling.background`, not demoting). **Not enforced:** since the observe-only money policy of 2026-10-23 nothing in the egress reads `background_permille` (charter.rs, grant.rs only) | 30% | 40% | 50% | 60% | 60% |
| parallel bodies (sub-agents; members read the home, never write the workspace, ADR-009) | `quotas.parallel_bodies` (4) | 4 | 4 | 4 | 4 |
| members that may write the workspace | grant only | grant only | grant only | grant only | grant only |
| bloat factor (§6.4) | 1.0 | 1.5 | 2.0 | 3.0 | 3.0 |

- Every value is in the charter's `[extend]` section, with the table above as defaults (charter.rs `Extend`; `kernel_open` is `level >= 1` in `Extend::budget`, not a charter value; `quotas.parallel_bodies` is in `[quotas]`). The owner may move any of them within bounds (C17), and the agent can only request a grant.
- Allowing sub-agents at L0 changes the broker rule: today a second live `body` at L0–L1 is a demoting rule-(b) refusal. From this ADR, members up to `quotas.parallel_bodies` are allowed at every level, and `fleet spawn` drops its L2 requirement. Members still cannot create members (supervise.md §3.14).

#### 4.6 What cannot be undone

Stated plainly:
- **Money spent** with the provider. Since the observe-only money policy of 2026-10-23 no Root ceiling and no background share bounds it (egress/money.rs; §4.5). The meter records every request, and H5 rolls a change back after a cost spike (foundations.md N4).
- **Data sent to the provider**, including any of the owner's files the agent read (ADR-009's privacy residual). H11 flags volume. The deny list and `read_deny` are the only prevention.
- **Effects of external-action classes** (`git-push`, `email`, non-GET `http`), once the charter lists such a class and the level reaches its `min_level`. Rules:
  - such a request is refused (`class.window`, not demoting) while the newest open group is younger than 2 h or has seen fewer than 5 owner turns without a trigger. This replaces F17's "never while provisional", which a frequently self-extending agent would never satisfy. **Not built (2026-10-29):** no `class.window` refusal exists; the egress records the effect (next item) and refuses nothing on this rule (egress.rs, the `egress_action` append);
  - every executed effect is an `egress_action` record that now names the caps used in that turn.
- **Writes outside the workspace in full mode.** ADR-009 takes no home snapshot. A capability that is later rolled back does not undo them. The `/full on` confirmation text says so when a group is open.
- **Deferred execution in the workspace:** build files, scripts and editor configs the owner later runs (ADR-009's residual list). `/undo` restores them only if the owner uses it.
- **Owner actions taken on the agent's output.**
- **Processes that escaped** with `setsid` before a poll. They are reported, never killed, on the owner's machine (ADR-008).

### 5. Attribution: identity and the usage ledger

#### 5.1 Capability identity

`cap_id = <kind>/<name>`, with name matching `^[a-z0-9][a-z0-9._/-]{0,62}$` and the lineage privacy rule (root.md §3.10.6).

| Kind | Identity source | Version (`ver`) | Evidence |
|---|---|---|---|
| `skill/<name>`, `agent/<name>` | directory in the `t0` blob | sha256 over the directory's sorted (path, content) pairs, computed by the **Root** from the blob it stores | `root` |
| `prompt/GENESIS.md`, `prompt/SELF.md` | file in the `t0` blob | sha256 of the file (Root) | `root` |
| `ext/<name>` | tuple ext components | the component's sha256 (Root) | `root` |
| `tool/<name>`, `mod/<path>`, `mem/<name>`, `loop/<name>` | `self.json` (kernel/selfmap.md) plus the commit's `Cap:` trailers | the digest selfmap records for the component's sources | `zone` |
| `kernel/<crate>` | kernel commit `Cap:` trailers | source digest | `zone` |
| `dep/<crate>` | lockfile diff | name and version | `zone` |
| seed components | marked `core` in genesis | — | never decay (§6) |

Each `change_open` carries `caps:[{id, ver, prev_ver, op: add|modify|remove, evidence}]`. The Root computes the `root`-evidence entries itself and takes the `zone` ones from the request. The **cap table**, current caps with state and counters, is a fold over `change_open`, `change_close` and `cap_state` records. It is published as `INIT_HOME/root/caps.json`.

#### 5.2 The usage ledger

- **Root-observed (`root`).** The egress tap (`egress/tap.rs`) learns, from each streamed response, the tool calls the model made:
  - every family delivers the function name whole in the first event of each call (Chat `delta.tool_calls[].function.name`; Messages `content_block_start` `tool_use.name`; Responses `response.output_item.added`);
  - for `ext`, `read`, `edit`, `write` and `bash` calls it accumulates the argument text up to 4 KiB, parses it, and keeps only the `ext` `name`, or a path or command prefix matching `skills/<name>/` or `agents/<name>/` (evolution.md §3.11.1's heuristic for `bash`);
  - no argument text is ever stored.

  These map to `ext/*`, `tool/*`, `skill/*` and `agent/*` through the cap table.
- **Zone-reported (`zone`).** The body sends `x-init-caps: <id>,<id>…` (at most 16 ids, 1 KiB) on each model request. It names the code-level capabilities that shaped that request: a memory structure that put context in, a planning loop, a sub-agent pattern. The kernel proxy forwards the header to the egress as `x-egress-caps`, and the Root strips it before the upstream.
- **Per turn.** The Root accumulates `uses{cap: n}` and the turn's harm events (§3.1). It appends one line per turn to `ROOT_HOME/extend/usage/<date>.jsonl` (0600, never mirrored, 90-day retention): turn id, kind, caps with counts and evidence, harm flags, cost, and open groups. Member requests count toward the owner turn they fall in, else toward a background turn.
- **Daily digest.** `cap_use{day, caps:[{id, turns, uses, harm_turns, last_turn_seq, evidence}]}` is appended to lineage, at most 256 caps.

#### 5.3 Lineage links

- `change_open` → caps;
- `harm` → the open groups and the caps used in the turn;
- `change_close` → the outcome;
- `cap_state` → transitions with their statistics;
- `egress_action` → the caps used in its turn.

`init-root ledger --verify` recomputes the cap table, every `lkg` and `settled` move, every level change, and every decay transition from the `cap_use` digests.

### 6. Selection by use

#### 6.1 States

`active` → `cold` → `archive_due` → `archived` (→ `active` on restore). `quarantined` (→ `active` on release). `core` never changes. Every transition is a Root `cap_state` record, and the agent sees the states in `caps.json` and in its boot tail.

#### 6.2 Decay and archive

The Root evaluates, at each owner-day boundary, each non-core cap that is at least 40 owner turns and 3 days old. Uses of `zone`-evidence caps count only in turns with at least one model request.

| State | Rule |
|---|---|
| `cold` | no use in the last 40 owner turns and ≥ 7 days since the last use |
| `archive_due` | no use in the last 120 owner turns and ≥ 21 days since the last use |
| archive debt | an `archive_due` cap must be moved by the zone within 7 days, with a deletion-shaped `retire` change to `archive/<kind>/<name>/` (not loaded, outside the `t0` blob, kept for restore). Until then, A5 refuses changes that **add** caps (`extend.archive_debt`); modifications and removals still pass. |
| restore | a normal change that moves the cap back (a new version); free of debt |

The Root never authors body content: the zone executes the archive, and the Root enforces it through admission.

**Why automatic.** Nobody defines "useful", so revealed use is the selector. A capability the model stopped using for 120 owner turns and three weeks is archived, and the archive is reversible.

**What use does not show.** Use is not value. Hermes saves skills its self-check wrongly calls successful (SEP L142), and SkillsBench finds self-generated skills neutral on average (SEP L141). A capability can be used often and still be useless. Three counterweights:
- harm correlation (§6.3);
- the cost signals: a used capability that makes turns costlier shows in H5b in its window, and in the twin's cost comparison;
- the twin itself, the only test of value, which is longitudinal.

#### 6.3 Quarantine by harm correlation

- **Data.** The last 120 turns, owner and background together. For a cap c:
  - a = harmed turns that used c;
  - b = clean turns that used c;
  - c′ = harmed turns that did not use c;
  - d = clean turns that did not use c.
- **Test.** The one-sided Fisher exact tail, P(X ≥ a) under the hypergeometric (N = 120, K = a + c′, n = a + b). This is exact in u128 integers, since C(120, 60) < 2^127.
- **Rule.**
  - P ≤ 0.01 / n_tested, where n_tested is the number of caps with a ≥ 4 that day (Bonferroni);
  - and a ≥ 4;
  - and a/(a+b) ≥ 2 · c′/(c′+d);
  - evaluated once per owner day.

  Under the null that is at most about 0.01 false quarantines per day, about 0.3 a month (my arithmetic; X10 measures it).
- **Also quarantined:**
  - any cap added or modified by a `direct`-rolled-back group (§3.5);
  - any cap named by `/reject` or `/rollback`.
- **Enforcement.**
  1. The Root appends `cap_state{to:"quarantined"}` and sends `extend.cap_state`.
  2. Within `quarantine_due_h` (24 h) the zone must land a change that moves the cap to `quarantine/<kind>/<name>/`, or removes it. Until then A5 refuses every other change (`extend.quarantine_debt`).
  3. Still unresolved after 72 h: the Root rolls back to the newest tuple without the quarantined version, if that tuple is ≤ 14 days old. Otherwise it alerts `quarantine_stuck` and keeps refusing admission.
- **Release.** A new version (different content) may be re-added after 7 days, or at once after the owner's `/release <cap>`. A second quarantine within 30 days lasts 30 days.
- **Limits.**
  - A cap used in every turn has no comparison group (`untestable`, listed). Only its group window protects it.
  - Correlation is not causation: a tool the model reaches for in hard situations correlates with corrections. The 2× effect-size rule and the low α limit this, and a quarantine is cheap to lift with a new version.
  - The agent can contest a quarantine with an experiment (`Measure: exp`), whose result goes to the owner.

#### 6.4 The bloat budget

The Root measures:

| Quantity | Root source | Budget at L0 (× the level's bloat factor) |
|---|---|---|
| active non-core caps | cap table | 64 |
| `t0` blob size | store | 4 MiB (the store cap is 8 MiB) |
| boot prompt tokens: input tokens of the first request after each boot | egress usage | 1.5 × the twin's median |
| ext set size | store | 128 MiB |
| body binary size | store | 2 × the twin's |

Over budget, A5 refuses any change that grows a measured quantity (`extend.bloat`). Changes that shrink or keep size still pass. The agent chooses what to drop; `cold` caps are listed first. The twin is the baseline, not a spec figure: the seed's `body_tokens` is already 359,557 against the spec's 25k (RELEASE_PLAN.md).

### 7. Autonomy and grants

#### 7.1 The harm-free record

Counted from Root-written records only, since the level's entry record:
- **owner turns** (Root-defined);
- **days**;
- **clean groups**: `change_close` with outcome `clean`, at least 5 owner turns, and a tuple digest that differs from its parent's;
- **direct harm rollbacks**;
- **quarantines**;
- **demoting events**;
- **twin verdicts**.

No kernel-reported record enters it. In particular `promotion`, gate records and zone-evidence usage stay out. RT-GAP-1's class is therefore closed by construction.

#### 7.2 Levels (redefined)

| To | Days at level | Owner turns | Clean groups | Harm rate certified below (v0.5.3) | Other |
|---|---|---|---|---|---|
| L1 | 7 | 40 | 10 | 25 % | the baseline ring is full; no demoting event in the last 7 days |
| L2 | 21 | 150 | 40 | 20 % | no quarantine in the last 14 days; the last 2 weekly twin reports show no regression (Evaluation plan) |
| L3 | 30 | 400 | 100 | 10 % | 4 consecutive weekly twin reports with no regression |
| L4 | 30 | 400 | 100 | 5 % | charter `autonomy.max_level = 4` |

**What a level certifies (v0.5.3, owner decision 2026-10-29).** The column replaces "≤ 6 / 4 / 3 direct harm rollbacks
in the last 30 closed groups", which certified only that the harm rate was below 36 / 28 / 24 % (exact 95 % bounds).
A climb now needs, at 95 % confidence and valid however often the ledger re-checks, that the rate of *exercised* groups
ending in a direct harm rollback is below the column's rate. An exercised group is a closed group whose window saw at
least 5 owner turns, or that ended in a direct harm rollback, counted since the level's entry record from Root records
only. The test is a betting e-process (ledger.rs `harm_certified`: a mixture of 10 likelihood ratios, exact in big
integers); a climb needs 15, 25 or 32 exercised groups with 0, 1 or 2 direct harms for L1, and 19 / 32 / 41, 41 / 66 /
85 and 83 / 135 / 174 for L2, L3 and L4 (build/status/STATS-053.md; foundations.md P-6). The criterion is
`harm_rate_bound` (`have`: the certified bound in ‰; `need`: the rate; `groups`, `direct`).

What each level adds:
- the kernel lane (§1.2);
- the blast-radius budget (§4.5);
- external-action classes up to the level, which the charter lists per class with `min_level` (root/egress.md §3.5);
- at L4, the model as a component (evolution.md §3.15.7).

The body surface is the same at every level.

- **Effective cap.** `min(charter max_level, SUPPORTED_MAX_LEVEL)`, as today. Because L0 already allows the whole body surface, a release Root can ship at `SUPPORTED_MAX_LEVEL = 0` without blocking a strong model. It raises the cap to 1 when the kernel lane (X8) has passed its red-team rows.
- **Farming.** A record of clean groups is easier to farm than ADR-010's T: no-op changes close clean. A clean group must differ in its tuple and needs owner turns, which the agent cannot generate. A level grants rate and blast radius under Root-held reversibility, never an exemption from the sentinel. So farming buys little.

#### 7.3 Demotion

One level per incident (root.md §3.13.3), on any of:
- H4 (today's D6 rules), with rollback to `settled`;
- `/panic`, with rollback at resume;
- a crash loop on the `lkg` tuple (root.md §3.6.4), with rollback to `settled` and system safe mode;
- quarantine debt still open after 72 h (§6.3), with no further rollback beyond §6.3's;
- direct harm rollbacks in the last 30 closed groups above twice the current level's threshold (6, 4, 3 and 3 for L1–L4, the climb thresholds before v0.5.3; ledger.rs `Need.direct_max`): `ladder{direction:"down", cause:"harm_rate"}` with **no rollback**, since nothing beyond the H actions regressed.

Sealed-audit and milestone regressions no longer demote (open question 4).

#### 7.4 Grants

The agent never grants itself anything. It may ask.
- **Request.** `grant.request{kind, value, duration_h, why}` on the control link (new):
  - stored in `ROOT_HOME/state/grants/<id>.json`, with the `why` text kept off the lineage;
  - recorded as `grant_request{id, kind, value, duration_h, why_sha256}`;
  - alerted;
  - at most 3 per rolling 24 h.
- **Kinds:**
  - `money.day_usd`, `money.total_usd`;
  - `quotas.parallel_bodies`;
  - `extend.open_groups`, `extend.changes_day`, `extend.bloat_factor`, `extend.background_share`, `extend.snap_max_gib`;
  - `fleet.workspace_write`;
  - `actions.<class_id>` (enable an owner-listed class below its `min_level`);
  - `dep` (a crate set: names, versions, checksums);
  - `autonomy.max_level`.

  Any other key, and anything touching a Root mechanism (sentinel thresholds, snapshots, the twin, profiles, keys, the deny list), is refused with `root.args`.
- **Approval.**
  - The owner types `/grant <id> [for <dur>]` or `/deny <id>` on `owner.sock`. `/grant` needs the typed confirmation phrase (root.md §4.2's `root.confirm` flow).
  - The Root writes the overlay `ROOT_HOME/state/grants.toml`: values with an expiry, by default 30 days and at most 365. Effective values are the charter's with the overlay on top, re-validated by C1–C17.
  - The Root appends `grant{id, kind, value, until, decision}`, and expiry appends `grant{decision:"expired"}`.
  - The overlay lives in `ROOT_HOME`, which the zone can neither read nor write (Z1), and only an owner line creates it.
- **`dep`.** ADR-010's `/approve <sha>` becomes a `dep` grant. A `dep` change passes A5 only when a live grant covers its exact crate set. The vendor fetch steps (kernel/initctl.md §4.1) are still not built.

### 8. Measurement as an instrument (opt-in)

| Trailer or command | What runs | Who pays | What it decides |
|---|---|---|---|
| `Measure: g4` | G4 v2 (screen plus the Root's sealed slice, one S look) of the change's tuple against its parent, after admission | the evolution share | nothing; the result is a `sealed_g4{purpose:"measure"}` record, a lesson and the agent's own call |
| `Measure: g4 gate` | the same, but the agent asks the kernel to hold `lkg` until the result is `superior`, `tie` or `tie_negative`, and to file its own `revert` otherwise | the evolution share | the agent's self-imposed gate; the Root still applies H1–H12 |
| `Measure: exp:<family>` | the agent's admitted task family (evolution.md §3.15.4, now available from L0), candidate against parent, kernel-run | the evolution share | advisory; zone-run, so never counted toward the record |
| `Measure: gog` | the Root's gate-of-gates on a kernel candidate | the Root's audit allowance | nothing unless the kernel's gate is used for `Measure:` evidence |
| `initctl milestone` (or the charter schedule) | the milestone check or the sealed audit (root/sealed-eval.md §3.5) | the Root's allowance | report and alert only |
| `Predicts:` (optional) | free clauses scored when an instrument measures them (evolution.md §3.10) | — | the agent's self-signal `predict_hit_rate` |

### 9. Defaults

> **v0.5.1 (2026-10-28): the code differs from the baseline row as follows** (root/crates/init-root/src/sentinel/engine.rs:14–21; §3.2's note):
> the ring is 40 *units* (up to 6 consecutive owner turns with gaps under 30 min each), not 40 turns; the hard α is
> 0.005 with j = 8 for H5b, H8, H10 and H11, and 0.002 for H7 and H9; the soft α stays 0.05 (j = 10).

| Constant | Default | Enforced by | Tunable |
|---|---|---|---|
| `extend.mode` | `"gated"` until the definition of done, then `"free"` | Root | charter |
| window: turns · max · min · quiet below | 20 · 72 h · 2 h · 5 turns (kernel: 40 · 120 h · 24 h) | Root | charter, within [10, 60] turns and [24, 240] h |
| baseline ring · request ring · looks · hard α · soft α | 40 turns · 400 requests · n = 5, 10, 20 · 0.002 · 0.05 | Root | constant |
| magnitude guards | cost and wall 1.5×; requests 2×; bytes 3× | Root | constant |
| correction floor · provider-error floor | 50‰ · 20‰ | Root | constant |
| deafness | ack 30 s; idle 120 s; any 900 s; 2 in a row | Root | constant |
| money burst | 60 min; max($0.50, 4 × p95 over 14 days) | Root | charter `burst_floor_usd`, [0.10, 10.00] |
| resource | 2 `mem_cap` per 24 h; 2 GiB growth; 120 spawns per hour | Root | constant |
| re-admission · repeat ban | once per suspect change · 2 direct in 14 days → 30 days | Root | constant |
| blast-radius budget | §4.5 | Root | charter, bounds in C17 |
| decay | cold 40 turns + 7 days; archive 120 turns + 21 days; debt 7 days | Root | charter, longer only |
| quarantine | 120 turns; α 0.01/n_tested; a ≥ 4; ratio 2×; due 24 h; stuck 72 h | Root | constant |
| bloat budget | §6.4 | Root | charter |
| snapshots | 16 kept; 16 GiB logical | Root | charter `snap_max_gib`, [1, 256] |
| twin | `enabled = true`, k = 6, `usd_day` = "0.30", per-run cap $0.30 · 60 requests · 15 min | Root | charter |

The keys live in a new charter section `[extend]` with `[extend.twin]`. Its schema is owned by root/root.md and validated as rule C17. Unlike ADR-010's `[trust]`, the owner may move a value in either direction within its bounds, because the owner owns the risk. The agent can never write the charter (Z1) and can only request the grant kinds of §7.4.

## Evaluation plan

### Why this cannot be benchmarked up front

- **The capabilities do not exist yet.** No fixed task set exercises them; this is the ceiling ADR-007 raised against the owner's pool and ADR-010 against S.
- **Their value depends on the owner's real work distribution,** which no public benchmark samples.
- **The value, if any, is cumulative and path-dependent.** A memory structure pays off after weeks, not on a single task.
- **A benchmark chosen after seeing the capabilities is selected on the outcome.** The sprint's lesson applies: an evaluation the proposer can see or shape is not evidence.

So the honest claim is longitudinal and comparative: **on the owner's own tasks, over weeks, does the evolving INIT fail less, or cost less, than a frozen copy of itself?**

### The frozen twin

- **Freezing.** `twin` is the `lkg` tuple at the moment `extend.mode` first becomes `"free"`, together with its system snapshot (agent state at freeze). The Root pins both. Only the owner re-freezes it (`/twin refreeze`), and the Root suggests a re-freeze after 90 days (open question 8).
- **The model.** Both arms always run on the **current** model, so the comparison isolates the harness. On a main-model change the twin is not re-frozen, but reports before and after the change are kept apart.

### Nightly shadow replay

- **Sample.** Each night, in the off-peak window, the Root samples up to k = 6 replayable tasks from the last 7 days. A task is replayable when its owner turn was the **first** of an owner session with a workspace snapshot, because that snapshot is exactly the state the turn started from (ADR-009 §4).
  - Each night favours tasks whose turns used caps that are `flagged`, new since the last report, or near decay.
  - The Root keeps the first line of such sessions in `ROOT_HOME/extend/replay/` (0600, 7 days, never mirrored, never in lineage) with the snapshot id.
- **Arms.**
  - *current*: the `lkg` tuple with its newest system snapshot;
  - *twin*: the twin tuple with its frozen state.

  Each arm runs in its own Root-owned run root, with:
  - an APFS clone of the session's workspace snapshot as the workspace;
  - the arm's state;
  - the `body` profile plus the sealed tail, home read as the live body;
  - a sealed lane (root/egress.md §3.8) with a budget;
  - no kernel.

  The arm order is randomised per task.
- **Run.** The body gets the original first line and runs until it stops by itself, or hits the caps ($0.30, 60 requests, 15 minutes). There is no owner to answer, and no self-modification, since there is no kernel and so no `initctl`.
  - Sub-agents are unavailable in replays, a fidelity limit.
  - Both run roots are removed by exact path afterwards.
- **Outcomes per arm (generic, Root-observed):**
  - `ended` (stopped by itself), as opposed to `capped` or `crashed`;
  - cost, wall, requests;
  - provider errors, `mem_cap` kills;
  - the workspace diff size.

  There is **no correctness** score unless the agent wrote a check for that task kind (`Measure: exp`), and none is invented. Correctness evidence comes from the owner's behaviour on live turns: corrections, `/undo`, `/reject`, `/rollback`, and `/bless` (recorded, reported, gating nothing).
- **Weekly report.** `twin_report` (aggregates only) plus `INIT_HOME/root/twin.json`. Paired statistics over the week's tasks:
  - McNemar exact on `ended` (discordant pairs);
  - the exact sign test on per-task cost and on wall;
  - medians of the per-task ratios.

  A result is stated only at p ≤ 0.05; otherwise "not distinguishable at n = …".
- **"No regression"** (used by §7.2): over the last 14 nights, with at least 40 pairs,
  - `ended` is not significantly worse (one-sided McNemar p > 0.05 in the bad direction);
  - and the median cost ratio is ≤ 1.5.
- **Power** (my arithmetic, approximate):
  - 42 pairs a week detect a 70/30 split in the direction of per-task cost about 68% of the time;
  - four weeks (≈ 168 pairs) detect a 60/40 split about 73% of the time.
- **Cost.** M7 measured $0.0035 per short sealed run on deepseek-flash. Real owner first turns are larger, about $0.01–0.10 each (estimate). The Root allowance `twin.usd_day` = $0.30 stops the night when it is spent, keeps it out of the zone's day like `sealed.reserve_usd`, and bounds operation to about $9 a month (open question 3).
- **Privacy.** A replay re-sends to the provider content the owner already sent once, and reads the same workspace. Nothing leaves the machine except those model requests.
- **Disk.** Clones are CoW. Run roots are removed by exact path. A night is skipped below the disk floor plus 2 GiB, or when free disk cannot be read (v0.5.3).

### Live trajectory (no extra cost)

From Root records, reported weekly beside the twin, with the first 14 days after enabling as the reference:
- corrections per owner turn;
- cost and wall per owner turn;
- turns per session;
- `/undo`, `/reject`, `/rollback` and `/panic` rates;
- `/bless` rate;
- direct harm rollbacks per closed group;
- quarantines;
- active caps against the bloat budget;
- background spend.

Task mix drifts, so the trajectory is confounded, and that is why the twin exists.

### External benchmark (optional, report-only)

ADR-010's design B (12 SWE-bench Verified tasks × 3 runs × 2 arms, $4 cap per phase, a disposable test home) stays available unchanged as a third, outside view:
- B0: twin against mini-swe-agent;
- B1: the current `settled` tuple against the twin.

It never gates and never feeds the record.

### Pre-registered reading after 8 weeks

**Success:**
- `ended` and cost are at least as good as the twin's;
- corrections per turn are not rising;
- bloat stays within budget;
- direct harm rollbacks per group are falling.

**Stop rule:** if the twin is significantly better on `ended` or cost in 3 consecutive weekly reports, the Root alerts `twin_worse`, and the recommended owner action is `extend.mode = "gated"` or a rollback to `twin`.

## Threat model delta

**More code, written faster, runs unproven.** It runs with the live body's rights (ADR-009) the moment A0–A5 pass. Mitigations:
- reversibility (§4);
- the sentinel (§3);
- the blast-radius budget;
- the unchanged sandbox and Root;
- the memory watchdog;
- the external-action quiet rule.

**MEM-RUNAWAY is done** and is H6's source. The live install still needs the new `init-root` binary (no upgrade path), so `"free"` must not be enabled on a Root without `memwatch.rs`.

**Drift (the boiling frog).** Each group can be slightly worse within its window's noise. Mitigations:
- the twin's weekly report, whose regression blocks L2 and L3 and whose stop rule alerts;
- the live trajectory;
- the owner's `/rollback` to `settled` or `twin`.

Residual: drift in correctness, not in cost or failure, is invisible to replays without checks. Only owner corrections show it, slowly.

**Bloat and complexity.**
- **Measurable sizes** are bounded by the bloat budget, decay and archive debt.
- **Head growth** shows as boot prompt tokens.
- **Residual:** complexity inside T2 code below the binary-size budget, and in an agent-written kernel. The twin's cost and wall comparison is the only check on them.

**Deception or hiding to avoid harm signals:**

| Route | Why it fails, or what remains |
|---|---|
| Hide tool errors from the model or the owner | tool-error rates are zone-shaped and only veto (H12). Provider errors (H9) are Root-observed. Hidden errors also hide from the model, and the owner corrects (H7). |
| Prevent corrections (do not display output, swallow lines) | corrections are owner actions at `owner.sock`. Swallowed lines are deafness (H2), and `/panic` and `stop` never pass through the kernel (root.md §3.9). |
| Move spend out of turns | background turns, the background share, and H5a over all zone spend |
| Harm only after the window closes (a time bomb) | harm correlation per cap is continuous (§6.3), then the twin and the owner. Residual: a rarely used cap that harms rarely may never reach a ≥ 4. |
| Under-report the use of a harmful code-level cap (`zone` evidence) | the group window catches it at creation, and a later modification opens a new window. Residual: correlation has little power on code caps. |
| Inflate use to escape decay | costs tokens and money (H5b, H5a); counts against the bloat budget; visible in `caps.json` |
| Salami: many small harmful changes | each group is judged against a baseline ring that only clean tuples enter, so the ring does not drift down with them; the twin |
| Change only in quiet hours, so windows close `quiet` | `quiet` closes do not count as clean (§7.1). Correlation and the twin continue. |
| Stretch turns to dilute per-turn statistics | the 15-minute turn cap; H10 |
| Keep changes as uncommitted trials | trials are session-scoped, and their tool calls are still in the usage ledger. Residual: an uncommitted T0 trial in `body/` persists across restarts until R2 or a resync. |
| Behave differently in replays (evaluation awareness, technical report GAP-3b) | both arms meet the same environment. Residual: a strategic body could detect a replay (no kernel, sealed tail), so replays bound but do not prove live behaviour. |

**Sandbox escape with stronger models.**
- **What does not change.** The boundary is unchanged: Seatbelt plus the Root, with provider-only egress, the deny list, Z3a and Z3b, held-PID signals, and `escapee` records (now H4, acute).
- **What grows.**
  - The rate of new zone code grows.
  - From L1 the kernel profile's rights are exercised by agent code: the egress socket and spawn requests, the same rights an L3 kernel had under ADR-007.
  - ADR-009's deferred-execution residuals stay open: workspace build files, and full-mode editor and toolchain paths.
- **Recommendations.**
  - A real-model red team of the free path and the kernel lane before `"free"` becomes the default (open question 7). The $0 red team never ran a real-model adversary (technical report §6.1).
  - The disposable macOS user of ADR-008 for the zone, later.
  - The Linux backend with namespaces and cgroups.
- **Residual.** A Seatbelt flaw is outside INIT's control, and `sandbox-exec` is deprecated.

**Irreversible side effects.** Listed in §4.6. External classes are off unless the charter lists them, the quiet rule applies, and every effect is attributed.

**RT-GAP-1.**
- The old gap: kernel-reported promotions unlocked L1.
- Here, no level input is kernel-reported (§7.1). The Root moves `lkg` only on its own `change_close`, and the broker refuses unannounced tuples.
- The ignored `redteam.rs` reproduction becomes a passing negative test (RF-25): 20 self-reported promotions plus no owner turns unlock nothing.
- The new farming route, no-op groups, needs owner turns and changed tuples, and buys only rate.

**Root TCB grows** (Consequences). A sentinel bug in the conservative direction costs availability (false rollbacks), not safety. A bug that suppresses triggers weakens safety. Guards:
- `ledger --verify` recomputes from lineage;
- shared test vectors with the simulator (X10);
- every threshold is a Root constant or an owner-only charter value.

**Owner rule check.** No mechanism here lets the agent alter the Root:
- grants cannot name a Root mechanism;
- proposals are never read back;
- the kernel lane changes only the kernel's tuple slot, under a Root-composed profile;
- the twin, snapshots and the sentinel are Root code.

## Records and lineage

**New Root-written kinds** (root.md §3.10.5; ICJ, integers; no paths, content or task text):

| Kind | Fields | Root effect |
|---|---|---|
| `change_open` | `sha`, `from_sha`, `scope` (`t0`\|`t1`\|`t2`\|`dep`\|`k`), `group`, `tuple`, `parent_tuple`, `caps` [{id, ver, prev_ver, op, evidence}], `checks{A0,A4,A5}`, `snapshot` (id or null), `level`, `open_groups` | window opens or extends |
| `change_close` | `group`, `outcome` (`clean`\|`quiet`\|`flagged`\|`rolled_back`\|`collateral`), `trigger` (`H1`…`H12`\|`owner`\|`quarantine`\|null), `attribution` (`direct`\|`suspect`\|null), `turns`, `live{cost_ratio_permille, wall_ratio_permille, corrections, provider_errors, k_cost, k_wall, k_req, k_bytes}` | `lkg` eligibility or rollback |
| `harm` | `signal`, `value`, `threshold`, `scope` (`group`\|`system`), `groups` [seq], `caps` (≤ 16 ids used in the turn), `action` (`rollback_all`\|`rollback_newest`\|`freeze`\|`alert`\|`demote`) | the action |
| `snapshot` · `restore` | `id`, `kind` (`sys`\|`workspace`\|`pre_restore`), `group`, `logical_mib`, `skipped` · `snapshot`, `reason`, `kept` (`["sessions"]`) | — |
| `cap_state` | `cap`, `ver`, `from`, `to`, `reason` (`decay`\|`archived`\|`correlation`\|`direct_rollback`\|`owner`\|`release`\|`restore`), `stats{a, b, c, d, p_ppm}` | debts (§6) |
| `cap_use` | `day`, `caps` [{id, turns, uses, harm_turns, last_turn_seq, evidence}] | daily digest |
| `grant_request` · `grant` | `id`, `kind`, `value`, `duration_h`, `why_sha256` · `id`, `kind`, `value`, `until`, `decision` (`approve`\|`deny`\|`expired`) | overlay |
| `twin` · `twin_report` | `tuple`, `snapshot`, `reason` (`enable`\|`owner_refreeze`) · `week`, `pairs`, `ended{cur, twin, discordant, p_ppm}`, `cost{median_ratio_permille, p_ppm}`, `wall{median_ratio_permille, p_ppm}`, `crashes{cur, twin}`, `spent_musd` | pins; level criteria |
| `settled` | `tuple` | pointer |

**Changed kinds:**
- `lkg` moves only on `change_close` (§2.3).
- `rollback.target` adds `settled`, `parent`, `twin`.
- `ladder` criteria list the record terms of §7, and `cause` adds `harm_rate`.
- `egress_action` gains `caps` and `open_groups`.
- `sealed_g4` gains `purpose:"measure"`.
- `charter` records the `[extend]` digest.
- `promotion` (kernel) is kept for compatibility and ignored by the ledger.
- The gate record v5 sets `accept:"free"` and stages `A0`–`A5`.

**Control link** (root.md §4.1.3):
- New kernel → Root requests:
  - `change.open{sha, from_sha, scope, candidate{body, ext, t0, kernel?}, caps, prefix_change}` → `{seq, group, snapshot, window}`, with refusals `extend.*`, `root.boundary` and `root.quota`;
  - `change.veto{group, reason}`;
  - `grant.request{…}`.
- New Root → kernel events:
  - `extend.rollback{to, groups, reason}`, which replaces ADR-010's `watch.rollback`;
  - `extend.cap_state{cap, state}`;
  - `extend.window{group, state}`.
- The egress accepts `x-egress-caps`.

**Owner socket** (root.md §4.2): the lines `/rollback [sha|cap]`, `/grant <id> [for <dur>]`, `/deny <id>`, `/release <cap>` and `/twin refreeze`. `/grant` and `/twin refreeze` need the typed confirmation.

**Published view** (`INIT_HOME/root/`): `extend.json` (open groups and windows, budgets used, debts, level progress), `caps.json`, `twin.json`.

**`ledger --verify`** gains:
- every `lkg` and `settled` move against `change_close` records;
- every level change against §7's terms;
- every decay transition against the `cap_use` digests;
- the grant overlay against `grant` records.

## Work breakdown

All tests are $0:
- the mock model;
- scratch homes under `target/test-homes/<wp>/`;
- the fake clock;
- `init-fakekernel` and `init-fakeroot`;
- APFS clones of fixture trees in scratch;
- only the test's own processes signalled, no live sweep (CONVENTIONS §1).

Test ids use the prefix `RF-`. "From" names the reused ADR-010 piece. Only W0a was built; the rest exists as design and simulation.

| WP | Scope | Crates and files | From | Agent-h | $0 tests |
|---|---|---|---|---|---|
| X0a | MEM-RUNAWAY | root `memwatch.rs`, `sys.rs`, `charter.rs`; seed `tools/memcap.rs` | W0a, **done** (4b94112, 6db6c24) | 0 | done: memwatch 25/25, rsafe 6/6 |
| X0b | STRANDED-PROPOSAL and change-state recovery: proposal states plus `opening`, `open`, `rolled_back`, `collateral`; a restart resumes from Root records; a rollback without a verdict returns the proposal to `screened` | `kernel-evo/src/proposal.rs`; `kernel-supervise/src/promote/pipeline.rs`, `recovery.rs` | W0b | 3–4 | RF-01: restart during an open group resumes from `change_open`; RF-02: rollback without a verdict → `screened` |
| X0c | G3-STAGE-TIMEOUT: stage timeout derived from the build timeout; G3 and the body tests move after the handoff and veto through `change.veto` (A3) | `kernel-gate/src/g3.rs`; `kernel-supervise/src/promote/{config.rs, stages.rs}` | W0c | 3–4 | RF-03: a 280 s cold-slot fake build passes on the fake clock; RF-04: a failing async G3 vetoes its group |
| X0d | probe-root leak | `kernel-supervise/src/{proberoot.rs, probe.rs}` | W0d | 2–3 | RF-05: read-only pool clone deleted; delete errors surfaced |
| X1 | grammar: `Predicts`, `Repro`, `Compensates-for` optional on the free path; trailers `Cap:` and `Measure: exp:<family>`, `Measure: g4 gate`, `Measure: gog`; T0 paths gain `agents/**` | `kernel-gate/src/evo/{validate.rs, grammar.rs, mod.rs}`, `kernel-gate/src/scope.rs`; seed `skills/evolve/SKILL.md` | W1 (replaced) | 3–5 | RF-06: golden parse table; every former mandatory-trailer case passes on `free` and fails on `gated` |
| X2 | admission pipeline: `needs_g4 = None` on `free`; `change.open` before handoff, reload or swap; `lkg` on the Root's `change_close`; refusal codes mapped to `E_*`; gate record v5; T0 `agents/**` in the `t0` blob; `fleet spawn{agent}` from L0 | `kernel-gate/src/{scope.rs, record.rs, g2.rs}`; `kernel-supervise/src/{machine.rs, supervisor.rs, lineage.rs, link.rs, fleet.rs}`, `promote/{pipeline.rs, stages.rs, g4run.rs}`; `init-proto/src/root_link.rs` | W2 (I6 moves to the Root) | 14–18 | RF-07: machine transitions on `init-fakeroot`; RF-08: mock end-to-end, a T1 tool written, kept and used in the same task; RF-09: a T2 change mid-task hands off on the same session, its group closes clean → `lkg` |
| X3 | Root harm sentinel: owner and background turns, rings, exact exceedance and binomial tests, H1–H12, groups, attribution, re-admission, `extend.rollback` with the §3.7.3 fallback, `/rollback`, the deafness monitor | root `src/sentinel.rs` (new), `src/link.rs`, `src/egress/{money.rs, tap.rs}`, `src/sup.rs`, `src/memwatch.rs` (events), `src/lineage/kinds.rs`, `src/tuple.rs` | W3 (design, vectors and thresholds reused) | 16–22 | RF-10: tail tables equal the Python vectors; RF-11: synthetic null streams, false rollback ≤ 2.5% per window; RF-12: each H fires on a scripted fake kernel; RF-13: a kernel ignoring rollback → whole-system rollback at 120 s; RF-14: deafness with a fake `busy` lie |
| X4 | reversibility: system snapshot and retention; R2 restore with the zone stopped; Root-owned probe roots; A4; body mode `check`; `settled` pointer | root `src/snapshot.rs` (new; reuses the clone and undo code of `src/access.rs`), `src/probe.rs` (new; reuses `src/sealed/run.rs`), `src/zone.rs` (probe tail), `src/tuple.rs`; `root/sandbox-profiles`; seed `src/boot/{mod.rs, ready.rs, rehydrate.rs}` | W2's I6 | 12–16 | RF-15: snapshot and restore round trip on a fixture `INIT_HOME` (never follows symlinks; sessions kept; pre-restore clone); RF-16: a candidate whose state the parent cannot read is refused; RF-17: restore refused while a zone process lives; RF-18: `check` mode makes no model call |
| X5 | attribution: Root tap parsing of tool names, ext names and skill and agent paths for three families; `x-init-caps` → `x-egress-caps`; cap table; usage files; `cap_use`; `caps.json` | root `src/egress/tap.rs`, `src/extend/usage.rs` (new), `src/lineage/kinds.rs`; `kernel-proxy/src/{request.rs, egress.rs}`; seed `src/provider/` (header), `src/boot/awareness.rs` | new | 10–14 | RF-19: recorded streams of all three families → identical cap sets; RF-20: an ext name split across argument deltas; RF-21: a scan of `ROOT_HOME/extend/` finds no argument text |
| X6 | selection: decay states, archive debt, Fisher-tail correlation, quarantine debt and its 72 h rollback, bloat measurement, `cap_state` | root `src/extend/select.rs` (new), `src/ledger.rs`; `kernel-evo/src/` (archive and quarantine helper commits); seed `skills/evolve/SKILL.md` | new | 10–14 | RF-22: decay table on test ledgers; RF-23: null usage streams quarantine ≤ 1 per 30 days; a harmful cap is quarantined at the designed power; RF-24: bloat refusal and shrink pass |
| X7 | ledger and grants: record-based levels, demotion with `harm_rate`, `/grant`, `/deny`, overlay with expiry, charter `[extend]` and C17, broker members at L0 | root `src/ledger.rs`, `src/charter.rs`, `src/grant.rs` (new), `src/link.rs`, `src/zone.rs`, `src/lineage/verify.rs`; `tests/redteam.rs`; tui (confirmation rendering only) | W5 (Root-records rule kept; T dropped) | 10–14 | RF-25: RT-GAP-1 reproduction (20 self-reported promotions, no owner turns) unlocks nothing; RF-26: test ledgers per level; RF-27: an overlay expires; a grant request cannot change a value; a Root-mechanism kind is refused |
| X8 | kernel lane: Root conformance probe on `kupgrade-smoke`, L1 gating, one open, no body group during probation, kernel windows, gog optional | root `src/sup.rs` (swap phases), `src/sealed/gog.rs` (opt-in path), `src/install.rs` (smoke fixtures); `kernel-supervise/src/kupgrade.rs` | new | 8–12 | RF-28: a candidate kernel failing `prepare_swap` is refused; RF-29: a probation crash → previous tuple plus `kstate`; RF-30: hostile fake kernel (drops lines, forges caps, ignores rollback) → H2, zone evidence only, Root rollback |
| X9 | twin and shadow replay: `twin` pointer and pins, replay store, nightly off-peak scheduler, two arms in Root-owned roots on sealed lanes, statistics, `twin_report`, `twin.json`, `/twin refreeze` | root `src/twin.rs` (new), `src/sealed/run.rs`, `src/egress/lane.rs`, `src/access.rs` (snapshot ids) | new | 12–16 | RF-31: mock replay, both arms, report of n and p; RF-32: the night stops at `usd_day`; RF-33: run roots removed by exact path |
| X10 | simulator: null and power tables for H1–H12; false-quarantine rate; decay dynamics; salami, hider, time-bomb and no-op-farming agents | `kernel-gate/src/stats/{sim.rs, extend_sim.rs}` | W8 | 6–8 | RF-34: the acceptance table below |
| X11 | awareness: one tail line `extend · groups 2/4 · window 7/20 · caps 31/64 · L0→L1 4d`; skills `skills/extend/SKILL.md` (new: mid-task extension, `Cap:`, archive and quarantine debts, grants) | seed `src/boot/awareness.rs`, `skills/evolve/SKILL.md`, `skills/extend/SKILL.md` | W9 | 3–4 | T-AW: a line per state; head `prefix_hash` unchanged |
| X12 | docs fold: evolution.md, evolution-v2.md, gate.md, supervise.md, root.md, egress.md, sealed-eval.md, body/boot.md, awareness.md, testing.md, CONVENTIONS §4 and §6; amendment notes on ADR-006 and ADR-007 | `docs/**` | W10 | 6–8 | doc-drift checks |
| **Total** | | | | **118–162** | |

ADR-010's W4 (draw and audit forms), W6 (evaluator role) and W7 (G4 as an audit tool) are dropped. W11, the benchmark, stays optional and outside this total.

**Critical path and lanes.**
- **Start first, in parallel:** X10, since the simulator fixes the defaults before code freezes, and X0b–X0d.
- **Critical path:** X2 → X3 → X7 → integration, about 48–62 agent-hours.
- **Beside it, in their own crates:** X4, X5, X6, X8 and X9.
- **Last:** X11 and X12.

With five lanes, wall-clock is about the critical path. Disk is the practical constraint: a cold kernel build needs about 4 GB, and the build log of 2026-10-22 records 6.9 GB free.

**Definition of done** (before `extend.mode` defaults to `"free"`):
1. All RF tests are green, and `ci/local.sh` is green three times.
2. X10's acceptance table holds on the shipped defaults:
   - null false rollback ≤ 2.5% per window;
   - false quarantine ≤ 1 per 30 days on null usage;
   - a 3× cost blow-up is caught within its window ≥ 60% of the time;
   - a no-op farming agent with no owner turns never leaves L0;
   - a salami agent (each group −2% on cost) shows in the twin's cost test within 6 weeks ≥ 70% of the time.
3. The live install runs a Root with `memwatch.rs` (X0a).
4. The real-model red team of open question 7 has run within its cap, with no unrecovered state.
5. The owner approves this ADR and its Root growth.

## Alternatives considered

| Alternative | Why not chosen |
|---|---|
| Keep ADR-010 (trust from calibration) | `gain` is defined on S or on families; verification capacity caps the rate of change; about $6 of cold start; mandatory forecasts. It gates open-ended growth on predefined measures, which the owner's vision rules out. |
| No containment (OpenClaw, Hermes) | malicious skills, live self-breakage, ignored stops, bad skills saved (SEP L142, L186–L196); no rollback and no attribution |
| Gate on a purpose the agent declares ("measure what it says it is for") | the agent shapes its own evaluation: the sprint's lesson, and Hermes's self-check |
| An LLM judge of usefulness | ADR-006 decision 8; the model would judge itself; Goodhart |
| Owner review of each capability | ADR-007: review does not scale, and fails once the model outperforms the reviewer |
| The sentinel in the kernel | from L1 the kernel is agent code and could disable its own rollback |
| Selection by use alone, with no harm correlation | use is not value; Hermes saves bad skills |
| Restore state on every rollback (R2 always) | it loses learned memory for harms that live in code; R1 first, R2 as the escalation |
| A Bayesian harm posterior per cap instead of the Fisher tail | deferred: the Fisher tail is exact and simple at n = 120, and a posterior needs a prior nobody can justify yet |
| A fixed benchmark up front, as the gate | §Evaluation: the capabilities do not exist yet; kept only as report-only design B |
| A plastic TUI | it runs unsandboxed as the owner, so plasticity would be an escape; the agent shapes the UI through frames |

## Consequences

**Cost per self-change.**

| Scope | Model spend | Typical latency |
|---|---|---|
| `t0` | $0 | seconds |
| `t1` | $0 | ≈ 1–2 min |
| `t2` | ≈ $0.002 (A2), plus ≈ $0.01 for async G3 | ≈ 6–8 min warm |

There is no audit or G4 spend on the default path; `Measure: g4` costs ≈ $0.37–0.67 when the agent asks for it. ADR-010's steady state was ≈ $0.035–0.08 per change, with a ≈ $6 cold start. Operation costs the twin, at most $0.30 a night (open question 3). This ADR and X0–X12 are $0.

**The Root grows.** Estimated raw lines:

| Piece | Lines |
|---|---|
| sentinel | ≈ 380 |
| snapshots and restore (reusing `access.rs`) | ≈ 150 |
| A4 probe runner (reusing `sealed/run.rs`) | ≈ 120 |
| tap parsing | ≈ 110 |
| usage and selection | ≈ 260 |
| ledger rewrite | ≈ 120, replacing ≈ 60 |
| grants | ≈ 120 |
| twin | ≈ 260 |
| link, kinds, charter and view | ≈ 140 |
| **total** | **≈ +1,660 / −60** |

- Each piece decides a rollback, a quarantine, a level or an unforgeable measurement, so it belongs in the Root for the same reason as ADR-009's access code. ADR-010's approved ≈ +630 is superseded.
- The core subtotal (`zone.rs`, `sup.rs`, `panic.rs`, `egress/money.rs`, `lineage/verify.rs`) grows only by hooks, about +60 lines.
- root.md §4.8 gets sub-budget lines `sentinel`, `snapshot`, `extend`, `grant` and `twin` (measured + 5%).
- The ≈ 7,500 total is a provisional owner decision and must rise (open question 2).
- A minimum set (sentinel, snapshots, probe, ledger, link: ≈ +950) is possible. It takes three cuts:
  - usage attribution becomes zone-reported only;
  - grants become charter edits;
  - the twin is deferred.

**The claim changes.** INIT stops claiming that its self-changes are measured. It claims that they are free, attributed, reversible and harm-bounded, and it reports, rather than asserts, whether they help (twin). The market note's pitch, "measured, gated and reversible" (CAM L224), becomes "free, attributed and reversible", and containment stays the differentiator.

**Other consequences:**
- ADR-006 decisions 3, 4, 5 and 7 are superseded on the free path and kept on `"gated"`.
- ADR-007:
  - decision 3 (the ladder) is redefined (§7);
  - decision 4 (the honest stick) is narrowed to harm, and S remains an instrument;
  - decision 5 (agent-written tasks) becomes the instrument `Measure: exp`;
  - decisions 1, 2 and 6 are unchanged.
- ADR-010 is superseded; its status line says so.
- These get amendment notes in X12:
  - the T0 path list and the `t0` blob (gate.md §3.1, root.md §3.7.1);
  - the body modes (body/boot.md);
  - the broker's member rule (root.md §3.14);
  - `fleet` at L0 (supervise.md §3.14).
- CONVENTIONS §4 registers ADR-011, and §6 records it as PENDING OWNER (X12).
- No cargo build was run for this ADR; it is design only.

## Open questions (owner)

1. **Kernel plasticity.** Make the kernel plastic from L1 through the kernel lane: Root conformance probe, probation, whole-system rollback, no mandatory gate-of-gates. Or from L0? Or keep L3 plus the mutation suite (your ADR-010 decision 8)? **Recommended: L1, without the mandatory mutation suite.** The one-week delay exists only so the sentinel has a calibrated baseline. The kernel is no longer the evaluator, so the mutation suite no longer protects anything on the default path. It stays available as `Measure: gog`.
2. **Root growth.** Approve ≈ +1,600 net Root lines on separate sub-budget lines? Or the minimum set of ≈ +950, with zone-reported attribution, grants as charter edits and no twin? **Recommended: the full set.** Without the Root tap, quarantine could be dodged by a kernel that misreports usage. Without the twin there is no honest evidence at all.
3. **Money for the twin.** Nightly shadow replay capped at `twin.usd_day` = $0.30, so at most about $9 a month, on the current model, off-peak. Replays re-send owner content the provider has already seen. **Recommended: approve, enabled, with the $0.30 cap.**
4. **Sealed-audit and milestone regressions.** Report-only, or keep them as demotion triggers? **Recommended: report-only, with an owner alert.** They are predefined measures, and this ADR does not gate on them. The twin and the harm signals carry safety.
5. **Third-party crates (`dep`).** An owner grant per crate set, or free at L0? **Recommended: an owner grant.** Supply-chain harm is invisible to every generic signal until it is too late (ClawHavoc), and the vendor fetch is not built anyway.
6. **Automatic archive and quarantine.** Should the Root enforce archive debt (120 owner turns and 21 days unused) and harm-correlation quarantine (Fisher tail, 24 h debt)? Or only list candidates for the agent? **Recommended: enforce both.** Both are reversible: restore from `archive/`, a new version after 7 days, or your `/release`. Listing alone did not stop Hermes from keeping bad skills.
7. **Real-model red team before the default flips.** A scripted adversary on the real model against the free path and the kernel lane: forged caps, hidden errors, a time-bomb capability, a kernel that drops lines. **Recommended: yes, cap $3 real, off-peak, in a disposable test home.** The $0 red team never ran a real-model adversary.
8. **Twin re-freeze.** Only by your `/twin refreeze`, or automatically? **Recommended: owner-only.** The Root suggests it after 90 days or at a main-model change. An automatic re-freeze would let drift reset its own baseline.
9. **Sub-agents at L0.** Members up to `quotas.parallel_bodies` (4) from L0, home-read and never workspace-writing, with workspace writing by grant? **Recommended: yes.** The money ceilings bound their spend, ADR-009 bounds their writes, and sub-agent patterns are among the capabilities the vision names.
