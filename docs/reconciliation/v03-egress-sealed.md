# v0.3 reconciliation: root/egress.md and root/sealed-eval.md

*2026-10-20. Applies reconciliation/v03-decisions.md (binding) and answers the cross-doc items other v0.3 docs addressed to these two files. Only root/egress.md and root/sealed-eval.md were edited. Both moved to draft v0.2.*

## Decisions applied

| # | Item | Decision | Change made |
|---|---|---|---|
| D2 | No hard-coding on self-written tasks | Admitted as families of ≥ 2 instances. At admission the sealer (kernel at L1–L2, Root at L3+) seals a random half and records a commitment hash; the agent sees only the visible half. `capability_gain` needs a significant gain (paired bootstrap L > 0, α = 0.05) on the sealed half, plus non-inferiority on S through the audit; `audit_required` stays | sealed-eval.md §3.6 rewritten: family format (enumerated or generated), rule 3, the "what a family can prove" paragraph; L3+ `family_eval` on Root sealed lanes; record `family_eval`; config `family_min_instances` |
| D3 | Exclusion distance and anchor | K = 5 promotions, counted as `promotion{lkg:true}` records on P's ancestry after the author sha. Pre-registration: the `task_admit` Root lineage seq < the seq of the promotion record of the LKG the candidate is judged against | sealed-eval.md §3.6 rules 4–5; `admit_k_promotions = 5` (a floor) |
| D5 | Sealed runs stay sealed | Sealed audits, gate-of-gates and L3+ family halves run as Root-launched sealed runs on sealed egress lanes. No kernel proxy, no trace store, run roots deleted by exact path. The GoG runs the candidate's `gate judge` in a non-persistent sealed instance; only its `--out` record is read | sealed-eval.md §3.2 (new); egress.md §3.8 sealed lanes, `sealed_lane(…, pool)` |
| D6 | Ceilings and demotion | Class/policy violations and level-gated refusals demote. A money refusal demotes only on a **guard bypass**: no `x-egress-guard` header, an epoch older than the ring or stale by > 30 s, or a request that already did not fit at the guard's epoch. Ordinary hits, peak-price gaps included, do not demote. Read-only class volume/rate hits do not demote. A lowered ceiling refuses requests; it never stops anything | egress.md §3.3 (no refusal to start), §3.6 table and guard-bypass rule, `epoch` in the money block |
| D10 | crates.io access | Built-in `crates` class: `GET`/`HEAD` to `index.crates.io:443` and `static.crates.io:443`, L0, no approval, off in safe mode; used by `initctl vendor` through `/x/crates/<host>/<path>` | egress.md §3.5 built-in table and limits; cross-doc to kernel/initctl.md (local-registry build) |
| CONVENTIONS §1 | Live-system safety | No pattern kills anywhere. `cancel_all` signals only the sealed module's own instance ids through the broker's D1 sweep; quotas refuse spawns, never signal; all new tests use mock models and scratch homes | egress.md §3.9, §8; sealed-eval.md §3.2, §4.2, §8 |

## Cross-doc items answered

