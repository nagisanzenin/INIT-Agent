# Foundations

*v1 · 2026-10-28 · the formal model INIT's guarantees are stated against.* This is the reference for what INIT promises,
what it measures, and what it does not promise. README, the site, the ADRs and the release notes state guarantees in
plain words. Each of those statements must reduce to one in this file, or it should not be made. The statements below are
design guarantees with the checks named next to them. None is model-checked or proven yet. That work is planned, and it
targets the trusted computing base in §9. The research behind this file is in `reports/Nền tảng toán học cho INIT.md`
(private) and the papers in §11.

## 1. The idea

A harness is everything around a model: its prompts, its tools, its loop and how it keeps context. Hand-written
harnesses are designed for the models their authors had, so a stronger model inherits a ceiling (Sutton's bitter
lesson). INIT lets the model rewrite its own harness, compiled core included, while it works. Two constraints make
that safe enough to try:
- One part, the **Root**, is fixed. It is outside everything the agent can write, and it decides only from its own
  observations.
- A change is not required to prove it is better, because nobody can write the score for a capability nobody foresaw.
  It is required to be **undoable, attributed and harm-bounded**.

## 2. What kind of system this is

INIT is a **reference monitor with rollback**. Five results fix the shape of what it can and cannot promise.

- **Monitors enforce safety properties** (Schneider 2000; Hamlen, Morrisett, Schneider 2006). A monitor sees only a
  prefix of the execution and can only refuse or stop, so it can enforce only "nothing bad happens" properties. "Every
  admitted change helps" is not a safety property, and by Rice's theorem it is undecidable for arbitrary code. INIT
  therefore does not promise it (§8, N1).
- **Rollback makes the Root stronger than a truncation monitor, but only on reversible state.** This is close to
  Ligatti, Bauer and Walker's edit automata. Money spent, bytes sent and actions taken outside INIT cannot be undone, so
  there the Root can only refuse beforehand and cap.
- **Supervisory control** (Ramadge and Wonham 1987). The model's choices are uncontrollable events. The Root controls
  the admission, the sandbox, egress, rollback and stop. A requirement can be enforced without restricting the model
  only if it is controllable. This is why, in full access, the Root protects INIT and not your files (ADR-012). There,
  writes to `$HOME` are neither controllable nor observable by the Root. Workspace mode makes them controllable.
- **Simplex** (Sha 2001). Rollback to the last known-good version (LKG) is a Simplex architecture: an unverified,
  high-performance component plus a fallback. The fallback here is known-good from a clean watch window, not verified.
  INIT switches *before* the boundary where it can (the disk floor, the snapshot cap, refusing a change whose parent
  cannot be kept). It switches after the fact for harm.
- **Seldonian admission and conservative exploration.** Gated mode's sealed test (G4) is a Seldonian test (Thomas et
  al. 2019): accept only if, with confidence 1−δ, the candidate is not worse than its parent on data the Root holds.
  Free mode is conservative exploration (Wu et al. 2016): deploy, watch live, fall back when a test fires. Its guarantee
  is about exposure, not about confidence before deployment.

INIT avoids the Löbian obstacle that Gödel machines and tiling agents meet: no generation has to prove its successor
trustworthy. Safety is a property of the fixed Root, quantified over every program the agent can write.

## 3. Objects and state

```text
Tuple     τ = (kernel, body, ext_set, t0)          id(τ) = "t" ‖ hex(SHA-256(canonical(τ)))[0..12]
par       : 𝒯 ⇀ 𝒯                                   the derivation tree; root = genesis; a rollback leaves a dead branch
X_rev     = 𝒯 × S_A                                 reversible: the tuple and the agent's state (body/, state/, vault/)
X_irr     = money spent ∪ bytes sent upstream ∪ external actions run ∪ writes outside the workspace in full access
log       the conversation log: append-only, outside rollback; every tuple must be able to read it

Root state ρ = ⟨Λ, B, π, 𝒢, ℓ, χ, μ, latch, phase⟩
  Λ        the lineage: append-only, hash-chained (SHA-256), each record signed (Ed25519) by the Root
  B        the blob store, content-addressed by SHA-256
  π        : {current, lkg, settled, audited, twin, genesis} → 𝒯 ∪ {⊥}
  𝒢        open change groups g = (parent τ_g⁰, snapshot s_g, window, outcome)
  ℓ        = f(Λ restricted to Root evidence, now) ∈ {0..4}      the autonomy level
  χ        the charter, written by the owner only
  μ        ∈ {normal, safe, stopped, panicked, halted};  latch: set by stop and panic, cleared only by resume
  phase    ∈ {idle, requested, prepared, installed, probation, committed}    (an upgrade)

Σ = Σ_c ⊎ Σ_u         controllable events (the Root performs or prevents them) and uncontrollable ones (the model's)
```

## 4. Transitions

