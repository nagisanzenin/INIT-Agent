# Reconciliation v0.3: kernel-supervise

```
Status: v0.3 R1 · 2026-10-20 · Area: kernel
Docs covered: kernel/supervise.md, kernel/sandbox.md, kernel/initctl.md; body/loop.md and body/boot.md for D11 only
Binding input: docs/reconciliation/v03-decisions.md (D1, D4, D7, D8, D9, D10, D11; D2, D3, D5, D6 where these docs touch them);
               CONVENTIONS.md §1 (live-system safety)
```

Sources:
- the decisions in v03-decisions.md;
- every cross-doc item that the other v0.3 docs and ledgers address to these three files: root/root.md, root/egress.md, root/sealed-eval.md, evolution.md, awareness.md, testing.md, kernel/gate.md, kernel/proxy.md, kernel/selfmap.md, kernel/config.md, and the ledgers v03-evolution.md, v03-kernel-gate.md and v03-egress-sealed.md.

Totals: **33 items**:
- 28 accepted, 4 of them (GT-SV5, GT-SV6, CF-IC1, EG-2) already applied and only re-checked;
- 2 accepted with a change (EV-L2, EV-Q);
- 1 needs no change here (SM-RT2);
- 1 answered with a different code (EV-RU);
- 1 is outside the files I may edit (EG-3).

**Live-system safety (CONVENTIONS §1).** In this round I ran no tests and killed or signalled no processes. Every test in these docs that sweeps, kills or signals a process the suite did not spawn is marked "disposable environment only":
- supervise SV-16 and SV-46;
- sandbox S-M40, S-M41, S-M47, S-L12, S-L17 and S-Z17a–f.

The SBPL texts did not change this round, so no profile re-test is due. The D8 clone layout should be re-run at M2 (sandbox S-M49).

**One correction made during reconciliation (EV-L2).** In the first pass of this round I rewrote the L2 fleet as "campaigns of explorers". I had read evolution.md while its author was still editing it, and that draft had campaigns. evolution.md's final text, and its ledger rows SV-16b, SV-16c and IC-8, keep the member design: members, qualified SHAs, adoption by `promote --sha`, and `fleet list/spawn/send/stop`. proxy.md (`member` scope) uses the same design. I reverted all three docs to members and kept the v0.3 changes: D7 records, D8 clones and the D6 ceiling.

## Ledger