| From | Item | Decision | Change made |
|---|---|---|---|
| root/root.md X5 | socket, key path, calls, `ceiling_hit`, `min_level`, model allowlist, safe mode, threading, ≤ 330 lines | `ROOT_HOME/run/egress.sock` (kernel profile only); `ROOT_HOME/keys/providers/<id>` and `keys/actions/<class>`; `panic`, `resume`, `reload`, `ceilings`, `set_level`, `sealed_lane`, `validate_actions`; `ceiling_hit{demotes}` per D6; `min_level` per class and per model; safe mode = `providers` with L0 models only; std threads; a 330-line budget table | egress.md §3.1, §3.2, §3.5, §3.6, §4.3, §6 |
| root/root.md X6 | egress dependencies | `ureq` 3.4.2 (rustls/ring, no gzip) and `httparse` 1.10.1; tokio/hyper rejected (runtime, size) | egress.md §6 dependency table |
| root/root.md §3.14 vs this doc | who enforces compute, disk and parallel-body quotas | the Root broker and watcher (root/root.md §3.14) own enforcement and the `quotas.*` keys; egress.md keeps no duplicate keys and records the platform notes | egress.md §3.9 rewritten; former egress quota keys removed |
| root/root.md §3.12.1 | charter money format | `money.day_usd`, `money.total_usd` as decimal strings; the egress parses them to integer micro-USD | egress.md §3.3 |
| root/root.md X7 | `audit`/`mutation` fields, `gog`, `cancel_all`, `validate`, post-upgrade trigger, broker quota, `[sealed]` section, ≤ 175 lines, audited-good pointer | records in sealed-eval.md §4.1; API §4.2; upgrade audit after probation, manual in Phase 0 (`init-root audit run --reason upgrade`); sealed instances count toward `max_instances` and the zone gets `max_instances − sealed.parallel`; `[sealed]` §4.3; 175-line table; the pointer moves only on a complete, non-regressing audit | sealed-eval.md §3.2, §3.5, §4 |
| kernel/proxy.md Q-RT0 | egress socket path delivery | from `hello.paths.egress_sock` (root/root.md §4.1.3), not an environment variable | egress.md §3.1 |
| kernel/proxy.md Q-EG2 | Root state for the kernel | (a) adopted: `models` list (provider, model, `min_level`, peak prices, `routable`) in `GET /_root/ceilings`; (b) declined: per-response cost header (cost is known only at stream end); `x-egress-reserved-musd` and `x-egress-epoch` provided, and the kernel re-reads ceilings after each request | egress.md §4.1, §4.2 |
| kernel/proxy.md Q-EG3 | retries and refusals | the Root retries only connect-phase failures (≤ 2), never on a status code; 401/403 bodies withheld; refusal mapping confirmed | egress.md §3.2, §3.4, §3.6 |
| kernel/proxy.md Q-EG4 | what counts as an attempt | the D6 guard-bypass rule with epochs and a 30 s staleness tolerance | egress.md §3.6 |
| kernel/proxy.md Q-EG5 | prices | the Root publishes its peak table read-only; the kernel keeps its window prices for measurement and warns on missing models | egress.md §4.2 |
| kernel/proxy.md Q-SE1 | sealed runs never touch the live proxy | adopted (D5) | sealed-eval.md §3.2 |
| kernel/gate.md Q-SE1 | one form for rules 3 and 4 | Root lineage seq anchor; K = 5 promotions (D3) | sealed-eval.md §3.6 |
| kernel/gate.md Q-SE2 | rule 1–2 details | (a) every solution run must score 1000; (b) no-op = initial workspace unchanged, "tests-only" dropped; (c) no-op ≤ 500 permille | sealed-eval.md §3.6 rules 1–2 |
| kernel/gate.md Q-SE3 | `audit_required` | definition confirmed; it brings the next audit forward (≤ 48 h, coalesced); the kernel suspends post-audited-good agent tasks on `audit{regression:true}` | sealed-eval.md §3.5, §3.6 |
| kernel/gate.md Q-SE4 | gate contract | confirmed: M, C1/C2 and the agreement check call `gate judge`; per-task data from the record's run data | sealed-eval.md §3.3 |
| kernel/gate.md item 5 | holdout staleness | Phase 1, L0–L2 only: retired public S tasks may rotate into the gate holdout through a control-link request; no rotation at L3+ | sealed-eval.md §3.6 last paragraph; cross-doc to root/root.md |
| evolution.md → egress (a) | crates class at L0 | D10 | egress.md §3.5 |
| evolution.md → egress (b) | per-level model allowlist | `models` table with `min_level` in the `providers` class; L0 = one owner model | egress.md §3.5 |
| evolution.md → sealed (a), (b) | K and anchor | D3 | sealed-eval.md §3.6 |
| evolution.md → sealed (c) | suspension after a demotion | the kernel suspends agent tasks registered after the audited-good point, on the Root's regression record | sealed-eval.md §3.5 |
| evolution.md → sealed (d) | G4 canary of 1000 as a demotion trigger | recommended; root/root.md owns the trigger list | sealed-eval.md §9 cross-doc |
| evolution.md → sealed (e) | bisection after a regression | Phase 1 only, capped by `bisect_usd` (0 = off), for diagnosis; the rollback target stays audited-good | sealed-eval.md §3.5 |
| evolution.md → sealed (f) | mandatory `solution.tar` | yes, for every family instance | sealed-eval.md §3.6 |
| evolution.md → sealed (g) | gain only on agent tasks while flat on S | closed by D2: the gain must be on the sealed half, plus non-inferiority on S through the audit | sealed-eval.md §3.6 |

## Other changes

| Item | Change |
|---|---|
| M2 and M6 framing | All mutants are now mutant *systems* (body commits on a control base) judged through `gate judge`, matching kernel/gate.md §4.1a's design check. Generators run at Root release time; mutants ship as bundles |
| Lineage volume | Model calls and read-only class calls are ledger records only; lineage gets `ceiling_hit` and a new `egress_action` kind for classes with external effects |
| Provider-executed tools | refused (`class.provider_tool`), since they reach the network outside every class |
| Helper verbs | `git-push` and `email` stay specified but are rejected by `validate_actions` in the Phase 0 Root (`SUPPORTED_ACTION_KINDS`) |
| UNVERIFIED count | egress.md 5, sealed-eval.md 2 |

## Still open (raised in the docs' cross-doc lists)

- root/root.md: kinds `egress_action` and `family_eval`; `task_admit` fields; tail access for sealed run roots; `max_instances − sealed.parallel`; control-link requests `sealed.family_eval` and holdout rotation; a canary as a demotion trigger.
- kernel/proxy.md: send `x-egress-guard: epoch=<E>`; consume `models`.
- kernel/initctl.md: `vendor` through the `crates` class with a cargo local-registry.
- body/boot.md: a body runs a task without `INIT_KERNEL_SOCK`.
- testing.md: a mock egress and the SE/EG/R3 scenarios.
- CONVENTIONS.md §3, §5: keys row and TLS dependencies move to `init-root`.
