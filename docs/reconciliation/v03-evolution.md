# v0.3 reconciliation: evolution, awareness, testing, roadmap

```
Status: v0.3 round · 2026-10-20 · Area: evolution (owner docs: evolution.md, awareness.md, testing.md, roadmap.md)
Binding input: docs/reconciliation/v03-decisions.md (D1–D13; D13 as settled by the owner on 2026-10-20),
               CONVENTIONS.md §1 (live-system safety rule)
Sources searched: the Open questions of root/root.md, root/egress.md, root/sealed-eval.md, kernel/*.md,
               body/*.md, ext/*.md (every item addressed to the four docs above)
```

**Totals:** 8 main-session decisions applied (D1, D2, D3, D4, D6, D7, D12, D13) and 21 incoming cross-doc items: 20 accepted, 1 accepted in part (selfmap.md Q-AW1, genesis half declined). Items still open after this round are listed in each doc's §9 under "still open".

## Main-session decisions

| id | decision | change made (doc §) |
|---|---|---|
| D1 | Sweep tests only in a disposable environment; live tests touch only what they created and hold by PID or path | testing.md §1 property 5; new §3.1a (live vs disposable environments, the runner's `"env"` field, `INIT_TEST_ENV` plus a root-owned sentinel with the allowed uid, live cleanup by held pgid only, escaped PIDs reported, never killed); new §3.13.0 SW-1…SW-6 (three-condition sweep with application-like decoys; the live-account refusal); F-hang (a), `escape_setsid`, F-kill-kernel, F-kill-root, A5, A6 steps 8–10 marked disposable; §3.5b the fake Root's broker kills only held PIDs; §5 "leaked processes" row rewritten (no marker sweeps); §3.12 CI split into full (live) and full (disposable); §4.2 `INIT_TEST_ENV`; §10 delta. roadmap.md: the disposable account as an M0 input (§3.9.1), disk row and peak (§3.8.2), lane rule (§3.8.3), M2 gated on SW-1…SW-6 per `init-root` build, WP3 +1 and WP21 +1, risk R18, §4.3 check commands, open questions 4–5 |
| D2 | Agent-authored tasks are families of ≥ 2 instances; a random half is sealed at admission (kernel L1–L2, Root L3+); `capability_gain` needs a significant gain on the sealed half and non-inferiority on S; `audit_required` still applies | evolution.md §3.15.4 rewritten (diagram, steps 1–8, gaming table row "hard-code the answers"), §3.15.3 in-flight row (families), §3.15.5 (sealed halves excluded from HGM counts), §7 known limit, §9 (former gap (g) closed), §10 new row. testing.md TA-1, TA-11, new TA-15…TA-18 (TA-18 records the "instances must differ in data" limit), MS-5 note. roadmap.md M7 (≈ 75 AH). awareness.md §3.7.1 (`source: agent` rows are visible halves only) |
| D3 | K = 5 promotions; pre-registration anchored to the Root lineage `seq` | evolution.md §3.15.4 step 6 and diagram, §9 internal Q5, §10 row (K = 5 promotions; commits are a weak unit). testing.md TA-8 (anchor), TA-9 (5 promotions vs 40 commits). Also answers kernel/gate.md Q-SE1 |
| D4 | No owner step anywhere: `needs_owner` removed; one escalated re-run, then `inconclusive_final`; `/accept`, `/bless`, `/reject` optional and never gating; `/approve`, `/deny`, `/clear-safe` removed; system safe mode self-clears after 24 h | evolution.md §3.1 owner row, §3.2 inconclusive rule (optional `/bless`/`/reject` may settle early), §3.7 (no early owner clear), §3.10.3 (`/accept` recorded, never scored or awaited). testing.md §1 property 4, NO-OWNER-1 (forbids signals and removed commands), new OPT-SIG-1 |
| D6 | Ceilings: soft budgets clamp at run time when a ceiling drops; only three "attempts" demote (R3 violation, level-gated refusal, a Root ceiling refusal that bypassed the kernel's guard); ordinary hits and the peak-price gap only log | evolution.md §3.14 (soft-layer paragraph, the charter's day boundary, no safety fraction), table row "Day spend (soft)", F19, §3.15.3 trigger row, §4.6 (`soft_day_fraction` removed earlier). testing.md RT-C1, RT-C2, RT-C4 (ordinary, no demotion), RT-C5, new RT-C9 (peak-price gap), RT-C10 (lowered ceiling clamps, nothing refuses to start) |
| D7 | `lineage.append` gets a dedup key; the Root accepts every kind the docs use (incl. `task_suspended`, `fleet_campaign`); the Root writes `lineage/kernel.git` (read-only to the zone) and supplies the kernel source bundle | evolution.md §3.15.6 "Source and lineage" (candidates fetched into the kernel's own area; the Root appends on install), stage 1–2 diagram, §4.4 (all kinds; dedup key `<job>/<kind>`), §9 (former root items b and d closed) |
| D12 | Pass rates and the capability self-model live only in the live copy `state/kernel/self.json`; G1 compares committed fields only | awareness.md §2 non-goals, §3.1 rows ("what exactly am I made of" names both files; capability row), SELF.md recipe (`jq '.live.capability' state/kernel/self.json`), §3.7.1 rewritten to selfmap.md §3.12's `live.capability` schema (category, source, model, tasks, pass/lo/hi permille, `low_n`, `sealed`), §3.9 row, §4.1 files, §5 failure row, T-AW-14 |
| D13 | Charter template decided by the owner: `max_level = 4` (start at L0, climb on record); `/panic` demotes one level, `stop` does not; lineage mirror `nagisanzenin/INIT-Lineage` created at M0 and pushed only by the Root; $5/day and $25 total; soft budgets stay in kernel config. The constitution text stays PENDING OWNER | roadmap.md §3.9 intro, §3.9.1 charter table (money, max level, panic, mirror rows marked decided; constitution row PENDING OWNER, read on GitHub), M0 DoD, §9 cross-doc. awareness.md: constitution wording (lineage "mirrored off this machine"; demotion "lowers your level by one. An ordinary stop does not"; now 649 words), §3.3 note on the decided values, the panic and stop boot lines, all money examples ($5.00 ceiling, $4.00 soft day after the run-time clamp), owner list in §9. evolution.md §3.14 L0 defaults and sizing (evolution share $2.40 under the $5 ceiling). testing.md charter fixture `phase0.toml`, E2E configuration, §10 row |

## Incoming cross-doc items

| id | from doc § | question | decision | change made (doc §) | follow-up for another doc |
|---|---|---|---|---|---|
| RT-X8 | root/root.md §9.1 X8 → awareness | purpose text and level: head or tail? constitution source | ACCEPT (answered in the first v0.3 pass) | awareness.md §3.3: `purpose.text` as a `## Purpose` block after the constitution; the level only at the tail; constitution from `charter.constitution.text` | body/log-context.md: the purpose block in the head |
| RT-X10 | root/root.md X10 → evolution | `propose-kernel` → `proposal.file`; seed sync from `kernel/seed/*.bundle` | ACCEPT | evolution.md §4.1 (`kernel propose`, alias `propose-kernel`, sends `proposal.file`), §3.7, §3.13.2 (bundles), §4.6 `seed_source = "kernel/seed"` | — |
| RT-X11 | root/root.md X11 → testing | a fake Root in `init-testkit`; A5 `/resume` through the Root; S-Z group | ACCEPT | testing.md §3.5b `init-fakeroot`, A5 (v0.3), §3.18 mapping, SW-* | kernel/sandbox.md: S-Z ids |
| SE-249 | root/sealed-eval.md §9 → evolution | `audit_required` and level gating of agent-task credit must agree with its §3.5–§3.6 | ACCEPT | evolution.md §3.15.4 step 7 follows sealed-eval §3.5 (confirmed at the next scheduled audit) and §3.6 (rules, provenance), plus D2 families | root/sealed-eval.md: the family format and the sealing record (D2) |
| CF-QAW1 | kernel/config.md Q-AW1 → awareness | owner display offset from the charter's day boundary | ACCEPT | awareness.md §3.5.1 `owner_offset` row (`ceilings.day_starts_at_utc_offset`, seen as `root.day_resets_at`) | — |
| CF-QTS1 | kernel/config.md Q-TS1 → testing | fixtures at `schema_version = 2`; mock egress or `[proxy.test_root]`; C-07 replaces the key-file test | ACCEPT | testing.md §3.4.6 rows `schema_version`, `proxy.test_mode` | — |
| CF-QEV1 | kernel/config.md Q-EV1 → evolution | (a) clamp at run time; (b) charter's day boundary; (c) `[evolution]` keys in config §4.7 | ACCEPT | evolution.md §3.14, §4.6 | kernel/config.md §4.7: drop `soft_day_fraction`; `[evolution.fleet]` is now `members_default = 4`, `share_halving_after = 3` (replacing `explorers`); add `g4_escalate_repeats`, `provisional_max_age_s`, `seed_sync_interval_s`, `[evolution.tasks] agent_stratum_max` |
| GT-QSE1 | kernel/gate.md Q-SE1 → evolution | one form for registration and K | ACCEPT (settled by D3) | evolution.md §3.15.4 step 6 | kernel/gate.md, root/sealed-eval.md: switch to K = 5 promotions |
| GT-QEV1 | kernel/gate.md Q-EV1 → evolution | (a) optional `bless`/`reject`/`accept`; (b) `capability_labels`; (c) codes; (d) the cost rule | ACCEPT all four | evolution.md §3.2 (a, c, d), §3.9 `Validated.capability_labels` and step 11 (b), §3.10.3 (a) | — |
| GT-QAW1 | kernel/gate.md Q-AW1 → awareness | the constitution's "never visible" claim and the gate's holdout from L3 | ACCEPT, no text change | awareness.md §3.3 design note: the text claims "tasks you never see" only for the Root's sealed set | — |
| IC-8 | kernel/initctl.md §9 item 8 → evolution | quotas `tasks_day` (20), `fleet_spawns_day` (50); members' budget-share policy | ACCEPT | evolution.md §3.14 table, §3.15.5 (share policy), §4.1, §4.2; testing.md TA-14 | — |
| SM-QAW1 | kernel/selfmap.md Q-AW1 → awareness | point "what am I made of" and the genesis at both files; maybe a boot capability line | ACCEPT IN PART | awareness.md §3.1 row and the boot capability line use the live copy. **Declined for the genesis prompt:** about 10 more tokens on every request would push it over 300 tokens; `SELF.md`, also in the cached head, names the live copy (§3.2 note) | — |
| PX-QEV1 | kernel/proxy.md Q-EV1 → evolution | (a) clamp and alert; (b) the Root's day; (c) is 0.9 right given the peak-price gap? | ACCEPT; (c) the fraction is removed: D6 makes peak-price hits ordinary | evolution.md §3.14 | — |
| PX-QTS1 | kernel/proxy.md Q-TS1 → testing | a mock egress in `init-testkit` for P-02…P-23 | ACCEPT | testing.md §3.5b (in the fake Root: `/_root/ceilings`, `x-init-egress-error`, `x-egress-*`, a peak-priced counter), RT-C9 | — |
| SV-16a | kernel/supervise.md §9 item 16 → evolution | online pass rate without `/accept` | ACCEPT (done in the first pass) | evolution.md §3.10.3 | — |
| SV-16b | same | fleet parent selection and budget-share growth are evolution's policy | ACCEPT | evolution.md §3.15.5 (HGM scores and a suggested parent in `fleet list`; share halving rule) | — |
| SV-16c | same | qualified versus adopted SHAs | ACCEPT | evolution.md §3.15.5 "Qualified versus adopted", diagram, §4.1 `promote --sha` | — |
| SV-16d | same | `kevo/<id>` branches use the same trailer grammar | ACCEPT | evolution.md §3.15.6 | — |
| SV-16e | same | `propose-kernel` becomes `proposal.file` | ACCEPT | as RT-X10 | — |
| SV-16f | same | upstream sync reads `kernel/seed/*.bundle` | ACCEPT | as RT-X10 | — |
| SV-19 | kernel/supervise.md §9 item 19 → testing | the fake Root for SV-3, SV-4, SV-16, SV-33…SV-46 | ACCEPT | testing.md §3.5b | — |

## Still open (raised by these four docs; details in each doc's §9)

- **root/root.md:** an agent-written audited-good kernel after a demotion below L3; the value-delivered formula; kernel behaviour when the Root is unreachable; a fake clock and test ledgers; the sentinel check as a Root-side refusal (D1 defence in depth); the D13 values in its template.
- **root/sealed-eval.md:** the family format, sealing record and "instances differ in data" rule (D2, TA-18); stand-in M fixtures; suspension after a demotion; canary in G4 as a demotion trigger.
- **kernel/gate.md:** `capability_gain` on the sealed half; the pool-versus-holdout divergence check for maintainer tasks (MS-5, roadmap R15); test-only kernel features for MS-G1…G3.
- **kernel/initctl.md:** `budget set`, `model set`, `E_BUDGET`.
- **kernel/sandbox.md:** SW-1…SW-5 and T-S5…T-S8 into the S-Z group, marked disposable-only.
- **CONVENTIONS §2, §6:** the `root/` workspace listing; §6 still lists E1, ET-E2 and OW-2 as pending (E1 settled by D13; OW-1 stays pending).
- **adr/006:** a v0.3 note that D4 supersedes Decision 4 and the `needs_owner` branch.
