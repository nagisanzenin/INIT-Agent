# v0.3 reconciliation: root/root.md

```
Status: v0.3 round · 2026-10-20 · Area: root (owner doc: root/root.md, now draft v0.2)
Binding input: reconciliation/v03-decisions.md (D1, D4, D6, D7, D8, D9, D13; D13 as settled by the owner),
               CONVENTIONS.md §1 (live-system safety)
Sources searched: the Open questions of every v0.3 doc, kernel/supervise.md §9 "Conflicts with root/root.md",
               kernel/sandbox.md §9 conflicts, and the v0.3 ledgers v03-egress-sealed.md, v03-evolution.md
Edited: root/root.md only
```

**Totals:** 7 main-session decisions applied and 52 incoming items. 47 were accepted, some in modified form as noted. Five were answered or confirmed with no design change: #1, #16, #17, #28 and #34. Items still open are listed in root.md §9.1.

**Realignment note.** During this round root.md was first moved to the names in root/egress.md's first draft: `[ceilings]`, `keys/<id>`, `cred/` and egress-measured quotas. root/egress.md v0.2, written in parallel, had already adopted root.md's original names. root.md was therefore put back to them: `[money]`, `keys/providers/`, `keys/actions/`, and quotas enforced by the broker and the watcher. The two docs now agree.

## Main-session decisions

