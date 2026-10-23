# Build deviations, open gaps and owner decisions (overnight build, 2026-10-20)

```
Status: v0.3 · 2026-10-20 about 16:20 +07 · Area: reconciliation (build → docs)
         v0.1 (about 05:55) covered the work up to FIX2's draft. v0.2 adds FIX2 (final), FIX3, FIX4, FIX5, FIX6,
         WP15 and the docs-sync commit 986669f, and checks every adopt row against the docs.
Sources: build/status/{WP0,WP1,WP2,WP3,WP4,WP5,WP6a,WP7,WP7b,WP8,WP8b,WP8c,WP9,WP10,WP11,WP12,WP13,WP14,WP15,
         WP20,WP21,WP21b,WP22,WP23,WP24,WP25,S1,FIX1,FIX2,FIX3,FIX4,FIX5,FIX6}.md; BUILD_LOG.md; git log (read-only,
         through 80afa41); the "Build sync 2026-10-20" block in each doc's Spec deltas section (commit 986669f:
         25 docs plus v03-decisions D14); the orchestrator's note on FIX6 and FIX7.
In progress (no status file yet): WP16 (Phase 0 measurements M-1/2/4/5, started 06:22) and FIX7 (the three
         promote tests that the holdout rule turned red, §2 #33). WP6b, WP0m and WP17–WP19 have not started.
Edited: this file only. No doc or code was changed.
v0.3 (2026-10-20, DOCS lane, release push): §3 records the provisional owner decisions of 12:16 (RELEASE_PLAN.md)
         and marks each covered item "taken provisionally". §5's six adopt rows are applied to their docs. §1 and
         §2 are unchanged: the 14 open §1 adopt rows and the fix-code rows stay with their lanes.
v0.3 release-push pass (about 16:20, DOCS2 lane; sources build/status/{R-SAFE,R-DEC,R-SEC,K-FLAKE,K-LINE,K-GATE,
         M5,REL,WP26,G0-ROOT,G0-SEED,S-REAL}.md and git log through baab414): every row fixed today names its
         fixing commit in its Tag cell (R-DEC's Root fix-code rows, §5 L4 and L5); the decide rows behind §3.9,
         §3.13 and §3.15 say "built"; §2 gaps 1, 20, 25, 26, 29, 32 and 33 are updated; new §6 lists the review
         findings F1–F7 and the bugs the release push found and fixed. The 14 open §1 adopt rows were not re-checked.
```

**Citations.** `WP1 D3` = build/status/WP1.md, Deviations item 3. `N` = Needs from others, `L` = Left, `F1` = Findings item 1. `Done`, `Tests` and `Size` name those sections. Commit ids are short shas from `git log`.

**Tags.**
- **adopt**: the code is right, so the doc should change. `adopt (applied 986669f)` means the docs-sync commit made that change. It was checked against the doc's Build sync block, not assumed.
- **fix-code**: the doc is right, so the code should change.
- **decide**: the owner must choose. Each one is listed in §3 with a recommended option.

Totals for §1: **149 rows**. **117 adopt**: 103 applied in 986669f and 14 still open, all of them new in v0.2. **19 fix-code**, one of which FIX6 has already fixed in code (80afa41). **13 decide**. v0.1 had 125 rows (96 / 17 / 12), and every one of its 96 adopt rows is applied. §5 lists 10 more docs-sync leftovers (6 adopt, 4 fix-code), counted separately. **v0.3:** of the 19 fix-code rows, 6 are fixed in Root code by R-DEC on 2026-10-20 (one of them, WP21 D9 / WP7 D4, only for its Root half), 1 was fixed earlier by FIX6, 1 (WP24 D7) is tagged v0.1.x, and 11 stay open. §5 L4 and L5 are fixed. §6 adds the release push (review F1–F7, the live G0/G1 blockers and the other bugs fixed), outside these totals.

---

## 1. Deviations by doc

### 1.1 `INIT Agent spec.md`