| Event | Kind | Guard | Effect |
|---|---|---|---|
| `propose(τ′)` | u | none | none |
| `admit(g, τ′)` | c | the Root's admission checks (ADR-011 §2.1): A0 the candidate's blobs verify in the store; A4 the candidate and its parent each boot on cloned state within 90 s (core changes); A5 the blast-radius, size and disk budgets | snapshot s_g; keep par(τ′) in B; add g to 𝒢; record `change_open`; current := τ′ |
| `observe(e)` | u | none | update the harm statistics |
| `rollback(G′, cause)` | c, forced when a harm signal fires | G′ = the newest group (chronic) or every open group (acute) | current := the parent of the oldest group in G′; optionally restore the agent state from s_g; record it |
| `close(g, o)` | c | the window ended with an allowed outcome; older groups closed | lkg := current; record `change_close` |
| `ladder(±)` | c | criteria on Root records since the level was entered | ℓ := f(Λ\|root, now); record it |
| `upgrade_commit(k)` | c | probation passed | each anchor in {audited, settled} gets kernel k, or ⊥ if unreadable |
| `stop`, `panic` | u (owner) | none | kill every zone process (c); set the latch; `resume` restarts an approved tuple |

## 5. Invariants

Every invariant except INV-7 is a safety property over what the Root observes. INV-7 is a 2-safety hyperproperty
(Clarkson and Schneider): it compares two runs, so no single-trace monitor checks it. It holds by construction and is
tested by self-composition. The status column is 2026-10-28, v0.5.1.