| id | from doc § | question | decision | change made (doc §) | follow-up that ANOTHER doc must make |
|---|---|---|---|---|---|
| D1 | v03-decisions D1 | sweep membership: sandboxed + reads the mark + denied the Root canary or a decoy; sweep tests only in a disposable environment | ACCEPT | supervise §3.8.2 states the three conditions (a)–(c), describes the 2026-10-20 incident, and limits "Testing the sweep" to disposable environments; SV-16 and SV-46 are marked disposable. sandbox §3.11 (settled), §8 "Live-system safety" with two halves; S-M40, S-M41, S-M47, S-L12 and S-L17 are disposable-only; S-M40b added (live variant, own PID). initctl §3.3 uses the same three conditions for peer attribution on macOS | root/root.md §3.5.4: add condition (c) |
| D4 | v03-decisions D4 | no owner step; one escalated re-run, then `inconclusive_final` (the incumbent wins); `/accept`, `/bless` and `/reject` are optional; `/approve`, `/deny` and `/clear-safe` are removed | ACCEPT | supervise §3.3 (G4 inconclusive → one escalated re-run → `inconclusive_final` → rollback), §3.6.6 (`lkg.json.reason` drops `bless`; `g4_inconclusive_retries` removed; alert `provisional_inconclusive`), §4.6 (`/bless` records `owner_signal` and commits nothing; `/reject` rolls back; `/approve`, `/deny` and `/clear-safe` removed), SV-38. initctl §3.4 (owner signals never gate), §4.2 (`gate.g4.needs_owner` dropped), `vendor` with no approval stage (IC-27) | root/root.md §3.15: drop `/approve`, `/deny` and `/clear-safe` |
| D7 | v03-decisions D7 | every lineage kind is accepted with a dedup key; the Root writes `lineage/kernel.git` and supplies the source bundle | ACCEPT | supervise §3.13.3 `lineage.append{kind, data, dedup_key}` with `dedup_key = <kboot>:<outbox seq>`; every kind goes to the Root, and `vault/lineage.jsonl` is the local copy. §4.7 has a kinds table. §3.11.1: `kernel.git` is Root-written, and the kernel archives candidates in `lineage/kernel-cand.git`. §3.15 step 5: `store.put` of both binaries plus a `kernel-src` git bundle. Layout rows updated | root/root.md: name the dedup field (this doc sends `dedup_key`); add `store.put kind:"kernel-src"`; add `lineage/kernel.git` to tail rule Z4 |
| D8 | v03-decisions D8 | `LINK_*` parameters versus V2 (canonical paths) | ACCEPT (supervise changes; V2 unchanged) | supervise §3.6.3: probe roots have no symlinks; `body/` uses the real `lineage/body.git` as `origin`; `bin/` and `kernel/` are `clonefile` CoW copies (bind mounts on Linux). §3.14 member roots work the same way. sandbox §3.3.3: `LINK_*` equal the real directories; a "no symlinks" paragraph; §4.1 `spawn_args` canonicalizes every path. The SBPL is unchanged; S-M49 to be re-run at M2 | — |
| D9 | v03-decisions D9 | `extra_read` versus tail Z2 | ACCEPT | sandbox §4.3: `extra_read` is readable when the charter allows it, because Z2 denies only Root paths; §3.11 marks the conflict settled | root/root.md: Z2 text per D9 (also v03-kernel-gate SB-4) |
| D10 | v03-decisions D10 | egress `crates` class | ACCEPT | initctl `vendor`: fetches through the class as `/x/crates/<host>/<path>` (`index.crates.io` sparse index, `static.crates.io` files) into a local registry, then runs cargo offline; IC-27. supervise §3.16 table row | — (egress.md §3.5 covers both hosts) |
| D11 | v03-decisions D11 | the body must accept `stop` while paused | ACCEPT | body/loop.md §3.8.1: reason `swap`; item 2.4 reads `ctl` until `resume`, `exit` or `stop`; new rule: `stop` while paused → `ext.shutdown()`, flush `out`, exit 0 at once, no log write and no re-lock; the stop table gains a "While paused" row. body/boot.md: "Stop while paused (v0.3)" paragraph. supervise §3.8.3 sends `stop` to paused bodies on the Root's `stop`/`prepare_swap`; SV-39 | — |
| D2/D3/D5/D6 | v03-decisions | families with a sealed half; K = 5; sealed runs Root-only; ceilings and demotion | ACCEPT (where these docs touch them) | initctl `task template/submit/list/show` (families ≥ 2 instances, sealed half never listed, K = 5); sandbox §3.9 (D5 sentence); supervise §3.14, §3.16 and initctl §3.5 (`E_CEILING` before any spawn, because a Root refusal demotes) | — |
| GT-SV5 | kernel/gate.md Q-SV5 | keep the G4 workspace when a probe stops | ACCEPT (already applied) | supervise §3.6.3 `ProbeOpts.keep_on_stop`, `ProbeOutcome.kept` | gate.md: close Q-SV5 (its §3.7 already uses it) |
| GT-SV6 | kernel/gate.md Q-SV6 | pass the level to gate jobs | ACCEPT (already applied) | supervise §3.16 "Propagating the level" | — |
| GT-IC2 | kernel/gate.md Q-IC2 | `task submit` takes a family dir and returns `{family, seq, visible, sealed}`; `kernel` jobs; register `gate.admit.*`, `gate.k*.*`; drop `needs_owner` | ACCEPT | initctl `task submit` result is now `{job, family, seq, visible, sealed, admitted, checks}`, recorded as `task_admit`; `task list/show` keyed by `family`; §4.2 registers the codes; `needs_owner` dropped | — |
| GT-RT1 | kernel/gate.md Q-RT1 | kernel kinds `task_admit`, `task_suspended` | ACCEPT | supervise §4.7 lists them under gate.md's kinds (was `task_registered`) | root/root.md §3.10.5 still lists both `task_registered` and `task_admit` |
| SM-SV1 | kernel/selfmap.md Q-SV1 | run serve mode on selfmap §3.11's triggers; add `state/kernel/self.json`; embed the component table; install recipes into `kernel/docs/` | ACCEPT (the recipe part is redirected) | supervise layout row `self.json`; §3.16 "Rebuilding selfmap's served view" (the triggers, atomic write, no sandbox, embedded table). K-F1 forbids kernel writes to `kernel/`, so the recipes ship in the Root's release (§9 item 5) | root/root.md: the release carries `kernel/docs/` and the `kupgrade` fixtures |
| SM-RT2 | kernel/selfmap.md Q-RT2 | `lineage/kernel.git` below L3 | NO CHANGE HERE | supervise reads it when present and never writes it (D7) | root/root.md decides |
| PX-SV1 | kernel/proxy.md Q-SV1 | `paths.egress_sock` to `Proxy::start`; relay ceilings and level; one `member` scope per member; `budget.json`; instance id for `x-egress-body` | ACCEPT | supervise §3.2 (proxy wiring, `shutdown(grace)`, never `panic()`), §3.14 (opens and closes a `member` scope with the member's share), §3.16 (`set_ceilings`, `set_level`), layout `budget.json` | proxy.md §3.4: `open_scope` must accept `run/fleet/<bid>/proxy.sock`. The share-sum bound differs between proxy.md §3.8 (soft day − evolution share) and evolution.md §3.15.5 (the evolution share) |
| PX-IC1 | kernel/proxy.md Q-IC1 | `budget set`; show level, `root_*` and budgets in `status` | ACCEPT | initctl `budget set` (`E_BUDGET`), `status.root`, `status.budget.soft`, `status.link` | — |
| CF-IC1 | kernel/config.md Q-IC1 | `[vendor] require_approval` removed | ACCEPT (already applied) | initctl `vendor`, §10 | — |
| EV-a…e | evolution.md §9 → supervise | `update_failed` stage `demotion`; `boot.json.evolution` fields; `seed_tag_moved` as a report line; automatic upstream sync | ACCEPT | supervise §4.3 (stages `system_rollback`, `demotion`, `gate_of_gates`; `evolution` copied verbatim), §4.8 (sync every 24 h; `seed_tag_moved` is a report line) | — |
| EV-IC | evolution.md §9 → initctl | `budget set`, `model set`, `E_BUDGET`, `E_BUSY` reason `kernel_candidate` | ACCEPT | initctl "`budget set`, `model set`", §3.5, §4.2 | — |
| EV-L1 | evolution.md §4.1 | `task template`, `task submit`, `task list`, `task show` | ACCEPT | initctl renames `task status` to `task list` + `task show` (visible half only) | — |
| EV-L2 | evolution.md §3.15.5, §4.1; ledger rows SV-16b, SV-16c, IC-8 | the fleet: members, qualified SHAs, adoption, `fleet list/spawn/send/stop`, `fleet_spawns_day` = 50 | ACCEPT with change (reverted my first-pass "campaign" design) | supervise §3.14 rewritten for members: `refs/init/lines/<bid>`, `refs/init/qualified/<sha>`, adoption by `promote --sha`, `fleet_member` and `fleet_campaign` records (D7), member roots without symlinks (D8); §3.11.1, §3.7, §4.7, §4.9 (`fleet.default_share_musd_day` restored), SV-40 to SV-43, §10. initctl lane `member`, matrix, `fleet list/spawn/send/stop`, `status.fleet`, the member-lane `promote`, `E_QUOTA fleet_spawns_day`, IC-25, IC-29, §10. sandbox: "fleet member" wording, §3.3.5 | config.md §4.7: `fleet.explorers` → evolution's `[evolution.fleet] members_default`; testing.md FL-1, FL-2, RT-L8 and roadmap.md M9 still say "explorers" |
| EV-L3 | evolution.md §3.15.6 | K0–K3; `E_GATE_FAILED{stage:"gate_of_gates"}`; clean start; one candidate at a time | ACCEPT | supervise §3.15; initctl `kernel promote/status`, `E_BUSY kernel_candidate`, SV-44, IC-26 | evolution.md §4.1: `--wait` returns at the gate-of-gates verdict (`awaiting_swap`), as its own stage 1–2 diagram shows, not at `awaiting_root` |
| EV-Q | evolution.md §3.14, §4.2 | quota `tasks_day` = 20 | ACCEPT with config.md's name | initctl uses `budget.task_submits_day` (config.md default 10; gate.md agrees) as `E_QUOTA{quota:"task_submits_day"}` | evolution.md §3.14, §4.1, §4.2 and testing.md TA-14: rename it, and use 10 or ask config.md to change the default |
| EV-RU | evolution.md §5 | `E_BUSY{reason:"root_unreachable"}` | ANSWERED with `E_ROOT` | initctl §3.5: every Root-link problem is `E_ROOT` (`E_BUSY` keeps "wait for another job"); initctl §9 item 3 | evolution.md §5: use `E_ROOT` |
| AW-1 | awareness.md (v0.3) | `boot.json.env.root` verbatim; `cause.kind`; `kernel_upgrade`; `panic.channel`; `status.root` | ACCEPT | supervise §4.3 (`env.root` replaces `autonomy`; `cause` `{kind: demotion\|rollback_system, trigger, seq, evidence}`; `kernel_upgrade {from, to, source}`; `panic {at, channel}`), example regenerated; initctl `status.root`, IC-21 | — |
| AW-2 | awareness.md / log-context | notices `level`, `audit`, `fleet`, `kupgrade`, `task_admitted` | ACCEPT | supervise §4.5 notice kinds; §3.16 relays level and audit | body/log-context.md: map the kinds (supervise §9 item 8) |
| TS-1 | testing.md §3.5b | `init-fakeroot` for SV-3, SV-4, SV-16, SV-33…SV-46 and IC tests | ACCEPT | supervise §8 and initctl §8 now say `init-fakeroot` ("fake Root" removed) | — |
| TS-2 | testing.md §3.7; v03-evolution | T-S5…T-S8 sandbox IDs | ACCEPT | sandbox §8 mapping table | testing.md: adopt the mapping |
| TS-3 | v03-evolution "still open" | SW-1…SW-5 into the S-Z group, disposable-only | ACCEPT | sandbox §8: S-Z17a–f map SW-1…SW-6, listed in the disposable half | testing.md: adopt S-Z17 IDs |
| EG-1 | root/egress.md; v03-egress-sealed | `vendor` through the `crates` class with a local registry | ACCEPT | see D10 | — |
| EG-2 | root/egress.md | bodies cannot reach egress; the kernel can | ACCEPT (already tested) | sandbox S-M32 (body deny), S-Z6 (kernel allow) | — |
| EG-3 | v03-egress-sealed "still open" | body/boot.md: a body runs a task without `INIT_KERNEL_SOCK` | NOT APPLIED (outside my D11-only edit scope for boot.md) | — | body/boot.md owner |
| SE-1 | root/sealed-eval.md | M7 isolation; sealed runs launched only by the Root (D5) | ACCEPT | sandbox S-X3 (M7), §3.9 (D5) | — |

## Remaining follow-ups for other docs (summary)

- **root/root.md:**
  - D1 condition (c) in §3.5.4;
  - drop `/approve`, `/deny` and `/clear-safe` from §3.15;
  - name the dedup field (`dedup_key`);
  - add `store.put kind:"kernel-src"`;
  - add `lineage/kernel.git` to Z4;
  - change the Z2 text (D9);
  - list only `task_admit` in §3.10.5;
  - put `kernel/docs/` and the `kupgrade` fixtures in the release.
- **evolution.md:**
  - `task_submits_day` instead of `tasks_day`;
  - `kernel promote --wait` returns at `awaiting_swap`;
  - `E_ROOT` instead of `E_BUSY root_unreachable`;
  - the share-sum bound must agree with proxy.md §3.8.
- **kernel/proxy.md:** `open_scope` must accept `run/fleet/<bid>/proxy.sock`.
- **kernel/config.md:** rename `fleet.explorers` to `members_default`.
- **testing.md and roadmap.md:**
  - use member wording in FL-1, FL-2, RT-L8 and M9;
  - TA-14 quota;
  - adopt the S-Z17 and T-S mappings.
- **kernel/gate.md:** close Q-SV5.
- **body/log-context.md:** the new notice kinds.
- **body/boot.md:** EG-3.
- **CONVENTIONS §3, §4:** the layout rows and the `INIT_BODY_ID` env row (supervise §9 item 9).