| WP (source) | What the code does differently | Why | Recommended doc change | Tag |
|---|---|---|---|---|
| WP20 D1, WP22 Size, WP21 Size, WP23 D13, WP25 Size, WP24 Size | The Root grew from 2,016 lines (WP20) to 3,392 (WP22), then 5,673 (WP21) and finally ≈ 6,450 (WP24), counted by the root.md §4.8 method. The spec's §13 says "about 1,000–1,500 lines". WP21b's install path and FIX5's `proposal.file` are not counted yet. 986669f records the ≈ 6,450 figure in §13 as "under owner review" and sets no new number. | Every specified feature was built and nothing was cut. The largest modules are egress at 1,472, sealed at 983, lineage at 942 and sup.rs at 655. | Replace the §13 figure with the budget the owner picks (§3.1). | decide |
| FIX5 Done 1 | `proposal.file` is built. It stores the proposal in `ROOT_HOME/state/proposals/`, records `proposal{id, title, sha256, bytes}`, alerts the owner and is never applied. | §13 says a proposal is never applied, but the Root used to answer `root.unknown_cmd` (gap #6). | §13: name the store and the record. | adopt (applied 986669f) |

### 1.2 `docs/CONVENTIONS.md`

| WP (source) | What the code does differently | Why | Recommended doc change | Tag |
|---|---|---|---|---|
| WP0 D1 | `seed/.cargo/config.toml` points at `directory = "../vendor"`, the single repo-root `vendor/`. §2 lists `seed/vendor/`. | BUILD_PLAN keeps one vendor dir for all four manifests (`ci/vendor.sh`) to save disk. | §2 should say that the dev checkout uses the repo-root `vendor/`. The installed body and the seed repo carry their own `vendor/` with `directory = "vendor"`, as WP21b's seed packaging writes it. | adopt (applied 986669f) |
| WP0 D4, WP5 Done | kernel-proxy has no rustls, hyper-rustls, rustls-platform-verifier or tokio-tungstenite. WP5 removed them, which took vendor/ from 290 crates to 265. | proxy.md v0.3 moved TLS to the Root (Q-CV1). | §5: the kernel's dependencies are tokio, hyper, hyper-util and http-body-util. TLS (ureq/rustls) lives in the Root only. | adopt (applied 986669f) |
| WP13 D4, WP20 Done, WP12 D13 | The constitution text exists in three copies: `root/crates/init-root/assets/CONSTITUTION.md` (embedded in the Root), `seed/src/boot/CONSTITUTION.md` (the seed copy) and `seed/src/context/testdata/head/` (the golden snapshot). §2 names a repo-root `CONSTITUTION.md`. | The installed body must be self-contained, and an editable `body/CONSTITUTION.md` must never shadow `kernel/CONSTITUTION.md`. | §2 should name one canonical source and list the copies. ci/local.sh should get a drift check, like the WIT copy check. | adopt (applied 986669f; the drift check is 283e8a1) |
| WP21b N, FIX4 Done 1 | The seed repository (branch `seed`, `seed-v*` tags, its own cut `vendor/`) is produced ad hoc by a test script. FIX4 moved the packaging step out of `s1-install.sh` (now removed) into `scripts/slice1/seedpack.py` (`--kind repo` or `--kind fakebody`). It stubs 24 platform-only crates (`windows*`, `winapi*`, `linux-raw-sys`). | No doc names who produces the seed repo or whether stubs are allowed. | §2 and the release process (§3.6). | decide |
| FIX4 Done 2 | `root/Cargo.toml` builds `sha2` and the crates its hashing runs through (`digest`, `block-buffer`, `crypto-common`, `hybrid-array`, `cpufeatures`) at `opt-level = 3` in the dev profile. | A debug Root's start → ready fell from 9.8 s to 1.7 s. Release builds are unchanged. | §5. | adopt (applied 986669f) |

### 1.3 `docs/root/root.md`

| WP (source) | What the code does differently | Why | Recommended doc change | Tag |
|---|---|---|---|---|
| WP20 D1, WP21 Size, WP23 D13, WP25 Size, WP6a D1 | §4.8 and RR-34 (`rootloc`) set ≤ 1,500 lines with the core ≤ 995. The Root is ≈ 6,450. On top of that, the V1–V5 validation and the Seatbelt composition live in `root/sandbox-profiles/`, outside the count. | Specified features (see §1.1). | Put the new total and a per-module table in §4.8 and RR-34, and say whether `root/sandbox-profiles/` counts (§3.1). | decide |
| WP21b D1 | §4.3 step 7's `bundle create … --branches=seed` leaves the branch out, because git appends `/*` to a pattern with no glob. The code names `refs/heads/seed` instead. | A test found it. | Fix the step 7 command. | adopt (applied 986669f) |
| WP21b D2, D3, D6 | `bin/init-root` and `install.json` are written before step 8. There is a new `--no-service` flag. The install-mode kernel writes no `zone_start`/`zone_exit` and is bounded by 1,800 s and the disk floor. | exec-clean and the broker's V1–V5 and class rules need those two files. The harness needs the real HOME without writing anything under `~/Library`. | §4.3: the order, the flag and the bounds. | adopt (applied 986669f) |
| WP21b D4, D5 | The Root writes the kernel `config.toml` template: `schema_version = 2` plus `[sandbox]` from install.json. The genesis tuple's `kernel.built_from` is `lineage/kernel.git` `main`. | The broker's class rules compare the toolchain exactly. | §4.3 step 5 and §3.7. | adopt (applied 986669f) |
| WP21b Done (faster start) | `owner.sock` is bound before tuple verification, and the first `start` runs on the supervisor thread. `start` re-checks the slot, so a kernel spawned into `Panicked`/`Halted` is killed. | A debug Root spent about 25 s hashing before it served `owner.sock` (S1 gap 4). | §3.6.5: the cold-start order. | adopt (applied 986669f) |
| WP20 D3 | ROOT_HOME may be at most 80 bytes in `test-fixtures` builds and 64 bytes in release. | Repo test homes are ≥ 66 bytes, and 80 still fits `sun_path`. | §3.2.1: add the test-mode length. | adopt (applied 986669f) |
| WP20 D4, D5, D11 | `start` always runs the daemon in-process (`--foreground` only echoes). R-F5 checks only the skeleton's top-level directories. A rejected owner-socket peer is closed without a reply. | launchd and systemd run plain `start`. A check over every entry would let the zone halt the Root (DoS). | §4.3 and R-F5. | adopt (applied 986669f) |
| WP20 D6 | The C13 production minimums are code-chosen: `hb_interval_s ≥ 1`, `crash_window_s ≥ 60`, `safe_auto_clear_s ≥ 3600`, `owner_queue_max ≥ 10` and `hb_stale_s ≥ 3 × hb_interval_s`. Test mode allows ≥ 0. | The doc gives only examples. | §3.12.1 C13: list these values. | adopt (applied 986669f) |
| WP20 D7, D9 | Charter details: `mirror.url` defaults to `""`; built-in `crates` has `paths = ["*"]`; `margin_max_permille`; `constitution: {sha256}`. The test-ledger spec format is defined in code. | ICJ allows integers only, and the doc leaves the rest open. | §3.12.1 and §3.12.5. The test-ledger format goes in testing.md. | adopt (applied 986669f) |
| WP23 D10, WP25 D11 | The template writes `[mirror] url = ""` until M0, with the GitHub URL as a comment. This was an orchestrator decision. | A production install from the template would otherwise push to a repo that does not exist yet. | §3.12.1/§3.12.2: show `""` until M0 (ratify in §3.5). | adopt (applied 986669f) |
| WP21 D1, D2 | Panic kills only held processes: each leader and its own process group. A `setsid` escapee survives until `live-sweep` is enabled, which is disposable-env only. Cold start never signals a previous Root's instances (pid reuse). The CLI panic fallback writes the latch only. | This is the owner's order after the 2026-10-20 incident (CONVENTIONS §1). | §3.9 and the RT-P* rows: add an owner-machine variant (held pids only; `ps`/`sandbox_check` checks instead of a scan). | adopt (applied 986669f) |
| WP21 D3, D4, WP21b D7 | A seedless home forms genesis from the Root-written `kernel/bin` and an empty body. With no kernel installed, the Root goes to `Stopped` with a notice rather than `Halted{no_tuple}`. | This keeps the WP20–WP25 test homes working. | §3.7. | adopt (applied 986669f) |
| WP21 D5 | The supervisor is a pure `escalate()`/`backoff_ms()` plus an imperative loop. The `ColdStart`, `Swapping` and `Paused` slots are not built. | Scope for tonight. | §3.6: describe the shape. `Swapping` is a gap (§2 #5). | adopt (applied 986669f) |
| WP21 D6, D7, D8, D11 | Heartbeat staleness counts from `hello`. The Root echoes only lines it queues while no kernel runs. A panic writes no `zone_exit`, and `signal` is null. `kill` counts the leader only. | The first exec under Seatbelt took > 3 s. This also avoids a double echo. | §3.6.3 and §3.10. | adopt (applied 986669f) |
| WP21 D9, WP7 D4 | `Halted{crash_loop_safe}`'s 4-a-day cap uses the UTC day, and halts are not persisted, so a Root restart retries. | Simplicity. | Keep the doc. The code should use the charter's owner day and fold halts from records. | fix-code (WP21 D9 fixed 518e68e: owner day, retries folded from `halt` records, a standing halt survives a restart; WP7 D4 open, kernel serve quotas, §1.7) |
| WP21 D10, D12 | `state/root.json` has no `counters` or `crash`. Kernel logs keep 10 files but are not capped at 4 MiB. | Not built. | Keep the doc. | fix-code (fixed 518e68e: `counters` and `crash` in root.json; `sup/klog.rs` caps each log at 4 MiB) |
| WP4 D1, N | The body lane is `"b0"` (supervise.md §3.4), but root.md's example shows `"live"`. | supervise.md owns lane ids. | Change the root.md example to `"b0"`. | adopt (applied 986669f) |
| WP7b D2, N | The kernel derives the canary path from `paths.egress_sock` (`ROOT_HOME/run/canary`). | The boot block has no ROOT_HOME. | §4.1 boot block: add `paths.canary`, or document the derivation. | adopt (applied 986669f) |
| WP23 D2 | `idem` is indexed before the record is written (write-ahead), and the `root.lineage.after_idem` point sits between the two. | A resend after a crash at any point cannot duplicate a record. | §3.10.3 and §4.7. | adopt (applied 986669f) |
| WP23 D4 | Demoting `ceiling_hit` records are capped at `kernel_lineage_per_hour`. Beyond the cap, only the egress usage ledger has them. | WP22 N: a hostile kernel could flood the chain. One incident costs one level anyway. | §3.10.5: "every" becomes "capped". Same change in egress.md's D6 text. | adopt (applied 986669f) |
| WP23 D5, D6 | There is an allowlist of Root kinds (`key_rotate` reserved). A kernel record's "required" field may be null. | Handoff promotions send `gate_job`, `scope` and `ext_set` as null. | §3.10.5. The `owner_signal` vs `owner` naming is in §1.6. | adopt (applied 986669f) |
| WP23 D3 | Fault points are compiled under `test-fixtures`. There is no `fault-injection` feature. | Cargo.toml belongs to WP21. | Keep the doc's feature name and add `fault-injection` to `root/crates/init-root/Cargo.toml` (§3.12). | fix-code (fixed 518e68e: `fault-injection`, turned on by `test-fixtures`) |
| WP23 D7, D8, D9, D11, D12 | Open uses a constant 1,000-record tail check, while `audit verify` reads `root.lineage_verify_tail`. A head one record behind is tolerated. Mirror details: `hash-object`, urgent kinds bypass backoff, `diverged` lasts until restart, the mirror-ref channel fires once per commit id, `file://` URLs are percent-encoded. `import_release` creates `main` only when there is exactly one bundle. The daily check halts through a hook. | | §3.10, §3.11 and §3.8.1. The open-time constant should read the charter key. | adopt (applied 986669f) |
| WP25 D1 | The L1 audit window spans the whole chain, not only the records since the entry record. | Every regression demotes and resets `entered_seq`, so "0 regressions in 14 days" would be vacuous. RT-L3 needs it to block. | §3.13.2. | adopt (applied 986669f) |
| WP25 D2, D4, D5, D7, D8 | An incident opens at the `ladder down` and closes at the next `zone_start` or after 600 s. A panic always costs one level. `body_safe` in system safe mode is not a trigger. The code defines demotion while Stopped, Halted or Panicked, and the fields of a level-refusal `ceiling_hit`. | | §3.13.3. | adopt (applied 986669f) |
| WP25 D3 | A mutation-suite failure does not roll back, matching testing.md RT-L7. root.md §3.13.3 step 3 rolls back for every trigger. | Nothing was swapped. | §3.13.3 step 3: add the exception. | adopt (applied 986669f) |
| WP25 D6 | A kernel that repeats a violation after each restart is demoted again, once per incident. At L0 this becomes a restart loop. | It follows the spec's incident rule. | §3.13.3: escalate repeated L0 demotions (§3.9). | decide (taken provisionally, §3.9; built 518e68e, wp25 `rt_l10`) |
| WP24 N | R7 treats every `mutation{pass:false}` as a demotion trigger. sealed-eval.md §3.3 says a maintainer-built candidate only alerts. | Not built. | Keep the doc. `ledger.rs` should check `data.source == "agent"`. | fix-code (fixed 518e68e) |
| WP6a N, WP24 N | root.md §3.3.6's sealed-run tail is prose only, so WP6a could not implement it. WP24 composes PRELUDE + the run root (rw) + its lane socket, and denies INIT_HOME, the rest of ROOT_HOME, IP, XPC and foreign signals. | | §3.3.6: adopt WP24's text verbatim, then drift-test it in `root/sandbox-profiles`. | adopt (applied 986669f) |
| FIX1 N | The kernel never chmods `run/` (Z4). The Root must keep creating `INIT_HOME/run` at 0700. | Z4 denies `file-write-mode` on `run/` itself. | Add a note to §3.3.2 Z4. | adopt (applied 986669f) |
| FIX5 D1–D4 | `proposal.file`: the digest and size are of `text`; the file is `# <title>`, a blank line, then the text (0600); ids are `p-NNNNNN`; a title that fails the §3.10.6 privacy rule gets `root.lineage`; "≤ 5 a day" is a rolling 24 h on the Root clock, seeded from the lineage so a restart resets neither; proposals are accepted at every level and in every mode; `status --proposals` prints `proposal <id> · <title> · <path>`. | root.md fixed only the record's fields. | §3.10.5, §3.12.4, §4.1.2, §4.1.3 and §4.3. | adopt (applied 986669f) |
| FIX4 N, FIX4 Tests | A cold install from the repo seed peaked at 1,573 MB of transient disk: the build slot was 963 MB, the seed packaging 188 MB, and the final `init-body` link adds about 0.5 GB at the very end. It took 209.9 s. A fake-body seed peaked at 213 MB and took 17.3 s. | Measured on the real install path. | §4.3 step 8 (bounds): plan for about 1.7 GB per cold seed install, not about 1 GB, so the `disk_min_free_gib` floor keeps its headroom. Same figure in supervise.md §3.6.2. | adopt |

### 1.4 `docs/root/egress.md`

| WP (source) | What the code does differently | Why | Recommended doc change | Tag |
|---|---|---|---|---|
| WP22 D1 | The egress is 1,472 lines, against §6's 330. | The Slice-1 feature set was built in full. | Update the §6 size table (§3.1). | decide |
| WP22 D2, D3 | Test mode loads only keys that start with `test-`. Others are refused with the alert `key_file_real`. Loopback destinations are allowed only in test mode, `crates` included, and test mode never forwards off-host. | This makes "refuses real key files" mechanical. | §3.2, plus testing.md §3.4.1. | adopt (applied 986669f) |
| WP22 D4 | Money details the doc leaves open: the total is checked before the day; a cost counts toward the day it was admitted; a non-2xx answer costs 0; a non-connect transport error is charged the reservation; rounding happens once over the sum. `start` records are fsynced one by one (no 5 ms group commit), and `end` records are not fsynced. | | §3.5. | adopt (applied 986669f) |
| WP22 D5 | On panic, kernel connections are shut at once. An in-flight upstream connection closes at its next upstream frame, not within 100 ms, and is charged its full reservation. | ureq 3.4.2 has no cancel handle. | §3.9: the bound is one reservation per in-flight request. | adopt (applied 986669f) |
| WP22 D6, D7, D9 | An unlisted model (`class.model`) demotes under rule b. Safe-mode refusals are `class.disabled`, which is not a hit. There is no `egress` lineage kind: the egress writes `ceiling_hit` and `egress_action`, plus a `refuse` ledger line. The `/_root/health` body is `{level, ok, panicked, test_mode, v}`. | D6 reading. | §3.6 and §4.2. | adopt (applied 986669f) |
| WP24 D5 | The sealed lane socket is `ROOT_HOME/run/sl-<run>.sock`. | `<run root>/run/proxy.sock` overflows `sun_path` for a 64-byte ROOT_HOME. | §3.8, plus sealed-eval.md §3.2. | adopt (applied 986669f) |
| WP24 D6, N | The sealed lane relays buffered, which is fine. Two gaps: no `start` record goes before the upstream call, so a crash loses that request's cost line; and the sealed reserve is not enforced cumulatively, only the per-audit and per-gog caps are. | Not built. | Keep the doc. The egress should enforce Σ `t:"sealed"` cost ≤ `sealed.reserve_usd` and write a `start` line. | fix-code (fixed 47c618f) |
| S1 (cost table) | The Root meters at the peak list price and bills cache hits at the input price. The real Slice-1 run cost 2,844 µUSD at the Root and 762 µUSD at the proxy (3.7×). FIX4's mock run on the install path shows the same shape: 7,281 against 1,890 µUSD (3.85×). | Conservative by design (D6). This was not flagged in any status file. | None is required. See §3.13. | decide (taken provisionally, §3.13; built cdb49e8: S1's usage 3.73× → 2.00×) |

### 1.5 `docs/root/sealed-eval.md`

| WP (source) | What the code does differently | Why | Recommended doc change | Tag |
|---|---|---|---|---|
| WP24 D12 | R6 is 983 lines, against §4.2's 175. The sealed lane alone is 177. | | Update the §4.2 size table (§3.1). | decide |
| WP24 D1, WP8c D1 | Candidates are trees: `--candidate`, `--running` and `--lkg` are directories. The real `init-kernel gate judge` exists (WP8c), but the gog test still defaults to `init-root-stubgate`, and the real gate runs only with `INIT_GOG_GATE_BIN`. | The stand-in body has no `body.git`. With D14, FIX2 and FIX5 report `INIT_GOG_GATE_BIN` runs green at the gate's defaults, and the stub stays the default only to avoid a kernel build (FIX2 N). | §3.2 and gate.md §4.1a: Phase 0 judges trees, and shas come once the home copy has `body.git`. | adopt (applied 986669f) |
| WP24 D2, D3, D4, D10 | M follows sealed-eval.md, and C3 is testing.md's extra control. Every mutant also carries C2's trim. Generators are exact-text edit specs (data), not bundles. `mutation` adds `failed` and `source`, and `tuple` is null in Phase 0. | Without the trim, MS-G1 could not fail. | §3.3 and §6. | adopt (applied 986669f) |
| WP24 D7 | Sealed instances are tracked by sealed's own pid registry. They are not in the broker's table and do not count against `quotas.max_instances`. On Linux they are unconfined until WP6b. | Z1 hides all of ROOT_HOME, so the zone tail cannot be reused. | Keep the doc, which says they count toward `max_instances` and are cancelled through the broker. The code should move sealed spawns into the broker's table. | fix-code (v0.1.x: design note in build/status/R-SAFE.md, 3ebbddc; the zone quota already reserves `sealed.parallel`, and sealed pids are signalled only through `sys::signal`) |
| WP24 D8, WP23 N | The leak scan runs over the whole lineage at the start of each audit and gog, not at every append. `cancel_all` at Root start is lazy. | | Keep the doc. Hook the scan into `lineage::kinds::check_kernel`, and run `cancel_all` at start. | fix-code (fixed 47c618f: leak check at every kernel append; 518e68e: stale run roots removed at start) |
| WP24 D9, D11 | The rotation cursor and the "double the next sample" flag live in `sealed/noise.json`. An underpowered audit records `partial` and `underpowered`. n = min(`audit_n`, \|S\|). SE-01's flaky case is tested as a broken check. | | §3.4. | adopt (applied 986669f) |
| WP8c F2 | The sealed profile denies `network-bind`, so the judge uses direct egress: runs are serialized, and usage comes from the `/_init/stats` difference. Inside the gog the judge cannot hold a per-run cap, so the lane's `gog_usd` cap applies. | | Add a note to §3.2. | adopt (applied 986669f) |
| WP8c F3 | M4 and M7 are caught only because the bodies can read the home copy's vault, which the leak canary watches. The judge cannot nest a Seatbelt profile inside the Root's instance. | | §3.3: inside the gog, the canary is the only defense against M4/M7-style reads. | adopt (applied 986669f) |
| WP8c F1 | (D14, see §1.10.) A correct gate must accept C1 (§3.3), but the divergence guard with δ = 0 rejected it. | | §3.3: cite D14. | adopt (applied 986669f) |
| FIX5 Done 2–3, D5, D6; FIX2 follow-up | The stand-in S has 12 tasks plus a canary (7 new deterministic `sx-*` tasks). The gog draws 8 pool + 4 holdout, which is §3.3's sample and the gate's own minimums, so the real judge runs with no `-c` overrides. With D14's floors the divergence threshold is 2·√(0.0625² + 0.125²) ≈ 0.28 (it was 1.0, the whole score range), so the guard is live. C1 is `accept/better` and M3 `score_inferior` on all 495 possible holdouts (exhaustive). S must keep at least 12 tasks: with 11 or fewer the holdout is 3 or fewer, C1 becomes `inconclusive: underpowered:holdout` and the gog fails. | `GAIN_MIN_HOLDOUT = 4` needs a 4-task holdout. | §3.3: the S size, the sample and the ≥ 12 floor. | adopt (applied 986669f) |
| FIX5 D5 (residual) | Nothing enforces the ≥ 12 floor. A leak drop (SE-02) that takes S to 11 or fewer fails the gog by C1's inconclusive verdict, and no test runs the real judge on a reduced S. | Not built. | Make the floor mechanical (§3.15). | decide (taken provisionally, §3.15; built 518e68e, wp24 `small_s…`) |

### 1.6 `docs/kernel/supervise.md`

| WP (source) | What the code does differently | Why | Recommended doc change | Tag |
|---|---|---|---|---|
| WP1 D4, WP4 D1, N | Control-link names follow root.md §4.1. `lineage.append` uses `idem`, while the local `vault/lineage.jsonl` keeps `dedup_key`. `system.swap` takes `{version, source_repo, source_sha, init_kernel, initctl, report}`. The `level` event is `{from, to, cap, cause, trigger, r4_seq, evolution_share_max_permille}`. | root.md owns the link. | Align §3.13 with root.md. | adopt (applied 986669f) |
| WP4 D3, WP7b D1 | `ProxyCtl` and the serve `Backend` live in the `init-kernel` binary, not in kernel-supervise. | Lane independence, and no Cargo.lock churn. | Update the module map. | adopt (applied 986669f) |
| WP4 D4 | Test mode only logs the 64-byte INIT_HOME rule; it does not enforce it. | Repo test homes are about 85 bytes. | §4.1: add a test-mode note. | adopt (applied 986669f) |
| WP4 D5–D11 | Supervise reads its own config sections strictly. Timing: the 90 s ready timeout starts at the spawn answer; a spawn refusal backs off without counting a crash; body-safe is decided at the 5th crash; a body that ignores `/stop` is killed after 30 s + `exit_grace_s`. The next spawn waits up to 12 s for the old instance's `exited`. Install sends `ready` right after `hello` and reports `promotion{scope:"seed"}`. A probe runs with `INIT_BODY_ID=probe`. Plus inbox compaction and the git/build helper choices. | | §3.3, §3.7, §3.9 and §3.11. | adopt (applied 986669f) |
| WP4 N, WP8 D12 | The revert-worktree identity is `INIT Kernel <kernel@init.local>` (kernel-gate `evo/repo.rs`). §3.11.2 says `init-kernel <kernel@init.invalid>`. | A helper's choice. | Keep the doc. | fix-code |
| WP4 N | `core.logAllRefUpdates` keeps no reflog for `refs/init/*`. | git behaviour. | Add a note to §3.11. | adopt (applied 986669f) |
| WP7b D3, WP8b D1, D2 | G0 runs as a direct child of the kernel inside the kernel profile. The fetch uses plain `git-upload-pack`. | The broker-side G0 runner and the `run/spawn.sock` relay are not built. | Keep §3.6.1/§3.6.2 (§2 #1, #2). | fix-code (open; since d05c1e3 the kernel profile's section 6a gives G0's children Unix sockets under the slot's `OUT` only, a temporary rule to drop when this row is fixed, §6) |
| WP7b D4 | `Env.level` is the level at kernel start, so the gate record's `level` is stale until a restart. The proxy labels are current. | There is one `Env` per kernel. | Keep the doc. | fix-code |
| WP7b D5, D6, D7 | A t0 on top of a provisional chain stays provisional and gets the asynchronous G4. A data-only promotion reports no `handoff` stage (progress `state: handing_off`). The G-46 path for a t0 commits through `g4_pass`. | A provisional binary never skips its G4. | §3.6.8. | adopt (applied 986669f) |
| WP8 N | Cargo reads `.cargo/config.toml` from every parent of the build slot. With INIT_HOME under `~`, it reads `~/.cargo/config.toml` despite a fresh `CARGO_HOME`. | The §3.6.2 claim is incomplete. | §3.6.2 should say so. The code should make sandbox.md §3.4's build profile deny parent `.cargo/` reads, or put the slot outside `~`. | fix-code |
| WP13 N, FIX3 N | `boot.json.env.root` lacks `cap` and `supported_max_level`, and `selfmap: {}` leaves out the body size. Since FIX3 the body falls back to the Root's published view or leaves the cap out, so nothing breaks, but it costs a file read and needs X2. | | Keep boot.md and awareness.md §4.5. The kernel should fill the fields and always send a non-empty `INIT_SHA`. | fix-code |
| WP23 D6 | supervise.md §4.7 names an `owner_signal` kind with no qid. root.md's `owner` kind is qid-checked (RR-18c). The Root records `owner_signal` uninterpreted. | The docs disagree. | Use one kind (§3.10). | decide |
| FIX1 N, WP15 D8, N | The live initctl lane comes up after the body is ready, because `serve::spawn` waits on `services()` with a 10 s timeout per try. WP15 measured the delay at about 2 s to more than 60 s, so its agents loop on `initctl ping` before `promote` or `reload`. WP15 suspects `KernelInfo`'s exe SHA over the 154 MB debug binary. | | None. Bind serve first and hash lazily. FIX6 did both (80afa41; §2 #24). | fix-code (fixed 80afa41) |
| FIX6 Done 1–3, D4 | Start-up order: `run_with_proxy` takes `run/kernel.lock` and writes `run/kernel.pid` before `on_start`, so `run/kernel.sock` is bound right after the lock (an early lane, about 10 ms after spawn) and the server adopts the same listener when supervise answers `Services`. The kernel exe hash (`KernelInfo.build_sha256`, about 2.7 s for the 154 MB debug binary) runs on its own thread: `promote`, `gate`, `rollback` and `kernel status` wait for the gate `Env` (`ENV_WAIT` 300 s), and `status` reports `pipeline:false` until then. | The hash used to sit before the bind, which cost 2 s to more than 60 s under load. | §3.9 step 1: the lock, then the early `kernel.sock` answering `E_BUSY{reason:"starting"}` (initctl.md §4.2) until the server is ready. | adopt |
| WP15 D1, N; FIX4 L | `init-kernel install --template` (§4.8) is not built. WP15 builds its own template homes in each test process (`target/t15/tm-{seed,fake,fx}`, about 200 MB), and every Slice-1 case is a cold install of about 3.5 min and 1.7 GB. | Not built. | Keep the doc. Build `install --template` (§2 #30). | fix-code |

### 1.7 `docs/kernel/initctl.md`

| WP (source) | What the code does differently | Why | Recommended doc change | Tag |
|---|---|---|---|---|
| WP1 D1, D2 | Wire names are dotted (`task.submit`, …), and the space form and aliases are also accepted. `ping.lane` is one of `live`, `member` or `probe`. | The doc fixes no string, and the `owner` lane is gone in v0.3. | §3.4 and §4.1. | adopt (applied 986669f) |
| WP1 D3 | The `level` result carries `ledger`/`ceilings` raw. §4.1's example (`next.checks`, `supported_max`) contradicts root.md §4.5 (`next.criteria[{id,met}]`, `supported_max_level`, `max_level`). | root.md owns `ledger.json`. | Align the §4.1 example with root.md. | adopt (applied 986669f) |
| WP1 D5–D11 | The job record has `RecordProgress`, `reason`, `timed_out` and a `Lane` enum. `rollback.reason` is required. CLI additions and exit details. `budget set` sends ints or strings. Open enums for extensible sets. `status.sandbox` is optional. | | §4.4 and §4.5. | adopt (applied 986669f) |
| WP7 D1–D3, D5–D7, D9, D10 | `wait:false` reports `queued`. Ordering is stricter: every job response counts as a pending final. Idempotency rules. Job ownership is per lane instance. `level` with the link down gives `E_ROOT`. Path resolution and its error codes. `reload --wait` is refused on the live lane only. `E_RATE` is checked after parsing. | | §3.5–§3.7 and §4.1. | adopt (applied 986669f) |
| WP7 D4 | Serve quotas count per UTC day. | | Keep the doc: use the owner day, as the Root's money does (one day rule; see the WP21 D9 row). | fix-code |
| WP7 D8 | There are two log sinks: serve writes `state/kernel/logs/kernel.jsonl`, and supervise's `klog` writes to stderr. | | §3.3: name both. | adopt (applied 986669f) |
| WP7b N | `promote`'s waited result has no `scope`/`lkg`; the job record's `result` carries them. | | §4.1. | adopt (applied 986669f) |
| FIX1 N | The kernel never chmods `run/` itself (Z4). | | §3.3, "Socket files". | adopt (applied 986669f) |
| FIX6 D1–D3 | Until its server is ready, the kernel's early lane answers every request, `ping` included, with `E_BUSY`, `data:{reason:"starting", retry_after_ms:1000}` (retry: yes, CLI exit 75), then closes the connection. A malformed line still gets `E_PROTO`. The §3.3 uid check and the lane's connection and rate limits apply, but no peer policy runs, because the boot block and `config.toml` arrive after the lock. init-proto's `BusyReason` is a closed enum without `Starting`, so `data` is written as raw JSON. | A client never meets a missing or refusing socket, and a successful `ping` means the server is up. | §4.2: add `starting` to `E_BUSY`'s `data.reason`, so init-proto can add `BusyReason::Starting`. §3.3 "Socket files": the bind order and the starting answer. | adopt |

### 1.8 `docs/kernel/proxy.md`

| WP (source) | What the code does differently | Why | Recommended doc change | Tag |
|---|---|---|---|---|
| WP5 D1, WP22 D8 | The proxy sends `x-egress-guard: epoch=<E>`. §3.8's `day=…;total=…;seen=…;r=…` form is dropped. | egress.md owns the format; the Root parses the epoch, and a malformed header demotes. | §3.8. | adopt (applied 986669f) |
| WP4 D2 | `x-egress-body` is the Root instance id (egress.md §4.1), not the boot id. | | §4.1. | adopt (applied 986669f) |
| WP5 D2–D10, D12 | The guard's view is `day_spent + day_reserved` plus settled estimates, and the soft total is checked against the Root's lifetime figure. No `models` table means "forward every priced model". `route.bad_path` is checked first. Neutral tiers are priced ×1. Member lanes live under `run/fleet/`. Unmetered routes. The estimate input. A cancelled request is settled at its full reservation. A `resp_type` field. The tap and trace rules are in doc comments. | | §3.5–§3.9. | adopt (applied 986669f) |
| WP5 D11 | A gate keeps its open-time routable set after a demotion. It can then send L1 models, which the Root refuses with `class.level`, and that refusal demotes again (rule b). RT-L8 (a demotion cancels G4) is not built. | | Keep the doc. On a downward `set_level`, cancel or re-snapshot open gate scopes. | fix-code |
| WP5 L | `cost_mismatch`/`root_cost_musd` are not built, because the Root publishes no per-request cost (egress.md §4.1). | The docs disagree. | Either drop them from proxy.md or add a per-request cost header to egress.md (§3.11). | decide |

### 1.9 `docs/kernel/sandbox.md`

| WP (source) | What the code does differently | Why | Recommended doc change | Tag |
|---|---|---|---|---|
| WP6a D2 | Both `LINK_*` forms are accepted (D8). A symlink is accepted only inside a probe or member root, and the live root's `LINK_*` must equal its target. root.md V2 says "a symlink or nothing", while §3.3.3 says "real canonical dirs". | The docs disagree. | §3.3.3 and root.md V2: one wording that matches the code. | adopt (applied 986669f) |
| WP6a D3 | Class rules use the exact table values. `KERNEL`/`BIN` may be INIT_HOME's or the instance root's. | §3.12 says "real INIT_HOME". | §3.12. | adopt (applied 986669f) |
| WP6a D5, D6, D8 | Seatbelt on 14.6 does not refuse socket creation, only bind, connect and sendto. S-M35, S-M37 and S-B4 therefore target the runner's own loopback. S-M16 and S-M17 are `sandbox_check` queries (a missing path reads as denied). S-M46 uses a local proc-macro crate. | Nothing leaves the host. | §8: record the on-machine targets. | adopt (applied 986669f) |
| FIX1 D2, D3 | Bind rules stay `subpath RUN`/`PROBE`, because the kernel also binds `run/spawn.sock` and the judge relay. New rows: S-Z8e–o, S-M39b–h, S-M49j and S-M57g. | | §8: add the rows. | adopt (applied 986669f) |
| FIX1 N | The body cannot `stat` `$INIT_KERNEL_SOCK`, because it has `file-read-metadata` on `RUN` only. Connecting works. | | §3.3: probe the socket by connecting. Same note in boot.md and awareness.md. | adopt (applied 986669f) |

### 1.10 `docs/kernel/gate.md`

| WP (source) | What the code does differently | Why | Recommended doc change | Tag |
|---|---|---|---|---|
| WP8 Tests (G-21), N | The G-21 targets (≥ 90% accept at 0 and ≤ 7% at −2δ, at n = 16) are not met. They hold only at n = 30, R = 3 with noise history (91.7% / 6.5%). | The noise in δ̂: at n = 16, only 4 flaky tasks feed it. | §3.8 and G-21 (§3.2). | decide |
| WP8c F1, FIX2 (35647a8), BUILD_LOG 05:36 | With δ = 0, the divergence guard rejected C1 as `pool_holdout_divergence` on 4 of 5 holdout draws. **D14** as implemented floors **the divergence guard only**: δ' = max(δ, 0.5/n<sub>set</sub>) per set. The split guards keep the measured δ, and the margin keeps max(2δ, 0.5/n). The gate record gains `delta_min_permille` and `delta_floored_permille`. At 35647a8, C1 was `accept/better` on every draw of the old 4 + 1 sample. Since the holdout rule (next row) it needs a 4-task holdout, and on FIX5's 8 + 4 it is accepted on all 495 draws. | A broader floor (split guards too) raised false accepts at −2δ from 4.3% to 12.9% at the old gog shape, 4 + 1 (FIX2 D1). | §3.8.3: add the divergence-only floor and the new record keys. G-49: at 10 + 6, δ_h is floored to 0.083 and the threshold becomes 0.23, still below 0.4 (FIX2 N). Add D14 to v03-decisions.md (§1.23). | adopt (applied 986669f) |
| FIX2 follow-up (194ba30), FIX2 D4 | The holdout rule: an accept that rests on a score gain (`better`, `capability_gain`, `repro_fixed`) on a holdout of fewer than `accept::GAIN_MIN_HOLDOUT` = 4 tasks becomes `inconclusive: underpowered:holdout`, code `gate.g4.underpowered:holdout` (the first G4 code with a colon). It is a constant, not a config key. It runs after `accept_rule` and in the §3.9.2 recompute, and escalates once like any inconclusive, then `reject: inconclusive_final`. `cheaper`, `smaller` and `perf_gain` are unaffected. | With the floor alone, a 1-task holdout could never trip the divergence guard, so a pool-only gain (the MS-5 signature) passed as `better` 30–33% of the time. With the rule it is 0% at every size. | §3.9, §3.9.2, §3.12, §4.1 (G4 codes) and G-22. | adopt (applied 986669f) |
| FIX2 D2, D3 | An empty split gets δ_min = 0, and the margin keeps m = ∞ at n = 0. In the §3.9.2 recompute without agent tasks, the floor follows that recompute's own pool size, while the measured δ and the margin stay the same. | No evidence means no floor. | §3.8.3 and §3.9.2. | adopt (applied 986669f) |
| WP8 D3–D11, D13 | The forbid check is a hand-written lexer (no syn). `ln` is floored at 1 unit. Medians of even counts. `noise.json` split keys `pass_rate@pool`/`@holdout`. Seed purposes. The futility margin. The capability test is per family. Eligibility edge cases. The pool-quota reading. G0 policy additions: `paths`/`include`, `@file` rustflags, `[alias]` shadowing and snake_case keys. | Each G0 bypass was reproduced on cargo 1.97.1. | §3.3, §3.7 and §3.8. | adopt (applied 986669f) |
| WP8b D3, D5 | One record per gate job: promote records `g0_g3`, the asynchronous G4 has its own record, `dep` carries G4 inside, and there is `Cmd::G4Reject`. G2 details. | | §3.5 and §3.10. | adopt (applied 986669f) |
| WP8b D8 | `cost_guard` uses `tasks_per_day = 10` as a placeholder. | No 30-day task-rate source exists yet. | §3.9.2. | adopt (applied 986669f) |
| WP8c D1–D5 | Judge works on trees (a cargo tree gives `unsupported_tree`). Script-body G0/G1 analogues reuse the gate codes. Judge-mode defaults: `ct_include_wall=false`, no noise history, no futility look, the constant deepseek-flash peak reference price. The record adds `runs`, `tasks`, `config`, `egress` and `candidate.form`, and the scope is `t2`. There is a global `-c key=value` option. | Determinism (§4.1a). | §4.1a. | adopt (applied 986669f) |
| WP9 D10, N | RunRecord v2 has no `ts`, `model` or `sealed`. Selfmap reads them when present. Without `ts`, the window is the last 20 jobs and `last_run` is null, although the schema wants a string. | | §3.10 RunRecord v2: add optional `ts`, `model` and `sealed`, which kernel-gate then writes. 986669f made them optional (gate.md §4.4); kernel-gate does not write them yet. | adopt (applied 986669f) |

### 1.11 `docs/kernel/selfmap.md`

| WP (source) | What the code does differently | Why | Recommended doc change | Tag |
|---|---|---|---|---|
| WP9 D1 | `files` and `source_digest` omit `vendor/`, `target/` and `.git/`. | This keeps the < 2 s target; `Cargo.lock` pins the vendored crates. | §3 inventory. | adopt (applied 986669f) |
| WP9 D3 | `bindgen!` reads the repo-root `wit/`, because there is no `kernel/wit`. | | Add a note. A `kernel/wit` is for L3 self-containment later (§3.12). | adopt (applied 986669f) |
| WP9 D2, D4–D7, D9, D11–D13 | The syn `printing` feature. `tools/mod.rs` and `profiles/mod.rs` are `mod/` components. Marker regions. `uncounted_tokens`. `describe` is called twice on one instance. A kernel component table with chosen contracts. CLI additions. The drift report names `kernel.files`. S-11 is a regression pin. | | §3 and §4. | adopt (applied 986669f) |
| WP9 D8, WP14 F (W-3) | A decoded import lists only the functions the component actually imports. | Measured. | `wit_imports[].functions` means the imported functions. | adopt (applied 986669f) |

### 1.12 `docs/kernel/config.md`

| WP (source) | What the code does differently | Why | Recommended doc change | Tag |
|---|---|---|---|---|
| WP21b D4 | The Root's kernel template is only `schema_version` + `[sandbox]`. | config.md has no template. | Give a minimal template. | adopt (applied 986669f) |
| WP4 D5, WP5 N | There is no full cross-section loader. Supervise reads its own sections strictly, and `ProxyConfig` is separate. | Not built (§2 #22). | None. | fix-code |

### 1.13 `docs/body/boot.md`

| WP (source) | What the code does differently | Why | Recommended doc change | Tag |
|---|---|---|---|---|
| WP13 D1 | Probe 7(b) makes 2 requests, not 3. | After a text-only reply there are no results to send, and a third request would end on an assistant turn. | §8 item 7(b). | adopt (applied 986669f) |
| WP13 D2, D3, D5, D8 | The probe driver is built in boot from the loop's seams. Test-only knobs: `INIT_TEST_LOCK_WAIT_MS`, the `INIT_FAULT` body action `panic`, and the `boot.*` points. Root status is rendered through `rootview.rs`. Head errors exit 70. | | §3.9 and §4.4. | adopt (applied 986669f) |
| FIX3 Done 1 | Boot starts the real Wasmtime host (`ext_host::from_env()`), and `init-body ext describe\|call\|inspect` runs it (exit 0, 1 load error, 2 usage). The boot facts' `ext` is `{"status":"loading"}` in normal and probe mode and `{"status":"disabled"}` in safe mode. If Wasmtime cannot start (engine, linker or ticker error), boot logs a warning, the facts read `{"status":"unavailable","error":…}`, and an `Unavailable` host answers every `ext` call with "the extension host did not start at boot (<cause>)" rather than the safe-mode text. `StubHost` is off the production path. | The `None` host's safe-mode text would mislead the agent. | §3.5 and the boot-facts example: the three states and the `Unavailable` answer. | adopt |

### 1.14 `docs/body/loop.md`

| WP (source) | What the code does differently | Why | Recommended doc change | Tag |
|---|---|---|---|---|
| WP11 D1–D3, D5–D7 | `model::Request` is byte-based. `ContextHooks` changes: `boundary_append` returns `BoundaryOut`, `on_response` takes `Timing`, and `has_input` and `session_id` are added; `results` is `(name, ToolResult)`, with one `ReloadIn` per tier and an `errors` list. `serve()` returns the exit code. The `Prepared`/`Ran` and `ToolCx` shapes. `ExtHostApi` is a trait with a type-erased `Snapshot`. | Log-context already renders the wire body. | §4.2. | adopt (applied 986669f) |
| WP11 D12–D17 | Pause emits `busy:false`. Queued follow-ups are appended as owner messages. Reload details. The error `where` values. Panic results carry no source location. The spill total is process-global. | | §3. | adopt (applied 986669f) |

### 1.15 `docs/body/tools.md`

| WP (source) | What the code does differently | Why | Recommended doc change | Tag |
|---|---|---|---|---|
| WP11 D8 | A similar name also qualifies at edit distance ≤ 2. | The doc's own example (`src/lop.rs`) fails its "first 3 characters" rule. | §3.2. | adopt (applied 986669f) |
| WP11 D4, D9–D11 | `CaptureSpec::Merged` gains `name`. `read` skips the generic 51,200-byte cap. The timeout trailer wording. CRLF becomes LF in the model text, while the spill file stays raw. | | §3.1, §3.5.6 and §3.5.7. | adopt (applied 986669f) |

### 1.16 `docs/body/provider.md`

| WP (source) | What the code does differently | Why | Recommended doc change | Tag |
|---|---|---|---|---|
| WP10 D1–D3 | Types live in `src/model.rs`. `provider::Prompt`. The trait gains `wire`, `classify` and `fallback`. Fallbacks rewrite the body JSON. | The context renders the bytes. | §3.2 and §3.3. | adopt (applied 986669f) |
| WP10 D4 | DeepSeek flatten adds `"reasoning_content": ""` to every assistant message. | DeepSeek requires the field in tool mode, and `--strict` enforces it. | §3.10 (V-DS-3 confirms live). | adopt (applied 986669f) |
| WP10 D5–D8, D10 | Decoder details. Anthropic/OpenAI mappings. Best-effort socket timeouts. `INIT_TEST_CONNECT_TIMEOUT_MS` is accepted but not applied. There is no native usage (`usage.native = None`). | | §3.8 and §3.9. | adopt (applied 986669f) |
| WP2 D1–D6 | Mock defaults: `"Mock reasoning."` unless `think:""`; `x-init-cap: day`, as proxy.md says, not `daily`; the id scheme; upstream-mode fallbacks; G3 and task detection by "contains" / a line. | Otherwise strict mode would reject the docs' own scripts. | §3.12, plus testing.md §3.4.4. | adopt (applied 986669f) |

### 1.17 `docs/body/log-context.md`

| WP (source) | What the code does differently | Why | Recommended doc change | Tag |
|---|---|---|---|---|
| WP12 D1–D14 | The module layout. Log-level input mirrors. The step-10-before-9 order. A header for an empty file. `mask.masked[]` gains `call_id`/`input`, with "long" above 64 tokens. Image masking. Placeholder and summary formats. At most 2 retries. The inflation check and thrash window. Signals. Resume without a failure. The fallback `update_failed` text. Golden snapshots. A missing `tools` hashes as null. | | §3.8–§3.14 and §4.2. | adopt (applied 986669f) |
| WP12 N | awareness.md §3.3 puts a charter `## Purpose` block in the head after the constitution. §3.8.2 does not list it, so it was not built. | The docs disagree. | Pick one (§3.8). | decide |

### 1.18 `docs/ext/host.md`, `wit.md`, `authoring.md`

| WP (source) | What the code does differently | Why | Recommended doc change | Tag |
|---|---|---|---|---|
| WP0 D2 | With `members = ["*"]`, a target dir inside `seed/ext/` becomes a member and cargo fails, so `CARGO_TARGET_DIR` must point outside it. | This settles an UNVERIFIED note. | authoring.md §3.3. | adopt (applied 986669f) |
| WP0 D6, WP14 Done | The template is both `seed/ext/count-lines`, the reference extension, and `seed/skills/ext-authoring/assets/template/`. | | authoring.md. | adopt (applied 986669f) |
| WP14 F | Q-H1: `concurrency_support(false)` works on 48.0.6. Q-H2: a component uses 3 core instances, 1 memory and 2 tables. Q-A2: imports take borrowed arguments. wasi-libc does not resolve relative paths against `initial-cwd`, so guests must use `/work/…`. WASI `exit` takes a result, so `exit(3)` arrives as `exit(1)`. Stack overflow usually surfaces as an out-of-bounds access. Guest stdout is line-buffered. | Measured. | host.md, wit.md §3.4 and authoring.md: replace the UNVERIFIED marks. | adopt (applied 986669f) |
| WP14 D1–D7, D9 | Shared types are re-exported from `tools/ext.rs`. The pool-exhaustion text. Extra report lines. Describe errors add a log tail. An Ok during an abort becomes `aborted`. SKILL.md has 2 extra lines. Fixtures are `.rs` files. H16 runs alone. | | host.md §4.1 and §4.3; authoring.md §3.9. | adopt (applied 986669f) |
| WP14 Measurements | The M-2 warm rebuild median is 380 ms, so C1 (5.0 s) is not triggered and the script lane is not needed. | Measured. | authoring.md §3.10: record it. | adopt (applied 986669f) |
| FIX3 D1 | `BootQuiet` (in boot, wrapped around WP14's host) drops a boot-queued report (`Boot`, `SessionStart`) only if it is `ok`, names no component and holds nothing but `(no changes)`, and no `Request`/`Kept` reload was ever asked. The registry swap still happens, and every `initctl reload` or kept-publish report passes unchanged. | Otherwise every boot of a body without extensions adds a context item, which also lands after the probe instruction and broke `probe_round_trip`. | host.md §3.9 and §4.3: a no-change boot load logs no `reload` event. The rule may move into the host or the loop later (FIX3 N). | adopt |
| FIX3 N | `init-body ext inspect` lists the world's exports as `call` and `describe`, and `count-lines` imports no `init:ext/process`. | Measured in `ext_cli_on_count_lines`. | host.md §4.5: show the exports in the example. | adopt |

### 1.19 `docs/evolution.md`

| WP (source) | What the code does differently | Why | Recommended doc change | Tag |
|---|---|---|---|---|
| WP8 N | The thin rule requires `Compensates-for` to equal the removed component's tag. A §3.12 batch thin of components with different tags can never validate. | A spec gap. | §3.12 (§3.7). | decide |
| WP8 D12 | New codes `trailer.invalid:<Key>` and `evo.io`. "Strict" perf means `<` on cost and time and `>` on `cache_hit`. `GIT_CEILING_DIRECTORIES` is set. `trace:` repro refs default to not found. There is a fixed `cache/revert-check`. | | §3.8–§3.10. | adopt (applied 986669f) |
| WP13 D6 | Git sync (§3.6) runs nothing without `body/.git`, sets `GIT_CEILING_DIRECTORIES`, and stops on a timeout. `main` moves only after `rolled-back/<sha12>` exists. A refused ff and a failed pop are warnings. | | §3.6. | adopt (applied 986669f) |

### 1.20 `docs/awareness.md`

| WP (source) | What the code does differently | Why | Recommended doc change | Tag |
|---|---|---|---|---|
| WP13 D7 | The capability recipe reads `state/kernel/self.json` (D12). `env_changed` ignores disk moves under 1,024 MB and root-only changes. A stats failure keeps the Root's ceiling. The level line also shows on a session's first boot. The owner offset comes from the budget. | | §3 and §4. | adopt (applied 986669f) |
| FIX3 Done 2, D3 | The probe deadline now covers `spawn`. A transient `ETXTBSY` at spawn is retried until the deadline, and a successful exit gets a 500 ms grace (was 50 ms) for the reader thread. The root cause of the `t_aw_5` failure: macOS checks a freshly written executable on its first exec, which took 150–260 ms against 3 ms on the second, after `spawn` returns. | Under load the fake `rustc` probed null. | §3.5 probe table (2 s per probe, spawn included) and T-AW-5 (fixtures are run once, untimed, before the timed probe). | adopt |
| FIX3 Done 3 | The autonomy line's cap comes from `boot.json.env.root.cap`, then the Root's `ledger.json`, then min(`max_level`, `supported_max_level`) from `ledger.json` or `root/charter.json`. If none is readable, ` (cap L?)` is left out (`- autonomy: L0 · at your cap: charter max L4`) and unknown parts of the at-cap text are dropped. `next: null` means "at your cap" even without a cap, and no Root status at all reads `- autonomy: L? · ?`. | The kernel sends no `cap` (§1.6). | §3.6 (line format) and §4.5. 986669f applied the boot.md half (§3.8, `rootview.rs`), not awareness.md's line format. | adopt |

### 1.21 `docs/testing.md`

| WP (source) | What the code does differently | Why | Recommended doc change | Tag |
|---|---|---|---|---|
| WP3 D1–D7 | The testkit uses `libc` and `toml`. Fake modes are read from `INIT_TEST_FAKEKERNEL_MODE`/`INIT_TEST_FAKEBODY_MODE` and an `<exe>.mode` sidecar. The fake lineage is unsigned. The fake `boot.json` is minimal. Fake-kernel modes were added. Runner additions. Hang modes exit on EOF. | Both Roots clear the environment. | §3.3, §3.5 and §3.6. | adopt (applied 986669f) |
| WP10 D9 | Provider fixtures live in `seed/src/provider/testdata/`. | A bin crate's `tests/` cannot reach internal modules. | §3.4.6. | adopt (applied 986669f) |
| WP23 D1 | testing.md calls them "RT-16…RT-19"; root.md calls them RR-16…RR-19. | | Use one set of ids. | adopt (applied 986669f) |
| WP24 D2, D11 | The MS-C1…C3 and MS-2/MS-6 descriptions differ from sealed-eval.md. SE-01's flaky case is tested as a broken check. | sealed-eval.md owns M. | Align with sealed-eval.md. | adopt (applied 986669f) |
| WP13 N | The mock's `probe_reply` does not answer `ready` when user notes follow the tool results (DeepSeek's `[init:signals]`), so the probe takes 3 requests. | | Keep the doc. Fix the testkit responder. | fix-code |
| WP15 D1, D2, D9 | Each acceptance test process builds template homes itself (`target/t15/tm-{seed,fake,fx}`: `lineage/body.git` + the `body/` clone, `lineage/trees/`, `bin/init-body{,.lkg}`, the seed `lkg.json`, `kernel/config.toml`, `kernel/CONSTITUTION.md`, wrapper binaries), and a scenario copies one with the runner's `fixture.work` entry `".."`. Homes live under `target/t15/`, because a probe lane socket `<home>/probe/<12 hex>/run/kernel.sock` must fit 103 bytes. A2's agent passes `--config source.vendored-sources.directory=<repo>/vendor` to cargo. | `install --template` is not built (§1.6), and the dev seed's `../vendor` does not resolve inside a test home. | §3.6 (around the `install --template` paragraph): describe the stand-in until §4.8 exists, and the home-path length rule. | adopt |
| WP15 D4, D5 | A3 and A4 run on WP8b's model-driven fixture body (`fixtures/fx-body/`, which execs `sv-agentbody`), with a mock that is not `--strict deepseek`. A4's patch is "print the boot reason" plus an injected panic, and the fix removes only the panic. | A G0 build of the real seed for each candidate does not fit the disk budget. `sv-agentbody` does not echo `reasoning_content`. The spec's change+fix pair would squash to an empty diff. The strict protocol is covered by A1, A2 and A6 on the real body. | §3.9 A3 and A4. | adopt |
| WP15 D6, D7, D11, D12 | A5-fake covers the kernel half only, under `init-fakeroot`. The Root half is WP21's `crash_loop_safe_mode_halt_retry_and_self_clear`. A6 drives the real Root directly (test mode, temp homes, the key `test-wp15-a6-mock-key`), uses `192.0.2.1` (TEST-NET-1) instead of `example.com`, adds three refusals and three controls, and does not checksum `state/kernel/`. NO-OWNER-1's static half checks that no scenario sends `/accept`, `/bless`, `/reject`, `/approve`, `/deny` or `/clear-safe`. The charter fixture is `fixtures/charters/phase0.toml` at the repo root. | The runner's `start_root` is disposable-only. A broken sandbox must still not reach the internet. | §3.9 (A5, A6), §3.13 and §3.17. Step 3 of A6 (line 548): use TEST-NET-1. | adopt |
| WP15 D3, D10, N | Runner gaps: `wait_file` reads the path, so it cannot wait for a socket; `start_kernel`'s `owner.sock` wait is a fixed 10 s, while `init-fakeroot` spends about 10 s hashing a 154 MB debug kernel with debug sha2 (hence the `exec` wrapper for `INIT_KERNEL_BIN`); `check_g4` takes the "last" G4 job in `read_dir` order and compares `lkg.json.sha` with the record's top-level `sha`, while the kernel writes `args.sha` (its accept branch is untested, since A3 rejected); the fake Root writes no `kernel/CONSTITUTION.md`. First-turn expectations were relaxed to `last_role:"user"` because of the boot `[init:reload]` report, which FIX3's `BootQuiet` has since removed. | | Keep the doc. Fix the testkit (WP3) and restore the strict first-turn check. FIX6 (80afa41) fixed the socket wait and the `owner.sock` wait; `check_g4`, the fake Root's `CONSTITUTION.md` and the first-turn check remain. | fix-code |
| FIX6 Done 4 | The runner has a `wait_socket` step (`{"op":"wait_socket","path":…,"timeout_s":60,"serving":false}`) and the helpers `wait_socket`, `wait_serving` and `is_starting`. With `serving:true` it waits until a `ping` gets a real answer, not `E_BUSY{starting}`. The `owner.sock` wait in `start_kernel` and `start_root` is `OWNER_SOCK_WAIT_S` = 60 s (was 10 s and 20 s), and a step can override it with `owner_sock_timeout_s`. | WP15 N. | §3.6: the new step and the wait. WP15's ping loops and its `INIT_KERNEL_BIN` wrapper can then go (FIX6 N). | adopt |
| FIX4 D1–D3 | The Slice-1 harness (`scripts/slice1/s1.sh`, `tests/slice1/run.sh`) builds every home through `init-root install --seed-source <bundle> --no-service` with the owner's real HOME, from `seedpack.py` (`--kind repo` at `S1_SEED_REV=HEAD` by default, or `--kind fakebody`). It reads back an install check (tuple sha = seed sha = `lkg.json`), guards disk (6,500 MB to start, 5,300 MB floor, sampled every 2 s) and records install, `owner.sock` and start → ready times. testing.md does not describe the harness. | Both workarounds (hand-written `lkg.json`, `git archive` into `body/`) are gone, and HEAD is reproducible. | Add a short Slice-1 harness note under §3 (modes, guards, disk need). | adopt |
| FIX5 D6, FIX2 follow-up | MS-G3 now shows 9 disagreements (was 2), an audit samples 8 of 12, the gog's `sample_n` is 12, and MS-C1 and TA-11 need a holdout of at least 4. | S grew to 12 tasks; the holdout rule. | §3.4.6, §3.14, §3.15 and §3.16. | adopt (applied 986669f) |

### 1.22 `docs/roadmap.md`

| WP (source) | What the code does differently | Why | Recommended doc change | Tag |
|---|---|---|---|---|
| WP6a D1, WP8 header | The crates are `kernel-*` libraries per BUILD_PLAN: kernel-gate holds `init-kernel::gate`/`evo`, and `root/sandbox-profiles` holds the Seatbelt texts, which the Root owns since v0.3. | Lane independence. | §3.3 rows: use the crate names. | adopt (applied 986669f) |
| BUILD_LOG | The build added WPs that are not in §3.3: WP7b, WP8b, WP8c, WP21b, S1 and FIX1–FIX5. | The work was split during the build. | Add the rows. | adopt (applied 986669f) |
| BUILD_LOG 06:26, FIX6 | FIX6 (early `kernel.sock` bind and runner socket waits, lane B2; done, 80afa41) and FIX7 (the three red promote tests, §2 #33; in progress) came after 986669f and have no §3.3 rows. | Split during the build. | Add the rows; FIX7's when it reports. | adopt |

### 1.23 `docs/reconciliation/v03-decisions.md`

| WP (source) | What the code does differently | Why | Recommended doc change | Tag |
|---|---|---|---|---|
| BUILD_LOG 05:36, WP8c F1 | D14 (the noise-floor minimum, half a task) is applied in code (FIX2) but is not in the decision table. | It was decided tonight. | Add a D14 row ("divergence guard only") whose owner docs are gate.md §3.8.3 and §8 (G-49), and sealed-eval.md §3.3. | adopt (applied 986669f) |

---

## 2. Open cross-lane gaps

These are unbuilt or unwired items. The owner WP is the lane that should close each one. Numbers are kept from v0.1, so a closed gap keeps its row.

**Since v0.1:** gaps 10, 12, 24 and 27 are closed. Gaps 6, 8, 11 and 31 are partly closed. Gaps 30–33 are new; 33 is in progress (FIX7). Gaps 23, 25 and 26 have new facts.

**Since v0.2 (release push, 2026-10-20):** gaps 32 and 33 are closed (518e68e; FIX7 37aef0c). Gaps 1, 11, 20, 25, 26 and 29 have new facts in their rows.

| # | Gap | State now | Owner WP | Source |
|---|---|---|---|---|
| 1 | **G0 not sandboxed** | G0's commands run as direct children of the kernel inside the kernel profile, through the `Env.g0` seam. Tests use `DirectSpawner`. | Root/X (broker-side G0 runner or a `build`-profile process handle) + K1 (wire `Env.g0`). v0.3: still open; the kernel profile's section 6a (d05c1e3) lets G0's children use Unix sockets under the slot's `OUT` only, until G0 runs in `build` instances (G0-ROOT D1, N) | WP8b D1, N; WP7b D3, N; G0-ROOT |
| 2 | **Sandboxed fetch relay** | The `run/spawn.sock` relay for `init-kernel sandboxed-upload-pack` is not built, so the fetch uses plain `git-upload-pack`. The Root's `sandboxed-upload-pack` is also left, and `kgit::fetch_agent` expects it. | K1 (WP4) + R (WP21) | WP4 N, L; WP8b D2; WP21 L; WP23 Done |
| 3 | **L1 mechanics** | G4 inputs from R4 records are missing: admission (G-40), eligibility and capability set A (G-41/42), repro tasks. Selection is L0 only. `task submit` admission, `budget set`, `model set` and `vendor` steps 3–8 return `E_INTERNAL`. `evo::predict` is not built. | K1/K2 (WP8b, WP7b, WP7) | WP8b D9, L; WP7b D8, L; WP7 N |
| 4 | **L2 mechanics** | Fleet member lanes are missing; `fleet spawn` gives its refusals, then "not built" (SV-40…43). SV-29 (kept sets with a binary handoff) is not built, and neither are the share bound §3.13.5 and RT-L8. | K1 (WP4/WP7b), R (WP25) | WP4 L; WP7b L; WP25 D9 |
| 5 | **L3 mechanics** | K0–K3 and `kernel promote` are "not built". `system.swap` answers `root.unknown_cmd` at L3+. The `Swapping` slot, kgit fetch/update/rollback in the swap, `sealed.family_eval` and kernel upgrades are all missing. | K1, R (WP21, WP24, WP25) | WP4 L; WP21 D5, D13, L; WP23 N; WP25 Done |
| 6 | **Root `proposal.file`, `sealed.*` on the control link** | **Partly closed** by FIX5 (54fb956). `proposal.file` is built on the real Root: it is stored, recorded, alerted, never applied and rate-limited (`tests/fix5.rs`). Still open: `sealed.family_eval` (L3+) and `sealed.holdout_rotate` answer `root.unknown_cmd` or `root.level`. A real-kernel `initctl kernel propose` end to end is not built; it needs a cold seed install (FIX5 D7). The frame shapes are pinned by `init-proto`'s golden test. | R (WP21, WP24) | WP21 D13; WP7b Done; WP3 Done; FIX5 Done 1, D7, L |
| 7 | **Linux sandbox (WP6b)** | No bwrap, Landlock or seccomp renderer, and no V6. `zone::launcher` errors on Linux, and sealed runs are unconfined there. The S-L rows, IC-12 and T-L1 are open. | WP6b (M7) | WP6a L; WP21 N; WP24 D7; WP7 L; WP11 L |
| 8 | **Startup hashing** | **Closed for the Root** by FIX4 (d84a321): the dev profile builds `sha2` and its hashing crates at `opt-level = 3`, and a debug Root's start → ready is now 1.5–1.7 s (9.8 s in WP21b's run, about 25 s in S1). **Off the kernel's start-up path** since FIX6 (80afa41): the exe hash (2.7 s idle) runs on its own thread, and only `promote`, `gate`, `rollback` and `kernel status` wait for it. Still slow in debug: `init-fakeroot` hashes its `--kernel` in about 10 s (WP15 D3), now inside FIX6's 60 s `owner.sock` wait. | orchestrator (optional: the same `[profile.dev.package.*]` lines in `kernel/Cargo.toml`, §3.12) | S1 gap 4; WP21b Done, N; WP7b D9; FIX4 Done 2; WP15 D3; FIX6 N |
| 9 | **Signal refresh** | The level and ceiling come from `boot.json` at boot only. The refresh from `initctl status` (every 10 min, every 20 batches, after a kernel notice) and the `SignalCx` update on `level` notices are not built. Awareness notice texts and T-AW-9/10/11 are open. | B2 (WP13) | WP13 L |
| 10 | **Ext host in body boot** | **Closed** by FIX3 (0a5f043). Boot starts the real Wasmtime host, and `init-body ext` runs it. `t_aw_5` is fixed: the root cause was macOS's first-exec check on a fresh executable. An A2 core-path e2e calls the real `count-lines` through the `ext` tool with the prefix unchanged. The seed suite is green twice (unit 269, boot e2e 16, `self_md` 8). | — | FIX3 |
| 11 | **Install path end to end** | **Mostly closed** by FIX4 (d84a321). `s1.sh` runs every mode on the real install path (seed bundle → `init-root install --seed-source` → install-mode kernel), with no hand-written `lkg.json` and no `git archive` into `body/`. `fakebody` and `mock` are green. Still open: the `watchdog` and `realdry` cases on the install path, held for disk (each cold case needs about 7.1 GB free to keep 5.3 GB at its peak); install at first start for seedless homes; `install --upgrade`. v0.3: `scripts/install.sh` → `init` → one mock turn → `scripts/uninstall.sh` passes end to end at $0 (REL 48bb3ba, `build/status/rel-e2e.json`); `install --upgrade` is still not built. | X (FIX4) + R (WP21b) + REL | WP21b L, D7, N; S1 gaps 1–2; FIX4 L, N |
| 12 | **D14 floor (FIX2)** | **Closed** by FIX2: the divergence-only floor (35647a8) and the holdout rule `GAIN_MIN_HOLDOUT = 4` (194ba30). FIX2.md is complete. FIX5 re-ran the real-judge gog on 194ba30 and it passed (2f35667). The docs record D14 (986669f, v03-decisions D14). G-21 proper is unchanged bit for bit. | — | FIX2; FIX5 Tests; BUILD_LOG 06:36 |
| 13 | **gate judge breadth** | Missing: sha candidates (home-copy `body.git`), cargo trees (real `g0::run`, selfmap G1), and G2/G3 in judge. The `test-weaken-canary`/`-reject-all`/`-score-skew` kernel features for MS-G1…G3 on the real binary are not built, and neither is the `gate simulate` CLI. Done since v0.1: FIX5 removed the `-c gate.g4.min_pool=4 -c gate.g4.min_holdout=1` overrides, so the judge runs at the gate's defaults. | K1 (WP8c), R (WP24) | WP8c L, N |
| 14 | **G4 extras** | Missing: the budget pre-flight, the disk and off-peak waits, result reuse within 3,600 s, the `noise_spike`/`task_drift` alerts, and the owner `bless`/`reject` records (G-44). F15 and F10 are untested. Also missing: the deps-cache fill and the 30-day expiry. | K1 (WP8b) | WP8b L |
| 15 | **Proxy rest** | Missing: the G3 `scripted` routing hook, the balance poll, WebSocket, `traces.retention_days` and the `init-kernel traces` CLI. P-11, P-12 and P-17 are open, as is the demotion-vs-open-gate problem (§1.8). | X (WP5) | WP5 L, D11; WP8b N |
| 16 | **Context renderers** | The Anthropic Messages and OpenAI Responses layouts and the B1–B3 breakpoints are not in `context::render`, and neither is the §3.10 call-id sanitizing. | B (WP12/WP10) | WP12 L; WP10 N |
| 17 | **Mock strict checks** | Anthropic and OpenAI are render-only. | T (WP2) | WP2 L; WP10 L |
| 18 | **Root R1 rest** | Missing: the disk pause while running, the CPU, disk and INIT_HOME quotas, and store pruning. `status --selftest` (WP6a's `selftest::run`) is not wired. The `mode`/`ceilings` events and the `root.spawn.*`/`root.rollback.*` fault points are missing. | R (WP21) | WP21 L; WP6a N; WP25 L |
| 19 | **Root R7 rest** | `ledger --verify` (RR-23), the share bound and RT-L8. | R (WP25) | WP25 L |
| 20 | **Sealed R6 rest** | Missing: a real S (M7), `family_eval`, scheduled and `audit_required` audits (SA-6/7, SE-33), the audit after a swap (SA-8), holdout rotation, bisection, sealed instances in the broker, cumulative reserve accounting and a sealed `start` line. The stand-in S is now 12 tasks plus a canary (FIX5); its ≥ 12 floor is gap #32. v0.3: cumulative reserve accounting and the sealed `start` line are built (47c618f); sealed instances in the broker are v0.1.x (§1.5 WP24 D7). | X (WP24), WP22 | WP24 L, N |
| 21 | **Supervise rest** | Missing: the selfmap served-view rebuild (§3.16) and the embedded kernel component table; `doctor`, `upstream sync` and `install --template`. SV-13 at the handoff and commit points, SV-23 on a small volume, and a t1 test with real wasm components are open. | K1 (WP4/WP7b), K2 (WP9) | WP4 L; WP9 N; WP7b L |
| 22 | **Config loader** | config.md's full cross-section schema and the `ProxyConfig` embedding are not built. | K1 (unassigned) | WP4 D5; WP5 N |
| 23 | **Body facts from the kernel** | `boot.json.env.root` lacks `cap`/`supported_max_level`. Since FIX3 (0a5f043) the body fills the cap from `INIT_HOME/root/ledger.json` or `root/charter.json`, or leaves it out, so the line never prints `L?`. That fallback needs body read access to `INIT_HOME/root/` (X2). The kernel still sends neither field. | K1 + R/X (profile) | WP13 N; FIX3 Done 3, N |
| 24 | **Early initctl** | **Closed** by FIX6 (80afa41). `run/kernel.sock` accepts about 10 ms after the kernel's spawn (it used to be 2 s to more than 60 s after the body was ready, WP15 D8), answers `E_BUSY{reason:"starting"}` until the server adopts it, and serves `ping` at about +110 ms, before the body is ready (−125 and −265 ms on two runs). The `init-kernel` e2e went from about 27 s to 12.9 s. The docs half is open (§1.6, §1.7 FIX6 rows). | — | FIX1 N; WP15 D8, N; FIX6 |
| 25 | **Measurements** | Not done: WP0m (M-1/M-2 JSON on `semver-sort`), M-3, M-4/M-5, the log-context RSS run, S-12 in release, RR-16 in release, and H16 on CI hardware. WP16 (T, in progress since 06:22) runs M-1/2/4/5, and M-3 waits for a budget; an untracked `init-testkit/src/bin/init-measure/` is in the working tree. v0.3: WP16 measured M-1, M-2 and M-4 (pass) and M-5 (fb5a5be); M-5 passes 3/3 under the provisional v0.1 rule (M5 3b02364, `measurements/m5-v0.1.json`). M-3 is still not run. | WP0m, WP16, M5 | WP0 L; WP3 L; WP9 L; WP11 L; WP12 L; WP14 L; WP23 L |
| 26 | **CI** | `.github/workflows/ci.yml` and the fmt/clippy tiers are missing. `rootloc` is not in `ci/local.sh`. The constitution drift check is in (283e8a1). The root `wp24` step now takes about 4.2 min, because S grew to 12 tasks (FIX5). v0.3: `.github/workflows/fast-linux.yml` is committed (REL b461f1f, baab414) and has not run on GitHub yet; `ci/local.sh` gained the `seed G0` (3610c17), `promote under Seatbelt` (900feef) and `tui` (367542b) steps; `rootloc` is still not built. | WP17, REL | WP0 L; WP20 N; FIX5 N |
| 27 | **Acceptance A1–A6, NO-OWNER-1** | **Closed for the $0 scope** by WP15 (9e4c2a1). A1, A2, A3, A4, A5-fake, A6 (live half, on the real Root) and NO-OWNER-1 pass: 7 tests, green on two runs. Still open: A5's Root half through `start_root`, A5-halt and A5-real (disposable env, #29); A6 steps 5–6 (G0 of the real seed; disk) and step 10 (`kill -STOP` of the Root, which the owner rule forbids here); A2b (keep the extension and promote t1; disk); the real-model variants (budget, §3.17). | R (WP15) | WP15 Left |
| 28 | **Live provider checks** | S1 made one real call. V-DS-3 (flatten), E2E-1/2/3 and T-PROV-1…6 are open, and provider.md item 4(i) needs the verbatim API example; the current one was reconstructed offline. | S1/WP10 | WP10 L, D9; WP12 L |
| 29 | **Disposable-env half** | See §4.3. It needs the sentinel account. WP15 adds A5's Root half via `start_root`, A5-halt, A5-real and A6 step 10. v0.3: not required for v0.1, which runs in owner-machine mode (ADR-008; R-SAFE 27dd505): signals only to held pids, `live-sweep` compiled out, the sweep dry-run over the live table. Still required before `live-sweep` is ever enabled (ADR-008 "Plan to enable live-sweep later"). | owner/integrator | WP3 N; WP21 N; WP6a L; WP15 Left |
| 30 | **`init-kernel install --template`** | Not built (supervise.md §4.8). WP15 builds template homes in every test process (about 200 MB, left in `target/t15/` until removed by exact path), and every Slice-1 cold case costs about 3.5 min and 1.7 GB. Inside a test home the dev seed's `../vendor` does not resolve, so A2's agent passes the vendor path to cargo. | K1 (WP4) + R (WP21b) | WP15 D1, D9, N; FIX4 L |
| 31 | **Testkit runner** | **Partly closed** by FIX6 (80afa41): a `wait_socket` step and a 60 s `owner.sock` wait. Still open: `check_g4` takes the last G4 job in `read_dir` order and compares `lkg.json.sha` with the record's top-level `sha` instead of `args.sha`, so its accept branch is wrong and untested. The fake Root writes no `kernel/CONSTITUTION.md`. | T (WP3) | WP15 D3, N; FIX6 Done 4 |
| 32 | **S size floor for D14** | **Closed** in v0.3 by R-DEC (518e68e, provisional decision §3.15): `sealed import` refuses an S below 12 tasks plus a canary, and a gog on a smaller S records `mutation{pass:null, underpowered:true}` with an alert and no demotion (wp24 `small_s…`). Before: nothing kept S at 12 tasks or more. A leak drop (SE-02) to 11 makes the gog's holdout 3, C1 becomes `inconclusive` and every gog fails. No test runs the real judge on a reduced S. | X (WP24) | FIX5 D5; §3.15 |
| 33 | **Promote tests red under the holdout rule** | **Closed** by FIX7 (37aef0c): the three fixtures use a 4-task holdout and assert `reason=better`. Before: **in progress (FIX7).** Three `kernel-supervise/tests/promote.rs` tests fail: `g31_a3_full_t2_path_to_lkg`, `g46_lineage_down_at_accept_then_lkg` and `sv26_t1_data_only_promotion_without_a_handoff`. Their G4 now ends `inconclusive, reason:"underpowered:holdout"`, because their fixtures use a 1-task holdout and FIX2's `GAIN_MIN_HOLDOUT = 4` (194ba30) applies. The rule is right. The fixtures need a holdout of 4 or more, or an `inconclusive` expectation. FIX6 found it; FIX6 did not cause it. | FIX7 (K1) | FIX6 Tests, N |

**Needs from others that were closed tonight.** For each need, the WP that asked → the WP that delivered:

| Need | Resolution |
|---|---|
| Backend + serve wired into `init-kernel`. Data-only promotions. `init-kernel selfmap` dispatch. | WP7, WP8b, WP9, WP4 → WP7b |
| scope/evo/g0/g1 wired into promote. | WP8 → WP8b |
| `init-kernel gate judge` for the gog. | WP24 → WP8c |
| Proxy start/levels/ceilings in supervise. Epoch guard header. | WP5 → WP4; WP22 → WP5 |
| `paths.egress_sock` in `hello`. panic/ladder race. sandbox-profiles linked. install-mode spawn, `b0`, early `ready`. | WP22, WP6a, WP4 → WP21, WP21b |
| `ceiling_hit` cap, demotion. `audit`/`mutation` records. | WP22 → WP23, WP25; WP25 → WP24 |
| Real kernel under the real Root. One Slice-1 run through the real egress. | WP21, WP5 → S1 |
| S1 gap 3 (`run/kernel.sock`). S1 gaps 1–2 (Root side, then the harness). Gap 4 (Root start hashing). | S1 → FIX1, WP21b, FIX4 (d84a321) |
| Loop wiring, `--manifest`, WP13 boot commit, provider client/summarizer, fallback warnings, golden SSE copies. | WP11, WP9, WP21b, WP10, WP2 → WP13 (1ca38e4), WP11 (5454e29), WP10 (09f0ece) |
| ci/local.sh: root tests with `test-fixtures` and `INIT_MOCKLLM_BIN`. | WP20, WP24 → WP22 |
| The real extension host in boot, `init-body ext`, `t_aw_5` (§2 #10). | WP13, WP14 → FIX3 (0a5f043) |
| `proposal.file` on the real Root (§2 #6, first half). | WP7b, WP3 → FIX5 (54fb956) |
| A stand-in S of 8 tasks or more with a holdout of at least 4, and a real-judge gog at the gate's defaults (WP8c option c). | WP8c, FIX2 → FIX5 (54fb956, re-run 2f35667) |
| D14 and the holdout rule recorded in gate.md, sealed-eval.md, testing.md and v03-decisions (FIX2 N, FIX5 N); every v0.1 adopt row. | FIX2, FIX5, this ledger → docs-sync (986669f) |
| A constitution drift check in `ci/local.sh` (§1.2). | WP13 → 283e8a1 |
| Charter fixtures owned by the acceptance lane (WP3 N). | WP3 → WP15 (9e4c2a1) |
| An early `run/kernel.sock`, a runner socket wait and a longer `owner.sock` wait (§2 #24, #31 in part). | FIX1, WP15 → FIX6 (80afa41) |
| Still open, low priority: switch the proxy tests to `init-fakeroot` (WP3 N); expose the task id to the summarizer (WP11 N, optional); the `wp11-check` copy's relative vendor path, which causes rebuild thrash (WP14 N). | — |

---

## 3. Owner decisions

**Provisional owner decisions, 2026-10-20 12:16.** The owner said not to wait ("chủ động làm đi đừng chờ tôi"), so the orchestrator took the decisions below for v0.1.0 (RELEASE_PLAN.md). Each one is a **provisional owner decision 2026-10-20 (owner may override)**, and every doc that carries one says so. The same table is in CONVENTIONS §6.

| Decision | Taken as | Docs that carry it |
|---|---|---|
| §3.1 Root size | Budget ≈ 7,500: per-module sub-budgets of measured + 5%, rounded up to 10, re-measured at release. Core (zone, sup, panic, egress money, lineage verify) is counted separately, and `root/sandbox-profiles` has its own lines. At cdb49e8: 7,085 measured (table total 7,540), core 1,656. At dd5c358 the figures were 7,053 and 1,650. RELEASE_PLAN first said ≈ 7,000, from WP24's ≈ 6,450 count. | spec §13, root.md §4.8 and RR-34, CONVENTIONS §4, roadmap R13 |
| §3.2 G4 power | n = 30, R = 3, plus one A/A calibration per model, as the default once S has ≥ 30 tasks (M7). v0.1 keeps the 12-task stand-in S. | gate.md §3.8.3 and §3.8.5, roadmap M7 |
| §3.4 Constitution | Built as written. It stays PENDING OWNER, and the owner should still read it. | CONVENTIONS §6 (OW-1) |
| M-5 method (FIX8) | `phys_footprint` on macOS, baseline at the first context epoch, thresholds unchanged | testing.md M-5 (lane M5) |
| §3.9 Repeated demotion | 3 demotions at L0 within 24 h trigger system safe mode | root.md §3.13.3 (lane R-DEC) |
| §3.13 Root money meter | Bill cache hits at the provider's cache-read price; keep the peak window | egress.md (lane R-DEC) |
| §3.15 Small S | S < 12 tasks gives `underpowered`, with no demotion, plus an alert | sealed-eval.md (lane R-DEC) |
| §3.17 Real-model batch | deepseek-flash only. Hard cap $1.00 Root-metered; no new checks after $0.50 Root-metered. | lane S-REAL |
| Disposable account (§3.14) | Not required for v0.1, under owner-machine mode (ADR-008) | ADR-008 (lane R-SAFE) |

The §3 items not in this table (§3.3, §3.5–§3.8, §3.10–§3.12, §3.16) were not taken on 2026-10-20. Each keeps its recommendation as the provisional default and stays open for the owner.

**As built (v0.3).** §3.13 in cdb49e8, and §3.9 and §3.15 in 518e68e (R-DEC; docs d524407). The M-5 method in 3b02364 (M5: 3 of 3 trials pass). The disposable-account row as owner-machine mode in 27dd505 and 7a460a6 (R-SAFE; ADR-008). §3.17's batch is S-REAL's, still running. §3.1 and §3.2 are doc-only for v0.1 (50c553e, 781504b); the Root has not been re-measured since cdb49e8.

### 3.1 Root line budget (WP20 D1, WP21, WP22 D1, WP23 D13, WP24 D12, WP25, WP6a D1)
- **Taken provisionally 2026-10-20 (owner may override):** option (a). The budget is ≈ 7,500, the sum of root.md §4.8's per-module sub-budgets: measured + 5%, rounded up to 10, re-measured at release. The core subtotal is 1,656 measured against a budget of 1,770. `root/sandbox-profiles` is counted separately: 821 lines of profile code (budget 870) and a 1,454-line self-test runner (budget 1,530). The count uses root.md §4.8's method. It gives 7,053 for `init-root` at dd5c358 and 7,085 at cdb49e8, where the table totals 7,540.
- **Facts.**
  - The docs set ≤ 1,500 lines with the core ≤ 995 (root.md §4.8, RR-34; spec §13). Sub-budgets: egress 330, R6 175, lineage 130.
  - The measured count is ≈ 6,450 at WP24. WP21b's install path and FIX5's `proposal.file` are not counted, and nothing has re-measured since WP24, because `rootloc` is not built (§2 #26). The orchestrator raised the budget to ≈ 5,000 provisionally at 02:35.
  - 986669f records ≈ 6,450 as "under owner review" in spec §13, root.md §4.8/RR-34 and roadmap M6/R13, with no new number.
  - `root/sandbox-profiles` (validation + composition) sits outside the count.
  - No WP cut a specified feature.
- **Options.**
  - (a) Raise the budget to the measured size plus headroom, and add a per-module table.
  - (b) Cut scope back toward 1,500.
  - (c) Keep 1,500 as a long-term target and accept the overrun for Phase 0.
- **Recommendation: (a).** Set ≈ 7,000 total, with per-module sub-budgets taken from the measured sizes. Keep a separately counted "core" sub-total (zone, sup, panic, egress money, lineage verify) for the owner's hand review, and count `root/sandbox-profiles` on its own line.
- **Why.** The 1,500 estimate predates v0.3's sealed evaluation, D1–D13 and the money layer. Every line maps to a doc section. Splitting into helper binaries would not shrink the trusted code, because the helpers would still run as the owner, outside every sandbox.
- **Quality guard.** `rootloc` per module in `ci/local.sh` (WP17) fails on growth. `unsafe` stays confined to `sys.rs`. All Root tests stay green (103 with `test-fixtures` since FIX5).
- **Cost.** $0. Review time grows.

### 3.2 G4 statistical power (WP8 Tests G-21, N)
- **Taken provisionally 2026-10-20 (owner may override):** options (c) and (a).
  - From M7, once S holds ≥ 30 tasks plus the canary, the default is n = 30 as 18 + 12 (minimums 18 / 12), R = 3 and `repeats_max` 4.
  - Each model gets one A/A calibration before its first real gate.
  - v0.1.0 keeps today's defaults and the 12-task stand-in S, so the gog's 8 + 4 needs no `-c`.
  - gate.md §3.8.3 and §3.8.5; roadmap M7.
- **Facts.** Accept rate at 0 / at −2δ, from 2,000 gates per cell:
  - n = 16, R = 2: 73.9% / 11.6% on a first gate, and 83.8% / 8.2% at steady state.
  - n = 30, R = 3: 90.5% / 7.2% on a first gate, and **91.7% / 6.5%** at steady state.
  - Only the n = 30, R = 3 steady-state cell meets the targets (≥ 90% and ≤ 7%). n = 30, R = 2 still fails (9.7%), and so does n = 16, R = 3 (8.4%).
  - FIX2 confirms that D14 leaves this grid bit-identical (`g21_calibration_reduced` passes), so the decision stands as it was (BUILD_LOG 06:36).
  - FIX2's full-rule simulation (the whole accept rule, split into pool and holdout, no noise history, 2,000 gates per cell) at −2δ on every task: 7.1% at 8 + 4, R = 2 (the gate's defaults); 6.0% at 8 + 4, R = 3; 6.55% at 18 + 12, R = 3, where a smaller candidate at a true 0 is accepted 89.15% of the time. That is close to the G-21 targets with no history, which supports (c).
- **Options (WP8).**
  - (a) Treat the first gate on a model as inconclusive, or run an A/A calibration job first.
  - (b) Raise `margin_factor` or the δ quantile at small n.
  - (c) Raise `min_pool`/`min_holdout` toward n ≈ 30 with R = 3.
  - (d) Relax the G-21 targets to the measured n = 16 values, which lowers rigor.
- **Recommendation: (c) together with (a).** Default to n = 30, R = 3, and run an A/A calibration so that `noise.json` history exists before the first real gate on a model. Option (b) alone moves the accept rate at 0 and at −2δ in the same direction, so it does not fix the δ̂ noise.
- **Knock-on for the gog.** Since FIX5 the gog runs the real judge at the gate's defaults (8 + 4 on the 12-task S). If the defaults rise toward n = 30, the stand-in S is too small. Until the real S lands (M7), let the gog pass `-c gate.g4.min_pool=8 -c gate.g4.min_holdout=4` and state in the gog record that it tests the 8 + 4 shape. Then size the real S at 30 tasks or more plus the canary, so the gog tests the production gate.
- **Cost.** G4 runs per arm grow from 16×2 = 32 to 30×3 = 90, about 2.8× per gate, plus one A/A job per model. The dollar cost per run is not measured yet, because benchmarks wait until the build is complete (BUILD_PLAN). The futility look at ⌈n/2⌉ (built, WP8/WP8b) stops clear losers early.
- **Quality guard.** The G-21 targets stay as written. D14 does not change G-21: FIX2 asserts that `margin` and `bootstrap_ni` are bit-identical, so WP8's grid stands.

### 3.3 D14: half-task noise floor plus the holdout rule (final; FIX2, FIX5; recorded in 986669f)
- **Facts.**
  - The problem: with a deterministic model δ = 0, so the guard `d_pool − d_holdout > 2·√(δ_p² + δ_h²)` rejected C1 on 4 of 5 holdout draws. sealed-eval.md §3.3 requires a correct gate to accept C1.
  - **Part 1, the floor (35647a8).** The divergence guard only uses δ' = max(δ, 0.5/n<sub>set</sub>) per split. The split guards keep the measured δ, and the NI margin keeps max(2δ, 0.5/n). A floor on every guard was measured and rejected: at 4 + 1 it raised false accepts at −2δ from 4.3% to 12.9%, and a holdout-only loss from 0% to 7.6% (FIX2 table A).
  - **Part 2, the holdout rule (194ba30).** `accept::GAIN_MIN_HOLDOUT = 4`, a constant: a score-gain accept on a holdout of fewer than 4 tasks becomes `inconclusive: underpowered:holdout`. It is needed because with the floor alone a 1-task holdout can never trip the guard (δ_h' = 0.5, threshold ≥ 1.0), and the pool-only cheat (the MS-5 signature) passed as `better` 30.5–33.3% of the time.
  - **Full-rule simulation** (FIX2 table B; 10 shapes × 3 false-accept scenarios = 30 cells, 2,000 gates each, no noise history): D14 equals pre-D14 in **28 of 30** false-accept cells. The other two are 4 + 4 at R = 2, +0.15 points (3 of 2,000 gates), a shape below `min_pool` 8 that no production gate or gog draws. The **pool-only cheat is accepted 0%** at every size. A true gain at holdout ≥ 4 is accepted 99–100% (80–100% before). At a 1-task holdout every true gain is inconclusive, which is the rule's intent.
  - **Gog** (FIX5): the stand-in S has 12 tasks plus a canary, so the gog draws 8 pool + 4 holdout at the gate's defaults with no `-c`. The divergence threshold is ≈ 0.28, so the guard is live. The real judge gives `pass:true`, 0 disagreements, M1–M7 rejected and C1–C3 accepted (94 s on 35647a8, 99 s on 194ba30, $0). C1 is accepted and M3 rejected on all 495 possible holdouts.
  - **S must keep at least 12 tasks.** With 11 or fewer the holdout is 3 or fewer, C1 becomes `inconclusive` and the gog fails (FIX5 D5). Nothing enforces this yet (§2 #32, §3.15).
  - The rule also turned three `kernel-supervise` promote tests red (g31, g46, sv26). Their fixtures use a 1-task holdout, and FIX7 is moving them to 4 tasks (§2 #33, in progress).
  - G-21 proper is unchanged bit for bit. The docs carry D14 in gate.md §3.8.3, §3.9, §4.1 and §8, sealed-eval.md §3.3, testing.md and v03-decisions D14 (986669f).
- **Recommendation.**
  - Ratify D14 as built, both parts. The docs already say it, so ratifying it means reading v03-decisions D14.
  - Keep `GAIN_MIN_HOLDOUT` a constant, so no config can tune it away.
  - Make the S ≥ 12 floor mechanical (§3.15).
- **Quality guard.** 28 of 30 false-accept cells are unchanged, and the two that moved are below the gate's minimums. The pool-only cheat is at 0% everywhere. MS-G1…G3 and SE-18 still fail as they should. G-21 is bit-identical. The real-judge gog passes on 8 + 4.
- **Cost.** $0. The root `wp24` suite grew from 171 s to 252 s.

### 3.4 Constitution text (OW-1; WP13 D4, BUILD_PLAN checklist)
- **Taken provisionally 2026-10-20 (owner may override):** built as written. OW-1 stays PENDING OWNER, and the owner should still read the text.
- PENDING OWNER. Three byte-identical copies are in use (§1.2). CONVENTIONS §6 recommends "approve as written".
- **Recommendation:** read it and approve, or edit it. The canonical file is named (CONVENTIONS §2, 986669f: the Root asset) and the drift check exists (283e8a1), so an edit is one file plus the two copies.

### 3.5 Mirror URL in the charter template (WP23 D10, WP25 D11)
- The orchestrator set `url = ""` until M0. root.md §3.11 and §3.12.1–2 now show `""` with the GitHub URL as a comment and point here for ratification (986669f).
- **Recommendation:** ratify it, and switch to `git@github.com:nagisanzenin/INIT-Lineage.git` when the repo is created at M0.

### 3.6 Seed repository release process (WP21b N, FIX4 Done 1)
- FIX4 already moved the packaging step into `scripts/slice1/seedpack.py` (`--kind repo` at a git revision, `--kind fakebody`), and both kinds get the same vendor cut.
- **Recommendation:** promote `seedpack.py --kind repo` to a maintainer release script (for example `scripts/release/seed.sh`) that produces branch `seed`, a `seed-v<x.y.z>` tag and a cut `vendor/` from a clean committed revision, with platform-only crates allowed as stubs. Keep `--kind fakebody` in `scripts/slice1/`.
- **Quality guard (WP21b Tests):** `cargo tree --frozen` must show 0 stubbed crates in the host build graph. The stubs are per target, so a Linux seed (WP6b) must not stub `linux-raw-sys`.

### 3.7 Batch thinning rule (WP8 N)
- **Recommendation:** one `Compensates-for` per `Thins` entry. This keeps batch thins possible. Allowing only same-tag batches would be stricter, but it makes batches rare.

### 3.8 Charter Purpose block in the head (WP12 N)
- **Recommendation:** include it, as awareness.md §3.3 says, and list it in log-context §3.8.2. It is owner text that changes rarely, so it costs little in cached tokens.

### 3.9 Repeated demotion at L0 (WP25 D6)
- **Taken provisionally 2026-10-20 (owner may override):** 3 demotions at L0 within 24 h trigger system safe mode (lane R-DEC builds it and edits root.md).
- **Recommendation:** after N demotions at L0 within a window (for example 3 in 24 h), enter system safe mode, reusing the crash-loop escalation path. Without this, a kernel can drive a rollback/restart loop.

### 3.10 `owner_signal` vs `owner` lineage kind (WP23 D6)
- **Recommendation:** use root.md's `owner{qid}`, so the Root can verify the relayed line (RR-18c). supervise.md §4.7 should emit that kind.

### 3.11 Proxy cost reconciliation (WP5 L)
- **Recommendation:** drop `cost_mismatch`/`root_cost_musd` from proxy.md for Phase 0. The Root's ceilings are authoritative, and the proxy's guard already reads Root figures (WP5 D2). Revisit at L2.

### 3.12 Build-profile items for the orchestrator (WP21b N, WP9 N, WP0 N, WP23 D3)
- The items:
  - ~~`[profile.dev.package.sha2] opt-level = 3` in `root/Cargo.toml`~~ **done** by FIX4 (d84a321), with `digest`, `block-buffer`, `crypto-common`, `hybrid-array` and `cpufeatures`. Debug start → ready went from 9.8 s to 1.7 s;
  - **new:** the same six `[profile.dev.package.*]` lines in `kernel/Cargo.toml`. `init-fakeroot` spends about 10 s hashing the 154 MB debug kernel (WP15 D3), and the kernel's own exe hash takes 2.7 s (about 0.3 s optimized, FIX6 N). FIX6 took that hash off the start-up path, so this is now a test-speed item only;
  - `[profile.dev.package]` opt-level for wasmtime and cranelift in `kernel/Cargo.toml`;
  - a `fault-injection` feature in `init-root`;
  - later, a `kernel/wit` for L3.
- **Recommendation:** do the kernel sha2 lines, the wasmtime/cranelift opt-level and `fault-injection` now, in one kernel rebuild. They affect debug builds only, and release behaviour is unchanged.
- **Guard:** the same tests stay green. The cost is one rebuild of each workspace's target, so watch the disk floor.

### 3.13 Root money meter (not flagged in a status file; from S1's numbers)
- **Taken provisionally 2026-10-20 (owner may override):** the Root bills cache hits at the provider's cache-read price and keeps the peak window (lane R-DEC builds it and edits egress.md).
- **Facts.** The Root billed 2,844 µUSD against the proxy's 762 (3.7×), because it uses peak list prices and bills cache hits at the input price. At this traffic shape, the $5/day template ceiling would stop at roughly $1.35 of proxy-priced spend. FIX4's mock run on the install path gives the same shape: 7,281 against 1,890 µUSD (3.85×).
- **Recommendation:** have the Root bill cache hits at the provider's cache-read price, while keeping the peak window. The Root stays an upper bound. Re-check the ratio on the first benchmark run.

### 3.14 Disposable environment (WP3 N, WP21 N, WP4 Tests, WP6a L)
- **Taken provisionally 2026-10-20 (owner may override):** v0.1.0 does not require a disposable account. It runs in owner-machine mode: signals go only to held PIDs, live-sweep is compiled out, and a sweep dry-run is done over the real process table (ADR-008, lane R-SAFE). The disposable-account tests below stay required before live-sweep is ever enabled.
- **Recommendation:** a separate macOS user account, which costs $0, with the root-owned sentinel `/var/db/init-test-disposable` containing that account's uid. That is cheaper than a VM, and it is enough for SV-16, RR-6/7/11–13, RT-P2/P5 (`live-sweep`), S-M40/41/47a/47b and S-Z17a–f. WP15 adds A5's Root half through `start_root`, A5-halt, A5-real and A6 step 10 (`kill -STOP` of the Root).

### 3.15 Stand-in S below 12 tasks (FIX5 D5; new)
- **Taken provisionally 2026-10-20 (owner may override):** option (b). S < 12 tasks gives `underpowered`, with no demotion, plus an alert (lane R-DEC builds it and edits sealed-eval.md).
- **Facts.**
  - Under D14 the gog needs a holdout of at least 4, which the split gives only when S has 12 tasks or more.
  - With 11 or fewer, for example after a leak drop (SE-02), C1 becomes `inconclusive` and the gog reports `pass:false`, which reads as a broken gate.
  - R7 currently treats every `mutation{pass:false}` as a demotion trigger (§1.3, WP24 N).
  - No test runs the real judge on a reduced S.
- **Options.**
  - (a) Leave it. The gog fails closed, but for the wrong reason, and it may demote.
  - (b) Refuse an S of fewer than 12 tasks plus the canary at `sealed import`. If a leak drop takes S below 12, record the gog as `underpowered` rather than `pass:false`, do not demote, and alert the owner to add tasks.
  - (c) Change the split so the holdout always has at least 4 tasks. On a small S that leaves the pool under `min_pool` 8, so the gate answers `insufficient` instead.
- **Recommendation: (b).** A too-small S is a measurement gap, not evidence that the gate is broken. (b) states that, and it keeps "the gog passed" meaning the 8 + 4 test ran.
- **Quality guard.** Add one wp24 test: an 11-task S gives `underpowered` plus an alert and no `ladder` record. SE-02 then re-runs on a 13-task S. MS-G1…G3 must still fail.
- **Cost.** $0, plus about 100 s of wp24 time.

### 3.16 Disk for the remaining $0 runs (FIX4 N, WP15 Left, BUILD_LOG ops; new)
- **Facts.**
  - Free disk tonight was between 6.1 and 8.4 GB, against the owner's 20 GB floor.
  - A cold seed install peaks at about 1.7 GB, so each Slice-1 case needs about 7.1 GB free to keep the harness's 5.3 GB floor.
  - Held for disk: the `watchdog` and `realdry` install-path cases; WP15's A6 steps 5–6 and A2b (a G0 build of the real seed); S-12 and RR-16 in release; the full kernel workspace build.
- **Recommendation.** Before the next session, the owner frees disk to the 20 GB floor; choosing what to move to the Trash is the owner's call. Then run `tests/slice1/run.sh` (its four cases run one at a time and stop at the first floor breach), then the WP15 items. Build `install --template` next (§2 #30), so each later case is an APFS clone of a warm home instead of a 1.7 GB cold build.
- **Quality guard.** The harness keeps its disk guards (6,500 MB to start, 5,300 MB floor, a 2 s sampler), and cleanup stays by exact path.

### 3.17 Live-model budget for the remaining provider checks (WP15 Left, WP10 L, WP12 L, WP13 L; new)
- **Taken provisionally 2026-10-20 (owner may override):** one batch on deepseek-flash only, with a hard cap of $1.00 Root-metered and no new checks after $0.50 Root-metered. This replaces the recommendation's $0.20 stop. Lane S-REAL runs it.
- **Facts.**
  - Tonight's real-model use was one S1 task on deepseek-flash. It cost under $0.01 (Root-metered 2,844 µUSD) inside a $1 cap.
  - Still open: V-DS-1…6 (V-DS-3, the flatten, first), E2E-1/2/3, T-PROV-1…6, live T-AW-9/10, and the real-model variants of A1–A6.
- **Recommendation.** Authorize one batch on deepseek-flash only, with a $1 hard cap and a stop at $0.20, under S1's guards (`S1_I_MEAN_IT=1`, `S1_SPEND_MAX_MUSD`, the key never echoed and removed on every exit). Order: V-DS-3, then A1-real, then E2E-1, then the rest. At S1's rate the expected spend is well under $0.20.
- **Quality guard.** Each check keeps its $0 mock twin as the regression test, and the run records Root and proxy cost per check (which also re-checks §3.13's ratio).

---

## 4. Test inventory

### 4.1 Tests reported green, by crate (latest status file for each)

| Workspace | Crate | Green | Notes | Source |
|---|---|---|---|---|
| kernel | `init-proto` + `initctl` | **60** | proto 35 unit + 6 golden (IC-1: 46 doc examples byte-equal); initctl 6 unit + 13 cli; 3× with no flakes | WP1 |
| kernel | `init-testkit` | **107** + **7** acceptance | 105 at WP3 (5× with no flakes, about 10 s a run; the mock alone was 70 at WP2), plus FIX6's 2 new `scenario` lib tests (`--lib scenario`: 5 pass). Acceptance (WP15): `tests/acceptance.rs` 6 (A1, A2, A3, A4, A5-fake, NO-OWNER-1; about 150 s) + `tests/acceptance_root.rs` 1 (A6 on the real `init-root`; about 13 s), green on two runs. FIX6 re-ran A1–A4 and A6 green. | WP3, WP2, WP15, FIX6 |
| kernel | `kernel-supervise` | **101** + 2, **3 red** | lib 66, slice1 10, handoff 8, recovery 5, process 3, probe 3, install 1, fakeroot_xcheck 1, serve_seam 1, promote 3 (of WP7b's 6); plus 2 with `fault-injection`. Red: `promote.rs` g31, g46 and sv26, which hit FIX2's holdout rule with a 1-task holdout (§2 #33, FIX7 in progress). | FIX6, WP7b |
| kernel | `kernel-proxy` | **83** | 72 unit + 11 integration. One early unexplained failure, then 20+ green runs in a row. | WP5 |
| kernel | `kernel-gate` | **202** + 2 ignored | lib **186** (D14: `d14_score_gain_needs_a_holdout_of_four`, `d14_c1_…`, `d14_delta_min_is_half_a_task`), `tests/g0.rs` **12** (real cargo 1.97.1), `tests/judge.rs` **4** (113 s, $0, real `gate judge` on FIX5's 12-task S). Ignored: `g21_full` (about 10 min) and the D14 full-rule simulation (171 s in release). | FIX2 |
| kernel | `kernel-selfmap` | **49** | 37 unit + 12 integration, about 34 s | WP9 |
| kernel | `kernel-serve` | **50** | 19 unit, 30 `ic` (driving the real `initctl` binary; FIX6 added `early_lane_answers_starting_then_the_server_adopts_it`), 1 `sandboxed_lane`. WP7 had 47, FIX1 49. | FIX6, FIX1, WP7 |
| kernel | `init-kernel` | **4** | unit 2; `e2e` 1 (real initctl → real init-kernel under init-fakeroot + mock, t0 promote to LKG; about 27 s, 12.9 s since FIX6, green twice); `judge_cli` 1 | FIX6, FIX2, WP8c |
| kernel | `kernel-sandbox` | none reported | Linux-only crate; WP6b has not started | WP0 |
| root | `init-root` | **103** (test-fixtures) | 65 unit + fix5 1 + wp20 9 + wp21 6 + wp21b 2 + wp22 8 + wp23 4 + wp24 5 (252 s, was about 171 s) + wp25 3. 62 unit without features. | FIX5 |
| root | `init-sandbox-profiles` | **17** | 10 unit, 2 doc-drift, 4 params, 1 self-test run | FIX1 |
| seed | `init-body` | **293**, 0 fail, 5 ignored | unit 269 + `tests/boot.rs` 16 + `tests/self_md.rs` 8, green on two runs back to back (load about 20). `t_aw_5` is fixed. New: `boot::ext::tests` (3), A2 core path `ext_tool_calls_kept_count_lines`, `ext_cli_on_count_lines`. Earlier breakdown by WP: log/context 61, provider 59, tools + loop 55, ext_host 21 (+1 ignored H16, run alone: ok). | FIX3, WP14, WP10–WP13 |
| seed/ext | `count-lines` | **3** | host tests; `.wasm` is 247,618 B | WP0, WP14 |
| harness | `tests/slice1/run.sh` | **2 of 4** on the install path | FIX4: `fakebody` and `mock` pass on the real install path (no workarounds). `watchdog` and `realdry` are pending disk; their artifacts are still S1's, from the old path, where the suite was 4/4. `s1.sh real` 1/1 (S1, old path). | FIX4, S1, FIX1 |

Sum of the crate rows: **1,079 green** Rust tests (plus 2 `fault-injection`, 3 red and 7 ignored). Each row is that crate's latest status file, not one simultaneous run.

**v0.3, after the release push** (the latest full run per workspace; the table above is left as it was at 06:55): kernel 716 passed, 0 failed, 5 ignored, 3 runs in a row (K-FLAKE, head 3008357); seed 300 (275 + 16 + 9), 5 ignored, 3 runs in a row (K-FLAKE; also in release mode, REL, and under the seed's own G0, G0-SEED); `init-root --features test-fixtures` 138 (87 unit + 51 integration) and `init-sandbox-profiles` 18, with the self-test at 240 / 0 / 11 of 251 (G0-ROOT); `init-tui` 56, 1 ignored (WP26); seed/ext 3. That is **1,231 green, 0 red**. Tests K-GATE and G0-SEED added after 3008357 passed in their own runs but are not in the kernel figure. docs/phase0-report.md §2.6 has the per-workspace sources.

Every status file reports `cargo clippy -- -D warnings` clean for its crates.

### 4.2 Notable measurements (Apple M2, macOS 14.6; debug builds unless noted)

| What | Value | Source |
|---|---|---|
| Clean builds (WP0 skeleton; load 15–19) | seed 116.6 s, seed/ext 32.9 s, kernel 54.6 s, root 8.5 s. Incremental `touch` rebuilds 0.19–0.48 s. Warm `ci/local.sh` about 2 s. | WP0, measurements/wp0-build-times.md |
| **M-2** warm rebuild of `count-lines` (a real recompile, `CARGO_INCREMENTAL=0`) | **median 380 ms**, max 385 ms (5 runs). C1's 5.0 s threshold is not triggered. | WP14 |
| Cranelift cold compile, `count-lines` | 55–113 ms | WP14 |
| **Instantiate** (Store + limiter + pool + drop) | **p50 31 µs, p99 154 µs** (300 runs). BUILD_LOG says 166 µs; the status file says 154. | WP14 |
| Whole `ext` call | p50 385–397 µs; p99 850–6,690 µs under parallel load | WP14 |
| H16, 10,000 calls run alone | 2.9 s, RSS growth 0 KiB | WP14 |
| Ext abort → result | 8–28 ms | WP14 |
| **`sync_data` latency** (T-LOG-9, F_FULLFSYNC) | **p50 2.81 ms**, p90 2.95, p99 3.64, max 9.40 (threshold 50 ms) | WP12 |
| Log crash durability (T-LOG-3) | 500 SIGKILLs mid-append, 0 confirmed events lost | WP12 |
| **Sandbox self-test** | **222 pass / 0 fail / 11 skipped** of 233 checks, about 8 s. WP6a had 202/0/11 of 213. | FIX1, WP6a |
| Loop timings | interrupt to idle < 100 ms; tool group dead < 200 ms; paused < 50 ms | WP11 |
| Provider | abort < 50 ms; 10k mutated streams in 0.41 s with no panic | WP10 |
| Proxy trace store | packs are 1.88% of the naive size; chain depth 64 | WP5 |
| Selfmap S-12 (seed + 3 components) | 3.6 s debug. The release target (< 2 s) is not measured. | WP9 |
| Lineage RR-16 (10,000 records, debug) | seal 6.6 s, full verify 12.3 s (≈ 1.2 ms per record), tail `audit verify` 2.5 s | WP23 |
| G-21 full grid | ≈ 10 min in release (2,000 gates per cell, B = 10,000) | WP8 |
| **D14 full-rule simulation** | 10 shapes × 5 scenarios, 2,000 gates per cell; 171 s in release (table A's first run took 616 s in debug). False accepts unchanged in 28 of 30 cells; pool-only cheat 0% | FIX2 |
| **Gog with the real gate** | 54 s on the 5-task S (WP8c); **92–99 s** on FIX5's 12-task S at 8 + 4 (the stub takes 97.6 s); `spent_musd` 106,884 (mock prices, $0) | WP8c, FIX2, FIX5 |
| Root `wp24` suite | 252 s (was about 171 s) | FIX5 |
| **Slice-1 real** (deepseek-flash) | owner line → idle in **2.39 s** (turn 1 ≈ 0.97 s, tool 7 ms, turn 2 ≈ 1.1 s); cold start → body ready 10.9 s; Root 2,844 µUSD, proxy 762 µUSD; turn 2 cache 4,480 hit / 190 miss (959‰); the key never appeared in 218 artifacts | S1 |
| **Slice-1 on the install path** (mock) | install 209.9 s (cold wasmtime build); `owner.sock` 349 ms; start → ready **1.7 s**; peak disk use 1,573 MB (build slot 963 MB); Root 7,281 µUSD, proxy 1,890 µUSD (mock prices). Fakebody: install 17.3 s, `owner.sock` 174 ms, start → ready 1.5 s, peak 213 MB. | FIX4 |
| **Root start hashing** | debug start → ready 9.8 s (WP21b) and ≈ 25 s (S1, 154 MB kernel at about 13 MB/s) → **1.5–1.7 s** with sha2 at opt-level 3 | S1, WP21b, FIX4 |
| **Kernel start-up** (real kernel under init-fakeroot, idle) | `run/kernel.sock` accepting at **+10 ms** after spawn, serving at **+110 ms**, body ready ≈ +390 ms, gate `Env` (exe hash, 2,678 ms, in the background) at +2,679 ms. Before FIX6: 2 s to more than 60 s after body ready (WP15). | FIX6, WP15 D8 |
| `init-fakeroot` hashing its `--kernel` (154 MB, debug sha2) | about 10 s | WP15 D3 |
| macOS first exec of a freshly written executable | 150–260 ms, against 3 ms on the second exec (the `t_aw_5` root cause) | FIX3 |
| **Acceptance scenarios** | A1 10–13 s; A2 57–103 s (cold wasm build); A3 42–49 s; A4 24–33 s; A5-fake 6 s; A6 13 s; NO-OWNER-1 < 1 s. After FIX6: A1 11.5 s, A2 43.8 s, A3 24.2 s, A4 19.8 s, A6 15 s. | WP15, FIX6 |
| A5-fake crash-loop restart gaps | 241 / 461 / 565 / 808 / 864 ms (backoff 50 → 400 ms plus about 250 ms of restart latency) | WP15 |
| Debug `init-kernel` size | ≈ 154 MB (it links selfmap/wasmtime) | WP7b D9 |
| Seed packaging | 131 MB (from ≈ 290 MB), 4.4 s (WP21b); 188 MB transient inside FIX4's mock install | WP21b, FIX4 |
| Tool registry | 3,405 B (fixture `tools_sha256` cross-checked with Python and shasum) | WP11, WP9 |
| Lowest free disk while working | 6.1 GiB (WP15), 6.4 GiB (FIX6), 6.5 GiB (05:52); 8.4 → 6.8 GB during FIX4's mock install | WP15, FIX6, FIX4 |

### 4.3 Not run on this machine

**Disposable-env only.** These need the sentinel plus `INIT_TEST_ENV=disposable` (§3.14).
- Sandbox: S-M40, S-M41, S-M47a, S-M47b and S-Z17a–f, which are 10 of the 11 self-test skips; RR-6, RR-7 and RR-11–RR-13 (WP6a, FIX1).
- Root: RR-6, RR-7 (live), RR-11–RR-13, and the full RT-P2 and RT-P5 with `--features live-sweep`. That feature compiles but has never run (WP21).
- Supervise: SV-16 (the panic sweep), the kill-by-sweep half of SV-46, F-hang (a), and the full F-kill-kernel chaos variant (WP4).
- Testkit: the `start_root` paths, `wp3-root-fakekernel.json` and the sweep tests. SW-6 was verified to skip (WP3).
- Acceptance: A5's Root half through `start_root`, A5-halt and A5-real (which also SIGKILLs the body), and A6 step 10 (`kill -STOP` of the Root). The owner rule allows no such kill on this machine (WP15).

**Not run for other reasons.**
- S-M38 (DNS): it would send traffic (WP6a).
- Linux only: the S-L rows, IC-12 and T-L1 (WP6a, WP7, WP11).
- Disk-limited:
  - S-12 in release (WP9);
  - RR-16 in release (WP23);
  - the full kernel workspace build in `target/t` (WP2, WP7);
  - the selfmap `--write`/`--check` build step (WP9);
  - the `watchdog` and `realdry` Slice-1 cases on the install path, about 7.1 GB free each (FIX4). WP21b's `s1-install.sh mock` run is no longer pending: FIX4 ran `mock` and `fakebody` on the install path;
  - WP15's A6 steps 5–6 (G0 refuses `unsafe` and a missing `forbid`) and A2b (keep the extension, promote t1), which both need a G0 build of the real seed;
  - free disk was 6.1–8.4 GB through the night, under the normal 20 GB floor (§3.16).
- SV-23 on a small volume, via hdiutil (WP4).
- Phase 0 measurements: M-1/M-2 JSON (WP0m), M-3, M-4, M-5 (WP11 item 21) and the log-context RSS run (WP12). WP16 is running M-1/2/4/5 (in progress; no status file yet); M-3 waits for a budget.
- macOS 15 and 26 (WP6a).
- Live provider checks beyond S1's one call: V-DS-1…6, E2E-1/2/3 and T-PROV-1…6 (WP10, WP12). T-AW-9/10 live and T-AW-11, which needs jq plus a selfmap fixture (WP13). The real-model variants of A1–A6 (WP15). All wait for a model budget (§3.17).

---

## 5. Leftovers from docs-sync

The nine docs-sync agents (986669f) applied every adopt row inside their own docs. They flagged the cross-doc items below but did not apply them, because each one touches a doc or a file outside their own area. Each item was re-checked against the tree for this ledger. None of them is counted in §1's totals.

| # | Item | Where now | Change | Tag |
|---|---|---|---|---|
| L1 | `ToolCx.cwd` is now `ToolCx.paths.cwd` (`ToolCx.paths: PathCx`, WP11 D5/D6). tools.md, loop.md and host.md were updated; two docs still say `ToolCx.cwd`. | body/boot.md §3.2 (the `cwd` bullet); ext/wit.md §3.4 (the task-workspace row) | Write `ToolCx.paths.cwd` in both places. | adopt (applied by DOCS, 2026-10-20) |
| L2 | supervise.md still has the kernel `store.put` a `kernel-src` git bundle so the Root can write `lineage/kernel.git`. root.md §3.8 has the Root fetch `source_repo` + `source_sha` itself through a sandboxed upload-pack, and supervise.md's own `system.swap` row and §3.15 step 5's `system.swap{…}` already use `source_repo`. | kernel/supervise.md §3.15 step 5 and the §9 item that adds `store.put kind:"kernel-src"` | Drop the `kernel-src` bundle and point at root.md §3.8 (phase 4 fetch). root.md owns the control link. No code exists on either side yet (L3). | adopt (applied by DOCS, 2026-10-20) |
| L3 | The `KernelConfig` struct comment says `schema_version` "must be 1", while rule 1 and every example say 2, and the code and the Root's template write 2. | kernel/config.md §3.2 (the struct block) | Change the comment to "must be 2". | adopt (applied by DOCS, 2026-10-20) |
| L4 | `lkg` is never written. Install step 6 says `refs/heads/main` and `refs/heads/lkg` of `lineage/kernel.git` point at the imported release, and §3.8's phase 7 moves `lkg`. The Root creates `main` only: `refs/heads/lkg` appears just in `kgit.rs`'s doc comment and a unit test. | root/root.md §4.3 step 6, §3.8 phase 7 | Keep the doc. The Root should write `refs/heads/lkg` at install step 6 (one `kgit::update`), plus a wp23 assertion. Phase 7 comes with L3. | fix-code (fixed 47c618f, R-DEC: `kgit::import_release` creates `lkg` with `main`; wp23 and kgit assertions) |
| L5 | Resume must be refused while the mirror's `refs/heads/panic` still exists. `panic::resume` checks only `panic.watch_paths`. The mirror-ref check exists only as the trigger, in `lineage/mirror.rs` (`poll_panic`, once per commit id). | root/root.md §3.9.5 step 1; `root/crates/init-root/src/panic.rs` | Keep the doc. `resume` should refuse with `root.mode` when the mirror thread's last `ls-remote` saw the branch. Low priority: the mirror is off (`url = ""`) until M0. | fix-code (fixed 7a460a6, R-SAFE: refused with `root.mode`, CLI exit 77, while the last poll saw the branch; a refusal re-polls at once; wp23 `rr19`; as-built note in root.md §3.9.5, 3ebbddc) |
| L6 | testing.md §2 says `init-root audit run`, `init-root test ledger` and `INIT_ROOT_TEST_NOW` are still placeholders. All three are built (`cli.rs`, `link.rs`, `testfix.rs`; WP20 D9, WP24). | docs/testing.md §2 ("Names") | Drop the placeholder sentence, or list only names that are still unbuilt. | adopt (applied by DOCS, 2026-10-20: the sentence now says all three are built) |
| L7 | testing.md's `reply_if` request may be stale. No `reply_if` exists in the code, and MS-1…MS-7 pass at $0 with WP24's exact-text edit specs and fixed per-task mock scripts. | docs/testing.md §3.14 (the intro paragraph) and §9 (the `→ body/provider.md` item) | Check MS-1 and MS-6 against WP24's fixtures. If the fixed scripts suffice, drop the `reply_if` request from both places. | adopt (applied by DOCS, 2026-10-20). The fixed scripts suffice: M1 and M6 are edit specs in `root/crates/init-root/tests/fixtures/sealed/M/suite.json`, and the request is withdrawn in both places. |
| L8 | testing.md §3.18 now uses root.md's `RR-<n>` ids (WP23 D1). The renumbered mapping was written from the testing side only. | docs/testing.md §3.18; root/root.md §8 ("Maps to" column) | The root.md owner checks each §3.18 row against root.md §8 and fixes either side. | adopt (applied by DOCS, 2026-10-20). testing.md §3.18 gained RT-C1 (RR-21), RT-L8/L9 (RR-23), A3 (RR-29) and RT-P1 (RR-30). root.md §8 gained RT-L4 (RR-21) and RT-W5/A5-halt (RR-33). |
| L9 | `kernel/Cargo.toml` `[workspace.dependencies]` still declares `tokio-tungstenite`, `rustls`, `hyper-rustls` and `rustls-platform-verifier`, with a stale TLS comment. No member uses them since WP5, and CONVENTIONS §5 (986669f) says the kernel has no TLS. | `kernel/Cargo.toml` lines 40–45 | Keep the doc. Delete the four entries and the comment, then confirm with `cargo metadata --offline` that `Cargo.lock` and `vendor/` do not change. No build is needed. | fix-code |
| L10 | The seed's copy of the session-event schema lags log-context §4.2. It lacks the two new optional fields of `mask.masked[]`, `call_id` and `input` (WP12 D5), which the code already writes. | `seed/src/log/session-event-1.schema.json` | Keep the doc. Copy §4.2's schema into the seed file, and the schema check test then covers the two fields. | fix-code |

Tags for §5: 6 adopt (L1, L2, L3, L6, L7, L8), 4 fix-code (L4, L5, L9, L10), 0 decide. The DOCS lane applied L1–L3 and L6–L8 on 2026-10-20, and each touched doc's Build sync block has a `build-deviations §5 L<n>` row. L4, L5, L9 and L10 are small code changes that fit one lane each: R for L4 and L5, the orchestrator for L9, B for L10. v0.3: L4 (47c618f) and L5 (7a460a6) are fixed; L9 and L10 stay open (`kernel/Cargo.toml` still declares the four TLS entries).

---

## 6. Release push 2026-10-20 (v0.3)

The v0.1.0 release push ran from 12:20 (RELEASE_PLAN.md). This section lists what it fixed that the rows above do not hold: the Root security review's findings, the blockers the real-model batch found live, and the other bugs found and fixed on the way. None of it is counted in §1's totals. Sources: build/status/{R-SAFE,R-DEC,R-SEC,K-FLAKE,K-LINE,K-GATE,G0-ROOT,G0-SEED,S-REAL}.md, build/review/root-security-2026-10-20.md (6803c69), BUILD_LOG.md 12:35–16:15, and git log.

### 6.1 Root security review findings (6803c69: 0 critical, 1 high, 3 medium, 4 low)

| Finding | What was wrong | Fixed by | Commit |
|---|---|---|---|
| F1 HIGH | Money admission added a zone-controlled reservation (`max_tokens`) with unchecked `u64` arithmetic | Saturating admission sums; the sealed lane's check and hold in one `checked_add`; `max_out` clamped at 2^20 tokens; wp22 sends `max_tokens = u64::MAX` and gets 429 `ceiling.day`, not forwarded | cdb49e8 (R-DEC) |
| F2 MEDIUM | Metering and model/level gating hung off a path-suffix heuristic, so billable non-completion routes went out unmetered | Default-deny: every model-class request must match an explicit `(provider, method, path)` row, billed or free; anything else is 403 `class.unmetered`, refused before a key is attached and never forwarded | e0e0834 (R-SEC) |
| F3 MEDIUM | One thread per connection on `egress.sock`, unbounded | At most 96 connections, 503 `root.busy` beyond; timed reads and writes; EMFILE backs off. Sealed lanes take the same pattern at 32 connections | e0e0834 (R-SEC); 3b08245 (R-DEC) |
| F4 LOW | Settle does not re-gate, and the input reservation was a bytes/3 estimate | The held reservation counts input at one token per request byte, so the ceiling is a hard wall; the kernel guard's figure is unchanged | cdb49e8 (R-DEC) |
| F5 LOW | A `setsid` child escapes panic without `live-sweep` | Escapee detection when the zone empties and at cold start: an `escapee` record and an owner alert, never a kill. The escape itself stays ADR-008's residual risk | 27dd505 (R-SAFE) |
| F6 LOW | The fd-channel stash could hold about 512 descriptors from a hostile kernel | At most 64 descriptors for at most 10 s; a datagram whose fd count differs from its tag is closed at once | 518e68e (R-DEC) |
| F7 LOW | `/_root/health` and `/_root/ceilings` were answered before the in-flight guard | Both run inside the `MAX_INFLIGHT` (64) guard and get `root.busy` above it | e0e0834 (R-SEC) |
| R-SEC follow-up | launchd's soft `RLIMIT_NOFILE` of 256 could be passed by the stash plus the egress | The daemon raises its soft limit to min(hard, 4096) at start and logs it | 7a460a6 (R-SAFE) |

### 6.2 Release blockers found live by the real-model batch (S-REAL)

| Blocker | Cause | Fixed by | Commit |
|---|---|---|---|
| Every T2 promote and quick gate rejected `gate.g0.forbid_missing` | `seed/tests/boot.rs` and `self_md.rs` lacked `#![forbid(unsafe_code)]`; a `self_md` test now checks every target root | orchestrator | fade67a |
| G0 `test` failed the seed in a real install (unit 238/33, boot 3/13) | (a) the sandbox denied every Unix socket the seed's tests bind; (b) `init-mockllm` was installed only with `--with-testkit`; (c) ext_host fixtures had no vendor config; (d) tests/boot.rs socket paths over 103 bytes; (e) a tools test read `../docs`; then test homes and sockets under the read-only export, and a mock-log race | (a) `build` profile: Unix sockets under `OUT` only; kernel profile section 6a: the same under the slot's `OUT`, while G0 runs as kernel children (G0-ROOT); (b) `init-mockllm` in every install, and a production release without it is refused (G0-ROOT); (c), (d) G0-SEED; homes, sockets and the mock-log wait G0-SEED; (e) S-REAL f4cde26 | d05c1e3, eec8d5e, 3008357, f76e749 |
| G1 `gate.g1.manifest_error` (`init-body --manifest: cannot start`) | `stages.rs` `g1()` ran selfmap in the `build` profile with `OUT = cache/gate/<job>/g1`, so the candidate's `bin/` and `ext/` were outside it; G3's inner gate had the same bug | `OUT` is the dir that holds `bin/` and `ext/` (sandbox.md §3.4, §3.9) | 70f30d3 (K-GATE) |

Guards added so these cannot come back unseen: the `seed G0` step in `ci/local.sh` runs the real G0 on the seed under the real Seatbelt texts with the shipped mock (3610c17, G0-SEED; green on 3610c17), and the `promote under Seatbelt` step runs the quick gate and promote G0 → G4 → LKG with every broker instance checked by the Root's own rules and launched under `sandbox-exec` (900feef, K-GATE; 3 runs in a row). c7a1319 adds that a kept T1 extension survives a T2 promote and handoff.

### 6.3 Other bugs found and fixed

| Bug | Effect before the fix | Fix | Commit |
|---|---|---|---|
| Oversized owner frames | A body display line near 1 MiB became a kernel `owner.out` line over the Root's 1 MiB limit, and a fast stream passed the Root's 10,000 frames a minute; either closed the link, the Root SIGKILLed the kernel and counted a crash, which could force a rollback | The kernel splits text into 128 KiB frames, cuts other kinds at 64 KiB with a marker, and paces `owner.out` at 100 frames/s (burst 300) | 15dbf74 (K-LINE) |
| Oversized owner lines | An owner line in the last ~95 bytes under 1 MiB overflowed `owner.in` and crash-looped the kernel on every redelivery | The Root refuses owner lines over 900 KiB with a notice | 44b9c6e (orchestrator) |
| Lost `HandoffDone` | When B, already committed, crashed while A retired, the machine dropped the handoff without answering, and `promote` waited forever (sv10's product half; its hang was a test-side fd leak) | Answer `ok`, then apply the crash rules | 5e50940 (K-FLAKE) |
| Build timeouts ignored | `BuildSlot::new` used the 900/300 s defaults, so `build_timeout_cold_s`/`_warm_s` were ignored and two cold installs died at 900 s under load | `BuildSlot::configured` feeds both build sites | 28f204d (K-FLAKE) |
| Builds waiting on descendants | `run_to_completion` waited for EOF on pipes a build's escaped descendant held | At most 5 s for EOF after the exit; the instance's group is signalled before the leader while the leader is unreaped | 2794924 (K-FLAKE) |
| `gate --quick` rev | `HEAD` or a snapshot commit id was fetched as `refs/heads/<rev>`, giving `E_FETCH` | `Git::fetch_rev`: remote HEAD, a raw id, a full ref or a branch; refspec metacharacters refused | f8c212d (K-FLAKE) |
| selfmap relative root | `selfmap --root .` passed a relative `./vendor`, so ext builds read `body/ext/vendor` and failed | Absolute root and target before building | 40600e0 (orchestrator) |

### 6.4 Doc changes that follow

- `init-mockllm` ships in every install (eec8d5e). CONVENTIONS §2–§3, testing.md §3.4.1 and §3.4.6, and supervise.md §3.1 now say so; only `init-fakebody` needs `--with-testkit`. The reconciliation ledgers (kernel-supervise TS-3, evolution-testing SV-17) keep their historical text.
- v0.1 runs the real Root on the owner's account in owner-machine mode (ADR-008). testing.md §1 and §3.1a and roadmap.md M0, §3.9, §3.9.1 and R18 now say that a disposable account is required only before `live-sweep` is enabled.
- kernel/sandbox.md §8 and testing.md §3.1 still name `init-kernel doctor --sandbox-selftest`, which is not built; the runner is `init-sbx-selftest run` (G0-ROOT N). Open.
