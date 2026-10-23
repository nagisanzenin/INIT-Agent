# Reconciliation v0.3: kernel-gate

```
Status: v0.3 R1 · 2026-10-20 · Area: kernel
Docs covered: kernel/gate.md, kernel/proxy.md, kernel/selfmap.md, kernel/config.md, adr/006-evolution-gate-policy.md (D4 only)
Binding input: docs/reconciliation/v03-decisions.md (D2, D3, D4, D5, D6, D12; D13 for the money numbers); CONVENTIONS.md §1 (live-system safety)
```

Sources:
- the main-session decisions in v03-decisions.md;
- every "Open questions → cross-doc" item that the other v0.3 docs (root/*.md, evolution.md, awareness.md, testing.md, roadmap.md, kernel/supervise.md, kernel/initctl.md, kernel/sandbox.md, ext/*.md) address to these four files.

Totals: **36 items**:
- 31 accepted; 7 of them (EV-b…e, TS-2, SE-1, EG-1) were already applied and only re-checked;
- 1 accepted with a change (EG-2);
- 4 need no change here: EXT-E9 and EXT-E10 were applied in R2, SB-4 is settled by D9 in root.md, and TS-5 already exists.

**Live-system safety (CONVENTIONS §1).** Every test added in this round that signals or kills processes (proxy P-23, gate G-50, the test-only gate features MS-G1…G3) is marked to run under `init-fakeroot` or in a disposable environment. No test kills by pattern.

## Ledger

| id | from doc § | question | decision | change made (doc §) | follow-up that ANOTHER doc must make |
|---|---|---|---|---|---|
| D2 | v03-decisions D2 | no hard-coding on self-written tasks | ACCEPT | gate.md §3.7.1 (task families: 2–8 instances, `family.json`), §3.7.2 (stratum uses only visible instances; capability set A uses only sealed ones), §3.7.5 (sealing ⌈n/2⌉ by the kernel at L1–L2 with a `sealed_commitment` in `task_admit`, by the Root at L3+; `vault/gate/sealed/`; the admission result names only visible ids), §3.9.1 (gain on the sealed half, family-clustered bootstrap; S non-inferiority through `audit_required`), `repro_fixed` on agent tasks counts only sealed instances; selfmap.md §3.12 excludes sealed halves; G-40, G-42 | root/sealed-eval.md: adopt the family sizes and the L3+ sealed-run request and answer (gate.md Q-SE2); evolution.md §3.15.4: families |
| D3 | v03-decisions D3 | K = 5 promotions; pre-registration anchored to the Root lineage seq | ACCEPT | gate.md §3.7.5 eligibility: registration `seq` < `seq` of the LKG's R4 `promotion` record; promotion distance d = 1 + the ancestor `promotion{lkg:true}` records after registration, eligible iff d > 5; `main_anchor` and commit generations dropped; G-41 | root/sealed-eval.md: change `admit_k_generations` to 5 promotions |
| D4 | v03-decisions D4 | no owner step anywhere; `needs_owner` removed | ACCEPT | gate.md §1, §3.7.1 (`/accept` is an optional signal), §3.12 (owner signals: `/bless` recorded, no state change; `/reject` rolls a provisional version back; `/approve` and `/deny` gone; state machine without the bless transition), §4.1 owner commands, §7, G-44; one escalated re-run, then `inconclusive_final` (already in place) | kernel/supervise.md: drop `/approve`, `/deny`, `/clear-safe` |
| D4-ADR | v03-decisions D4 | adr/006 owner gating | ACCEPT | adr/006: status note; Decision 4 (inconclusive → one escalated re-run, then `inconclusive_final`; owner signals optional); Decision 5 (`needs_owner` cost branch → `costlier_than_budget`; `repro_fixed` without owner acceptance); Decision 9 (no owner per-campaign cap; the L2 fleet unlocks through the ladder). Nothing else in the ADR changed | — |
| D5 | v03-decisions D5 | sealed runs stay sealed | ACCEPT | proxy.md §2 (no sealed traffic through the proxy; `judge` mode keeps observations in memory only, no trace store); gate.md §3.7.1a (S is run only by the Root's isolated evaluator), §3.7.5 (at L3+ sealed family halves are run by the Root), §4.1a (`gate judge` launched by the Root in its isolated environment, on its own egress lane, nothing persists); P-25, G-50 | root/sealed-eval.md: launch `judge` as specified (gate.md Q-SE4) |
| D6 | v03-decisions D6 | clamp, never refuse; what demotes | ACCEPT | proxy.md §3.8: effective soft day = min(`day_usd`, ceiling − `request_max_usd`, `soft_day_fraction` × ceiling); soft total = ceiling − `request_max_usd` (new 429 `budget.total`); clamped on every `ceilings` event; the three demotion cases; new header `x-egress-guard` so that the Root can tell a bypass from an ordinary hit; §5 failure row; P-22; config.md §3.2 rule 6, §4.3, C-14 | root/egress.md and root/root.md (R7): classify ceiling refusals with `x-egress-guard` (proxy.md Q-EG4) |
| D12 | v03-decisions D12 | pass rates only in the live copy; G1 compares committed fields | ACCEPT | selfmap.md §3.8 (G1 drops `live` from both sides; a committed `live` → warning `live_ignored`), §3.10, §4.2 (`live` optional, never in the committed file), §4.3 example, S-13; gate.md G1 code `gate.g1.live_in_commit` removed | awareness.md: read pass rates from `state/kernel/self.json` only |
| D13 | v03-decisions D13 | template money: $5/day, $25 total; soft budgets stay in kernel config | ACCEPT (numbers) | config.md §2.1 (E1 text: the template ceilings are PENDING OWNER; under them the effective soft day is $4.00 and the evolution share $2.40, which still covers a default off-peak G4), §4.10 example comment, C-15 | — |
| EV-a | evolution.md §9 → gate (a) | replace `needs_owner`, bless/reject, approve/deny | ACCEPT | see D4: codes `inconclusive_final`, `costlier_than_budget` | — |
| EV-b | evolution.md §9 → gate (b) | agent stratum, eligibility, `audit_required` | ACCEPT (already applied; adjusted for D2, D3) | gate.md §3.7.2, §3.7.5, §3.9.2 | — |
| EV-c | evolution.md §9 → gate (c) | `calibrate` jobs queued by the kernel | ACCEPT (already applied) | gate.md §3.1 kinds table | — |
| EV-d | evolution.md §9 → gate (d) | scope `k`, stages K0–K3 | ACCEPT (already applied) | gate.md §3.1; plus K0 `gate.k0.test_feature` | — |
| EV-e | evolution.md §9 → gate (e) | `source.kind: "agent"`, `accepted.by: "admission"` | ACCEPT (already applied) | gate.md §3.7.1 task.json v2 | — |
| EV-g | evolution.md §9 (g) | §17 gap: a body hard-codes its own old tasks or seen pool prompts and stays flat on S | ACCEPT | D2 sealing closes the agent-task route; the new **divergence guard** (`pool_holdout_divergence`, reject when D_pool − D_holdout > 2·√(δ_p² + δ_h²)) covers seen pool prompts; gate.md §3.9, G-49 | — |
| EV-cfg | evolution.md §9 → config | new `[evolution]` keys; `evolution_share` a maintainer default; clamp to ceiling − `request_max_usd` | ACCEPT | config.md §4.7 (keys, already applied), §4.3 (maintainer defaults, already applied), §3.2 rule 6 (clamp, D6) | — |
| RT-X4 | root/root.md §9.1 X4 → proxy, config | keys, `keys` CLI, money caps and `ProxyHandle::panic` to the Root; HTTP/1.1 over `egress.sock`; clamp; remove `notify_cmd`; schema bump? | ACCEPT | proxy.md §3.1 (socket path from the boot block `paths.egress_sock`; panic removed), §4.2 (`kernel.panicked` removed), §4.4 (`panic`/`resume` removed, `set_ceilings` added), §3.8 clamp; config.md §4.8 (`notify_cmd` removed, rule 10 error). Schema bump: **yes**, `schema_version = 2` | — |
| RT-X9 | root/root.md §9.1 X9 → gate | confirm the agent-safe `gate` subset | ACCEPT with additions | gate.md §3.13: the full record stays kernel-side in `vault/lineage.jsonl`; R4 gets root.md's subset plus `level`, `scope`, `escalated`, `audit_required`, `charter_sha256`, `n`, `capability` aggregates | root/root.md §3.10.5: allow the added fields, and the kinds `task_admit` and `task_suspended` (gate.md Q-RT1, D7) |
| SV-13 | kernel/supervise.md §9 item 13 | `task_admit` data; registration anchor = the returned seq; who revokes after a demotion? | ACCEPT | gate.md §3.7.5: `task_admit` fields listed; anchor = `seq`; the gate suspends families (R4 `task_suspended`) on a Root `rollback` or `ladder{down}` past their registration; families are never deleted | — |
| SV-14 | kernel/supervise.md §9 item 14 | rename `needs_owner`; the re-run sample; the agent-safe subset; `/bless`/`/reject` | ACCEPT | gate.md §3.12 (re-run: R = 4 and every pool and holdout task the gate budget allows), §3.13 (subset), §3.12 (signals) | — |
| SV-15 | kernel/supervise.md §9 item 15 → proxy | member lanes with daily shares; egress socket from the boot block; keys, caps and 503-on-panic to the Root; clamp; L1 routing and budget setting | ACCEPT | proxy.md §3.4 (`member` scope, `run/fleet/<bid>/proxy.sock`, share from `fleet spawn` or `fleet.default_share_musd_day`), §3.8 (`budget.member`), §3.1, §3.5a and §3.8 (already applied); P-24 | kernel/supervise.md: open a `member` scope per member (proxy.md Q-SV1) |
| IC-7 | kernel/initctl.md §9 item 7 | `task.json` fields for agent tasks; the `--agent` path of `gate task import`; `task_admit` data | ACCEPT | gate.md §3.7.1 (family layout), §3.7.5 (`init-kernel gate task import <dir> --agent`; `task_admit` fields; result `{family, seq, visible, sealed}`) | kernel/initctl.md: `task submit` takes a family directory (gate.md Q-IC2) |
| AW-1 | awareness.md §9 → selfmap | capability field: categories, n, pass_permille, lower bound; holdout, canary and sealed excluded; outside the drift check | ACCEPT | selfmap.md §3.12: sources `pool` and visible `agent` only; holdout, canary, sealed halves and S excluded; the Root's summary moved to `live.sealed_audit`; outside drift by D12; S-16 | — |
| EXT-E9 | ext/authoring.md §9, ext/host.md §9 → gate | compensating checks for the `forbid` exemption | NO CHANGE (applied in R2) | gate.md §3.3 step 3 (`ext_not_cdylib`, `ext_dependency`) | ext docs: close the item |
| EXT-E10 | ext/authoring.md §9, ext/host.md §9 → gate | source-less lockfile entries are not a `dep` change | NO CHANGE (applied in R2) | gate.md §3.1 (testing.md already lists it as closed) | ext docs: close the item |
| EXT-E12 | ext/host.md §9, ext/wit.md §9 → selfmap | `bindgen!` path `kernel/wit/init-ext/v1` | ACCEPT | selfmap.md §3.6: `bindgen!({ path: "../../wit/init-ext/v1", world: "init:ext/tool" })`, the kernel tree's WIT; delta row | ext docs: close E12 |
| SB-4 | kernel/sandbox.md §9 item 4 → config | `extra_read` versus tail Z2 | NO CHANGE | D9 settles it in root/root.md (Z2 denies only Root paths); `[sandbox] extra_read` stays | — |
| RM-1 | roadmap.md §9 → proxy | kernel tests under `init-fakeroot`; no standalone mode | ACCEPT | proxy.md §3.1 (no standalone mode), §3.7 and §4.1 (tests via `init-fakeroot` or the real Root), §4.4, §4.5 (`--standalone` removed); config.md (`base_url`, `[proxy.test_root]` removed; `test_mode` only marks a test home) | — |
| TS-1 | testing.md §9 → gate | a pool-versus-holdout divergence check (MS-5, R15) | ACCEPT | see EV-g | testing.md: MS-5 now expects `reject: pool_holdout_divergence` |
| TS-2 | testing.md §9 → gate | `escalated: true` in the re-run record | ACCEPT (already applied) | gate.md §3.13 (`escalated`), §4.1 `GateKind::G4{escalated}` | — |
| TS-3 | testing.md §9 → gate | test-only features `test-weaken-canary`, `test-reject-all`, `test-score-skew` | ACCEPT | gate.md §4.1 (features, expected outcomes; release builds refused at K0 with `gate.k0.test_feature`; disposable environments only) | — |
| TS-4 | testing.md §9 → proxy, supervise | run under `init-fakeroot`; never talk to a provider directly | ACCEPT | see RM-1 | — |
| TS-5 | testing.md §9 (closed list) → gate | a test switch that excludes `wall_ms` from `cost_x_wall` | NO CHANGE (exists) | `gate.g4.ct_include_wall = false` (gate.md §3.9) | testing.md: close the item |
| SE-1 | root/sealed-eval.md §9 → gate | consume the admission rules on the import path; the gate-of-gates handshake; `audit_required` | ACCEPT (already applied; updated for D2, D3, D5) | gate.md §3.7.5, §4.1a, §3.9.2 | — |
| SE-2 | root/sealed-eval.md §9 → config | `audit_required` and level gating of agent-task credit agree | ACCEPT | no config key is involved: agent tasks exist only from L1, and `audit_required` is gate.md §3.9.2's; config.md has no admission keys (§4.6) | — |
| EG-1 | root/egress.md §9 → proxy | connect to the egress socket; key, pool and WebSocket code move to the Root; map refusals; fold `/_root/ceilings` into stats | ACCEPT (already applied) | proxy.md §3.7, §3.9, §4.2, §4.3 (`root` key); path naming per root.md (proxy.md Q-EG6) | root/egress.md: use `ROOT_HOME/run/egress.sock` (supervise.md item 12 asks the same) |
| EG-2 | root/egress.md §9 → config | `[budget]` money keys stay as soft budgets; note `day_usd ≤ ceilings.day_musd` | ACCEPT with change | config.md §3.2 rule 6, §4.3: the relation is the stricter D6 clamp, ceiling − `request_max_usd` | — |

Counts: accepted 31 (RT-X9 with additions; 7 already applied), accepted with change 1 (EG-2), no change here 4 (EXT-E9, EXT-E10, SB-4, TS-5).

## Items raised by these docs and still open (no change needed from the main session)

- gate.md Q-SV5 (supervise.md: keep the G4 workspace on probe stop), Q-SE2 and Q-SE4 (sealed-eval.md: family parameters, the L3+ sealed-run interface, the `judge` launch), Q-RT1 (root.md: allowed `gate` fields and kinds), Q-EV1 (evolution.md: `capability_labels`, the cost rule, families), Q-IC2 (initctl.md), Q-AW1 (awareness.md).
- proxy.md Q-EG2…Q-EG7 (egress.md: forwarded models, one retry owner, D6(c) classification with `x-egress-guard`, prices, naming, the `member` scope), Q-PV1, Q-IC1, Q-SV1, Q-TS1, Q-CV1.
- selfmap.md Q-RT2 (`lineage/kernel.git` below L3), Q-SE1 (sealed-audit summary), Q-SV1, Q-AW1.
- config.md Q-EG1, Q-IC1, Q-AW1, Q-TS1, Q-CV1, Q-EV1 (whether `soft_day_fraction` stays next to the D6 bound).