| # | Decision | Change made (root.md §) |
|---|---|---|
| D1 | Sweep safety: three conditions | §3.5.4 rewritten. A process other than the leader is signalled only if (a) it is sandboxed, (b) it may read the instance mark, and (c) it is denied **both** the sibling decoy `INIT_HOME/run/mark/decoy` and the canary `ROOT_HOME/run/canary`. The generic sweep uses the zone mark for (b). A missing `sandbox_check` degrades to the leader and its group only, never a scan (RF13). The decoy is created at install (§3.2.2, §4.3 step 5). Live-system safety paragraph added; `status --selftest` never sweeps; tests that kill are marked **D** (disposable environment only) in §8; new test RR-7. |
| D4 | No owner step required anywhere | `/approve`, `/deny` and `/clear-safe` removed (§2.3, §3.15, §4.2); `resume --clear-safe` removed (§3.6.4, §4.3). System safe mode clears only automatically after 24 h. New §3.6.6: an R1 pause that resumes itself, and `Halted` that retries itself (`crash_loop_safe` after `halt_retry_s` 21,600 s; `config` on a digest change; `disk` when free space returns). Integrity halts (layout, tooling, lineage, no tuple) stay stopped as a fail-safe, not as an approval. `/accept`, `/bless` and `/reject` are optional; the kernel records them as `owner` records whose `qid` the Root checks (§3.10.5). |
| D6 | Ceilings and demotion | §3.13.3 rewritten: only (a) a class or policy violation (including volume and rate on effectful classes), (b) a level-gated refusal, and (c) a guard bypass (root/egress.md §3.6's `x-egress-guard` epoch rule) demote. Ordinary hits, read-only volume and rate, and quotas are logged only. A lowered ceiling clamps the soft budgets and halts nothing (§3.12.4). `ceiling_hit` gains `rule`. Spec delta updated. |
| D7 | Lineage interface | `lineage.append` takes `idem` (`<boot_id>:<outbox_seq>`). A duplicate returns the original `seq` and `dup:true`; the index is `lineage/idem.jsonl` (§3.10.3, §4.1.3). All kernel and evolution kinds are accepted, plus an `x_*` escape for L3 kernels (§3.10.5). New §3.8.1: the Root writes `lineage/kernel.git` (zone read-only, tail Z4 `Z_KGIT`). It is created at install from `kernel/src/<version>.bundle`, and an admitted agent commit is fetched at swap install through a sandboxed upload-pack. Refs: `main`, `lkg`, `refs/upstream/*`, `refs/init/karchive/*`, `refs/init/rolled-back/*`. |
| D8 | Probe links | V2 exception for `LINK_*`: the parent is canonical and the last component literal, which must be a symlink or absent (§3.3.4); test RR-4b. supervise.md needs no change. |
| D9 | `extra_read` | Z2 now denies only the Root's paths outside `ROOT_HOME` (its service file). The fixed read allowlist is gone, and the `/opt/homebrew` var/etc deny moved into the inner profiles' section 8 (§3.3.2, §3.3.3). The charter gains `sandbox.extra_read` (C14), rendered as Root-generated `Z_EXTRA_READ_<i>` lines and Linux `--ro-bind-try` (§3.3.1, §3.4). V3 accepts those roots. Read containment now rests on the Root-embedded inner profiles plus V3 (§3.3.2, §7). Re-tested on 2026-10-20. |
| D13 | Charter template (owner-decided) | `autonomy.max_level = 4` (the agent starts at L0); panic demotes one level and `stop` does not (§3.9); the mirror is `git@github.com:nagisanzenin/INIT-Lineage.git`, created at M0 and pushed only by the Root with its deploy key (§3.11); money is `"5.00"`/`"25.00"`; soft budgets stay in kernel config (§3.12.1, §3.12.2). PENDING markers for CH-1, CH-2 and CH-3 removed; §9.2 is empty. |

## Incoming cross-doc items

| # | From | Item | Decision | Change made (root.md §) |
|---|---|---|---|---|
| 1 | supervise.md §9 #3 | does body safe mode clear itself? | answered there (via the Root's 24 h restart); no Root change | §3.6.4 notes that the restart ends body safe mode |
| 2 | supervise.md #4 | sweep membership | D1 | §3.5.4 |
| 3 | supervise.md #5 | owner commands | D4 | §3.15, §4.2 |
| 4 | supervise.md #6 | `lineage.append` idempotency | D7, accepted as proposed | §3.10.3, §4.1.3 |
| 5 | supervise.md #7 | fleet and kernel-candidate kinds | D7: accepted | §3.10.5 |
| 6 | supervise.md #8 | `kernel/` is Root-written: docs and fixtures | ACCEPT | release layout and install steps carry `docs/`, `kupgrade-smoke/`, `wit/`, `src/` (§3.2.2, §4.3) |
| 7 | supervise.md #9 | L3 kernel source | D7: ACCEPT | `kernel/src/<version>.bundle` + `lineage/kernel.git` (§3.8.1) |
| 8 | supervise.md #10 | no `state/kernel/` snapshot at a swap | ACCEPT (both layers) | the old kernel `store.put`s a `kstate` blob before exit 75; a probation rollback restores it first (`hello.boot.kstate`) (§3.7.1, §3.8, §4.1.3, RF21) |
| 9 | supervise.md #11 | V2 and `LINK_*` | D8 | §3.3.4 |
| 10 | supervise.md #12 | egress socket naming | ACCEPT | `ROOT_HOME/run/egress.sock`, delivered as `hello.paths.egress_sock` (no environment variable, as root/egress.md answered proxy Q-RT0) |
| 11 | supervise.md #19 | a fake Root | ACCEPT | §3.16: the contract is §4.1 + §3.5; `init-fakeroot` and `init-fakekernel` (testing.md) |
| 12 | sandbox.md conflict 3 | per-instance sweep rule | D1 | §3.5.4 |
| 13 | sandbox.md conflict 4 | `extra_read` versus Z2 | D9 | §3.3.1–§3.3.4, C14 |
| 14 | sandbox.md conflict 5 | `LINK_*` canonicalization | D8 | §3.3.4 |
| 15 | sandbox.md conflict 6 | fleet lane sockets | ACCEPT (text) | §3.3.3 names `run/fleet/<bid>/` and `spawn.sock` |
| 16 | sandbox.md 7 / initctl.md 5 | X3: path-filter `sandbox_check` from inside a sandbox | answered (S-Z10) | §3.3.5 cites it; the UNVERIFIED mark is removed |
| 17 | sandbox.md 8 | Linux peer attribution | answered there; no Root change | — |
| 18 | initctl.md 4 | owner lane removed; `/approve` and `/deny` | D4 | §3.15, §4.2 |
| 19 | testing.md §9 | fake clock `INIT_ROOT_TEST_NOW` | ACCEPT | §3.16 (feature `test-fixtures` + `root.test_mode`; `test.clock`) |
| 20 | testing.md §9 | `init-root test ledger` | ACCEPT | §3.16, §4.3: a throwaway key, `genesis{test:true}`, `trigger.kind:"test_fixture"`; a production Root halts on a test chain |
| 21 | testing.md §9 | the contract `init-fakeroot` implements | ACCEPT | §3.16 |
| 22 | testing.md §9 | does the Root re-adopt zone processes after a restart? | it sweeps them; never re-adopts | §3.6.5 step 5 |
| 23 | testing.md / CONVENTIONS §1 | test IDs and live-system safety | ACCEPT | §8: testing.md's RT-* adopted; Root-local cases renamed RR-*; kill, sweep and panic cases are disposable-environment only |
| 24 | roadmap.md | the Root–kernel contract, both fakes | ACCEPT | §4.1, §3.16 |
| 25 | roadmap.md | CH-1 4 versus 0; day ceiling 5 versus 10 | D13 | template `max_level = 4`; `"5.00"` a day |
| 26 | awareness.md | JSON shapes of `ledger.json`, `status.json`, `ceilings.json`, `charter.json` | ACCEPT | §4.5, §3.12.5: `criteria[{id,have,need,met}]`, `max_level`, `supported_max_level`, `last_change`, `audited_good`, `constitution_sha256` |
| 27 | awareness.md | X2: body reads `INIT_HOME/root/` | ACCEPT | §3.2.2 row; the rule goes to kernel/sandbox.md (§9.1 X2) |
| 28 | awareness.md | X8 answered (purpose in the head, level at the tail) | noted | §3.12.1 purpose row |
| 29 | awareness.md | trigger names in the demotion line | ACCEPT | `ladder` gains `cause`; `trigger.kind` enumerated (§3.10.5) |
| 30 | evolution.md (a) | kernel evolution frozen below L3 after a demotion | ACCEPT | §3.8 last paragraph |
| 31 | evolution.md (b) | `task_suspended`, `fleet_campaign`, `audit_required` | D7: ACCEPT | §3.10.5 |
| 32 | evolution.md (c) | the share-growth formula | ACCEPT as proposed | new §3.13.5 (600–900‰, +50 per good window, −100 per demotion; `value_window`, `share` records) |
| 33 | evolution.md (d) / gate.md Q-RT2 | who writes `lineage/kernel.git`; where K0 finds the candidate | D7: the Root | §3.8.1. The candidate stays in the kernel's own area until the Root fetches it at install; `system.swap` names `source_repo` and `source_sha` |
| 34 | evolution.md (e) | kernel behaviour when the Root is unreachable | confirmed: the kernel exits within 10 s of EOF on fd 3 | §4.1.1 (unchanged) |
| 35 | evolution.md (f) | X10: `propose-kernel` → `proposal.file` | ACCEPT | §4.1.3 (unchanged) |
| 36 | evolution.md (g) | CH-1 | D13 | §3.12 |
| 37 | evolution.md (d) / sealed-eval.md | a G4 canary trip as a demotion trigger | ACCEPT | §3.13.3 trigger row; `gate{reason:"canary"}`; spec delta |
| 38 | sealed-eval.md §9 | the `family_eval` kind | ACCEPT | §3.10.5 |
| 39 | sealed-eval.md §9 | `task_admit` fields (rule 6) | ACCEPT | §3.10.5 |
| 40 | sealed-eval.md §9 | `audit` and `mutation` fields (§4.1) | ACCEPT | §3.10.5 |
| 41 | sealed-eval.md §9 | zone limit `max_instances − sealed.parallel` | ACCEPT | §3.14 (12 − 2) |
| 42 | sealed-eval.md §9 | the tail for sealed run roots | ACCEPT | §3.3.6: kind `sealed`, `body`/`check` profile + the sealed tail (own root only; writes and sockets only there) |
| 43 | sealed-eval.md §9 | `sealed.family_eval` (L3+) and holdout rotation (Phase 1) | ACCEPT | §4.1.3. Rotation hands task bundles to the kernel as fds and never writes zone paths |
| 44 | sealed-eval.md (first draft) | lineage API, ledger, pointers, Root-launched instances, the `[sealed]` home | ACCEPT | §3.7.2, §3.10, §3.13, §3.3.6, §3.12.1 |
| 45 | egress.md §9 | the `egress_action` kind | ACCEPT | §3.10.5; the volume paragraph follows egress (no per-request records) |
| 46 | egress.md §9 | `ceiling_hit{source:"egress", demotes}` | ACCEPT | §3.10.5, §3.13.3 |
| 47 | egress.md §9 | `ceilings.get` embeds `egress::ceilings()`; call `set_level` on level and mode changes | ACCEPT | §3.14, §4.5, §3.13.1, §3.6.4 |
| 48 | egress.md §9 | `keys/actions/` beside `keys/providers/`; process model for the body count; the charter class format | ACCEPT | §3.2.1; §3.14 (the broker counts bodies); `[actions.<id>]` tables with `min_level` and models (§3.12.1, template) |
| 49 | egress.md §3.9 | quotas: the broker and watcher enforce them | ACCEPT (realigned) | §3.14: CPU from `wait4`, the disk floor and `INIT_HOME` walk, the R1 pause on a persistent floor breach |
| 50 | gate.md Q-RT1 | submitting `gate`, `owner` and `task_registered` with an acknowledged `seq` | ACCEPT | `lineage.append` returns `seq`. `lineage/body.git` and `refs/notes/gate` stay kernel-written; R4 signs the records |
| 51 | config.md Q-RT1, Q-RT2 | charter digest and level published; L3 config location | ACCEPT; no Root change for Q-RT2 | `charter_sha256` in `hello`, `charter.json`, `status.json`; `kernel/config.toml` stays owner-edited in the Root-written `kernel/` |
| 52 | selfmap.md Q-RT1, Q-RT2; proxy.md Q-RT1, Q-EG4 | ladder publication; `kernel.git` at every level; the admitted build digest; the level change notice; "attempt" definition | ACCEPT | §3.13.1 (`level` event + `set_level` + view within a second); §3.8.1 (`kernel.git` at every level); `hello.boot.tuple_kernel.init_kernel`, `status.json.kernel_build`; D6 (c) |

## Process note (live-system safety)

The first-draft session stopped its own two scratch socket servers with `pkill -f` and a pattern specific to its scratch path, before CONVENTIONS §1's rule existed. The pattern matched only those servers. In this round every experiment touched only files and processes it created, and processes were stopped by the PIDs held for them. No sweep, kill-by-pattern or broad predicate ran on the live system. Scratch files were deleted by exact path.
