# POOL-LITE: visible maintainer pool v1 (lean) and the $0 failure-class script

Lane POOL-LITE of the lean proof sprint (build/specs/PROOF-LEAN.md D-L4; docs/m7-design.md §3.3, §3.7 step 2,
§3.8 DEMO). Status: **done** (see Tests).

## Done

### 1. 20 visible pool tasks: `fixtures/pool-v1/`
Location per m7 §3.3 (`fixtures/pool-v1/` in the repo; imported frozen into `vault/gate/pool/` and mirrored
read-only at `INIT_HOME/kernel/pool/`).

| Family | # | Tasks | Source (pinned, MIT) |
|---|---|---|---|
| e (Exercism, unmutated, n-z slugs, not in the polyglot slice) | 8 | py: rectangles, word-search, rational-numbers, state-of-tic-tac-toe; rust: tournament, sublist, rail-fence-cipher, yacht | exercism/python 6105f6d, exercism/rust cc099ae (same pins as S1-E) |
| c (CRUX-style change tasks) | 5 | find-spans (sample_759), align-lines (749), rotate-at (786), value-counts (753), squeeze-ws (736) | facebookresearch/cruxeval 190faf1 (S1-C's pin); none of S1-C's 14 samples nor sample_0 |
| t (terminal, written fresh) | 7 | sqlite-report, gitignore-rules, release-backport (git cherry-pick), manifest-opts (getopts/sh), pkg-layout (`python3 -m`), wordfreq, patch-port | INIT; disjoint from S1-T's 14 templates (confirmed with S1-T) |

- Ids `pool-<family>-...`; labels per m7 §3.3: `exercise`+`py`/`rust`, `repair`+`py`, `terminal`.
- task.json **v3**, field for field as root/sealed/packs/s1/FORMAT.md: check `check/run` 60 s, limits wall 600 s,
  max_cost 100,000 µUSD (≥ one request's reservation), max_turns E 8 / C 8 / T 10, 24k out, 400k in; `family`,
  `template` (= id), `difficulty`; `source.kind` maintainer.
- Visible tests in the workspace are `protected.json` entries (an edit scores 0 with `tamper`); hidden tests and
  data live in `check/hidden/`. Exercism: 35-45% of the official tests visible, the full file hidden. CRUX: a few
  visible examples, 20-26 hidden `py_cases` from the new reference. Terminal: 4-25 named checks per task,
  hidden data sets so outputs can't be hard-coded.
- Licences: `fixtures/pool-v1/LICENSES/{Exercism-MIT,CRUXEval-MIT}.txt` (verbatim from the pins); every derived
  task has a `NOTICE` (attribution, changes, MIT text) beside task.json and inside the workspace.
- `pool.json`: composition, per-task source (repo, commit, slug/sample), licences, the disjointness sets (for
  S1-CORE's lint M7-P08: Exercism slugs n-z; polyglot list; S1-C's samples).

### 2. Tools: `fixtures/pool-v1/tools/`
- `pool.py build|validate|import|manifest|cases` (python3 ≥ 3.9 stdlib; run with /usr/bin/python3 -I).
  - **validate**: deterministic build (twice, byte-identical), task.json v3 schema, source rules (n-z, polyglot,
    CRUX disjointness), check ×3 on the initial workspace (equal, ≤ 500: the s1 validator's no-op rule), ×3 on
    solution.tar (= 1000), empty workspace < 1000, an edited protected file → tamper (exit 3), optional partial
    fixture strictly between 0 and 1000, every check ≤ 10 s.
  - **import --home H**: runs the full validation, freezes each task into `H/vault/gate/pool/<id>/` (`accepted`
    {by: maintainer, at}), mirrors `H/kernel/pool/<id>/` read-only (task.json, workspace.tar, check/, NOTICE; **no
    solution.tar**) plus `pool.json` and `LICENSES/`, appends `H/vault/gate/pool-import.jsonl`. Re-import is
    idempotent; a different frozen task needs `--replace`. Refuses `~/.init` and `~/.init-root`.
- `poolcheck.py` (runner id `poolcheck/1`), the pool's check runner, copied into every task's `check/` with a
  small `check/run` sh wrapper. Own code (FORMAT.md keeps s1 pack files out of fixtures/ and INIT_HOME; approved
  by the orchestrator), same contract as s1check for the kinds the pool uses: `py_unittest`, `py_cases`,
  `cargo_test`, `sh` with FORMAT.md's keys, scoring, exit codes (0 / 3 tamper / 2 spec / 4 toolchain) and the
  tamper line; S1-CORE's two cargo fixes included (fresh target dir per check, fresh mtimes, no --quiet).

### 3. The $0 failure-class script: `scripts/m7/failclass.py`
Classifies failing visible-pool runs (RunRecords `vault/gate/jobs/*/runs.jsonl` with `set == "pool"`, retained
logs `vault/gate/jobs/<job>/sessions/<run_id>.jsonl`, trace request records), S-REAL artifact dirs, or a
manifest, into `stopped_tests_failing`, `path_confusion`, `cap_hit_exploring`, `edit_tool_mismatch`, `other`, and
names m7 §3.7's candidate for the largest class (verify-on-finish / workspace-map). Signal ids are exactly
evolution-v2 §3.4.2's (`tool.error:<tool>:<code>`, `path.home_relative`, `path.doubled_prefix`,
`path.guessed_absolute`, `bash.exit:<cmd>`, `bash.timeout`, `initctl.refused:<code>`, `retry.same_call`,
`loop.repeat_*`, `cap.hit:<cap>`, `finish.untested`, `finish.tests_failing`, `task.fail`), with §3.4.2's
expected-failure exclusion. Holdout, canary, capability and S runs are never opened; any `sealed`/`.init-root`
path is refused. Output is ICJ-like (sorted keys, integers, permille) and byte-deterministic.
S-REAL fixtures (assumed failing in `scripts/m7/testdata/sreal.json`): a1 → other (path.home_relative recovered;
the failing unittest is an expected failure), a2 → path_confusion (3 path instances + cap.hit:cost while editing),
e2e2 → path_confusion (path.doubled_prefix, fatal), h5-a3 → other (finish.untested; path.guessed_absolute
recovered; initctl.refused:trailer.subject).

## For GATE-V2b (pool cache, E3 screen) and EVO-LITE
- **The pool's checks run through `poolcheck.py`**, not s1check: invoke each frozen task's `check/run` exactly as
  kernel/gate.md §3.7.1 prescribes (cwd = OUT/ws, env_clear + PATH with the toolchain bin, HOME, TMPDIR,
  INIT_CHECK_DIR=OUT/check). `check/run` execs `python3 -I -S -B check/poolcheck.py check/`. Rust tasks need
  cargo and rustc on PATH (the toolchain bin, as the kernel's check_env already provides).
- v3 score parsing: a tamper prints `{"score_permille":0,"tamper":true}` with **exit 3** (same as s1check), so a
  pre-v3 parser scores it 0 and a v3 parser reads the `tamper` key from the last line whatever the exit code.
- The current kernel loader (`kernel-gate g4.rs load_task`) accepts v 1 and 2 only; the pool is v3, so it needs
  GATE-V2a's v3 support before G4 can load it.
- Retained failing pool-run logs at `vault/gate/jobs/<job>/sessions/<run_id>.jsonl` are what failclass.py reads.

## Tests
All run with /usr/bin/python3 (3.9.6), one at a time in the light slot, niced
(`INIT_SLOT_POOL=light bash ci/slot.sh nice -n 10 ...`).
- `POOL_SLOW=1 /usr/bin/python3 -I -m unittest discover -s fixtures/pool-v1/tools -p 'test_*.py'` → **7 tests OK**
  (581 s; nearly all of it is POOL-IMPORT's full validation, cargo included):
  - Fast: pool.json current; composition e 8 / c 5 / t 7; n-z slugs, none in the polyglot slice, no S1-C CRUX
    sample; licences and NOTICEs; all 20 build byte-identically twice and are valid task.json v3 with no s1 file
    shipped; poolcheck tamper (edited or deleted protected test → exit 3 + tamper line) and spec error (exit 2).
  - POOL-IMPORT: all **20/20 validate** (no-op ×3 equal and ≤ 500: 0-307; solution ×3 = 1000; empty workspace
    < 1000; protected edit → tamper; partial fixtures 571 / 576 / 666 / 681 / 793; every check ≤ 10 s, max ≈ 6.6 s
    for Rust under load). Imported into a test home under target/test-homes/pool-lite/: vault copies with
    `accepted`, read-only mirror without solution.tar, pool.json and LICENSES; re-import unchanged 20/20; refuses
    ~/.init and ~/.init-root.
  - POOL-PARITY: py_unittest (rectangles), py_cases (squeeze-ws), cargo_test (yacht), sh (wordfreq) × {solution,
    no-op, partial, test edit}: poolcheck/1 and s1check/1 (root/sealed/packs/s1/runner at e81e4d5, copied to a
    scratch dir only) give identical score, tamper flag, exit code and **last stdout line**; the tamper case is
    exactly `{"score_permille":0,"tamper":true}` with exit 3 (GATE-V2a's check_score). Re-run after that
    assertion was added: `-k Fast -k parity` → 6 tests OK (73 s).
- `/usr/bin/python3 -I -m unittest discover -s scripts/m7 -p 'test_*.py'` → **31 tests OK** (0.6 s): S-REAL
  fixtures, one synthetic session per class plus the expected-failure exclusion, a pool home with holdout,
  canary and capability records that are never opened, determinism and integer-only JSON.
- `/usr/bin/python3 -I scripts/m7/failclass.py --manifest scripts/m7/testdata/sreal.json` → largest
  path_confusion (2 of 4 assumed failures), candidate workspace-map.

## Deviations
- D1 Location `fixtures/pool-v1/` (m7 §3.3), not `kernel/pool/` in the repo; `INIT_HOME/kernel/pool/` is the
  installed mirror.
- D2 Own runner `poolcheck/1` instead of a copy of s1check (FORMAT.md forbids pack files in fixtures/ and
  INIT_HOME; approved). Parity with s1check is a test (POOL-PARITY).
- D3 The import is `pool.py import` (maintainer tool), not yet `init-kernel gate task import --pool` (not
  implemented in the kernel): it applies gate.md §3.7.1's import validation but cannot send the `import` record to
  Root lineage; it logs `vault/gate/pool-import.jsonl` instead. install.sh is not changed (REL's file): a release
  install gets the pool only when the M7 home runs the import (see Needs).
- D4 The mirror holds `check/` (visible by design, m7 §3.3, so the agent can practise and score itself) but not
  `solution.tar`.
- D5 20 tasks (D-L4) instead of m7's 40: no HumanEval smoke, no `ext` task, no CRUX tasks from S1-C's generator
  (5 hand-written CRUX change tasks instead), terminal 7 instead of 4 + 8 repo/tool tasks.
- D6 Source `.gitignore` files are stored as `dot.gitignore` (renamed at build), so the INIT repo never applies a
  task's ignore rules to the fixtures.
- D7 gitignore-rules uses scoring `all` (its broken .gitignore already gets 60% of the probes right, above the
  no-op ≤ 500 rule).
- D8 Difficulty is set by design (35-45% visible tests, spec edge cases, hidden data); the 30-70% pass target for a
  cheap model is unmeasured at $0 and should be read from the first real pool-cache run (incumbent per-task
  pass rates).

## Needs from others
- GATE-V2a: task.json v3 loading (the pool is v3); `tamper` read from the last line with exit 3.
- REAL-M7 (or REL): run `/usr/bin/python3 -I fixtures/pool-v1/tools/pool.py import --home "$INIT_HOME"` on the M7
  test home after `init-root install` (needs cargo on PATH or `--toolchain <bin>`).
- S1-CORE: lint M7-P08 can read `fixtures/pool-v1/pool.json` (`tasks[].source.slug` / `.sample`).

## Left
- Nothing in scope. Per-task difficulty calibration needs real runs (D8).
