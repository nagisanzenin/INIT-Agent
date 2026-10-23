# ADR-010: Trust-first evolution (accept first, verify after, trust from calibration)

Status: **Superseded by ADR-011** (docs/adr/011-reward-free-self-extension.md, 2026-10-22). Before that: accepted by the owner 2026-10-22, with the decisions recorded under Open questions; enabled only behind charter `trust.accept = "self"` once the definition of done is met (amends ADR-006 decisions 3, 4, 5 and 7 and ADR-007 decisions 3 and 5; ADR-007's Root is unchanged) · Area: evolution (cross-cuts root and kernel) · folded into the owner docs by lane DOC-010 (W10)
Depends on: ADR-006, ADR-007, ADR-008, ADR-009, evolution.md (§3.2, §3.8–§3.10, §3.14–§3.15), evolution-v2.md (§3.4, §3.5, §3.6, §3.9.3), m7-design.md (§3.2, §3.2.13), kernel/gate.md (§3.1, §3.9, §3.12, §3.13), root/root.md (§3.7, §3.10.5, §3.13, §3.15), root/sealed-eval.md (§3.5, §3.6), root/egress.md (§3.3, §3.8), RELEASE_PLAN.md (v0.1.x findings), measurements/m7-results.md, reports/INIT proof sprint verdict.md, reports/Benchmark harness có dữ liệu công khai.md   Defines: self-acceptance on mechanical invariants (I0–I6); the watch window and the Root sentinel's rollback triggers; forecast clauses (`harm`, `gain`, `cost_up`, `live_worse`); the Root audit draw and the audit forms; the trust score T and what it sets (audit rate, evolution share, level); the proposer/evaluator split and Root-sealed generated eval families from L0; the records `watch_open`, `audit_draw`, `watch_close`, `audit_change`, `audit_kernel`, `trust`, `trust_epoch`, `family_band`; the charter section `[trust]`

## Context

The proof sprint (measurements/m7-results.md) ran the whole loop on real deepseek-flash for $2.67. The miner found a real friction (`cap.hit:turns`), the agent wrote proposal p-000001, and its chosen variant `c88be5f` passed G0–G3 and hot-swapped into the live session. Then:
- the agent's own `Predicts:` said **+15 points** on S;
- the visible-pool screen estimated **+12** (`est 120‰`);
- the sealed G4 measured **−5** (`d = −50‰`, status `inferior`), and the change was rolled back.

The sprint proved two things at once. First, the honest instrument works: the sealed, Root-held measurement caught a change that both the proposer and the visible screen believed in. Second, the per-change gate does not scale:
- **Resolution.** At standard tier G4 is an anti-regression gate. It certifies only broad gains of about 17 points, accepts a no-op as `better` about 1 time in 28, and costs $0.37–0.67 and one of 20 S looks per attempt (m7-design.md §3.2.6). Real self-changes are mostly small.
- **Fragility.** One verdict took four G4 attempts and seven harness deviations (disk, a stranded proposal, a G3 timeout, the money guard).
- **Ceiling.** A fixed, human-authored test set caps what the agent can show, which is the same objection ADR-007 raised against the owner's task pool. A post-AGI model's changes will be many, small, and in capabilities no maintainer thought to test.

The owner concluded that requiring a pre-proven, statistically significant gain on a human-authored set for every self-change blocks the AI and does not scale. The direction agreed: the AI owns the measuring; the anchor is reality, never opinion; accept first and verify after; trust equals calibration; only the Root stays fixed.

The sprint's lesson has to survive the redesign: **an evaluation the proposer can see or shape will fool it.** So this ADR keeps the measurement honest and Root-held, but moves it from a per-change admission gate to sampled verification after acceptance. The proposer's own judgment becomes a forecast that is scored against outcomes and is never used as evidence.

## Decision

1. **Self-acceptance.** A body change (scopes `t0`, `t1`, `t2`) becomes the running version once it passes the mechanical invariants I0–I6 (Mechanisms §1): it builds, its tests pass, it describes itself, it boots, it can still edit itself, its parent can boot on its state, and the Root's boundary is intact. G4 is not on the default path. A `dep` change (new third-party crates) never self-accepts: it keeps the synchronous G4 before the handoff and also needs the owner's approval (owner decision 5, 2026-10-22; Mechanisms §1).
2. **Verify after, on reality.** Every self-accepted version runs under a **watch window**. The Root's **sentinel** measures real owner turns (money, time and owner corrections, all observed by the Root itself) against the last known-good version, and rolls back on degradation. Nothing becomes LKG before its window closes without a trigger.
3. **Every change forecasts.** Each commit carries four protocol-fixed forecast clauses in `Predicts:` with probabilities (`harm`, `gain`, `cost_up`, `live_worse`). A forecast never accepts, never rejects, and never lowers its own change's audit probability.
4. **Random Root audits.** At watch open the Root draws, with a secret key, whether the change is audited. Audited changes are compared with their parent on Root-run sources (sealed S, and AI-authored generated eval families sealed by the Root), plus kernel-run sources that can only veto.
5. **Trust is calibration.** The Root scores audited forecasts with a Brier skill score against the no-effect forecaster (Mechanisms §5). The resulting trust T sets the audit rate, the evolution share and the autonomy level. It replaces ADR-007's fixed ladder thresholds.
6. **The AI owns the measuring.** An **evaluator** role, separate from the proposer, writes eval-family generators and declarative monitors from L0. Humans stop authoring per-change tests. Sealed S stays as the Root's fixed anchor, one audit source among several.
7. **Only what the Root measured counts toward trust.** Kernel-reported data (promotions, pool runs, friction rates, AI monitors) can trigger a rollback but never raise T. This closes RT-GAP-1's class of bug.
8. **The Root is the fixed point.** Stop and panic, money ceilings, keys and egress, scope, signed lineage, rollback, the trust formula and the audit floor are Root code. No level, trust value or record gives the agent a path to change or remove the Root.
9. **Rollout switch.** Charter key `trust.accept = "self" | "g4"`. `"g4"` keeps today's G4 path; `"self"` enables this ADR. The default becomes `"self"` only when the definition of done (Work breakdown) is met.

## What is removed and what is kept

| Item | Today | Under ADR-010 |
|---|---|---|
| G4 per change (gate.md §3.9, m7 §3.2.5) | required for `t1`/`t2` to become LKG; synchronous for `dep`, `t0_escalated` | **removed from the default path.** Its machinery (screen, Root slice, exact sign-flip, bootstrap NI, cost per pass) becomes the S audit form. The trailer `Measure: g4` (evolution-v2 §3.6.3) still asks for a synchronous G4 when the agent wants a certified verdict. K3 for kernel candidates is unchanged. **`dep` keeps its synchronous G4** and adds the owner's approval (owner decision 5). |
| Accept rule v2, tie-breaks, `lineage_gain` ratchet, `audit_required`, `capability_gain` | decide LKG | used only by `Measure: g4` and K3. On the default path, audit outcomes are mapped to forecast events (§4) instead of verdicts. |
| Sealed S v1, looks cap, rotation, canaries, leak scans | the gate's gain test | **kept as one audit source** (the anchor). 20 looks per epoch still bind. |
| Milestone check on the untouched reserve, the `confirmed` pointer | triggered by a full ratchet span | **kept** as the lineage-level anchor. New trigger: every 12 self-accepted LKG moves since `confirmed`, or 30 days; never more often than 7 days. |
| Scheduled sealed audit, `audited` pointer, demotion rollback to audited-good | as root/sealed-eval.md §3.5 | unchanged |
| G0–G3 | promote stages | kept as invariants I1–I4, plus I0, I5, I6 |
| Visible pool and screen E0–E3 | reject-only screen | kept for search (engine variants) and as a veto-only audit form |
| `t0_batch`, T0 debt | deferred batched G4 for T0 | subsumed: every T0 commit has its own window and audit draw. `Repro:` for behaviour-changing T0 text stays (I5). |
| Provisional state, depth cap 3, no external actions while provisional (F17) | kept | kept. The provisional period is now the watch window. |
| `Predicts:` scored, never gated on (ADR-006 decision 7) | hit rate is a self-signal | free clauses unchanged. The four fixed forecast clauses feed T, which sets audit intensity and level, never a single change's fate. |
| Ladder L0–L4 rights, `SUPPORTED_MAX_LEVEL`, charter cap | thresholds on promotion counts, days, audits | rights unchanged; unlock criteria from T (§6). `SUPPORTED_MAX_LEVEL` still gates each Root release (roadmap R14). |
| Agent-authored task families (L1+, kernel-sealed at L1–L2) | evidence for `capability_gain` | from L0, **generated** families sealed and run by the Root count toward T; enumerated families are veto-only. |

## Mechanisms

### 1. Invariants (all mechanical; about $0.01–0.02 per change)

| Id | Check | Runs in | Cost | Fails as |
|---|---|---|---|---|
| I0 | **Boundary self-check.** The Root re-hashes its own binary against `root_start`, the active charter digest, the embedded sandbox profiles and the lineage tail, and records the kernel-reported scope. The Root, keys, ceilings, scope and the trust code live outside the zone by construction (tail Z1–Z7), so no body change can reach them; I0 makes that a recorded fact per change. | Root, at `watch_open` | $0 | `root.boundary` (alert; no watch opens) |
| I1 | G0: offline build, `clippy -D warnings`, tests, `forbid(unsafe_code)`, build-config policy and shape checks (they already refuse mutant class M2, weakened tests) | kernel build slot | $0 | `gate.g0.*` |
| I2 | G1: self-map, tag markers, lesson evidence | kernel | $0 | `gate.g1.*` |
| I3 | G2: boot probe of candidate and parent, prefix hash, continuity, tools | kernel probe | ≈ $0.002 | `gate.g2.*` |
| I4 | G3: scripted self-edit through G0–G2 (needs the G3-STAGE-TIMEOUT fix, W0c) | kernel probe | ≈ $0.01 | `gate.g3.*` |
| I5 | Evidence and scope rules: the commit grammar and validator (evolution.md §3.9), attested `Repro:` for behaviour-changing T0 text and for `fix`/`guard`/prefix changes (evolution-v2 §3.6.1), the four forecast clauses (§2). A change touching `skills/evaluate/**` is forced into an audit (§4) | kernel validator | $0 | `trailer.*`, `predicts.forecast_missing:<event>`, `evo.scope_paths` |
| I6 | **Rollback probe.** After G2's candidate boot writes its session log and state, the parent body boots on that state and must reach `ready` and rehydrate. A change whose state the parent cannot read is not rollback-safe. | kernel probe | $0 (no model call) | `gate.g2.rollback_incompatible` |

`t0` runs I0, I1, I2, I5. `t1` and `t2` run all seven. `dep` runs all seven, then the synchronous G4 and the owner's approval (below) before the handoff. Kernel candidates (scope `k`, L3+) keep K0–K3 and the gate-of-gates unchanged.

**`dep` (owner decision 5, 2026-10-22).** A change with scope `dep` (gate.md §3.1: anything under `vendor/`, or a registry entry added, removed or changed in a lockfile) never self-accepts, in either `trust.accept` mode. After I0–I6 it runs today's synchronous G4 (accept rule v2) and then waits for the owner: `/approve <sha>` in the owner channel, reintroduced by this decision for `dep` candidates only (v03-decisions D4 removed the general `/approve`). The Root relays the line and checks its origin like every `owner` record (root.md §3.10.5), and it refuses `watch_open{scope:"dep"}` until it holds both a `sealed_g4{purpose:"gate"}` accept and `owner{kind:"approve", sha}` for that sha. A candidate still unapproved after 7 days returns to `screened` with `evo.dep_unapproved` (not a demotion). Its G4 counts as the change's forced S-form audit (§4) and is scored like any other. The reasons are supply-chain risk, and that the agent cannot fetch crates as built anyway: the vendor job's fetch steps (kernel/initctl.md §4.1 steps 3–8, over the egress class `crates`) are not built. This supersedes CONVENTIONS §6 ESC-1 ("no approval for crates").

### 2. Forecasts

The `Predicts:` grammar (evolution.md §3.10.1) gains one clause kind and an optional probability on metric clauses. The clause limit rises from 5 to 9.

```abnf
clause          = metric-clause / fixes-clause / risks-clause / forecast-clause
forecast-clause = event SP "p=" prob
event           = "harm" / "gain" / "cost_up" / "live_worse"
prob            = 1*2DIGIT "%"                         ; 1..99; scored as permille, clamped to [20, 980]
metric-clause   = metric [ "@" scope ] *WSP op *WSP target [ SP "p=" prob ]   ; p optional, reported, not in T
```

Every candidate on the self-accept path must carry all four forecast clauses exactly once (`predicts.forecast_missing:<event>`, `predicts.forecast_duplicate`). Example:

```
Predicts: harm p=5%; gain p=40%; cost_up p=10%; live_worse p=8%; pass_rate@label:interactive > parent
```

The events are fixed by the protocol, so the agent chooses only probabilities, never what is scored:

| Event | Outcome = 1 when | Observed for | Null reference p0 (default until measured) |
|---|---|---|---|
| `harm` | the primary audit form fails non-inferiority (`inferior`, `inferior_early`) or trips a candidate tripwire (`canary`, `tamper`, `leak`, `health`); **or** the change was drawn for audit and was rolled back by a Root hard trigger or owner `/reject` during its window | drawn changes only (the draw is random and made before any outcome, so the scored sample is unbiased) | the form's A/A false-reject rate: S standard **86‰** (FR from `sealed_calibrate`, M7 line C) |
| `gain` | the primary form's exact sign-flip test gives p ≤ 0.25 and D ≥ 1/n (the ratchet signal of m7 §3.2.13; `superior` implies it) | drawn changes whose audit completed | the form's A/A rate of that signal: **180‰** (m7 §3.2.13, effect 0) |
| `cost_up` | the primary form's cost-per-pass ratio > 1.2 | as `gain` | **50‰**, refitted from the A/A cost noise |
| `live_worse` | the sentinel's soft check flags the window, or a hard trigger rolled it back (§3) | every change whose window saw ≥ 5 owner turns and no other open change (`confounded_by` empty) | the sentinel's own split-baseline A/A: **90‰** |

The Root writes each form's measured p0 into its calibration records (`sealed_calibrate` gains `ratchet_fa_permille` and `cost_up_fa_permille`; `family_band` carries a family's own; the sentinel's A/A goes into `trust`). Free clauses keep today's scoring in `refs/notes/predict`.

### 3. The watch window and the Root sentinel

**Sequence.**
1. Promote, validate, invariants I1–I6 (kernel).
2. The kernel appends `watch_open{sha, from_sha, scope, depth, forecast}`. The Root runs I0, retains the parent tuple, draws the audit (§4), appends `audit_draw`, and replies.
3. Handoff (`t2`, `dep`) or reload (`t0`, `t1`): the version is current and provisional.
4. The sentinel measures owner turns until the window closes, and rolls back on a hard trigger.
5. The Root appends `watch_close{outcome}`. If the change was drawn, its audits run within `audit_due_h` (72 h).
6. The Root moves `lkg` to the change's tuple only when its close is `clean` or `quiet`, its audit (if drawn) has `harm = 0`, and every ancestor is LKG. It refuses a kernel `promotion{lkg:true}` that does not meet this, so LKG is Root-anchored in both modes (in `"g4"` mode it requires a Root `sealed_g4` accept instead).

**What a turn is (Root-observable at every level).** A turn starts when the Root relays an owner `line`. It ends at the first 30 s with no zone egress request in flight, or after 15 minutes. The engine's idle-time worker starts only after 900 s of idle (evolution-v2 §3.4.5), so its spend falls outside turns.

| Signal per turn | Source (all Root-held) |
|---|---|
| `cost` | zone egress spend in the turn (egress ledger) |
| `wall` | turn duration |
| `correction` (0/1) | any of: an `interrupt` frame, a line relayed while the turn is in progress (a steer), `/undo`, `/stop`, `/reject`, `/panic` (first words the Root already keeps, root.md §3.15) |
| `crash` | zone or body exit during the turn (`zone_exit`, `body_safe`) |
| `zone_spend_per_turn` (window level) | all zone spend during the window ÷ turns, so spend moved out of turns is still counted |

**Baseline.** A ring of the last m = 40 turns on the `lkg` tuple, plus a second ring of 60 turns recorded on the `confirmed` tuple (for drift).

**Test.** The exact exceedance test, distribution-free under exchangeability of baseline and window turns. With threshold = the j-th largest of the m baseline values (j = 10, the 75th percentile) and K = the number of the n window values above it:

```text
P(K = k) = C(j−1+k, k) · C(m−j+n−k, n−k) / C(m+n, n)
```

Corrections use a binomial tail with rate r0 = max(50‰, (b+1)/(m+2)), b = corrections in the baseline. All of it is integer combinatorics, about 40 lines.

| Trigger | Rule (looks at n = 5, 10, 20 window turns) | Action |
|---|---|---|
| **H1 crash** | `crash_loop` or `body_safe` (existing), or ≥ 2 body crashes in the window | rollback |
| **H2 owner** | `/reject <sha>` (existing), `/panic` (existing: rollback and demotion) | rollback |
| **H3 cost** | exceedance tail ≤ 0.002 and window median ≥ 1.5 × baseline median: K ≥ 5 of 5, ≥ 8 of 10, ≥ 13 of 20 | rollback |
| **H4 wall** | as H3 on `wall` | rollback |
| **H5 corrections** | binomial tail ≤ 0.002 at r0 (e.g. r0 = 100‰: ≥ 4 of 5, ≥ 5 of 10, ≥ 8 of 20) | rollback |
| **H6 resources** | ≥ 2 memory-watchdog kills (`mem_cap` records of root.md §3.14.1, MEM-RUNAWAY) of the version's processes in the window | rollback |
| **H7 kernel veto** | the kernel's friction monitor or an AI monitor (§7) requests a rollback | rollback (always honoured: it is the safe direction) |
| **S1 soft** | at close (n = 20): exceedance tail ≤ 0.05 on `cost` or `wall` (K ≥ 10 of 20), or on corrections, or on `zone_spend_per_turn` | `live_worse = 1`; forced audit if not drawn |
| **S2 drift** | at close: last 60 turns against the `confirmed` ring, tail ≤ 0.01 | queue a milestone check (overrides `milestone_min_days`) |

Design simulation (scratch, 20,000 null windows, lognormal turn costs σ = 1, corrections at 10%): the hard triggers roll back a harmless change in **1.0%** of windows. They catch a 2× cost blow-up within the window 20% of the time and a 3× one 62%; corrections rising from 10% to 50%, 82%. The soft check flags a 2× cost rise 54%, 1.5× 24%, and corrections 10% → 30% 58%. Hard triggers are for blatant harm; audits and milestones are for the rest.

**Window close.** At `watch_turns` (20) owner turns, or `watch_max_h` (72 h), whichever is first, and never before `watch_min_h` (2 h). With fewer than 5 turns at 72 h the close is `quiet`: there is no reality signal, so the change's audit probability is raised to at least `quiet_audit_permille` (500) by a second draw at close.

**Rollback mechanics.** The Root sends `watch.rollback{to}`; the kernel rolls the body back at its next step boundary. If the zone is not on the target tuple within 120 s, the Root performs the whole-system rollback of root.md §3.7.3. With several open changes (depth ≤ 3) the target is the newest LKG; every open change is marked `collateral` except the newest, and collateral changes may be re-promoted one at a time (depth 1) so the next trigger is attributable. A sha rolled back by H1–H6 cannot be re-promoted for 7 days. When a watch opens with a workspace set, the Root takes an APFS snapshot (ADR-009 §4), so `/undo` reaches the pre-change state.

### 4. Audits

**Draw.** At each 7-day trust epoch the Root draws a key K_e from `/dev/urandom` and appends `trust_epoch{epoch, key_sha256}`; at the epoch's end it appends `trust_epoch{epoch, key}`, revealing it.

```text
u_j      = first 4 bytes of HMAC-SHA256(K_e, "audit" ‖ watch_seq ‖ sha) as u32
selected ⇔ u_j < ⌊a_j · 2^32⌋
a_j      = max(a(T_lcb), forced_j)                       // §6
forced_j = 1 if scope = dep, an upstream merge, a model change, `Measure: g4`, soft flag S1,
              a change touching skills/evaluate/**,
              p_gain ≥ 800‰ or p_harm ≥ 200‰ (a forecast can raise, never lower, its own audit),
              or the time floor (no S-form audit in the last 7 days while changes landed)
         = quiet_audit_permille at a quiet close (second draw, label "quiet")
```

The kernel commits the sha, and with it the forecast, before the draw, and cannot compute the HMAC. After the reveal, `init-root ledger --verify` (and the agent) can check every draw.

**Forms.** One primary form per drawn change is scored; the others are secondary.

| Form | Runner | What | Cost per audit (off-peak, measured $0.0035/run) | Use |
|---|---|---|---|---|
| **S** | Root | the G4 v2 slice of the change's tuple against its **parent** tuple (not only `lkg`: `sealed.g4_slice` gains `purpose:"audit"`, and the Root retains the tuples of open watches), standard tier, incumbent cache | ≈ $0.37 cached, $0.67 fresh; uses one look | primary when the S sub-draw selects it |
| **F** (families) | Root | 6 fresh instances from Root-sealed generated families (§7) at R = 3 both arms, same statistics | ≈ $0.07 cached, $0.13 fresh | primary otherwise; secondary when S is primary (so the divergence check of §7 gets data) |
| **P** (pool) | kernel | the visible-pool screen (12 tasks, R = 1, candidate fresh) | ≈ $0.04 | veto only (`audit_kernel`) |
| **E** (replay) | kernel | E1/E2 replay of the change's own `Repro:` evidence | $0 | veto only |

**S sub-draw.** `s = 1` while the anchor group's mass is below `E_min` (§5); otherwise `s = clamp(250‰, 1000‰, looks_left / expected drawn changes to the epoch's end)`, the expectation taken from the last 7 days. If no S look is left and no family has weight, the change still self-accepts, its kernel forms still veto, and it adds no trust: verification capacity, not the gate, limits how fast trust grows.

**Due time and budget.** Root forms are charged to a Root allowance `trust.audit_usd_day` (default $1.00, kept from the zone inside `money.day_usd` like `sealed.reserve_usd`), so the agent can neither starve nor inflate them. When the allowance is spent, the Root refuses new `watch_open` with `root.audit_budget`: the agent cannot change faster than it can be verified. A drawn audit not finished within 72 h of the close (Root outage, peak window, allowance) keeps the change provisional; if it is still unaudited 24 h after its due time, it is rolled back as `audit_overdue`, which is not a demotion.

**Outcome.** `harm = 1` rolls the change back (and its descendants, as collateral). Every outcome is an `audit_change` record (Root forms) or `audit_kernel` record (kernel forms).

### 5. Trust T

For each scored change j and event e with forecast p_je (clamped to [20, 980]‰), outcome o_je and null reference p0_e of the form that produced the outcome:

```text
S_j   = Σ_e w_e · [ (p0_e − o_je)² − (p_je − o_je)² ]        // improvement over "the change does nothing"
E0_j  = Σ_e w_e · p0_e · (1 − p0_e)                          // the null forecaster's expected error
T     = Σ_j S_j / (Σ_j E0_j + κ)
T_lcb = (Σ_j S_j − z · √n · sd(S)) / (Σ_j E0_j + κ)
w_harm = 2, w_gain = w_cost_up = w_live_worse = 1;  κ = 1.0;  z = 1;  both clamped to [−1, 1]
window: the last 60 scored changes
```

All arithmetic is integer (permille inputs, micro-unit products, integer square root), in the Root.

**Why this form.**
- **Honest reporting is optimal.** The denominator does not depend on the forecasts, so each S_j is a strictly proper Brier improvement. In simulation, shading a skilled agent's gain forecasts by ±10 points lowered its expected T from 0.59 to 0.56.
- **No credit for the obvious.** Events are fixed, so trivially true clauses cannot be picked. A flood of no-op changes forecast at p0 earns S_j ≈ 0 and grows the denominator: it lowers T.
- **Overconfidence costs.** The proof-sprint change, scored with plausible forecasts (harm 5%, gain 80%, cost_up 10%; outcome harm 1, gain 0, cost_up 0), gives S = −0.749, E0 = 0.352, **T = −0.55**: audits stay at 100%.

**Two groups.** The **anchor** group holds S-form events, `live_worse`, and harm from Root hard triggers; the **agent** group holds F-form events. T_g is computed per group, mass_g = Σ E0 in the group, E_min = 2.0 (about 6 S-audited changes).
- Both groups have mass ≥ E_min: T = min(T_anchor, T_agent), and the same for T_lcb.
- Only the agent group: T = min(T_agent, 0.5). Trust above 0.5 needs Root-anchored evidence.
- Only the anchor group: T = T_anchor. Neither: cold.

**Cold start.** `a = 1` (every change audited) until n_scored ≥ 20, Σ E0 ≥ 4.0 and anchor mass ≥ E_min.

**Fast reactions.**
- **Surprise boost.** An audited harm with p_harm < 200‰ sets a ≥ 500‰ for the next 10 changes.
- **Resets to cold.** A milestone `regression`, a sealed-audit regression, and a change of the default model (calibration is model-specific, like thinning).

**Design simulation** (2,000 replications per cell, events drawn from each agent's true rates):

| Agent | n = 20: T / T_lcb / audit rate | n = 60: T / T_lcb / audit rate |
|---|---|---|
| skilled (knows which changes help or risk harm) | +0.54 / +0.32 / 0.49 | +0.59 / +0.45 / 0.32 |
| modest (honest; changes help 40%, harm 6%; cannot tell which) | +0.13 / +0.01 / 0.91 | +0.14 / +0.06 / 0.87 |
| honest, changes do nothing | 0.00 / 0.00 / 1.00 | 0.00 / 0.00 / 1.00 |
| overconfident (says gain 80%, true 30%) | −0.60 / −0.84 / 1.00 | −0.67 / −0.84 / 1.00 |
| hider (true harm 20%, says 2%) | −0.08 / −0.15 / 1.00 | −0.09 / −0.13 / 1.00 |

An honest agent that cannot tell its good changes from its bad ones keeps near-dense audits. That is intended: trust is earned by knowing which changes help, not by being harmless on average (open question 4).

### 6. What T sets

**Audit rate.** `a(T_lcb) = max(a_floor, (1 − max(0, T_lcb))²)`, a_floor = 100‰.

| T_lcb | ≤ 0 | 0.15 | 0.30 | 0.45 | 0.60 | ≥ 0.684 |
|---|---|---|---|---|---|---|
| a | 1.00 | 0.72 | 0.49 | 0.30 | 0.16 | 0.10 (floor) |

**Evolution share.** The Root's bound becomes `share_max = clamp(600, 900, 600 + 300 · max(0, T_lcb))` permille at every level, minus 100 per demotion (root.md §3.13.5's rule; `value_window` stays a reported metric).

**Level.** Rights per level are unchanged (root.md §3.13.4), except that the evaluator role (§7) is an L0 right. Unlock criteria, all from Root records since the level's entry:

| To | T_lcb | Scored changes | Other | Min hold |
|---|---|---|---|---|
| L1 | ≥ 0.15 | ≥ 20 | anchor mass ≥ E_min; 0 canary, tamper or leak trips; 0 demoting ceiling hits | 7 days |
| L2 | ≥ 0.30 | ≥ 40 | anchor mass ≥ 4.0; ≥ 1 milestone `confirmed` since L1 and no milestone regression | 14 days |
| L3 | ≥ 0.45 | ≥ 60 | `mutation{pass:true}` (unchanged) | 30 days |
| L4 | ≥ 0.60 | ≥ 80 | charter `max_level = 4` | 30 days |

**Demotion.** The existing triggers keep their rollback (root.md §3.13.3). New trigger: T_lcb below the current level's threshold minus 0.10 appends `ladder{direction:"down", cause:"trust"}` with **no rollback**, because nothing regressed; only the evidence for the level did. The charter may make every value stricter, never looser (rule C7, extended).

### 7. Proposer and evaluator

| Role | Instance | Writes | Never sees |
|---|---|---|---|
| **Proposer** | the live agent; the evolution worker (lane `evolve`) | body changes and their forecasts | family generators and instances, S, audit internals; its own audit result until the window closes (only the draw and rounded aggregates after) |
| **Evaluator** (new) | probe-kind instance, purpose `evaluate`, fresh session; lane allows `eval.*`, `status`, `wait`, `job.*` only | eval-family **generators** and checks; declarative monitors | proposals, variants, screens, forecasts, watch state, pending diffs (its evidence directory holds the friction index, the capability self-model and lineage up to LKG) |
| **Auditor** | the Root (S, F, sentinel, T) and the kernel (P, E, monitors) | measurements | — (no LLM anywhere in it) |

**Families that count.** `initctl eval submit <dir>` runs the kernel admission rules (root/sealed-eval.md §3.6 rules 1–2), then hands the generator and check to the Root (`store.put kind:"family"`, control-link request `sealed.family_admit`). The Root copies them into `ROOT_HOME/sealed/families/<f>/`, draws instances from fresh seeds, validates each (solution 1000 ×3, no-op ≤ 500 ×3), and runs a band A/A on the current LKG (R = 4, split 2 + 2). It appends `family_band{family, incumbent_permille, p0, weight}`, with weight 1 iff the incumbent pass rate lies in [150, 850]‰ and at least 6 instances are valid. The kernel then deletes its copy by exact path. Every audit draws **fresh** instances, so no instance is ever judged twice. Enumerated families (instances the agent wrote) never count toward T; they stay kernel-held, veto-only.

**Collusion guards** (proposer and evaluator are the same model):
- pre-registration: a family judges a change only if its `task_admit` seq < the change's `watch_open` seq (D3);
- the min over groups (§5);
- **divergence suspension:** over the last 20 changes with both S and F outcomes, if the F gain rate exceeds the S gain rate by more than 250‰, the families admitted in that span get weight 0 and the alert `family_divergence` fires (MS-5's logic, generalized).

**Monitors** are declarative metrics over the miner's signal vocabulary (`{id, signal_pattern, direction:"up_is_worse", min_turns}`, at most 16). They run no code. The kernel evaluates them with the sentinel's exceedance test at α = 0.002, and they can only veto (H7). A monitor that vetoes 3 changes that later audit clean is retired automatically, so a veto-only metric cannot freeze evolution.

### 8. Defaults

| Constant | Default | Enforced by | Tunable |
|---|---|---|---|
| `watch_turns` · `watch_max_h` · `watch_min_h` · min turns for statistics | 20 · 72 · 2 · 5 | Root | charter, stricter (longer) only |
| baseline rings | 40 turns (`lkg`), 60 (`confirmed`) | Root | constant |
| hard α per test · looks | 0.002 · n = 5, 10, 20 | Root | constant |
| soft α · drift α | 0.05 · 0.01 | Root | constant |
| magnitude guard (H3, H4) | median ≥ 1.5 × baseline | Root | constant |
| correction rate floor | 50‰ | Root | constant |
| depth of open changes | 3 | kernel and Root | constant |
| `audit_floor_permille` | 100 | Root | charter, higher only |
| `quiet_audit_permille` | 500 | Root | charter, higher only |
| cold start | 20 scored, Σ E0 ≥ 4.0, anchor ≥ 2.0 | Root | charter, higher only |
| κ · z · weights · clamp | 1.0 · 1 · harm 2, others 1 · [20, 980]‰ | Root | constant |
| trust window | last 60 scored changes | Root | constant |
| surprise boost | p_harm < 200‰ → a ≥ 500‰ for 10 changes | Root | constant |
| forced audit thresholds | p_gain ≥ 800‰, p_harm ≥ 200‰ | Root | constant |
| `audit_due_h` | 72 | Root | constant |
| `trust.audit_usd_day` | "1.00" | Root egress | charter |
| milestone cadence | 12 LKG moves or 30 days; min 7 days | Root | charter (existing keys) |
| re-promote ban after H1–H6 | 7 days | kernel | constant |
| family weight band · instances per audit | [150, 850]‰ · 6 at R = 3 | Root | constant |
| divergence threshold | 250‰ over 20 changes | Root | constant |

The charter keys live in a new `[trust]` section (schema owned by root/root.md, validated as rule C16): `accept`, `audit_usd_day`, `audit_floor_permille`, `quiet_audit_permille`, `cold_min_scored`, `watch_turns`, `watch_max_h`, plus the level thresholds of §6 under `[ladder.lN]`. Every value may only be made stricter than the built-in one.

## Threat model delta

**Self-accepted changes run unproven on the owner's machine.** They run with the live body's rights (ADR-009: home read, workspace write). Mitigations:
- invariants I0–I6;
- the sandbox and the Root boundary (unchanged);
- no external actions while provisional (F17);
- the workspace snapshot at watch open;
- hard triggers within the window;
- depth ≤ 3;
- forced audits for `dep` and upstream merges.

**MEM-RUNAWAY is a prerequisite**, not a footnote: under self-acceptance a runaway change runs at once on the owner's disk. W0a's Root memory watchdog (per-instance phys_footprint cap, kill by held PID, ADR-008-compliant) must land before `trust.accept = "self"` is allowed. Residual: a change that harms quietly within one window (below the sentinel's power) is caught only by an audit draw, a later milestone or a scheduled audit.

**Goodhart on self-chosen metrics.** What could be gamed and why it fails:
- *Trivial forecasts.* The events are protocol-fixed, and scoring is against the no-effect forecaster.
- *Friction suppression* (hiding error text to lower the friction rate). Friction is a search signal and a veto, never trust. A hidden error also hides from the model, and task outcomes in audits drop.
- *Easy self-authored evals.* Only Root-run generated families count. They need a discriminating band, are judged on fresh instances, are capped by the anchor min and the 0.5 cap, and are suspended on divergence from S.
- *The Root's live metrics.* They are fixed by the Root release, not chosen by the agent. No single metric is a target: `live_worse` is one of four events, and a turn's cost cut by doing less shows up as audit harm.
- *Veto-only metrics used to block.* The monitor auto-retire rule.
- Residual: S and the reserve stay maintainer-generated. If their templates stop matching real usage, the anchor weakens (R-ROOT-1).

**Silent degradation (the boiling frog).** Each change is slightly worse but within noise, so parent-relative tests never fire. Mitigations:
- the milestone check of the current tuple against `confirmed` on the untouched reserve (79% power at a cumulative −20, m7 §3.2.13), at least every 12 LKG moves or 30 days;
- drift check S2 against the `confirmed` ring, which queues a milestone early;
- the time floor of one S-form audit per 7 days of changes;
- the weekly sealed audit when the charter enables it.

Residual: between milestones, damage accumulates up to the milestone's power.

**Early miscalibration.** Cold start audits every change; κ shrinks T while evidence is thin; T_lcb is a lower bound; the surprise boost reacts at once; milestone and audit regressions and model changes reset to cold.

Residual, a **strategic agent:** it behaves well until a = a_floor, then lands harmful changes. At the 10% floor the expected number of unaudited harmful changes before the first audited one is 9. Each is still bounded by the window's hard triggers, the next milestone (rollback to `confirmed` with a demotion), the T crash that follows the first catch, and the Root boundary. The floor is a Root constant the charter can only raise.

**Rollback failure modes.**

| Failure | Handling |
|---|---|
| target blobs missing or corrupt | the existing fallback chain: LKG, audited, genesis, else `Halted{no_tuple}` (root.md §3.7.3) |
| the parent cannot read the new state | I6 refuses such changes before they run |
| the kernel ignores `watch.rollback` (L3) | the Root's whole-system rollback after 120 s |
| the target crash-loops | the existing crash-loop path to safe mode |
| harm found after LKG (milestone, scheduled audit) | the existing paths: rollback to `confirmed` or `audited`, with a demotion |
| harm flagged after LKG by a kernel signal | the kernel files a `revert` commit through the normal self-accept path |
| oscillation | the 7-day re-promote ban; `promotes_day` (12) |
| interrupted watch or audit (STRANDED-PROPOSAL) | W0b: proposal states `watching`, `audit_pending`, `rolled_back{trigger}` and `collateral`; a restart resumes the window from Root records; a rollback without a verdict returns the proposal to `screened` with a history entry |
| external effects | none possible while provisional (F17) |
| owner files | the watch-open snapshot and `/undo` (ADR-009) |

**RT-GAP-1 interaction.** RT-GAP-1 counted kernel-self-reported promotions toward L1, and the kernel could fill the Root's store itself. Under self-acceptance every promotion is self-reported by design, so counting them would be worse than before. Hence:
- T and every level criterion use only Root-written records: `audit_change`, `watch_close`, `sealed_g4`, `milestone`, `audit`, `mutation`, `family_band`;
- `min_promotions` is deleted from the ladder;
- draws need a Root secret and are verifiable after the reveal;
- the Root moves `lkg` only on its own `watch_close` and audit records;
- the broker refuses a live body whose binary, ext set and t0 digest match neither `lkg` nor an open watch (`root.unwatched_change`, a demoting rule-(b) hit), so an L3 kernel cannot run an unannounced change.

The ignored `redteam.rs` test becomes a passing negative test (W5).

**New Root code, larger TCB.** A sentinel bug in the conservative direction costs availability (false rollbacks), not safety. A trust-ledger bug could lower audits wrongly. Guards: `ledger --verify` recomputes T and every draw from lineage; shared test vectors with the simulator; a_floor and cold start are constants.

## Records and lineage

New kinds (ICJ, integers, no task ids, no instance ids, no paths, no content):

| Kind | Writer | Fields | Root effect |
|---|---|---|---|
| `watch_open` | kernel | `sha`, `from_sha`, `scope`, `depth`, `forecast{harm,gain,cost_up,live_worse}` (permille), `proposal` | I0; retain parent tuple; draw |
| `audit_draw` | Root | `watch_seq`, `sha`, `epoch`, `a_permille`, `selected`, `forms`, `reason` (`random`\|`cold`\|`forced:<why>`\|`quiet`), `due_at` | — |
| `watch_close` | Root | `watch_seq`, `sha`, `outcome` (`clean`\|`quiet`\|`rolled_back`\|`collateral`), `trigger` (H1–H7 or null), `turns`, `live{cost_ratio_permille, wall_ratio_permille, corrections, k_cost, k_wall, soft, drift}`, `confounded_by` | rollback or LKG eligibility |
| `audit_change` | Root | `watch_seq`, `sha`, `parent`, `form` (`S`\|`F`), `primary`, `events{harm,gain,cost_up}`, `n`, `repeats`, `d_permille` (rounded to 50), `m_permille`, `cpp_ratio_permille`, `families` (count), `spent_musd` | rollback on harm; scoring |
| `audit_kernel` | kernel | `watch_seq`, `form` (`P`\|`E`), `veto`, `detail` | recorded; never scored |
| `trust` | Root | `T_permille`, `T_lcb_permille`, `anchor{T, mass}`, `agent{T, mass}`, `n_scored`, `cold`, `audit_permille`, `share_max_permille`, `boost_until_seq`, `model`, `reason` | sets the audit rate, share and level |
| `trust_epoch` | Root | `epoch`, `key_sha256` at start; `key` at the reveal | verifiable draws |
| `family_band` | Root | `family`, `incumbent_permille`, `p0{harm,gain,cost_up}`, `weight`, `spent_musd` | F-form weight |

Changed kinds:
- `promotion` gains `watch_seq`; the Root refuses `lkg:true` without a qualifying close and audit.
- `sealed_g4` gains `purpose` (`gate`\|`audit`) and `watch_seq`.
- `sealed_calibrate` gains `ratchet_fa_permille` and `cost_up_fa_permille`.
- `ladder` gains `cause:"trust"`, and its `criteria` lists the trust terms.
- `task_admit` gains `sealed_by:"root"` at every level for generated families.
- The gate record v4 omits `G4` and adds `accept:"self"` and `stages.I0`/`I6`.
- `refs/notes/predict` gains each forecast clause with its outcome and Brier terms, visible after the close.

`ledger --verify` gains three checks: every `audit_draw` against the revealed key, every `lkg` move against its `watch_close` and audit, and T recomputed from records.

## Work breakdown

All tests are $0: mock model, scratch homes under `target/test-homes/<wp>/`, fake clock, `init-fakekernel`/`init-fakeroot`, and only the test's own processes signalled. Test ids use the prefix `TF-`.

| WP | Scope | Crates and files | Agent-h | $0 tests |
|---|---|---|---|---|
| **W0a** | MEM-RUNAWAY: Root per-instance memory watchdog (`proc_pid_rusage` phys_footprint, `killpg` of a held leader only), charter `quotas.instance_mem_mib` (4096); kernel tool-runner cap; bash child watchdog | root `src/zone.rs`, `src/sys.rs`, `src/charter.rs`; `kernel-supervise/src/instance.rs`; seed `src/tools/bash.rs` | 10–14 | TF-01: a fixture child allocating past a 64 MiB test cap is killed by held PID, decoys untouched, no swap growth |
| **W0b** | STRANDED-PROPOSAL and the new proposal states | `kernel-evo/src/proposal.rs`; `kernel-supervise/src/promote/pipeline.rs`, `recovery.rs` | 3–4 | TF-02: restart during watch and audit resumes from Root records; rollback without verdict → `screened` |
| **W0c** | G3-STAGE-TIMEOUT: stage timeout derived from the build timeout | `kernel-gate/src/g3.rs`; `kernel-supervise/src/promote/config.rs` | 2–3 | TF-03: a 280 s cold-slot fake build passes on the fake clock |
| **W0d** | probe-root leak | `kernel-supervise/src/proberoot.rs`, `probe.rs` | 2–3 | TF-04: read-only pool clone deleted; delete errors logged |
| **W1** | forecast grammar and validator | `kernel-gate/src/evo/{predicts.rs, grammar.rs, validate.rs}`; seed `skills/evolve/SKILL.md` | 4–6 | TF-05: golden parse table; each missing or duplicate event refused; p range and clamp |
| **W2** | self-accept pipeline: scope table `needs_g4 = false`, I6 rollback probe, `watch_open` before handoff, LKG only on Root approval, depth and collateral, gate record v4 | `kernel-gate/src/{scope.rs, g2.rs, record.rs}`; `kernel-supervise/src/{machine.rs, supervisor.rs, lineage.rs}`, `promote/{pipeline.rs, g4run.rs}`; `init-proto/src/root_link.rs` | 16–22 | TF-06: machine transitions on `init-fakeroot`; TF-07: mock e2e, a T2 change self-accepts and closes clean → LKG; TF-08: an I6-incompatible change is refused |
| **W3** | Root sentinel: turn tap, rings, exact exceedance and binomial tests, H1–H7, S1, S2, `watch.rollback` with the §3.7.3 fallback, workspace snapshot at open | root `src/sentinel.rs` (new), `src/link.rs`, `src/egress/money.rs`, `src/lineage/kinds.rs`, `src/tuple.rs`, `src/access.rs` | 12–16 | TF-09: tail tables equal the Python vectors; TF-10: synthetic turn streams, null false rollback ≤ 1.5%, the power table; TF-11: a kernel ignoring rollback → whole-system rollback at 120 s |
| **W4** | draw and forms: epoch keys, commit-reveal, forced rules, S form on parent tuples, Root-initiated F form, due and overdue, `audit_usd_day`, event mapping | root `src/draw.rs` (new), `src/sealed/{slice.rs, audit.rs, run.rs, mod.rs}`, `src/egress/money.rs`; `kernel-supervise/src/promote/g4v2.rs` (becomes the kernel-form driver); `kernel-gate/src/g4v2.rs` | 14–18 | TF-12: draw determinism and verify after reveal; TF-13: forced rules; TF-14: overdue → rollback, not demotion; TF-15: S and F forms on the mock |
| **W5** | trust ledger in R7: groups, cold start, boost, resets, audit rate, share, level criteria, `cause:"trust"`; RT-GAP-1 closed; `[trust]` charter section | root `src/ledger.rs`, `src/charter.rs`, `src/sealed/stats.rs`, `src/lineage/verify.rs`; `tests/redteam.rs` | 12–16 | TF-16: test ledgers for each level; TF-17: no-op flood and forecasts at p0 never raise T above 0.05; TF-18: proof-sprint replay gives T = −554‰; TF-19: RT-GAP-1 (20 self-reported promotions plus 1 audit) does not unlock L1 |
| **W6** | evaluator role: purpose `evaluate` (proxy refuses it on run, probe, screen and sealed lanes), `initctl eval submit\|list`, Root sealing and band A/A at all levels, divergence suspension, declarative monitors with auto-retire | `kernel-proxy/src/{scope.rs, routes.rs}`; `kernel-evo/src/eval.rs` (new); `initctl/src/cli.rs`; `kernel-gate/src/tasks.rs`; root `src/sealed/admit.rs`; seed `skills/evaluate/SKILL.md` | 12–16 | TF-20: evaluator lane cannot read proposals; TF-21: enumerated family never scored; TF-22: divergence suspends; TF-23: a veto-spamming monitor retires |
| **W7** | G4 to audit tool: `accept_v2` kept for `Measure: g4` and K3; milestone cadence of 12 LKG moves | `kernel-gate/src/accept.rs`; root `src/sealed/mod.rs` | 4–6 | TF-24: milestone queued at the 12th LKG move, not before 7 days |
| **W8** | policy simulator (`init-kernel gate simulate --rule trust`): agent types of §5 plus salami, strategic and colluding-evaluator agents; shares vectors with W3 and W5 | `kernel-gate/src/stats/{sim.rs, trust_sim.rs}` | 8–10 | TF-25: the acceptance table below |
| **W9** | awareness: one tail line `trust 310‰ · audit 49% · window 7/20`; forecast guidance in skills | seed `src/boot/awareness.rs`, `skills/evolve/SKILL.md` | 3–4 | T-AW: line per state; head `prefix_hash` unchanged |
| **W10** | docs fold: evolution.md, evolution-v2.md, gate.md, root.md, sealed-eval.md, m7-design.md, ADR-006 and ADR-007 amendment notes, CONVENTIONS §4 and §6, awareness.md, testing.md | `docs/**` | 6–8 | doc-drift checks |
| **W11** | benchmark harness, design B (next section) | `tests/bench/swebench/` (new) | 8–12 | dry run on the mock: oracle check (gold passes, empty fails) on 2 tasks |
| **Total** | | | **116–158** | |

**Critical path and lanes.** W0a and W8 start first and in parallel: W0a blocks enabling, and W8 validates the defaults before code freezes. Then W1 → W2 → W5 → integration (about 40–55 agent-hours) is the critical path. W3, W4, W6 and W0b–W0d run beside it in their own crates. W10 and W11 run last. With five lanes, wall-clock is about the critical path.

**Definition of done** (before the default becomes `"self"`):
1. All TF tests green, and `ci/local.sh` green three times.
2. W8 acceptance table on the shipped defaults:
   - null false rollback ≤ 1.5% per window;
   - honest forecasts maximise expected T (within 0.02);
   - no-op flood and p0 forecasts keep T ≤ 0.05;
   - the overconfident agent and the hider stay at a = 1;
   - a colluding evaluator cannot push T above 0.5 without anchor evidence;
   - a salami agent (each change −2 points) is rolled back at a milestone in ≥ 70% of runs by the 24th change.
3. W0a verified live on the owner's machine with held processes only (no sweep).
4. The owner approves this ADR and its Root growth.

## Benchmark plan

The benchmark is **external and report-only**. It is a human-authored public set, probably contaminated, so it never feeds T, a gate, lessons or the live install's lineage. It answers two questions at the honest resolution of a small sample:
- Q1: does a trust-first lineage stay at least as good on outside tasks as its genesis?
- Q2: what does INIT cost per solved task against a strong minimal harness?

It reuses design B of reports/Benchmark harness có dữ liệu công khai.md unchanged: 12 SWE-bench Verified tasks (10 mid-difficulty, 35–65% solved across public entries, in django, sympy, pytest and sphinx, plus 2 anchors solved by ≥ 90%), deepseek-flash, native grader, no Docker.

| Phase | Arms | Runs | Real $ (off-peak) | When |
|---|---|---|---|---|
| **B0** | INIT genesis tuple (the tuple frozen when `"self"` is enabled) vs mini-swe-agent | 12 × 3 × 2 = 72 | $0.7–3.6, hard cap $4 | before or at enabling |
| **B1** | INIT evolved (the then-current `confirmed` tuple) vs INIT genesis, paired and interleaved; mini again only if the served snapshot changed | 72 | $0.7–3.6, hard cap $4 | at the first milestone `confirmed`, or 20 LKG moves, or 30 days |
| C (optional) | Claude Code, 1 run per task | 12 | ≤ $1 | only if B0 spent ≤ $2.5 |

**Fixed conditions** (from the report):
- one checkout per task at the base commit, remotes and future history removed; one uv venv per repo version, outside the repo; reset and clean between runs;
- the same Seatbelt profile for every arm (mini wrapped in `sandbox-exec`), network only to `api.deepseek.com`;
- 250 steps, 60 s per command, 30 minutes and $0.30 per run;
- the $4 cap per phase is enforced outside the agent: INIT's Root for the INIT arms, `MSWEA_GLOBAL_COST_LIMIT` for mini;
- the per-process memory watchdog (W0a, or m7watch.py until it lands);
- the same effort on every arm, the served snapshot recorded from each API response, arm order randomised per (task, run), off-peak only, stop if the alias changes.

**INIT-specific isolation.** Every INIT arm runs in a disposable test home, never the live install, so the live miner never sees SWE-bench content. The task checkout is the run's workspace. The test patches and the grader live outside it, in the test home's `sandbox.read_deny`.

**Procedure** (the report's steps 0–8):
1. The owner authorises each phase's $4 cap and creates the sb-cli key.
2. A pre-registration file is committed first: tasks, K = 3, arms, metrics, stop rules.
3. A $0 oracle check per task: the gold patch passes, an empty patch fails.
4. A 2-task probe rescales K: drop to K = 2 if the projection exceeds $3.5.
5. The main runs.
6. Native grading, cross-checked once by submitting INIT's run-1 patches to sb-cli.
7. Disagreeing tasks are audited by hand: trajectory and patch.

**Pre-registered claims.** Reported:
- pass@1 with a task-bootstrap 95% CI, and pass^3;
- tokens per task and $ per solved task (the primary claims: continuous and least noisy);
- a sign-flip permutation p on per-task differences.

With 12 × 3 the test detects a +32-point difference only 77% of the time, so any |Δ| below about 30 points is reported as "not distinguishable", never as a win or a regression. B1's claim is "no detectable regression at this power, at X% of genesis's tokens per task". Results are exported in SWE-bench's `results.json` and `per_instance_details.json` formats.

**Internal success metrics.** These come from lineage, at $0 beyond operation. They are the measures this ADR is judged by against the G4 regime:
- verification $ per LKG change (target ≤ $0.10 in steady state, against $0.37–0.67 per G4 attempt);
- hours from promote to LKG;
- false-rollback rate;
- harmful-kept rate, as later found by milestones and scheduled audits.

## Alternatives considered

| Alternative | Why not chosen |
|---|---|
| Keep G4 per change (status quo) | It blocked the only real change in the sprint at $2.67. It certifies only gains of ≥ 17 points, and the human-authored set caps growth. Kept as the opt-in `Measure: g4`. |
| Accept first with live monitoring only, no audits | Silent degradation and Goodhart go unchecked; live signals cannot see correctness on rare task kinds. |
| A fixed audit rate (for example 20%) | It wastes money on a calibrated agent, under-audits a miscalibrated one, and gives no reason to forecast honestly. |
| Trust from kernel-reported outcomes (pool, friction, promotion counts) | RT-GAP-1's class: the zone grading itself. |
| Agent-chosen clauses for calibration | Trivially true or false clauses would buy trust. The event set is therefore fixed. |
| Log score instead of Brier | Unbounded: one confident miss erases all trust, and it is fragile at small n. Reported as a diagnostic only. |
| Plain Brier (no null reference) | It rewards no-op changes, whose outcomes are easiest to forecast. |
| An LLM judge or reviewer to accept | No LLM judge anywhere (ADR-006 decision 8): a reviewer can only block. |
| A Bayesian harm-rate posterior as the only trust | It ignores what the agent knows about its own changes, and the S form's 8.6% false-harm rate makes it slow to tighten. Kept in spirit as the surprise boost. |
| Sentinel in the kernel, not the Root | From L3 the kernel is agent code, and it could disable its own rollback. The Root must own the rollback decision (the owner's rule that only the Root stays fixed). |

## Consequences

**Cost per self-accepted change** (off-peak, $0.0035 per run as measured in M7; invariants ≈ $0.015, kernel forms ≈ $0.04, F ≈ $0.07, S ≈ $0.37):

| Phase | a | s | ≈ $ per change |
|---|---|---|---|
| cold start, building the anchor (about the first 6) | 1.0 | 1.0 | 0.50 |
| cold start after anchor mass (to 20 scored) | 1.0 | 0.25 | 0.22 |
| T_lcb = 0.45 (the skilled agent at n = 60) | 0.30 | 0.25 | 0.08 |
| floor | 0.10 | 0.25 | 0.035 |
| plus milestones | | | ≈ $0.43 per check, at most one per 7 days |

So a cold start of 20 scored changes costs about $6 real, and steady state falls about 5–20× below the G4 regime. Real operation needs the owner's money authorization and a cap (open question 2). This ADR and W0–W10 are $0.

**The Root grows.** Estimated: the sentinel ≈ 260 lines, draw ≈ 100, trust ≈ 180 (replacing ≈ 60 lines of ladder criteria), sealed changes (audit purpose, parent tuples, Root-initiated F form, band A/A) ≈ 150, link and kinds ≈ 60: about **+690 / −60** raw lines. They are Root code for the same reason as ADR-009's: each decides a rollback or the trust that sets verification. root.md §4.8 gets sub-budget lines `sentinel` and `trust`, measured at release (measured + 5%). The ≈ 7,500 total is a provisional owner decision, so the owner may need to raise it.

**Other consequences:**
- "Sticks" changes meaning: a change sticks when its window closes clean and its audit, if drawn, finds no harm. The lineage claim with an error rate is the milestone's, as in m7 §3.2.13.
- The evolution engine's go/no-go rule (evolution-v2 §3.5.3) stops spending G4 looks. The screen still ranks variants, and `est` becomes a prior for the agent's `gain` forecast.
- ADR-006 decisions 3, 4, 5 and 7, and ADR-007 decision 3, get amendment notes pointing here (W10). ADR-007's decisions 1, 2, 4 and 6 are unchanged: the Root, the zones, the honest stick and "the agent can never remove the Root".
- CONVENTIONS §4 registers ADR-010 and §6 records it as PENDING OWNER (W10).
- No cargo build was run for this ADR; it is design only.

## Open questions (owner)

1. **Root growth.** Approve about +630 net Root lines for the sentinel, the draw and the trust ledger? The alternative, kernel-side, repeats RT-GAP-1.
2. **Money.** Cap for real operation: about $6 real for the cold start of 20 scored changes, then about $0.04–0.08 per change, with `trust.audit_usd_day` at $1.00. Plus the benchmark's B0 and B1 at $4 each, authorised separately.
3. **Audit floor.** 10% (proposed; about 9 unaudited harmful changes expected from a strategic agent before the first catch) or 5% (cheaper, about 19)?
4. **Calibrated but not sharp.** Should a demonstrated low harm rate also lower audits for an agent that cannot tell good changes from bad (the "modest" row)? Proposed: no in v1, revisit with real data.
5. **`dep` scope.** Self-accept plus a forced audit (proposed), or keep today's synchronous G4 before the handoff for new third-party code?
6. **Owner feedback.** Only negative signals (`/reject`, `/undo`, interrupts, steers) count, so the agent gains nothing by soliciting approval. Should `/bless` count for anything?
7. **The anchor's authorship.** S and the reserve stay maintainer-generated, frozen per Root release, as the one human-authored element. Accept that, with Root-fetched fresh public benchmarks (R-ROOT-1) as the long-term replacement?
8. **L3.** Keep the mutation-suite pass beside trust for kernel evolution (proposed), or let trust alone unlock it?
