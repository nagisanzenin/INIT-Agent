# FIX5: Root `proposal.file`, and a stand-in S with a live divergence guard (lane B1)

Status: **done** (2026-10-20, about 06:25 +07). Crate: `root/crates/init-root/` (`src/**`, `tests/**`) only.
Inputs: build-deviations.md §2 gaps 6 and 20 and the §3 D14 note; WP24, WP8c and FIX2 status files.

## Done
1. **`proposal.file` on the control link** (`sup.rs::proposal`, root/root.md §4.1.3, §3.12.4, spec §13). Before this, the real Root answered `root.unknown_cmd`, so `initctl kernel propose` failed against it.
   - `{title, text}` is stored as `ROOT_HOME/state/proposals/<id>.md` (0600, `write_atomic`): `# <title>`, a blank line, then the text.
   - It is recorded as a Root record `proposal{id, title, sha256, bytes}`, where `sha256` and `bytes` are of `text`. The text itself never enters the lineage. Then the `proposal` alert fires.
   - The answer is `{id}`, with ids `p-000001`, `p-000002`, …, which is the shape the kernel's `serve.rs` reads.
   - It is never applied: nothing in the Root reads `state/proposals/` back. The CLI only lists it (below).
   - Refusals:
     - `root.args`: missing or non-string fields; a title that is empty, blank, over 200 bytes or holds control characters; text over 64 KiB.
     - `root.lineage`: a title that holds the owner's home, ROOT_HOME, INIT_HOME or an e-mail address (the §3.10.6 rule, via `lineage::private`), because the title is mirrored off the machine.
     - `root.rate`: a sixth proposal within 24 h.
   - The id counter and the 24 h window are seeded once per Root process from the lineage's `proposal` records, so a restart resets neither.
   - On a failed append, the file is removed by its exact path.
   - `init-root status --proposals` now prints `proposal <id> · <title> · <path>`, sorted, in place of bare file names.
2. **The stand-in S grew from 5 tasks to 12, plus the canary** (`tests/fixtures/sealed/S-src/`, `mock/`).
   - Seven new deterministic tasks: `sx-cubes`, `sx-fib`, `sx-countdown`, `sx-triangle`, `sx-pow2`, `sx-sort` (words.txt) and `sx-reverse` (lines.txt).
   - Each one writes one 10-line file graded 100‰ per line, and has a 3-turn mock script and a unique `SX-` marker.
   - Each costs one perl body run with 3 mock turns and a `sed` check.
   - **The gog now draws 4 holdout + 8 pool.** That is sealed-eval.md §3.3's sample (sample_n 8, holdout 4) and gate.md's own minimums. The real judge therefore runs with its defaults: WP8c's `-c gate.g4.min_pool=4 -c gate.g4.min_holdout=1` overrides are gone from `tests/wp24.rs`.
   - The split rule is unchanged; it is now a `gog::split(n)` function with a unit test.
3. **The divergence guard is now live on S.** With D14's floors at 8 + 4, the threshold is 2·√(0.0625² + 0.125²) = **0.28**, against 1.0 before, which was the whole score range. A gain confined to the pool fires it: for example, +1000‰ on 3 of the 8 pool tasks and 0 on the holdout gives 0.375.
   - MS-C1 stays correct on every draw. Over all 495 possible holdouts, C1's worst d_pool − d_holdout is 0.1625, below 0.28.
   - M3's mean d is −0.158, against a margin of 0.042, so M3 is `score_inferior` on every draw. Both were checked by exhaustive enumeration.
   - The reason is that M3 (dropping the last line of any write over 3 lines) now costs 100‰ on each 10-line task, so C1's gain is spread across the sets. `sx-calc-fix` (1000‰) and the two 1-line tasks (0) keep it heterogeneous.