| ID | Statement | Check | Status |
|---|---|---|---|
| INV-1 mediation | every zone process start, every egress request and every change that goes live is a Root event | red-team suite; sandbox self-test (relies on A_sb) | tested |
| INV-2 Root integrity | no zone event changes Root state except through a typed control-link record; the zone cannot write the Root's files | zone-tail and sandbox tests | tested; **gap:** ADR-011's A0 also re-hashes the Root binary, charter and profiles at every admission against a `root_start` record — not built (no `root_start` is written; the record's `"A0":"pass"` covers the blob check only) |
| INV-3 no unwatched code | current is a tuple the Root admitted or installed itself: π.lkg, an open group's τ_g, or a rollback target (settled, audited, genesis) | refusal `root.unwatched_change` (free path) | **gap:** on the free path a promotion is matched to its window by commit sha only, so a promotion can move current and lkg to a tuple built from its own blobs; no model test yet |
| INV-4 recoverable region | π.lkg, π.settled, π.twin and every parent τ_g⁰ are in B; every g has s_g; disk ≥ floor + 2 GiB at admission; snapshot ≤ cap | admission refusals | no model test yet |
| INV-5 LKG provenance | π.lkg moves only at: the genesis install, a gated promotion, a change window closing clean (free path), a sealed audit, an upgrade commit, or an upgrade rollback's restore | — | **gap:** a sealed audit compares against lkg as read when it started, so it can move lkg backwards or onto an open group's tuple; no model test yet |
| INV-6 lineage integrity | Λ is append-only, chained, signed, and `verify(Λ)` holds | `init-root audit verify` | tested; truncation is detected only with `--head` |
| INV-7 evidence integrity | ℓ, π.lkg, quarantine and goal `met` are functions of Root-evidence records only | — | holds by construction; no differential test yet |
| INV-8 ladder discipline | ℓ′ ≤ ℓ + 1; every demotion trigger demotes; ℓ ≤ min(charter max, supported max); no level grants a write to the Root | `ledger --verify` | partly tested |
| INV-9 bounded churn | admissions per owner-day ≤ cap(ℓ); bounded re-admission; repeated rollback leads to quarantine | count tests | partly tested |
| INV-10 anchors after an upgrade | after `upgrade_commit(k)`, every anchor is ⊥ or has kernel k, and resume and demotion install only tuples whose kernel is the release's | tests/upgrade.rs (v0.5.1) | **known gap:** the genesis last resort still holds the install's original kernel (scheduled for v0.5.2) |
| INV-11 latch | latch ⇒ no spawn; only `resume` clears it; this holds across a Root crash and restart | — | no model test yet |
| INV-12 one swap, probation | at most one swap in flight; during probation π.lkg never names the candidate | — | no model test yet |

## 6. Assumptions

- **A_sb.** The OS sandbox (Seatbelt, bubblewrap) enforces the profile the Root composes, always invoked and
  unmodifiable by the zone. It is assumed, not proven. The 2026-10-28 git-config escape was a hole in INIT's profile,
  not in Seatbelt.
- **A_root.** The Root's implementation refines the transition system above. It is the only assumption INIT can
  strengthen by its own work: tests, then model checking, then proofs, all on the trusted computing base (§9).
- **A_res.** The machine keeps the resources the Root reserved: disk, copy-on-write clones, store quota. Where it
  cannot, admission refuses, so safety holds and liveness is lost.
- **A_exch** (statistical statements only). Under no change, the units in a watch window are exchangeable with the
  baseline. Turn-level testing violated this: 9.7–14.8 % false rollbacks under the null, measured on clustered data.
  That is why the sentinel tests units of up to 6 consecutive owner turns.

## 7. The two foundational guarantees

**FG-1 Recoverability.** For every program the agent can write, the Root alone can return the whole system to the last
known-good tuple and the matching agent state. It needs no cooperation from the agent, completes within bounded time,
and keeps the conversation. This holds for code and agent state (X_rev). Irreversible effects (X_irr) are bounded by
ceilings and action classes, not undone. *Assumes A_sb, A_root, A_res.*

**FG-2 Root-only evidence.** Autonomy, the known-good pointers, quarantine and goal completion are functions of records
the Root writes from its own observations. Anything the agent reports can only make the system more cautious. No
sequence of agent actions raises autonomy or moves a known-good pointer, except by producing outcomes the Root itself
observes, on owner turns the agent cannot generate. *Assumes A_root.*

**Separation.** FG-1 and FG-2 are statements about the Root alone, quantified over every program in the zone. So no
generation of self-modification can invalidate them; only a change to the Root can. Every Root change goes through the
maintainer's release protocol (docs/release-protocol.md), never through the agent.

## 8. What INIT does not promise

| # | Non-claim | Why |
|---|---|---|
| N1 | that an admitted change helps, or that INIT improves | a liveness and statistical question; undecidable for arbitrary code; INIT measures and reports it (the twin, goals) |
| N2 | to detect harm it cannot observe | e.g. subtly wrong code the owner accepts, with no correction, crash or cost signal |
| N3 | immunity to Goodhart pressure | the ladder rewards clean records; "reward-free" describes admission only; levels widen speed and blast radius only under reversibility, never the Root's powers |
| N4 | to undo irreversible effects | on X_irr the Root caps and refuses; it cannot roll back money spent or data sent |
| N5 | that the OS sandbox is correct | A_sb is assumed |
| N6 | that gates run by the kernel (G0–G3) are guarantees | from L1 the agent can change the kernel; they become quality filters; only Root-run checks are guarantees |
| N7 | that safety improves with the model | the Root is the maintainer's code; the bitter lesson is applied to capability and deliberately not to safety |

## 9. Statistical statements

These are statements about measured rates, not guarantees.
- **P-3 watch.** Under A_exch, today's sentinel bounds the false-rollback probability per window from its six chronic
  tests only by a union bound over its looks and tests: 3 looks × (4 exceedance tests × 0.005 + 2 binomial tests ×
  0.002) = 7.2 %. The acute signals and H5a are outside this bound. The bound is loose because the looks are nested
  and correlated. The rate for the current code must be measured.
- **P-4 bounded churn.** Each admission or re-admission spends a daily budget that refills only at the day boundary,
  so no day has an infinite rollback and re-admit cycle. This is boundedness, not convergence.
- **H-grow** (a hypothesis, falsifiable). Let Δ(m, t) be the evolved harness's advantage over the frozen twin on model
  m after time t. "The harness grows with its model" means that for a stronger model m₂ ≻ m₁, the long-run advantage
  on m₂ is at least that on m₁, and is non-negative within noise. It is tested by comparing twin reports across model
  generations.

## 10. The trusted computing base

FG-1 and FG-2 depend only on these parts of the Root:
- the spawn broker and the profile builder (INV-1, INV-2);
- the store, the pointers and tuple installation (INV-3 to INV-5, INV-10, INV-12);
- snapshots and restore (FG-1);
- the lineage's append, sign and verify (INV-6);
- the ledger fold and the permission table (INV-7, INV-8);
- kill and the panic latch (INV-11);
- the control-link parser.

The harm statistics, the twin, goals and the sealed evaluation affect only P-3, P-4 and measurements, provided their
outputs reach the TCB only as rollback requests, refusals or records, that is, only in the more cautious direction. All
formal effort (property tests, model checking, Kani proofs) goes on the TCB, and the release protocol reports its size
separately from the whole Root (20,334 lines without tests in v0.5.0).

## 11. How this file is used

- A guarantee in README, the site, an ADR or release notes cites an FG, an INV or a P. Otherwise it is rewritten or
  dropped.
- Every Root refusal code protects a named invariant. A refusal that protects none is surplus governance.
- Every number here comes from a constant in the code or a measurement file, as release-protocol §4.4 requires.
- A change to this file is a change to INIT's promises. It ships in a release, with its CHANGELOG line, and the
  release's adversarial review reads it.

**References.** Anderson 1972 (reference monitor). Schneider 2000, *Enforceable Security Policies*. Hamlen, Morrisett,
Schneider 2006, *Computability Classes for Enforcement Mechanisms*. Ligatti, Bauer, Walker 2005, *Edit Automata*.
Clarkson, Schneider 2010, *Hyperproperties*. Ramadge, Wonham 1987, *Supervisory Control of a Class of Discrete Event
Processes*. Sha 2001, *Using Simplicity to Control Complexity*. Aubin 1991, *Viability Theory*. Thomas et al. 2019,
*Preventing undesirable behavior of intelligent machines* (Science). Wu, Shariff, Lattimore, Szepesvári 2016,
*Conservative Bandits*. Schmidhuber 2003, *Gödel Machines*. Zelikman et al. 2023, *Self-Taught Optimizer (STOP)*.
Zhang et al. 2025, *Darwin Gödel Machine*. Skalse et al. 2022, *Defining and Characterizing Reward Hacking*.