## Tests
- `cd root && CARGO_INCREMENTAL=0 cargo test --offline -p init-root --features test-fixtures`: **all pass**. Unit 65; fix5 1; wp20 9; wp21 6; wp21b 2; wp22 8; wp23 4; **wp24 5 (252 s, previously about 171 s)**; wp25 3.
- `cargo test --offline -p init-root` (no features): **62 unit tests pass**. The integration tests are feature-gated.
- `cargo clippy --offline -p init-root --all-targets [--features test-fixtures] -- -D warnings`: **clean** for both feature sets.
- **`tests/fix5.rs::proposal_file_is_stored_recorded_alerted_and_never_applied`** (4.3 s, fake kernel through its `work/fk-call.json` hook, fake clock) checks:
  - the stored file, its 0600 mode, the record (`by:"root"`, the text's digest, no text), the alert, and `status --proposals`;
  - that nothing is applied: the active charter is byte-identical, `level`, `max_level`, `charter_sha256`, `tuples` and `mode` are unchanged, and no new `charter` or `ladder` record appears;
  - 4 `root.args` refusals and 2 `root.lineage` refusals (the owner's home path, an e-mail address), each of which stores and records nothing;
  - five proposals, then the sixth refused with `root.rate`; one more after `test clock --advance 86400`;
  - after a **Root restart**: p-000007…p-000010, then `root.rate`. 10 files, 10 records, 10 alerts.
- **Gate-of-gates (MS-1…MS-7, MS-C1…C3) on the new S:**

  | Gate | Time | Verdicts | Record |
  |---|---|---|---|
  | stub (default) | 97.6 s | M1 `g0_selftest`, M2 `g0_policy`, M3 and M5 `score_inferior`, M4 and M7 `canary`, M6 `g1_manifest`; C1 `better`, C2 `cheaper`, C3 `smaller` | **`pass:true`**, `sample_n` 12 |
  | **real `gate judge`**, run once with `INIT_GOG_GATE_BIN=kernel/target/k1/debug/init-kernel` (the FIX2 build, D14 floors present) | 94 s, $0 (mock model) | M1 `test`, M2 `forbid_missing`, M3 and M5 `score_inferior`, M4 and M7 `canary`, M6 `manifest_error`; **C1 `accept/better`**, C2 `cheaper`, C3 `smaller` | `mutation{total 7, rejected 7, controls 3, controls_ok 3, disagreements 0, failed [], pass:true, sample_n 12, spent_musd 106884}` |

  The real run used **no `-c` overrides**.
- **Re-run on FIX2's holdout rule (194ba30, binary rebuilt 06:22).** A score-gain accept on a holdout of fewer than 4 tasks is now `inconclusive/underpowered:holdout`. The re-run gave the same 10 verdicts and `pass:true`: gate digest `9f1decf113ae`, 99 s, $0, the same `spent_musd` 106884, `sample_n` 12. The first run above used the 35647a8 build.
- MS-G1…G3 and SE-18 (stub, broken gates) still fail as they should. MS-G3 now shows **9** disagreements (previously 2): the nine 10-line tasks score 900‰ on C1's regressed parent, and the skew lifts each to 1000.

## Deviations
1. **Proposal digest and file format.** root.md fixes only the record's fields. `sha256` and `bytes` are of `text`, as in the testkit's fake Root. The file is `# title\n\ntext\n`.
2. **"≤ 5 per day"** is read as a rolling 24 h on the Root clock, and it survives restarts through the lineage seed.
3. **A title that fails privacy is refused with `root.lineage`**, the code §4.1.2 gives for privacy, even though the record is Root-written. The text is not privacy-checked: it stays in ROOT_HOME, and only its digest is mirrored.
4. **Proposals are accepted at every level and in every mode the kernel runs in.** root.md names no gate for them.
5. **S has 12 tasks plus the canary, not 8.** Under 194ba30, 12 is also the floor for the real gate: with 11 or fewer tasks the gog draws 3 holdout, so C1 becomes `inconclusive` and the gog fails. This would happen, for example, after a leak drop (SE-02), but no test runs the real judge on a reduced S. That count is what makes the gog's existing split give the spec's 4 + 8 without changing the split rule, and it lets the real judge run at its production minimums. The cost is wp24 wall time, 171 → 252 s.
6. **wp24 expectations moved with S:**
   - imported and listed: 13;
   - audits n = 8 (`audit_n` = 8 of 12);
   - SA-2: n = 8;
   - gog `sample_n`: 12;
   - MS-G3: 9 disagreements.
7. **Not built:** a real-kernel `initctl kernel propose` end to end. It needs WP21b's install path, with a cold seed build, so it is not cheap. The kernel side's frame and answer shapes are pinned by `init-proto`'s golden test (`proposal.file{text, title}` → `{id}`), which this Root matches.

## Needs from others
- **Docs owner:**
  - root.md §4.1.3 `proposal.file`: the digest is of `text`; the file format; the `root.lineage` refusal for the title; "per day" is a rolling 24 h.
  - build-deviations.md §2 gap 6: `proposal.file` is done. `sealed.family_eval` and `sealed.holdout_rotate` remain open.
  - testing.md MS-G3: the count is now 9.
  - WP8c and FIX2: `INIT_GOG_GATE_BIN` runs are green at the gate's defaults.
- **ci/local.sh owner:** the root wp24 step now takes about 4.2 min.

## Left
- `sealed.family_eval` (L3+) and `sealed.holdout_rotate` on the control link: still unknown or `root.level`.
- A real-kernel propose end to end (deviation 7).
- The real S from benchmark snapshots (M7) replaces this stand-in.
